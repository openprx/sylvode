#!/usr/bin/env node
// Browser behaviour checks for `confirmAction` and `ConfirmHost`, run against the internal test
// page `/admin/ui` in headless Chromium with real key presses.
//
//   node scripts/ui-behaviour-confirm.mjs --base-url http://127.0.0.1:18201 --api http://127.0.0.1:18083
//
// Read-only: every non-GET request to the API is aborted. Credentials come from the environment
// (P3X_U1_EMAIL, P3X_U1_PASSWORD) or from the env file named by UI_SHOTS_ENV_FILE; they are never
// printed. Browser: the local open-source Chromium (CHROMIUM_BIN overrides the path).

import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';

const DEFAULT_CHROMIUM = '/Applications/Chromium.app/Contents/MacOS/Chromium';

function parseFlags(argv) {
	const flags = {};
	for (let i = 0; i < argv.length; i += 1)
		if (argv[i].startsWith('--')) flags[argv[i].slice(2)] = argv[i + 1] ?? true;
	return flags;
}

async function credentials() {
	let email = process.env.P3X_U1_EMAIL;
	let password = process.env.P3X_U1_PASSWORD;
	const file = process.env.UI_SHOTS_ENV_FILE;
	if ((!email || !password) && file && existsSync(file)) {
		for (const line of (await readFile(file, 'utf8')).split('\n')) {
			const match = line.match(/^(?:export\s+)?(\w+)=(.*)$/);
			if (!match) continue;
			const value = match[2].trim().replace(/^['"]|['"]$/g, '');
			if (match[1] === 'P3X_U1_EMAIL') email ||= value;
			if (match[1] === 'P3X_U1_PASSWORD') password ||= value;
		}
	}
	if (!email || !password) throw new Error('no credentials (P3X_U1_EMAIL, P3X_U1_PASSWORD)');
	return { email, password };
}

async function signIn(api) {
	const response = await fetch(`${api}/api/v1/auth/login`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify(await credentials())
	});
	const body = await response.json().catch(() => null);
	if (!response.ok || body?.code !== 0 || !body?.data?.tokens?.access_token)
		throw new Error(`sign-in failed with HTTP ${response.status}`);
	return body.data;
}

const flags = parseFlags(process.argv.slice(2));
const baseUrl = String(flags['base-url'] || '').replace(/\/$/, '');
const api = String(flags.api || 'http://127.0.0.1:18083').replace(/\/$/, '');
if (!baseUrl) throw new Error('--base-url is required');

const { chromium } = await import('playwright-core');
const executablePath = process.env.CHROMIUM_BIN || DEFAULT_CHROMIUM;
if (!existsSync(executablePath)) throw new Error(`Chromium not found at ${executablePath}`);

const session = await signIn(api);
const browser = await chromium.launch({ executablePath, headless: true });
const context = await browser.newContext({
	viewport: { width: 1280, height: 900 },
	locale: 'en-US',
	reducedMotion: 'reduce'
});
await context.addInitScript(
	({ tokens, user }) => {
		try {
			localStorage.setItem('locale', 'en');
			localStorage.setItem('theme', 'light');
			localStorage.setItem('auth_token', tokens.access_token);
			localStorage.setItem('refresh_token', tokens.refresh_token);
			localStorage.setItem('auth_user', JSON.stringify(user));
		} catch {
			// storage unavailable
		}
	},
	{ tokens: session.tokens, user: session.user }
);
await context.route('**/api/**', (route) => {
	const req = route.request();
	const method = req.method();
	if (
		method === 'GET' ||
		method === 'HEAD' ||
		method === 'OPTIONS' ||
		new URL(req.url()).pathname === '/api/v1/auth/refresh'
	)
		return route.continue();
	return route.abort('blockedbyclient');
});

const page = await context.newPage();
await page.goto(`${baseUrl}/admin/ui`);
await page.waitForSelector('[data-gallery-section="confirm"]');

let failures = 0;
const results = [];
async function check(name, body) {
	try {
		const detail = await body();
		results.push(`ok   ${name}${detail ? ` -- ${detail}` : ''}`);
	} catch (error) {
		failures += 1;
		results.push(`FAIL ${name} -- ${error instanceof Error ? error.message : error}`);
	}
}
function expect(condition, message) {
	if (!condition) throw new Error(message);
}

const DIALOG = '[data-testid="confirm-dialog"]';
const active = () =>
	page.evaluate(() => {
		const el = document.activeElement;
		return el
			? {
					tag: el.tagName.toLowerCase(),
					testid: el.getAttribute('data-testid'),
					text: (el.textContent || '').trim()
				}
			: null;
	});
const outcome = () => page.getAttribute('[data-testid="confirm-result"]', 'data-outcome');
const gone = () => page.waitForSelector(DIALOG, { state: 'detached' });
const open = async (testid) => {
	await page.focus(`[data-testid="${testid}"]`);
	await page.keyboard.press('Enter');
	await page.waitForSelector(DIALOG);
	await page.waitForFunction(() =>
		document.activeElement?.closest('[data-testid="confirm-dialog"]')
	);
};

await check(
	'destructive confirmation is an alertdialog with a name and a description',
	async () => {
		await open('confirm-opener');
		const info = await page.evaluate((s) => {
			const el = document.querySelector(s);
			const name = document
				.getElementById(el.getAttribute('aria-labelledby') ?? '')
				?.textContent?.trim();
			const desc = document
				.getElementById(el.getAttribute('aria-describedby') ?? '')
				?.textContent?.trim();
			return { role: el.getAttribute('role'), modal: el.getAttribute('aria-modal'), name, desc };
		}, DIALOG);
		expect(info.role === 'alertdialog' && info.modal === 'true', JSON.stringify(info));
		expect(info.name === 'Delete member Ana?', JSON.stringify(info));
		expect(info.desc?.startsWith('Ana loses access'), JSON.stringify(info));
		await page.keyboard.press('Escape');
		await gone();
	}
);
await check('focus starts on Cancel and the confirm button names verb and object', async () => {
	await open('confirm-opener');
	const first = await active();
	expect(first.tag === 'button' && first.text === 'Cancel', JSON.stringify(first));
	const buttons = await page.locator(`${DIALOG} footer button`).allInnerTexts();
	expect(buttons.at(-1).trim() === 'Delete member Ana', JSON.stringify(buttons));
	await page.keyboard.press('Escape');
	await gone();
});
for (const width of [1440, 390]) {
	await check(
		`a long confirm label stays inside the dialog and never covers Cancel at ${width} wide`,
		async () => {
			await page.setViewportSize({ width, height: 900 });
			await open('confirm-long-opener');
			const info = await page.evaluate((s) => {
				const dialog = document.querySelector(s);
				const box = (el) => {
					const r = el.getBoundingClientRect();
					return { l: r.left, t: r.top, r: r.right, b: r.bottom };
				};
				const buttons = [...dialog.querySelectorAll('footer button')];
				const confirm = buttons.at(-1);
				const label = confirm.querySelector('.confirm-label');
				return {
					dialog: box(dialog),
					cancel: box(buttons[0]),
					confirm: box(confirm),
					cancelText: buttons[0].textContent.trim(),
					aria: confirm.getAttribute('aria-label'),
					title: confirm.getAttribute('title'),
					text: label.textContent.trim(),
					truncated: label.scrollWidth > label.clientWidth,
					ellipsis: getComputedStyle(label).textOverflow
				};
			}, DIALOG);
			const inside = (b) =>
				b.l >= info.dialog.l - 0.5 && b.r <= info.dialog.r + 0.5 && b.t >= info.dialog.t - 0.5;
			const overlap = !(
				info.cancel.r <= info.confirm.l + 0.5 ||
				info.confirm.r <= info.cancel.l + 0.5 ||
				info.cancel.b <= info.confirm.t + 0.5 ||
				info.confirm.b <= info.cancel.t + 0.5
			);
			expect(info.text.length >= 80, `label has ${info.text.length} characters`);
			expect(info.cancelText === 'Cancel', info.cancelText);
			expect(
				inside(info.cancel) && inside(info.confirm),
				`buttons leave the dialog ${JSON.stringify(info)}`
			);
			expect(!overlap, `Cancel is covered ${JSON.stringify(info)}`);
			expect(
				info.aria === info.text && info.title === info.text,
				'full label is the name and title'
			);
			expect(info.truncated, 'a label this long is cut');
			expect(info.ellipsis === 'ellipsis', `text-overflow ${info.ellipsis}`);
			const stacked = info.confirm.t >= info.cancel.b - 0.5;
			await page.keyboard.press('Escape');
			await gone();
			await page.setViewportSize({ width: 1280, height: 900 });
			return `${stacked ? 'two rows' : 'one row'}, truncated ${info.truncated}`;
		}
	);
}
await check('Escape resolves false and focus returns to the caller', async () => {
	await open('confirm-opener');
	await page.keyboard.press('Escape');
	await gone();
	expect((await outcome()) === 'no', `outcome ${await outcome()}`);
	const back = await active();
	expect(back.testid === 'confirm-opener', JSON.stringify(back));
});
await check('Enter on the initial focus (Cancel) resolves false', async () => {
	await open('confirm-opener');
	await page.keyboard.press('Enter');
	await gone();
	expect((await outcome()) === 'no', `outcome ${await outcome()}`);
});
await check('the confirm button resolves true and focus returns to the caller', async () => {
	await open('confirm-opener');
	await page.locator(`${DIALOG} footer button`).last().click();
	await gone();
	expect((await outcome()) === 'yes', `outcome ${await outcome()}`);
	await page.waitForFunction(
		() => document.activeElement?.getAttribute('data-testid') === 'confirm-opener'
	);
});
await check('two calls in a row are shown one after the other', async () => {
	await open('confirm-twice-opener');
	expect((await page.locator(DIALOG).count()) === 1, 'only one dialog at a time');
	const titleOf = () => page.locator(`${DIALOG} h2`).innerText();
	expect((await titleOf()) === 'Delete member Ana?', await titleOf());
	await page.locator(`${DIALOG} footer button`).last().click();
	await page.waitForFunction(() =>
		document.querySelector('[data-testid="confirm-dialog"] h2')?.textContent?.includes('webhook')
	);
	expect((await page.locator(DIALOG).count()) === 1, 'still one dialog');
	const second = await active();
	expect(second.text === 'Cancel', `second dialog focus ${JSON.stringify(second)}`);
	const label = (await page.locator(`${DIALOG} footer button`).last().innerText()).trim();
	expect(label === 'Remove webhook hooks.example', label);
	await page.keyboard.press('Escape');
	await gone();
	await page.waitForFunction(
		() => document.activeElement?.getAttribute('data-testid') === 'confirm-twice-opener'
	);
});
await check('a neutral confirmation is an alertdialog with a primary confirm button', async () => {
	await open('confirm-neutral-opener');
	const role = await page.getAttribute(DIALOG, 'role');
	expect(role === 'alertdialog', role);
	const first = await active();
	expect(first.text === 'Cancel', JSON.stringify(first));
	await page.locator(`${DIALOG} footer button`).last().click();
	await gone();
	expect((await outcome()) === 'yes', `outcome ${await outcome()}`);
});
await check('a confirmation opened from inside another dialog: Escape closes only it', async () => {
	await page.focus('[data-testid="confirm-nested-opener"]');
	await page.keyboard.press('Enter');
	await page.waitForSelector('[data-testid="confirm-nested-dialog"]');
	await page.waitForFunction(() =>
		document.activeElement?.closest('[data-testid="confirm-nested-dialog"]')
	);
	await page.focus('[data-testid="confirm-nested-ask"]');
	await page.keyboard.press('Enter');
	await page.waitForSelector(DIALOG);
	await page.waitForFunction(() =>
		document.activeElement?.closest('[data-testid="confirm-dialog"]')
	);
	await page.keyboard.press('Escape');
	await gone();
	expect(
		await page.locator('[data-testid="confirm-nested-dialog"]').isVisible(),
		'outer dialog still open'
	);
	await page.waitForFunction(
		() => document.activeElement?.getAttribute('data-testid') === 'confirm-nested-ask'
	);
	await page.keyboard.press('Escape');
	await page.waitForSelector('[data-testid="confirm-nested-dialog"]', { state: 'detached' });
	await page.waitForFunction(
		() => document.activeElement?.getAttribute('data-testid') === 'confirm-nested-opener'
	);
});
await check('Tab stays inside the confirmation', async () => {
	await open('confirm-opener');
	for (let i = 0; i < 5; i += 1) {
		await page.keyboard.press('Tab');
		const inside = await page.evaluate(
			() => !!document.activeElement?.closest('[data-testid="confirm-dialog"]')
		);
		expect(inside, `Tab ${i + 1} left the dialog`);
	}
	// The close button shows a tooltip while focused; the first Escape hides it, the next closes.
	for (let i = 0; i < 2 && (await page.locator(DIALOG).count()) > 0; i += 1) {
		await page.keyboard.press('Escape');
		await page.waitForTimeout(150);
	}
	await gone();
});
await check('navigation while open declines the request and removes the dialog', async () => {
	await open('confirm-opener');
	// Use a real client navigation through the app router: a link click.
	await page.evaluate(() => {
		const a = document.createElement('a');
		a.href = '/admin/users';
		a.id = '__nav';
		document.body.appendChild(a);
		a.click();
	});
	await page.waitForURL('**/admin/users');
	await page.waitForSelector(DIALOG, { state: 'detached' });
	return 'dialog removed after navigation';
});

console.log(results.join('\n'));
console.log(`\n${results.length - failures} passed, ${failures} failed`);
await browser.close();
process.exit(failures > 0 ? 1 : 0);
