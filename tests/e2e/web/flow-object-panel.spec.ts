import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test, type APIRequestContext, type Page, type Request } from '@playwright/test';

/**
 * The Flow object page's right panel (`/workspace/{id}/flow/{objectId}`) against a real API +
 * PostgreSQL: Share, Relations, History semantic diff and object export.
 *
 * Over REST an owner creates a workspace with Flow enabled (member baseline `edit`), adds member
 * B, creates a Page and a target Page, links the Page to the target (`link` command) and inserts
 * one block (`insert_block`), so the Page has a relation and a content history entry.
 *
 * In the browser the owner:
 * - opens Share, sees full access and inheritance on, and gives B `view` (an upgrade-only change:
 *   one PUT grants, `dry_run:false`, no dialog);
 * - stops inheriting: the dialog shows the server's dry-run consequence for B (edit -> view), and
 *   confirming sends the real request under the dry run's key;
 * - opens Relations and sees the link to the target, which navigates there;
 * - opens History, expands the block entry and sees the semantic diff (added block text);
 * - exports the Page as Markdown, the job completes, and the downloaded file's SHA-256 equals the
 *   job checksum.
 *
 * B then opens the Page: Share shows only B's own level (view) and inheritance off, no roster and
 * no write controls, and a write over REST is refused.
 *
 * Needs BASE_URL serving the built frontend with `/api` proxied to the API on the same origin
 * (`scripts/e2e-web-stack.sh up`). Registration after the first account needs an instance admin:
 * ADMIN_EMAIL / ADMIN_PASSWORD when both are set, otherwise the run registers the first account.
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = {
	email: `panel-owner-${runId}@e2e.sylvode.test`,
	password: `PanelOwner-${runId}-1!`
};
const member = {
	email: `panel-member-${runId}@e2e.sylvode.test`,
	password: `PanelMember-${runId}-1!`
};
const MEMBER_NAME = `Panel Member ${runId}`;

type Envelope<T> = { code: number; message: string; data: T; error_code?: string };
type LoginData = { tokens: { access_token: string; refresh_token: string }; user: { id: string } };

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
	const admin = {
		email: `panel-admin-${runId}@e2e.sylvode.test`,
		password: `PanelAdmin-${runId}-1!`
	};
	await unwrap(
		await request.post('/api/v1/auth/register', { data: { ...admin, name: 'Panel Admin' } })
	);
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

function bodyOf(request: Request): Record<string, unknown> {
	return JSON.parse(request.postData() ?? '{}') as Record<string, unknown>;
}

test('object panel: share with dry-run consequences, relations, history diff and export', async ({
	browser,
	request
}) => {
	const admin = await instanceAdmin(request);
	const ownerLogin = await register(request, admin, owner, 'Panel Owner');
	const memberLogin = await register(request, admin, member, MEMBER_NAME);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const memberAuth = { Authorization: `Bearer ${memberLogin.tokens.access_token}` };

	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `pnl-${runId}`, name: `Panel ${runId}` }
		})
	);
	await unwrap(
		await request.post(`/api/v1/workspaces/${workspace.id}/members`, {
			headers: auth,
			data: { user_id: memberLogin.user.id, role: 'member' }
		})
	);
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: auth,
			data: { enabled: true, default_member_level: 'edit', idempotency_key: crypto.randomUUID() }
		})
	);
	const pageTitle = `Panel page ${runId}`;
	const targetTitle = `Linked target ${runId}`;
	const created = await unwrap<{ object: { id: string } }>(
		await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
			headers: auth,
			data: { object_type: 'page', title: pageTitle, idempotency_key: crypto.randomUUID() }
		})
	);
	const target = await unwrap<{ object: { id: string } }>(
		await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
			headers: auth,
			data: { object_type: 'page', title: targetTitle, idempotency_key: crypto.randomUUID() }
		})
	);
	const objectId = created.object.id;
	await unwrap(
		await request.post(`/api/v1/flow/objects/${objectId}/commands`, {
			headers: auth,
			data: {
				command: {
					type: 'link',
					payload: { target_object_id: target.object.id, relation_type: 'related' }
				},
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	const blockText = `Hello diff ${runId}`;
	await unwrap(
		await request.post(`/api/v1/flow/objects/${objectId}/commands`, {
			headers: auth,
			data: {
				command: { type: 'insert_block', payload: { block_id: `blk-${runId}`, text: blockText } },
				idempotency_key: crypto.randomUUID()
			}
		})
	);

	// A workspace bot the owner can name in a grant (the add picker lists bots too).
	const botName = `Panel bot ${runId}`;
	const bot = await unwrap<{ id: string }>(
		await request.post(`/api/v1/workspaces/${workspace.id}/bots`, {
			headers: auth,
			data: { name: botName, transport_surface: 'rest' }
		})
	);

	// ---- owner ---------------------------------------------------------------------------------
	const ownerContext = await browser.newContext({ acceptDownloads: true });
	const page = await ownerContext.newPage();
	await signInAs(page, ownerLogin);
	const puts: Array<{ path: string; body: Record<string, unknown> }> = [];
	page.on('request', (req) => {
		if (req.method() === 'PUT' && /\/flow\/objects\/[^/]+\/(grants|inheritance)$/.test(req.url())) {
			puts.push({ path: new URL(req.url()).pathname, body: bodyOf(req) });
		}
	});
	await page.goto(`/workspace/${workspace.id}/flow/${objectId}`);

	const tabs = page.getByRole('tablist', { name: 'Page panel sections' });
	await expect(tabs.getByRole('tab', { name: 'Info' })).toHaveAttribute('aria-selected', 'true');
	await tabs.getByRole('tab', { name: 'Info' }).focus();
	await page.keyboard.press('ArrowRight');
	const shareTab = tabs.getByRole('tab', { name: 'Share' });
	await expect(shareTab).toBeFocused();
	await expect(shareTab).toHaveAttribute('aria-selected', 'true');
	await expect(shareTab).toHaveAttribute('aria-controls', 'flow-panel-share');

	const share = page.getByTestId('flow-share-panel');
	await expect(share.getByTestId('flow-share-effective')).toHaveText('Full access');
	await expect(share.getByTestId('flow-share-inheritance')).toHaveText('On');
	await share
		.getByRole('combobox', { name: 'Person or bot', exact: true })
		.selectOption({ label: MEMBER_NAME });
	await share
		.getByRole('combobox', { name: 'Access', exact: true })
		.selectOption({ label: 'Can view' });
	await share.getByRole('button', { name: 'Add', exact: true }).click();
	await expect(share.getByTestId('flow-share-grant')).toHaveCount(1);
	await expect(share.getByTestId('flow-share-grant')).toContainText(MEMBER_NAME);
	await expect(share.getByRole('combobox', { name: `Access for ${MEMBER_NAME}` })).toHaveValue(
		'view'
	);
	expect(puts).toHaveLength(1);
	expect(puts[0].path).toBe(`/api/v1/flow/objects/${objectId}/grants`);
	expect(puts[0].body).toMatchObject({
		grants: [{ principal_kind: 'user', principal_id: memberLogin.user.id, level: 'view' }],
		dry_run: false,
		confirm_self_lockout: false
	});

	// Breaking inheritance lowers B (baseline edit -> explicit view): dry run, dialog, confirm.
	await share.getByRole('button', { name: 'Stop inheriting' }).click();
	const dialog = page.getByTestId('flow-share-confirm');
	await expect(page.getByRole('heading', { name: 'Review the access change' })).toBeVisible();
	await expect(dialog.getByTestId('flow-share-confirm-you')).toHaveText(
		'You: Full access → Full access'
	);
	await expect(dialog.getByTestId('flow-share-confirm-affected')).toHaveText(
		`${MEMBER_NAME}: Can edit → Can view`
	);
	expect(puts).toHaveLength(2);
	expect(puts[1].path).toBe(`/api/v1/flow/objects/${objectId}/inheritance`);
	expect(puts[1].body).toMatchObject({
		inherit_from_parent: false,
		dry_run: true,
		confirm_self_lockout: false
	});
	expect(puts[1].body).not.toHaveProperty('initial_grants');
	// The dry run changed nothing on the server.
	const during = await unwrap<{ inherit_from_parent: boolean }>(
		await request.get(`/api/v1/flow/objects/${objectId}/grants`, { headers: auth })
	);
	expect(during.inherit_from_parent).toBe(true);

	await page.getByRole('button', { name: 'Confirm change' }).click();
	await expect(share.getByTestId('flow-share-inheritance')).toHaveText('Off');
	await expect(share.getByTestId('flow-share-receipt')).toHaveText(
		'Last change affected 1 people.'
	);
	expect(puts).toHaveLength(3);
	expect(puts[2].body).toMatchObject({
		inherit_from_parent: false,
		dry_run: false,
		confirm_self_lockout: false
	});
	expect(puts[2].body.idempotency_key).toBe(puts[1].body.idempotency_key);
	const after = await unwrap<{
		inherit_from_parent: boolean;
		items: Array<{ principal_id: string; level: string }>;
	}>(await request.get(`/api/v1/flow/objects/${objectId}/grants`, { headers: auth }));
	expect(after.inherit_from_parent).toBe(false);
	expect(after.items.map((item) => [item.principal_id, item.level])).toEqual([
		[memberLogin.user.id, 'view']
	]);

	// The bot is offered under "Bots" and a grant to it is sent as a bot principal.
	const subject = share.getByRole('combobox', { name: 'Person or bot', exact: true });
	await expect(subject.locator('optgroup[label="Bots"] option')).toHaveText([botName]);
	await subject.selectOption({ label: botName });
	await share
		.getByRole('combobox', { name: 'Access', exact: true })
		.selectOption({ label: 'Can edit' });
	await share.getByRole('button', { name: 'Add', exact: true }).click();
	await expect(share.getByTestId('flow-share-grant')).toHaveCount(2);
	expect(puts).toHaveLength(4);
	expect(puts[3].body).toMatchObject({
		grants: [
			{ principal_kind: 'user', principal_id: memberLogin.user.id, level: 'view' },
			{ principal_kind: 'bot', principal_id: bot.id, level: 'edit' }
		],
		dry_run: false
	});
	const withBot = await unwrap<{
		items: Array<{ principal_kind: string; principal_id: string; level: string }>;
	}>(await request.get(`/api/v1/flow/objects/${objectId}/grants`, { headers: auth }));
	expect(
		withBot.items
			.map((item) => [item.principal_kind, item.principal_id, item.level])
			.sort((x, y) => x.join().localeCompare(y.join()))
	).toEqual(
		[
			['user', memberLogin.user.id, 'view'],
			['bot', bot.id, 'edit']
		].sort((x, y) => x.join().localeCompare(y.join()))
	);

	// Relations: lazy-loaded on first open, the link's target is shown and navigable.
	await tabs.getByRole('tab', { name: 'Relations' }).click();
	const relations = page.getByTestId('flow-relations-panel');
	const relationRow = relations.getByTestId('flow-relation-row');
	await expect(relationRow).toHaveCount(1);
	await expect(relationRow).toContainText('Links to · related');
	const targetLink = relationRow.getByRole('link', { name: targetTitle });
	await expect(targetLink).toHaveAttribute(
		'href',
		`/workspace/${workspace.id}/flow/${target.object.id}`
	);
	await expect(relations.getByTestId('flow-relation-unavailable')).toHaveCount(0);

	// History: the newest entry is the inserted block; its semantic diff shows the added text.
	await tabs.getByRole('tab', { name: 'History' }).click();
	const firstEntry = page.getByTestId('flow-history-entry').first();
	const showChanges = firstEntry.getByRole('button', { name: 'Show changes' });
	await expect(showChanges).toHaveAttribute('aria-expanded', 'false');
	await showChanges.click();
	await expect(firstEntry.getByRole('button', { name: 'Hide changes' })).toHaveAttribute(
		'aria-expanded',
		'true'
	);
	const addedLine = firstEntry.locator('[data-testid="flow-history-diff-line"][data-op="added"]');
	await expect(addedLine).toHaveCount(1);
	await expect(addedLine).toContainText('Added');
	await expect(addedLine).toContainText(blockText);
	await expect(firstEntry.getByTestId('flow-history-diff')).not.toContainText(`blk-${runId}`);

	// Export: markdown, poll to completed, download and check the bytes' SHA-256.
	await tabs.getByRole('tab', { name: 'Info' }).click();
	const exportBox = page.getByTestId('flow-panel-export');
	await expect(exportBox.getByRole('combobox', { name: 'Format', exact: true })).toHaveValue(
		'markdown'
	);
	await expect(
		exportBox.getByRole('combobox', { name: 'Format', exact: true }).locator('option')
	).toHaveText(['JSON', 'Markdown', 'Sylvode package']);
	await expect(exportBox.getByLabel('Include history')).toBeDisabled();
	const exportResponse = page.waitForResponse(
		(res) =>
			res.request().method() === 'POST' &&
			res.url().endsWith(`/api/v1/flow/objects/${objectId}/exports`)
	);
	await exportBox.getByRole('button', { name: 'Export', exact: true }).click();
	const exportReply = await exportResponse;
	const exportBody = bodyOf(exportReply.request());
	expect(exportBody).toMatchObject({ format: 'markdown', include_history: false });
	expect(typeof exportBody.idempotency_key).toBe('string');
	const exportJob = ((await exportReply.json()) as Envelope<{ job_id: string }>).data;
	await expect(exportBox.getByTestId('flow-panel-export-status')).toHaveText('Completed');
	const downloadPromise = page.waitForEvent('download');
	await exportBox.getByRole('button', { name: 'Download' }).click();
	const download = await downloadPromise;
	expect(download.suggestedFilename()).toMatch(
		new RegExp(`-${exportJob.job_id.slice(0, 8)}\\.md$`)
	);
	const savedPath = join(tmpdir(), `${runId}-${download.suggestedFilename()}`);
	await download.saveAs(savedPath);
	const saved = readFileSync(savedPath);
	expect(saved.toString('utf8')).toContain(pageTitle);
	const job = await unwrap<{ status: string; checksum: string; format: string }>(
		await request.get(`/api/v1/flow/exports/${exportJob.job_id}`, { headers: auth })
	);
	expect(job.status).toBe('completed');
	expect(job.format).toBe('markdown');
	expect(createHash('sha256').update(saved).digest('hex')).toBe(job.checksum);

	await ownerContext.close();

	// ---- member B: read-only -------------------------------------------------------------------
	const memberContext = await browser.newContext();
	const memberPage = await memberContext.newPage();
	await signInAs(memberPage, memberLogin);
	const memberPuts: string[] = [];
	memberPage.on('request', (req) => {
		if (req.method() === 'PUT') memberPuts.push(req.url());
	});
	await memberPage.goto(`/workspace/${workspace.id}/flow/${objectId}`);
	await memberPage
		.getByRole('tablist', { name: 'Page panel sections' })
		.getByRole('tab', { name: 'Share' })
		.click();
	const memberShare = memberPage.getByTestId('flow-share-panel');
	await expect(memberShare.getByTestId('flow-share-effective')).toHaveText('Can view');
	await expect(memberShare.getByTestId('flow-share-inheritance')).toHaveText('Off');
	await expect(memberShare.getByTestId('flow-share-restricted')).toBeVisible();
	await expect(memberShare.getByTestId('flow-share-grant')).toHaveCount(0);
	await expect(memberShare.getByRole('button')).toHaveCount(0);
	await expect(memberShare.getByRole('combobox')).toHaveCount(0);
	expect(memberPuts).toEqual([]);
	const memberView = await unwrap<{
		items: unknown[];
		inherited: unknown[];
		effective_level: string;
	}>(await request.get(`/api/v1/flow/objects/${objectId}/grants`, { headers: memberAuth }));
	expect(memberView).toMatchObject({ items: [], inherited: [], effective_level: 'view' });
	const write = await request.post(`/api/v1/flow/objects/${objectId}/commands`, {
		headers: memberAuth,
		data: {
			command: { type: 'set_title', payload: { title: 'member edit' } },
			idempotency_key: crypto.randomUUID()
		}
	});
	const writeBody = (await write.json()) as Envelope<unknown>;
	expect(writeBody.code).not.toBe(0);
	expect(writeBody.error_code).toBe('policy_rejected');
	await memberContext.close();
});
