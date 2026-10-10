import { expect, test, type APIRequestContext, type Page, type Request } from '@playwright/test';

/**
 * Workspace Flow operations panel (`/workspace/{id}/settings/flow/operations`) against a real
 * API + PostgreSQL.
 *
 * The owner creates a workspace with one Flow page over REST, enters the panel from the Flow
 * settings page and checks: health shows the server's real numbers (storage bytes equal a direct
 * GET) with an un-alerted dead-letter card; the lag badge and per-object rows include the page;
 * the integrity table lists the page's document. Verify returns a dry-run receipt. Compact and
 * rebuild-projection each return a dry-run receipt, keep execute disabled until the exact target
 * id is typed (a wrong id keeps it disabled), then execute -- the wire log proves the execute
 * carries `dry_run:false`, the typed id, the dry-run's head and a key different from the
 * dry-run's. Delivery replay over the last hour previews 0 deliveries and executes after the
 * acknowledgement; a window starting 31 days ago is refused locally without a request.
 *
 * Needs BASE_URL to serve the built frontend with `/api` proxied to the API on the same origin
 * (as `frontend/nginx.conf` does). Registration after the first account needs an instance admin:
 * ADMIN_EMAIL / ADMIN_PASSWORD when both are set, otherwise the run registers the instance's
 * first account itself (a fresh database).
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = {
	email: `ops-owner-${runId}@e2e.sylvode.test`,
	password: `OpsOwner-${runId}-1!`
};

type Envelope<T> = { code: number; message: string; data: T; error_code?: string };
type LoginData = {
	tokens: { access_token: string; refresh_token: string };
	user: { id: string };
};

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
		return login(request, { email: process.env.ADMIN_EMAIL, password: process.env.ADMIN_PASSWORD });
	}
	const admin = { email: `ops-admin-${runId}@e2e.sylvode.test`, password: `OpsAdmin-${runId}-1!` };
	await unwrap(
		await request.post('/api/v1/auth/register', { data: { ...admin, name: 'Ops Admin' } })
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

function localInput(ms: number): string {
	const date = new Date(ms);
	const pad = (value: number) => String(value).padStart(2, '0');
	return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

test('admin reads health/lag/integrity and runs verify, compact, rebuild and replay through dry-run gates', async ({
	page,
	request
}) => {
	const admin = await instanceAdmin(request);
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...owner, name: 'Ops Owner' }
		})
	);
	const ownerLogin = await login(request, owner);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `ops-${runId}`, name: `Ops ${runId}` }
		})
	);
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: auth,
			data: { enabled: true, idempotency_key: crypto.randomUUID() }
		})
	);
	const created = await unwrap<{ object: { id: string; document_id: string } }>(
		await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
			headers: auth,
			data: {
				object_type: 'page',
				title: `Ops page ${runId}`,
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	const objectId = created.object.id;
	const documentId = created.object.document_id;
	const health = await unwrap<{ storage_bytes: number; connections: number }>(
		await request.get(`/api/v1/admin/workspaces/${workspace.id}/flow/health`, { headers: auth })
	);
	expect(health.storage_bytes).toBeGreaterThan(0);

	const writes: Array<{ path: string; body: Record<string, unknown> }> = [];
	page.on('request', (req: Request) => {
		if (req.method() !== 'POST' || !req.url().includes('/api/v1/admin/')) return;
		writes.push({
			path: new URL(req.url()).pathname,
			body: req.postDataJSON() as Record<string, unknown>
		});
	});

	await signInAs(page, ownerLogin);
	await page.goto(`/workspace/${workspace.id}/settings/flow`);
	await page.getByRole('link', { name: 'Open operations' }).click();
	await expect(page).toHaveURL(new RegExp(`/workspace/${workspace.id}/settings/flow/operations$`));
	await expect(page).toHaveTitle('Sylvode - Flow Operations');

	// ---- health ----
	const healthCard = page.getByTestId('flow-ops-health');
	await expect(healthCard.getByTestId('flow-ops-health-status')).toHaveText('Healthy');
	await expect(healthCard.getByTestId('flow-ops-health-storage')).toHaveText(
		`${health.storage_bytes} bytes`
	);
	await expect(healthCard.getByTestId('flow-ops-dead-letter')).toHaveAttribute(
		'data-alert',
		'false'
	);
	await expect(healthCard.getByTestId('flow-ops-dead-letter-total')).toHaveText('0');
	await expect(healthCard.getByTestId('flow-ops-cancelled-count')).toHaveText('0');
	await expect(healthCard.getByTestId('flow-ops-cancelled')).toContainText(
		'Not counted as dead-letter'
	);
	await healthCard.getByRole('button', { name: 'Pause auto-refresh' }).click();
	await expect(healthCard).toContainText('Auto-refresh is paused.');
	await healthCard.getByRole('button', { name: 'Resume auto-refresh' }).click();
	await expect(healthCard).toContainText('Refreshes every 15 seconds.');

	// ---- lag ----
	const lag = page.getByTestId('flow-ops-lag');
	await expect(lag.getByTestId('flow-ops-lag-badge')).toBeVisible();
	await expect(lag.getByTestId('flow-ops-lag-projection-max')).toHaveText(/^\d+$/);
	await expect(lag.getByTestId('flow-ops-lag-items')).toContainText(objectId);

	// ---- integrity ----
	const integrity = page.getByTestId('flow-ops-integrity');
	await expect(integrity.getByTestId('flow-ops-integrity-status')).toHaveText('Healthy');
	const row = integrity.locator(`tr[data-document-id="${documentId}"]`);
	await expect(row).toBeVisible();
	await expect(row).toContainText(objectId);

	const drawer = page.getByTestId('flow-ops-drawer');

	// ---- verify (dry-run only) ----
	await row.getByRole('button', { name: 'Verify' }).click();
	await expect(drawer).toHaveAttribute('data-kind', 'verify');
	await expect(drawer.getByTestId('flow-ops-drawer-target')).toHaveText(documentId);
	await drawer.getByRole('button', { name: 'Run verify' }).click();
	await expect(drawer.getByTestId('flow-ops-preview-mode')).toHaveText('Dry run (no changes)');
	await expect(drawer).toContainText('Preview only. Nothing was changed.');
	await expect(drawer.getByTestId('flow-ops-drawer-execute')).toHaveCount(0);
	await page.getByRole('dialog').getByRole('button', { name: 'Close', exact: true }).click();
	await expect(drawer).toHaveCount(0);

	// ---- compact: dry-run -> wrong id disabled -> exact id -> execute ----
	await row.getByRole('button', { name: 'Compact' }).click();
	await expect(drawer).toHaveAttribute('data-kind', 'compact');
	await expect(drawer.getByLabel('Expected head seq')).toHaveValue(/^\d+$/);
	const head = Number(await drawer.getByLabel('Expected head seq').inputValue());
	await drawer.getByRole('button', { name: 'Run dry run' }).click();
	await expect(drawer.getByTestId('flow-ops-preview-mode')).toHaveText('Dry run (no changes)');
	const execute = drawer.getByTestId('flow-ops-drawer-execute');
	const confirm = drawer.getByLabel('Type the document id to confirm');
	await expect(confirm).toHaveValue('');
	await expect(execute).toBeDisabled();
	await confirm.fill(objectId);
	await expect(drawer).toContainText('This id does not match the target.');
	await expect(execute).toBeDisabled();
	await confirm.fill(documentId);
	await expect(execute).toBeEnabled();
	await execute.click();
	await expect(drawer.getByTestId('flow-ops-executed-mode')).toHaveText('Executed');
	await expect(drawer.getByTestId('flow-ops-executed')).toContainText('Snapshot checksum');
	await page.getByRole('dialog').getByRole('button', { name: 'Close', exact: true }).click();

	const compactWrites = writes.filter((write) =>
		write.path.endsWith(`/documents/${documentId}/compact`)
	);
	expect(compactWrites).toHaveLength(2);
	expect(compactWrites[0].body).toMatchObject({ dry_run: true, expected_head_seq: head });
	expect(compactWrites[0].body).not.toHaveProperty('confirm_document_id');
	expect(compactWrites[1].body).toMatchObject({
		dry_run: false,
		expected_head_seq: head,
		confirm_document_id: documentId
	});
	expect(compactWrites[1].body.idempotency_key).not.toBe(compactWrites[0].body.idempotency_key);
	const verifyWrites = writes.filter((write) =>
		write.path.endsWith(`/documents/${documentId}/verify`)
	);
	expect(verifyWrites.map((write) => write.body.dry_run)).toEqual([true]);

	// ---- rebuild projection from the same row ----
	await row.getByRole('button', { name: 'Rebuild projection' }).click();
	await expect(drawer).toHaveAttribute('data-kind', 'rebuild');
	await expect(drawer.getByTestId('flow-ops-drawer-target')).toHaveText(objectId);
	await drawer.getByRole('button', { name: 'Run dry run' }).click();
	await expect(drawer.getByTestId('flow-ops-preview')).toContainText('Projection hash before');
	const rebuildExecute = drawer.getByTestId('flow-ops-drawer-execute');
	await expect(rebuildExecute).toBeDisabled();
	await drawer.getByLabel('Type the object id to confirm').fill(documentId);
	await expect(rebuildExecute).toBeDisabled();
	await drawer.getByLabel('Type the object id to confirm').fill(objectId);
	await rebuildExecute.click();
	await expect(drawer.getByTestId('flow-ops-executed-mode')).toHaveText('Executed');
	await expect(drawer.getByTestId('flow-ops-executed')).toContainText('Projection hash after');
	await page.getByRole('dialog').getByRole('button', { name: 'Close', exact: true }).click();
	const rebuildWrites = writes.filter((write) =>
		write.path.endsWith(`/objects/${objectId}/rebuild-projection`)
	);
	expect(rebuildWrites.map((write) => [write.body.dry_run, write.body.confirm_object_id])).toEqual([
		[true, undefined],
		[false, objectId]
	]);

	// ---- delivery replay over the past hour ----
	const replay = page.getByTestId('flow-ops-replay');
	const now = Date.now();
	await replay.getByLabel('From (inclusive)').fill(localInput(now - 3_600_000));
	await replay.getByLabel('To (exclusive)').fill(localInput(now - 60_000));
	await replay.getByRole('button', { name: 'Run dry run' }).click();
	await expect(replay.getByTestId('flow-ops-replay-preview')).toHaveAttribute(
		'data-mode',
		'rebuild'
	);
	await expect(replay.getByTestId('flow-ops-replay-preview-count')).toHaveText('0');
	const runReplay = replay.getByRole('button', { name: 'Run replay' });
	await expect(runReplay).toBeDisabled();
	await replay
		.getByLabel('I reviewed this summary and want to run the replay for this exact window.')
		.check();
	await expect(runReplay).toBeEnabled();
	await runReplay.click();
	await expect(replay.getByTestId('flow-ops-replay-result-count')).toHaveText('0');
	const replayWrites = writes.filter((write) => write.path.endsWith('/flow/deliveries/replay'));
	expect(replayWrites.map((write) => [write.body.dry_run, write.body.confirm])).toEqual([
		[true, true],
		[false, true]
	]);
	expect(replayWrites[1].body.idempotency_key).not.toBe(replayWrites[0].body.idempotency_key);
	expect(replayWrites[1].body.from).toBe(replayWrites[0].body.from);
	expect(replayWrites[1].body.to).toBe(replayWrites[0].body.to);

	// ---- a window older than replay_max_window_days is refused locally ----
	await replay.getByLabel('From (inclusive)').fill(localInput(now - 31 * 86_400_000));
	await replay.getByRole('button', { name: 'Run dry run' }).click();
	await expect(replay.getByTestId('flow-ops-replay-invalid')).toHaveText(
		'The window must start less than 30 days ago.'
	);
	expect(writes.filter((write) => write.path.endsWith('/flow/deliveries/replay'))).toHaveLength(2);

	// ---- Flow disabled: the panel stays usable (admin endpoints work; projection-lag is refused) ----
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: auth,
			data: { enabled: false, idempotency_key: crypto.randomUUID() }
		})
	);
	await page.goto(`/workspace/${workspace.id}/settings/flow/operations`);
	await expect(page.getByTestId('flow-ops-health-status')).toHaveText('Healthy');
	await expect(page.getByTestId('flow-ops-health-connections')).toHaveText(/^\d+$/);
	await expect(page.getByTestId('flow-ops-health-storage')).toHaveText(/\d/);
	await expect(page.getByTestId('flow-operations-forbidden')).toHaveCount(0);
	await expect(page.getByTestId('flow-ops-lag-projection-max')).toHaveText(/^\d+$/);
	await expect(page.getByTestId('flow-ops-projection-unavailable')).toHaveText(
		'Flow is off for this workspace, so there is no per-object lag to show.'
	);
	await expect(page.getByTestId('flow-ops-integrity-status')).toHaveText('Healthy');
});
