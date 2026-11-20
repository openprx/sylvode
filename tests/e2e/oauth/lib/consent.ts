// Drives the real Sylvode pages in headless Chromium: sign-in, the consent page, and the
// "Connected applications" sections. Selectors are the data-testid values of the frontend.

import { expect, type Locator, type Page } from '@playwright/test';
import type { Account } from '../config.ts';
import { redactor } from './redact.ts';
import { check, escapeRegExp } from './util.ts';

export type ConsentMode = 'allow' | 'deny' | 'no_account';

export interface ConsentChoice {
	mode: ConsentMode;
	/** Required unless mode is no_account (which is chosen while signed out). */
	login?: Account;
	workspaceId?: string;
	/** 'all' or the names of the projects to tick in the picker. */
	projects?: 'all' | { names: string[] };
	/** Optional scopes to tick (only meaningful when the client requested them). */
	optionalScopes?: string[];
}

export type ConsentView = 'invalid' | 'login' | 'consent' | 'other';

const tid = (page: Page, id: string): Locator => page.locator(`[data-testid="${id}"]`);

async function waitForPageReady(page: Page): Promise<string> {
	await page.waitForSelector('[data-testid="consent-page"]:not([data-view="loading"])', {
		timeout: 20_000
	});
	return (await tid(page, 'consent-page').getAttribute('data-view')) ?? 'other';
}

/** Signs in on the login page (the page the consent page sends an unauthenticated person to). */
export async function fillLogin(page: Page, account: Account): Promise<void> {
	await page.waitForSelector('#email', { timeout: 20_000 });
	await page.fill('#email', account.email);
	await page.fill('#password', account.password);
	await page.click('button[type="submit"]');
}

/** Signs in through the login page and waits until the app has left it. */
export async function signIn(page: Page, uiOrigin: string, account: Account): Promise<void> {
	await page.goto(`${uiOrigin}/auth/login`, { waitUntil: 'load' });
	await fillLogin(page, account);
	await page.waitForURL((url) => !url.pathname.startsWith('/auth/login'), { timeout: 30_000 });
}

async function consentErrorText(page: Page): Promise<string> {
	const box = tid(page, 'consent-error');
	if ((await box.count()) === 0) return '';
	return redactor.text((await box.innerText()).slice(0, 300));
}

/**
 * Opens an authorization URL and takes the person through it. Returns the view the page ended in
 * when nothing was decided (for the error page case) and otherwise once the decision was clicked;
 * the caller then waits for the redirect at its loopback listener.
 */
export async function driveConsent(
	page: Page,
	authorizeUrl: string,
	choice: ConsentChoice
): Promise<{ view: ConsentView; errorText: string }> {
	await page.goto(authorizeUrl, { waitUntil: 'load' });
	let view = await waitForPageReady(page);
	if (view === 'invalid') return { view: 'invalid', errorText: '' };

	if (view === 'login') {
		if (choice.mode === 'no_account') {
			await tid(page, 'consent-no-account').click();
			return { view: 'login', errorText: '' };
		}
		check(choice.login !== undefined, 'driveConsent: a login is needed for this choice');
		await tid(page, 'consent-login-button').click();
		await page.waitForURL(/\/auth\/login\?return_to=/, { timeout: 20_000 });
		await fillLogin(page, choice.login);
		await page.waitForSelector(
			'[data-testid="consent-form"], [data-testid="consent-error-page"], [data-testid="consent-no-eligible"]',
			{ timeout: 30_000 }
		);
		view = await waitForPageReady(page);
	}
	check(view === 'consent', `the consent page did not show the form after sign-in (view: ${view})`);

	if (choice.mode === 'deny') {
		await tid(page, 'consent-deny').click();
		return { view: 'consent', errorText: '' };
	}
	check(choice.mode === 'allow', 'driveConsent: unexpected mode');
	check(
		choice.workspaceId !== undefined && choice.projects !== undefined,
		'driveConsent: workspace and projects are needed to allow'
	);

	await tid(page, `consent-workspace-${choice.workspaceId}`).check();
	if (choice.projects === 'all') {
		await tid(page, 'consent-projects-all').check();
	} else {
		await tid(page, 'consent-projects-selected').check();
		await page.waitForSelector('[data-testid="consent-project-select"]', { timeout: 20_000 });
		await tid(page, 'consent-project-select').click();
		for (const name of choice.projects.names) {
			await page.getByRole('option', { name: new RegExp(escapeRegExp(name)) }).click();
		}
		await page.keyboard.press('Escape');
	}
	for (const scope of choice.optionalScopes ?? [])
		await tid(page, `consent-scope-${scope}`).check();

	const allow = tid(page, 'consent-allow');
	await expect(allow).toBeEnabled({ timeout: 10_000 });
	await allow.click();
	// Give a refused decision a moment to show its message so the caller can report it.
	await page.waitForTimeout(500);
	return { view: 'consent', errorText: await consentErrorText(page) };
}

const dialog = (page: Page): Locator => page.locator('[data-testid="confirm-dialog"]');

async function confirmDisconnect(page: Page): Promise<void> {
	await dialog(page).waitFor({ timeout: 15_000 });
	await dialog(page)
		.getByRole('button', { name: /^Disconnect / })
		.click();
}

/**
 * Personal settings: lists the visible Connected applications rows (grant ids) and optionally
 * disconnects one through the confirmation dialog. `onResponse` is called with a monotonic time
 * when the DELETE answer is seen, so the caller can time "from the response of the trigger".
 */
export async function personalSettings(
	page: Page,
	uiOrigin: string,
	action: { disconnectGrantId?: string; onDeleteResponse?: (status: number) => void }
): Promise<string[]> {
	await page.goto(`${uiOrigin}/settings`, { waitUntil: 'load' });
	await page.waitForSelector('[data-testid="connected-apps-personal"]', { timeout: 20_000 });
	await page.waitForSelector(
		'[data-testid="connected-app-row"], [data-testid="connected-apps-personal-empty"]',
		{ timeout: 20_000 }
	);
	const ids = await tid(page, 'connected-app-row').evaluateAll((rows) =>
		rows.map((row) => row.getAttribute('data-grant-id') ?? '')
	);
	if (action.disconnectGrantId !== undefined) {
		const row = page.locator(
			`[data-testid="connected-app-row"][data-grant-id="${action.disconnectGrantId}"]`
		);
		check((await row.count()) === 1, 'the grant to disconnect has no row in personal settings');
		const answered = page.waitForResponse(
			(response) =>
				response.request().method() === 'DELETE' && response.url().includes('/connected-apps/'),
			{ timeout: 20_000 }
		);
		await row.locator('[data-testid="connected-app-disconnect"]').click();
		await confirmDisconnect(page);
		action.onDeleteResponse?.((await answered).status());
	}
	return ids;
}

/** Workspace settings, Connected applications: disconnects one member's connection. */
export async function workspaceDisconnect(
	page: Page,
	uiOrigin: string,
	workspaceId: string,
	grantId: string,
	onDeleteResponse: (status: number) => void
): Promise<void> {
	await page.goto(`${uiOrigin}/workspace/${workspaceId}/settings`, { waitUntil: 'load' });
	const row = page.locator(`[data-testid="workspace-app-row"][data-grant-id="${grantId}"]`);
	await row.waitFor({ timeout: 30_000 });
	const answered = page.waitForResponse(
		(response) =>
			response.request().method() === 'DELETE' && response.url().includes('/connected-apps/'),
		{ timeout: 20_000 }
	);
	await row.locator('[data-testid="workspace-app-disconnect"]').click();
	await confirmDisconnect(page);
	onDeleteResponse((await answered).status());
}
