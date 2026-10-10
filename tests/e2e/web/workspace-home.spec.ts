import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

/**
 * Workspace home (`/workspace/{id}`, task FP-N6) against a real API + PostgreSQL.
 *
 * The owner reaches the home by clicking the workspace on `/workspace`; the page renders (no 404)
 * with the `Sylvode - <name>` title, the slug, the role badge (the API has no workspace
 * description field, so the optional description line never renders here), the project total with the five
 * most recent projects, the Flow card in its "enable" form while Flow is off, and the four admin
 * links. A plain member sees neither the admin card nor, while Flow is off, the Flow card; once
 * Flow is on both viewers get the "Open Flow" link. An empty workspace shows the empty state
 * whose action leads to the project list, an unknown workspace id shows the not-found state, and
 * `/workspace/{id}/connections` carries the `Sylvode - ` title prefix.
 *
 * Needs BASE_URL to serve the built frontend with `/api` proxied to the API on the same origin
 * (as `frontend/nginx.conf` does). Registration after the first account needs an instance admin:
 * ADMIN_EMAIL / ADMIN_PASSWORD when both are set, otherwise the run registers the instance's
 * first account itself (a fresh database). The owner and the member are registered fresh per run.
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = { email: `home-owner-${runId}@e2e.sylvode.test`, password: `HomeOwner-${runId}-1!` };
const member = { email: `home-member-${runId}@e2e.sylvode.test`, password: `HomeMember-${runId}-1!` };

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
	const admin = { email: `home-admin-${runId}@e2e.sylvode.test`, password: `HomeAdmin-${runId}-1!` };
	await unwrap(await request.post('/api/v1/auth/register', { data: { ...admin, name: 'Home Admin' } }));
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

const ADMIN_LINKS = ['Members', 'Webhook', 'Operation Records', 'Workspace Settings'];
const SIDEBAR_ADMIN_LINKS = [...ADMIN_LINKS, 'Flow settings'];

test('workspace home: entry from the list, cards by role and Flow flag, titles', async ({ browser, request }) => {
	const admin = await instanceAdmin(request);
	const ownerLogin = await register(request, admin, owner, 'Home Owner');
	const memberLogin = await register(request, admin, member, 'Home Member');
	const ownerAuth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspaceName = `Home ${runId}`;
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: ownerAuth,
			data: { slug: `home-${runId}`, name: workspaceName }
		})
	);
	const emptyWorkspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: ownerAuth,
			data: { slug: `home-empty-${runId}`, name: `Empty ${runId}` }
		})
	);
	await unwrap(
		await request.post(`/api/v1/workspaces/${workspace.id}/members`, {
			headers: ownerAuth,
			data: { user_id: memberLogin.user.id, role: 'member' }
		})
	);
	const projectIds: string[] = [];
	for (let i = 1; i <= 6; i += 1) {
		const created = await unwrap<{ id: string }>(
			await request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
				headers: ownerAuth,
				data: { key: `HP${i}`, name: `Home project ${i}` }
			})
		);
		projectIds.push(created.id);
	}
	const homeUrl = `/workspace/${workspace.id}`;

	// ---- owner, Flow off ----
	const ownerContext = await browser.newContext();
	const page = await ownerContext.newPage();
	await signInAs(page, ownerLogin);

	await page.goto('/workspace');
	await page.getByRole('button', { name: new RegExp(workspaceName) }).first().click();
	await expect(page).toHaveURL(new RegExp(`${homeUrl}$`));
	await expect(page).toHaveTitle(`Sylvode - ${workspaceName}`);
	await expect(page.getByText('404')).toHaveCount(0);
	await expect(page.getByTestId('workspace-home-name')).toHaveText(workspaceName);
	await expect(page.getByTestId('workspace-home-slug')).toHaveText(`home-${runId}`);
	await expect(page.getByTestId('workspace-home-role')).toContainText('Owner');
	// Entered from the list with no reload: the sidebar reads the role for this workspace now.
	const sidebar = page.locator('aside');
	for (const label of SIDEBAR_ADMIN_LINKS) {
		await expect(sidebar.getByRole('link', { name: label, exact: true })).toBeVisible();
	}

	await expect(page.getByTestId('workspace-home-project-total')).toHaveText('Total: 6');
	const recent = page.getByTestId('workspace-home-recent').getByRole('link');
	await expect(recent).toHaveCount(5);
	const firstHref = await recent.first().getAttribute('href');
	expect(projectIds.some((id) => firstHref === `/workspace/${workspace.id}/projects/${id}`)).toBe(true);

	const adminCard = page.getByTestId('workspace-home-admin');
	await expect(adminCard).toBeVisible();
	for (const label of ADMIN_LINKS) {
		await expect(adminCard.getByRole('link', { name: label, exact: true })).toBeVisible();
	}
	await expect(adminCard.getByRole('link')).toHaveCount(4);

	const flowCard = page.getByTestId('workspace-home-flow');
	await expect(flowCard.getByRole('link', { name: 'Enable in Flow settings' })).toHaveAttribute(
		'href',
		`/workspace/${workspace.id}/settings/flow`
	);
	await expect(flowCard.getByRole('link', { name: 'Open Flow' })).toHaveCount(0);

	await page.getByTestId('workspace-home-projects').getByRole('link', { name: 'All projects' }).click();
	await expect(page).toHaveURL(new RegExp(`${homeUrl}/projects$`));

	await page.goto(homeUrl);
	await page.getByTestId('workspace-home-admin').getByRole('link', { name: 'Operation Records' }).click();
	await expect(page).toHaveURL(new RegExp(`${homeUrl}/connections$`));
	await expect(page).toHaveTitle('Sylvode - Operation Records');

	// ---- the owner moves client-side to a workspace where it is only a member ----
	const memberAuth = { Authorization: `Bearer ${memberLogin.tokens.access_token}` };
	const foreign = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: memberAuth,
			data: { slug: `home-foreign-${runId}`, name: `Foreign ${runId}` }
		})
	);
	await unwrap(
		await request.post(`/api/v1/workspaces/${foreign.id}/members`, {
			headers: memberAuth,
			data: { user_id: ownerLogin.user.id, role: 'member' }
		})
	);
	await page.goto(homeUrl);
	await expect(page.locator('aside').getByRole('link', { name: 'Members', exact: true })).toBeVisible();
	// Client-side only (no reload): sidebar link to the list, then the foreign workspace.
	await page.locator('aside a[href="/workspace"]').click();
	await expect(page).toHaveURL(/\/workspace$/);
	await page.getByRole('button', { name: new RegExp(`Foreign ${runId}`) }).first().click();
	await expect(page).toHaveURL(new RegExp(`/workspace/${foreign.id}$`));
	await expect(page.getByTestId('workspace-home-role')).toContainText('Member');
	for (const label of SIDEBAR_ADMIN_LINKS) {
		await expect(page.locator('aside').getByRole('link', { name: label, exact: true })).toHaveCount(0);
	}

	// ---- an instance admin that is not a member gets no workspace-admin links ----
	const adminContext = await browser.newContext();
	const adminPage = await adminContext.newPage();
	await signInAs(adminPage, admin);
	await adminPage.goto(homeUrl);
	await expect(adminPage.locator('aside').getByRole('link', { name: 'Projects', exact: true })).toBeVisible();
	for (const label of SIDEBAR_ADMIN_LINKS) {
		await expect(adminPage.locator('aside').getByRole('link', { name: label, exact: true })).toHaveCount(0);
	}
	await adminContext.close();

	// ---- member, Flow off ----
	const memberContext = await browser.newContext();
	const memberPage = await memberContext.newPage();
	await signInAs(memberPage, memberLogin);
	await memberPage.goto(homeUrl);
	await expect(memberPage).toHaveTitle(`Sylvode - ${workspaceName}`);
	await expect(memberPage.getByTestId('workspace-home-role')).toContainText('Member');
	await expect(memberPage.getByTestId('workspace-home-recent').getByRole('link')).toHaveCount(5);
	await expect(memberPage.getByTestId('workspace-home-admin')).toHaveCount(0);
	await expect(memberPage.getByTestId('workspace-home-flow')).toHaveCount(0);
	for (const label of ADMIN_LINKS) {
		await expect(memberPage.getByRole('main').getByRole('link', { name: label, exact: true })).toHaveCount(0);
	}

	// ---- Flow on: both viewers get the Open Flow link ----
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: ownerAuth,
			data: { enabled: true, idempotency_key: crypto.randomUUID() }
		})
	);
	await memberPage.goto(homeUrl);
	await expect(memberPage.getByTestId('workspace-home-flow').getByRole('link', { name: 'Open Flow' })).toHaveAttribute(
		'href',
		`/workspace/${workspace.id}/flow`
	);
	await expect(memberPage.getByTestId('workspace-home-admin')).toHaveCount(0);

	await page.goto(homeUrl);
	await page.getByTestId('workspace-home-flow').getByRole('link', { name: 'Open Flow' }).click();
	await expect(page).toHaveURL(new RegExp(`${homeUrl}/flow$`));
	await expect(page).toHaveTitle('Sylvode - Flow');
	await expect(page.getByText('Flow is not enabled')).toHaveCount(0);

	// ---- empty workspace and unknown workspace ----
	await page.goto(`/workspace/${emptyWorkspace.id}`);
	await expect(page).toHaveTitle(`Sylvode - Empty ${runId}`);
	const empty = page.getByTestId('workspace-home-empty');
	await expect(empty.getByText('No projects yet')).toBeVisible();
	await empty.getByRole('link', { name: 'New project' }).click();
	await expect(page).toHaveURL(new RegExp(`/workspace/${emptyWorkspace.id}/projects$`));

	await page.goto('/workspace/00000000-0000-4000-8000-000000000000');
	await expect(page.getByTestId('workspace-home-not-found')).toBeVisible();
	await expect(page).toHaveTitle('Sylvode - Workspace Home');

	await memberContext.close();
	await ownerContext.close();
});
