import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

/**
 * Workspace Flow settings page (`/workspace/{id}/settings/flow`) against a real API + PostgreSQL.
 *
 * An owner enables Flow from the page (the Flow sidebar entry appears and `/flow` is reachable),
 * disables it through the confirmation dialog (the entry disappears and `/flow` shows the
 * disabled page), changes the default member level through its confirmation dialog (the
 * authorization epoch advances), and re-enables it. A plain member of the same workspace sees no
 * settings entry and gets the forbidden-safe empty state on the page; the server refuses that
 * member's PUT as well.
 *
 * Needs BASE_URL to serve the built frontend with `/api` proxied to the API on the same origin
 * (as `frontend/nginx.conf` does). Registration after the first account needs an instance admin:
 * ADMIN_EMAIL / ADMIN_PASSWORD when both are set, otherwise the run registers the instance's
 * first account itself (a fresh database). The owner and the member are registered fresh per run.
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = { email: `flow-owner-${runId}@e2e.sylvode.test`, password: `FlowOwner-${runId}-1!` };
const member = { email: `flow-member-${runId}@e2e.sylvode.test`, password: `FlowMember-${runId}-1!` };

type Envelope<T> = { code: number; message: string; data: T; error_code?: string };
type LoginData = {
	tokens: { access_token: string; refresh_token: string };
	user: { id: string };
};

async function envelope<T>(response: Awaited<ReturnType<APIRequestContext['get']>>): Promise<Envelope<T>> {
	expect(response.ok(), `${response.url()} answered HTTP ${response.status()}`).toBe(true);
	return (await response.json()) as Envelope<T>;
}

async function unwrap<T>(response: Awaited<ReturnType<APIRequestContext['get']>>): Promise<T> {
	const body = await envelope<T>(response);
	expect(body.code, `${response.url()}: ${body.message}`).toBe(0);
	return body.data;
}

async function login(request: APIRequestContext, who: { email: string; password: string }): Promise<LoginData> {
	return unwrap<LoginData>(await request.post('/api/v1/auth/login', { data: who }));
}

async function instanceAdmin(request: APIRequestContext): Promise<LoginData> {
	if (process.env.ADMIN_EMAIL && process.env.ADMIN_PASSWORD) {
		return login(request, { email: process.env.ADMIN_EMAIL, password: process.env.ADMIN_PASSWORD });
	}
	const admin = { email: `flow-admin-${runId}@e2e.sylvode.test`, password: `FlowAdmin-${runId}-1!` };
	await unwrap(await request.post('/api/v1/auth/register', { data: { ...admin, name: 'Flow Admin' } }));
	return login(request, admin);
}

async function register(
	request: APIRequestContext,
	admin: LoginData,
	who: { email: string; password: string },
	name: string
): Promise<LoginData> {
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...who, name }
		})
	);
	return login(request, who);
}

async function signInAs(page: Page, login: LoginData): Promise<void> {
	await page.addInitScript(
		({ accessToken, refreshToken, authUser }) => {
			localStorage.setItem('auth_token', accessToken);
			localStorage.setItem('refresh_token', refreshToken);
			localStorage.setItem('auth_user', JSON.stringify(authUser));
			localStorage.setItem('locale', 'en');
		},
		{ accessToken: login.tokens.access_token, refreshToken: login.tokens.refresh_token, authUser: login.user }
	);
}

test('owner toggles Flow and its baseline; a member gets the forbidden-safe state', async ({ browser, request }) => {
	const admin = await instanceAdmin(request);
	const ownerLogin = await register(request, admin, owner, 'Flow Owner');
	const memberLogin = await register(request, admin, member, 'Flow Member');
	const ownerAuth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: ownerAuth,
			data: { slug: `flow-${runId}`, name: `Flow ${runId}` }
		})
	);
	await unwrap(
		await request.post(`/api/v1/workspaces/${workspace.id}/members`, {
			headers: ownerAuth,
			data: { user_id: memberLogin.user.id, role: 'member' }
		})
	);
	const settingsUrl = `/workspace/${workspace.id}/settings/flow`;
	const flowUrl = `/workspace/${workspace.id}/flow`;

	// ---- owner ----
	const ownerContext = await browser.newContext();
	const page = await ownerContext.newPage();
	await signInAs(page, ownerLogin);

	// Entry card on the workspace settings page leads here.
	await page.goto(`/workspace/${workspace.id}/settings`);
	await page.getByRole('button', { name: 'Open Flow settings' }).click();
	await expect(page).toHaveURL(new RegExp(`${settingsUrl}$`));
	await expect(page).toHaveTitle('Sylvode - Flow Settings');

	const sidebar = page.locator('aside nav');
	const status = page.getByTestId('flow-settings-status');
	await expect(page.getByTestId('flow-settings-enabled')).toHaveText('Disabled');
	await expect(sidebar.getByRole('link', { name: 'Flow settings' })).toBeVisible();
	await expect(sidebar.getByRole('link', { name: 'Flow', exact: true })).toHaveCount(0);
	const epochBefore = Number(await page.getByTestId('flow-settings-epoch').textContent());

	// Enable: no confirmation, entry appears, toast carries the event id.
	await page.getByRole('button', { name: 'Enable Flow' }).click();
	await expect(page.getByTestId('flow-settings-enabled')).toHaveText('Enabled');
	await expect(page.getByText(/^Saved\. Event [0-9a-f-]{36}$/).first()).toBeVisible();
	await expect(sidebar.getByRole('link', { name: 'Flow', exact: true })).toBeVisible();
	await expect(page.getByTestId('flow-settings-epoch')).toHaveText(String(epochBefore + 1));

	await sidebar.getByRole('link', { name: 'Flow', exact: true }).click();
	await expect(page).toHaveURL(new RegExp(`${flowUrl}$`));
	await expect(page.getByText('Flow is not enabled')).toHaveCount(0);

	// Disable: confirmation dialog, confirm is disabled until the user acknowledges.
	await page.goto(settingsUrl);
	await page.getByRole('button', { name: 'Disable Flow' }).click();
	const dialog = page.getByRole('dialog');
	await expect(dialog.getByText('Active collaboration connections are closed.')).toBeVisible();
	const confirmDisable = dialog.getByRole('button', { name: 'Disable Flow' });
	await expect(confirmDisable).toBeDisabled();
	const ack = dialog.getByRole('checkbox', { name: 'I understand the consequences of disabling Flow' });
	await expect(ack).not.toBeChecked();
	await ack.check();
	await confirmDisable.click();
	await expect(page.getByTestId('flow-settings-enabled')).toHaveText('Disabled');
	await expect(sidebar.getByRole('link', { name: 'Flow', exact: true })).toHaveCount(0);

	await page.goto(flowUrl);
	await expect(page.getByRole('heading', { name: 'Flow is not enabled' })).toBeVisible();
	await expect(sidebar.getByRole('link', { name: 'Flow', exact: true })).toHaveCount(0);

	// Baseline change: confirmation dialog, epoch advances.
	await page.goto(settingsUrl);
	const epochBeforeLevel = Number(await page.getByTestId('flow-settings-epoch').textContent());
	await page.getByLabel('Default member level').selectOption('comment');
	await page.getByRole('button', { name: 'Apply level' }).click();
	const levelDialog = page.getByRole('dialog');
	await expect(levelDialog.getByText(/from Edit to Comment/)).toBeVisible();
	const confirmLevel = levelDialog.getByRole('button', { name: 'Change level' });
	await expect(confirmLevel).toBeDisabled();
	await levelDialog.getByRole('checkbox').check();
	await confirmLevel.click();
	await expect(status.getByTestId('flow-settings-level')).toHaveText('Comment');
	await expect(page.getByTestId('flow-settings-epoch')).toHaveText(String(epochBeforeLevel + 1));

	// Re-enable: entry back, /flow reachable.
	await page.getByRole('button', { name: 'Enable Flow' }).click();
	await expect(page.getByTestId('flow-settings-enabled')).toHaveText('Enabled');
	await expect(sidebar.getByRole('link', { name: 'Flow', exact: true })).toBeVisible();
	await page.goto(flowUrl);
	await expect(page.getByRole('heading', { name: 'Flow is not enabled' })).toHaveCount(0);
	await expect(sidebar.getByRole('link', { name: 'Flow', exact: true })).toBeVisible();

	const server = await unwrap<{ flow_enabled: boolean; default_member_level: string }>(
		await request.get(`/api/v1/workspaces/${workspace.id}/features/flow`, { headers: ownerAuth })
	);
	expect(server.flow_enabled).toBe(true);
	expect(server.default_member_level).toBe('comment');
	await ownerContext.close();

	// ---- plain member ----
	const memberContext = await browser.newContext();
	const memberPage = await memberContext.newPage();
	await signInAs(memberPage, memberLogin);
	await memberPage.goto(`/workspace/${workspace.id}/projects`);
	const memberSidebar = memberPage.locator('aside nav');
	await expect(memberSidebar.getByRole('link', { name: 'Flow', exact: true })).toBeVisible();
	await expect(memberSidebar.getByRole('link', { name: 'Flow settings' })).toHaveCount(0);

	await memberPage.goto(`/workspace/${workspace.id}/settings`);
	await expect(memberPage.getByRole('button', { name: 'Open Flow settings' })).toHaveCount(0);

	await memberPage.goto(settingsUrl);
	await expect(memberPage.getByTestId('flow-settings-forbidden')).toBeVisible();
	await expect(memberPage.getByText('Flow settings are not available')).toBeVisible();
	await expect(memberPage.getByRole('button', { name: /Enable Flow|Disable Flow/ })).toHaveCount(0);
	await expect(memberPage.getByLabel('Default member level')).toHaveCount(0);

	// The server is the real guard.
	const refused = await envelope<null>(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: { Authorization: `Bearer ${memberLogin.tokens.access_token}` },
			data: { enabled: false, idempotency_key: `member-${runId}` }
		})
	);
	expect(refused.code).toBe(403);
	await memberContext.close();
});
