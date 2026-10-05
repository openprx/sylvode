import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

/**
 * A bot token created on the Members page works on the surface the user picked.
 *
 * Every bot credential is bound to one transport surface and refused on every other. Before the
 * form offered the choice, every token it created was `rest`-only and got 401 through the MCP
 * server and the CLI. This spec creates a token for `mcp_http` through the UI, then checks the
 * stored surface through the bots API and that the API accepts the token exactly where the
 * surface says: on MCP HTTP, and not as a plain REST caller.
 *
 * It creates its own workspace. The user is TEST_EMAIL / TEST_PASSWORD when both are set (as in
 * the other specs); otherwise a fresh user is registered, which works on an instance whose first
 * account has not been created yet (registration is admin-only after that).
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const email = process.env.TEST_EMAIL ?? `surface-${runId}@e2e.sylvode.test`;
const password = process.env.TEST_PASSWORD ?? `Surface-${runId}-Pass1!`;

type Envelope<T> = { code: number; message: string; data: T };
type Tokens = { access_token: string; refresh_token: string };
type LoginData = { tokens: Tokens; user: unknown };
type Bot = { id: string; name: string; transport_surface: string; permissions: string[] };

async function unwrap<T>(response: Awaited<ReturnType<APIRequestContext['get']>>): Promise<T> {
	expect(response.ok(), `${response.url()} answered HTTP ${response.status()}`).toBe(true);
	const body = (await response.json()) as Envelope<T>;
	expect(body.code, `${response.url()}: ${body.message}`).toBe(0);
	return body.data;
}

async function signIn(page: Page): Promise<LoginData> {
	if (!process.env.TEST_EMAIL || !process.env.TEST_PASSWORD) {
		await unwrap(await page.request.post('/api/v1/auth/register', { data: { email, password, name: 'Surface E2E' } }));
	}
	const login = await unwrap<LoginData>(await page.request.post('/api/v1/auth/login', { data: { email, password } }));
	await page.addInitScript(
		({ accessToken, refreshToken, authUser }) => {
			localStorage.setItem('auth_token', accessToken);
			localStorage.setItem('refresh_token', refreshToken);
			localStorage.setItem('auth_user', JSON.stringify(authUser));
			localStorage.setItem('locale', 'en');
		},
		{ accessToken: login.tokens.access_token, refreshToken: login.tokens.refresh_token, authUser: login.user }
	);
	return login;
}

test('a token created for MCP over HTTP is stored and accepted as mcp_http only', async ({ page }) => {
	const login = await signIn(page);
	const auth = { Authorization: `Bearer ${login.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await page.request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `surface-${runId}`, name: `Surface ${runId}` }
		})
	);

	await page.goto(`/workspace/${workspace.id}/members`);
	await page.getByRole('button', { name: 'Create Token' }).first().click();

	const dialog = page.getByRole('dialog');
	const tokenName = `MCP HTTP agent ${runId}`;
	await dialog.getByLabel('Token Name').fill(tokenName);

	const surfaces = dialog.getByRole('group', { name: /Where will this token be used/ });
	await expect(surfaces.getByRole('radio')).toHaveCount(6);
	await expect(surfaces.getByRole('radio', { checked: true })).toHaveCount(0);

	// The choice is required: submitting without one creates nothing.
	await dialog.getByRole('button', { name: 'Create Token' }).click();
	await expect(dialog.getByLabel('Token Name')).toBeVisible();
	expect(await unwrap<Bot[]>(await page.request.get(`/api/v1/workspaces/${workspace.id}/bots`, { headers: auth }))).toHaveLength(0);

	// Keyboard only: focus the first radio, arrow to MCP over HTTP.
	await surfaces.getByRole('radio', { name: /REST API/ }).focus();
	await page.keyboard.press('ArrowDown');
	await expect(surfaces.getByRole('radio', { name: /MCP over HTTP/ })).toBeChecked();
	await dialog.getByRole('button', { name: 'Create Token' }).click();

	const reveal = page.getByTestId('created-token-surface');
	await expect(reveal).toContainText('This token works only with: MCP over HTTP');
	const rawToken = (await page.locator('code').first().textContent())?.trim() ?? '';
	expect(rawToken.startsWith('opr_')).toBe(true);
	await page.getByRole('button', { name: 'Close' }).last().click();

	const listed = page.getByTestId('bot-transport-surface');
	await expect(listed).toHaveText('MCP over HTTP');
	await expect(listed).toHaveAttribute('data-surface', 'mcp_http');

	const bots = await unwrap<Bot[]>(await page.request.get(`/api/v1/workspaces/${workspace.id}/bots`, { headers: auth }));
	const created = bots.find((bot) => bot.name === tokenName);
	expect(created?.transport_surface).toBe('mcp_http');

	// The binding is what the server enforces: the token is a valid MCP HTTP credential and is
	// refused when presented as a plain REST caller.
	const asMcp = await page.request.get(`/api/v1/workspaces/${workspace.id}/bots`, {
		headers: { Authorization: `Bearer ${rawToken}`, 'x-sylvode-mcp-surface': 'mcp_http' }
	});
	const asMcpBody = (await asMcp.json()) as Envelope<unknown>;
	expect(asMcpBody.code, asMcpBody.message).not.toBe(401);
	const asRest = await page.request.get(`/api/v1/workspaces/${workspace.id}/bots`, {
		headers: { Authorization: `Bearer ${rawToken}` }
	});
	const asRestBody = (await asRest.json()) as Envelope<unknown>;
	expect(asRestBody.code).toBe(401);
});
