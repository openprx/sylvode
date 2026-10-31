import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

/**
 * Daily editing on a Flow page: Markdown block triggers, inline bold, the link popover, and
 * navigator pages past the first list response.
 *
 * The list API caps one response at 100. This spec splits a real list into two responses so the
 * navigator has to follow `next_cursor` instead of stopping after the first page.
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;

function account(label: string): { email: string; password: string } {
	return {
		email: `marks-${label}-${runId}@e2e.sylvode.test`,
		password: `Marks-${label}-${runId}-1!`
	};
}

type Envelope<T> = { code: number; message: string; data: T };
type LoginData = {
	tokens: { access_token: string; refresh_token: string };
	user: { id: string };
};
type ObjectView = { id: string; title: string; object_type: string };

async function unwrap<T>(response: Awaited<ReturnType<APIRequestContext['get']>>): Promise<T> {
	expect(response.ok(), `${response.url()} answered HTTP ${response.status()}`).toBe(true);
	const body = (await response.json()) as Envelope<T>;
	expect(body.code, `${response.url()}: ${body.message}`).toBe(0);
	return body.data;
}

async function login(
	request: APIRequestContext,
	who: { email: string; password: string }
): Promise<LoginData> {
	return unwrap<LoginData>(await request.post('/api/v1/auth/login', { data: who }));
}

async function instanceAdmin(request: APIRequestContext): Promise<LoginData> {
	if (process.env.ADMIN_EMAIL && process.env.ADMIN_PASSWORD) {
		return login(request, {
			email: process.env.ADMIN_EMAIL,
			password: process.env.ADMIN_PASSWORD
		});
	}
	const admin = account('admin');
	await unwrap(
		await request.post('/api/v1/auth/register', {
			data: { ...admin, name: 'Marks Admin' }
		})
	);
	return login(request, admin);
}

async function signInAs(page: Page, session: LoginData): Promise<void> {
	await page.addInitScript(
		({ accessToken, refreshToken, authUser }) => {
			localStorage.setItem('auth_token', accessToken);
			localStorage.setItem('refresh_token', refreshToken);
			localStorage.setItem('auth_user', JSON.stringify(authUser));
			localStorage.setItem('locale', 'en');
		},
		{
			accessToken: session.tokens.access_token,
			refreshToken: session.tokens.refresh_token,
			authUser: session.user
		}
	);
}

test('editor: markdown heading, bold, and a link', async ({ page, request }) => {
	test.setTimeout(120_000);
	const admin = await instanceAdmin(request);
	const owner = account('editor-owner');
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...owner, name: 'Marks Owner' }
		})
	);
	const ownerLogin = await login(request, owner);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `marks-${runId}`, name: `Marks ${runId}` }
		})
	);
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: auth,
			data: { enabled: true, idempotency_key: crypto.randomUUID() }
		})
	);
	const created = await unwrap<{ object: { id: string } }>(
		await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
			headers: auth,
			data: {
				object_type: 'page',
				title: `Marks ${runId}`,
				idempotency_key: crypto.randomUUID()
			}
		})
	);

	await signInAs(page, ownerLogin);
	await page.goto(`/workspace/${workspace.id}/flow/${created.object.id}`);
	const editor = page.locator('.flow-canvas .ProseMirror');
	await expect(editor).toBeVisible();
	await editor.click();
	await page.keyboard.type('# Hello');
	await expect(editor.locator('h1')).toHaveText('Hello');

	await page.keyboard.press('Enter');
	await page.keyboard.type('world');
	for (let i = 0; i < 'world'.length; i += 1) {
		await page.keyboard.press('Shift+ArrowLeft');
	}
	await page.keyboard.press('ControlOrMeta+b');
	await expect(editor.locator('p strong')).toHaveText('world');
	await expect(page.getByTestId('flow-selection-toolbar')).toBeVisible();

	await page.getByTestId('flow-mark-link').click();
	await page.getByTestId('flow-link-input').fill('https://example.com/flow');
	await page.getByTestId('flow-link-apply').click();
	await expect(editor.locator('p a')).toHaveAttribute('href', 'https://example.com/flow');
});

test('navigator follows the list cursor past the first page', async ({ page, request }) => {
	test.setTimeout(120_000);
	const admin = await instanceAdmin(request);
	const owner = account('nav-owner');
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...owner, name: 'Pages Owner' }
		})
	);
	const ownerLogin = await login(request, owner);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `pages-${runId}`, name: `Pages ${runId}` }
		})
	);
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: auth,
			data: { enabled: true, idempotency_key: crypto.randomUUID() }
		})
	);
	const firstTitle = `Alpha ${runId}`;
	const secondTitle = `Beta ${runId}`;
	for (const title of [firstTitle, secondTitle]) {
		await unwrap(
			await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
				headers: auth,
				data: { object_type: 'page', title, idempotency_key: crypto.randomUUID() }
			})
		);
	}

	let held: ObjectView[] = [];
	await page.route(/\/api\/v1\/workspaces\/[^/]+\/flow\/objects(?:\?.*)?$/, async (route) => {
		if (route.request().method() !== 'GET') {
			await route.continue();
			return;
		}
		const url = new URL(route.request().url());
		if (url.searchParams.has('cursor')) {
			await route.fulfill({
				status: 200,
				contentType: 'application/json',
				body: JSON.stringify({ code: 0, message: 'ok', data: { items: held } })
			});
			return;
		}
		const response = await route.fetch();
		const json = (await response.json()) as Envelope<{ items: ObjectView[]; next_cursor?: string }>;
		if (json.code === 0 && json.data.items.length >= 2) {
			held = json.data.items.slice(1);
			json.data.items = json.data.items.slice(0, 1);
			json.data.next_cursor = 'e2e-page-2';
		}
		await route.fulfill({
			status: 200,
			contentType: 'application/json',
			body: JSON.stringify(json)
		});
	});

	await signInAs(page, ownerLogin);
	await page.goto(`/workspace/${workspace.id}/flow`);
	await expect(page.getByRole('treeitem', { name: firstTitle })).toBeVisible();
	await expect(page.getByRole('treeitem', { name: secondTitle })).toHaveCount(0);
	await page.getByTestId('flow-nav-load-more').click();
	await expect(page.getByRole('treeitem', { name: secondTitle })).toBeVisible();
});
