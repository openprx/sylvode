import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

/**
 * The webhook form offers every subscribable event, and a subscription to one of the events it
 * used to hide is stored as chosen.
 *
 * It creates its own workspace. The user is TEST_EMAIL / TEST_PASSWORD when both are set;
 * otherwise a fresh user is registered, which works on an instance whose first account has not
 * been created yet. The webhook URL must pass the API's outbound validation, so it points at a
 * public host by default (WEBHOOK_TARGET_URL overrides it).
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const email = process.env.TEST_EMAIL ?? `webhook-${runId}@e2e.sylvode.test`;
const password = process.env.TEST_PASSWORD ?? `Webhook-${runId}-Pass1!`;
const targetUrl = process.env.WEBHOOK_TARGET_URL ?? `https://example.com/sylvode-e2e/${runId}`;

type Envelope<T> = { code: number; message: string; data: T };
type LoginData = { tokens: { access_token: string; refresh_token: string }; user: unknown };
type Webhook = { id: string; url: string; events: string[] };

async function unwrap<T>(response: Awaited<ReturnType<APIRequestContext['get']>>): Promise<T> {
	expect(response.ok(), `${response.url()} answered HTTP ${response.status()}`).toBe(true);
	const body = (await response.json()) as Envelope<T>;
	expect(body.code, `${response.url()}: ${body.message}`).toBe(0);
	return body.data;
}

async function signIn(page: Page): Promise<LoginData> {
	if (!process.env.TEST_EMAIL || !process.env.TEST_PASSWORD) {
		await unwrap(await page.request.post('/api/v1/auth/register', { data: { email, password, name: 'Webhook E2E' } }));
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

test('the webhook form offers all 14 events and stores a sprint subscription', async ({ page }) => {
	const login = await signIn(page);
	const auth = { Authorization: `Bearer ${login.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await page.request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `webhook-${runId}`, name: `Webhook ${runId}` }
		})
	);

	await page.goto(`/workspace/${workspace.id}/webhooks`);
	await page.getByRole('button', { name: 'Create Webhook' }).first().click();
	const dialog = page.getByRole('dialog');
	const events = dialog.getByRole('group', { name: 'Event Types' });
	await expect(events.getByRole('checkbox')).toHaveCount(14);
	await expect(dialog.getByTestId('webhook-event-group')).toHaveCount(5);

	await dialog.getByLabel('URL').fill(targetUrl);
	await events.getByRole('checkbox', { name: /Work item created/ }).uncheck();
	await events.getByRole('checkbox', { name: /Sprint started/ }).check();
	await events.getByRole('checkbox', { name: /AI task failed/ }).check();
	await dialog.getByRole('button', { name: 'Create' }).click();
	await expect(page.getByRole('cell', { name: targetUrl })).toBeVisible();

	const hooks = await unwrap<Webhook[] | { items: Webhook[] }>(
		await page.request.get(`/api/v1/workspaces/${workspace.id}/webhooks`, { headers: auth })
	);
	const list = Array.isArray(hooks) ? hooks : hooks.items;
	const created = list.find((hook) => hook.url === targetUrl);
	expect(created?.events.sort()).toEqual(['ai.task_failed', 'sprint.started']);
});
