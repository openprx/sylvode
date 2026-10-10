import { expect, test, type APIRequestContext, type Page, type Request } from '@playwright/test';

/**
 * Page slash menu against a real API + PostgreSQL: the Collection embed and the Forms Bridge
 * Reference / Embed / Convert entries (`ui-surface-v1.md` v0.6 row; `versions/v0.7-forms-bridge.md`
 * "Web 交互").
 *
 * Over REST the owner creates a workspace with Flow enabled, a project, a form with one text
 * field and one record, a second form, and one Page.
 * In the browser:
 *
 * 1. `/` -> Collection: the first `create_collection_embed` is failed by the network (the
 *    provisional block disappears, an error with Retry is shown), Retry resends the same body and
 *    key, and the embedded table appears with an Open link. Exactly two command requests were
 *    sent, both `create_collection_embed`, and no `POST .../flow/objects`. The navigator lists the
 *    new Collection under the Page, the API reports it as the Page's child, and a reload still
 *    shows the embed (it is a server-written block in the Page document).
 * 2. `/` -> Reference a Form: project -> form -> record, the request carries
 *    `display.mode=reference`, a card with the record title and a permission badge appears;
 *    removing it needs a confirmation, sends only `DELETE .../references/{id}`, and the record
 *    itself is still there afterwards.
 * 3. `/` -> Embed a Form: the card shows the record's field values. A second Embed (the whole
 *    second form) then loses its viewing right through a Forms policy change; after a reload the
 *    bridge omits it, the Page placeholder shows as unavailable, and the browser never requests
 *    that form's data.
 * 4. `/` -> Convert to Forms…: lands on the conversion wizard.
 *
 * Needs BASE_URL to serve the built frontend with `/api` proxied to the API on the same origin.
 * Registration after the first account needs an instance admin: ADMIN_EMAIL / ADMIN_PASSWORD
 * when both are set, otherwise the run registers the instance's first account itself.
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = {
	email: `slash-owner-${runId}@e2e.sylvode.test`,
	password: `SlashOwner-${runId}-1!`
};

type Envelope<T> = {
	code: number;
	message: string;
	data: T;
	error_code?: string;
};
type LoginData = {
	tokens: { access_token: string; refresh_token: string };
	user: { id: string };
};
type ObjectView = {
	id: string;
	title: string;
	object_type: string;
	parent_id: string | null;
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
		return login(request, {
			email: process.env.ADMIN_EMAIL,
			password: process.env.ADMIN_PASSWORD
		});
	}
	const admin = {
		email: `slash-admin-${runId}@e2e.sylvode.test`,
		password: `SlashAdmin-${runId}-1!`
	};
	await unwrap(
		await request.post('/api/v1/auth/register', {
			data: { ...admin, name: 'Slash Admin' }
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

interface Fixture {
	ownerLogin: LoginData;
	auth: Record<string, string>;
	workspaceId: string;
	projectId: string;
	formId: string;
	recordId: string;
	recordTitle: string;
	hiddenFormId: string;
}

let fixture: Fixture | null = null;

async function setup(request: APIRequestContext): Promise<Fixture> {
	if (fixture) return fixture;
	const admin = await instanceAdmin(request);
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...owner, name: 'Slash Owner' }
		})
	);
	const ownerLogin = await login(request, owner);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `slash-${runId}`, name: `Slash ${runId}` }
		})
	);
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: auth,
			data: { enabled: true, idempotency_key: crypto.randomUUID() }
		})
	);
	const project = await unwrap<{ id: string }>(
		await request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
			headers: auth,
			data: { name: `Slash project ${runId}`, key: 'SLB' }
		})
	);
	const schema = {
		version: 'openpr.form.schema.v1',
		fields: [{ field_id: 'fld_note', key: 'note', label: 'Note', type: 'text' }]
	};
	const form = await unwrap<{ id: string }>(
		await request.post(`/api/v1/projects/${project.id}/forms`, {
			headers: auth,
			data: { key: 'plans', name: `Plans ${runId}`, schema }
		})
	);
	const recordTitle = `Launch record ${runId}`;
	const record = await unwrap<{ id: string }>(
		await request.post(`/api/v1/forms/${form.id}/records`, {
			headers: auth,
			data: { title: recordTitle, values: { note: `Bridge note ${runId}` } }
		})
	);
	const hidden = await unwrap<{ id: string }>(
		await request.post(`/api/v1/projects/${project.id}/forms`, {
			headers: auth,
			data: { key: 'hidden', name: `Hidden ${runId}`, schema }
		})
	);
	fixture = {
		ownerLogin,
		auth,
		workspaceId: workspace.id,
		projectId: project.id,
		formId: form.id,
		recordId: record.id,
		recordTitle,
		hiddenFormId: hidden.id
	};
	return fixture;
}

async function newPage(request: APIRequestContext, f: Fixture, title: string): Promise<string> {
	const created = await unwrap<{ object: { id: string } }>(
		await request.post(`/api/v1/workspaces/${f.workspaceId}/flow/objects`, {
			headers: f.auth,
			data: {
				object_type: 'page',
				title,
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	return created.object.id;
}

async function openSlashMenu(page: Page): Promise<void> {
	const editor = page.locator('.flow-canvas .ProseMirror');
	await expect(editor).toBeVisible();
	await editor.click();
	await page.keyboard.type('/');
	await expect(page.getByRole('listbox')).toBeVisible();
}

test('slash Collection: one atomic command, failure drops the provisional block, retry reuses the key', async ({
	page,
	request
}) => {
	const f = await setup(request);
	const pageTitle = `Embed host ${runId}`;
	const objectId = await newPage(request, f, pageTitle);
	const commandBodies: Array<Record<string, unknown>> = [];
	const objectCreates: string[] = [];
	page.on('request', (req: Request) => {
		if (req.method() !== 'POST') return;
		const path = new URL(req.url()).pathname;
		if (path === `/api/v1/flow/objects/${objectId}/commands`)
			commandBodies.push(req.postDataJSON() as Record<string, unknown>);
		if (path === `/api/v1/workspaces/${f.workspaceId}/flow/objects`) objectCreates.push(path);
	});

	await signInAs(page, f.ownerLogin);
	await page.goto(`/workspace/${f.workspaceId}/flow/${objectId}`);
	await openSlashMenu(page);
	const option = page.getByTestId('flow-slash-collection');
	await expect(option).toContainText('Collection');
	await expect(option).toContainText('belongs to this Page');
	await option.click();

	const dialog = page.getByTestId('flow-slash-collection-dialog');
	await expect(dialog).toBeVisible();
	const titleInput = page.getByTestId('flow-slash-collection-title');
	await expect(titleInput).toHaveValue('Untitled collection');
	const collectionTitle = `Roadmap ${runId}`;
	await titleInput.fill(collectionTitle);

	// The first attempt dies in the network: nothing reaches the server.
	let failNext = true;
	await page.route(`**/api/v1/flow/objects/${objectId}/commands`, async (route) => {
		if (failNext) {
			failNext = false;
			await new Promise((resolveLater) => setTimeout(resolveLater, 300));
			await route.abort('connectionreset');
			return;
		}
		await route.continue();
	});
	await page.getByTestId('flow-slash-collection-submit').click();
	await expect(page.getByTestId('flow-embed-provisional')).toHaveCount(1);
	await expect(page.getByTestId('flow-embed-provisional')).toContainText(collectionTitle);
	const failed = page.getByTestId('flow-embed-failed');
	await expect(failed).toBeVisible();
	await expect(failed).toContainText(collectionTitle);
	await expect(page.getByTestId('flow-embed-provisional')).toHaveCount(0);
	await expect(page.getByTestId('flow-embedded-collection')).toHaveCount(0);

	await page.getByTestId('flow-embed-retry').click();
	const embedded = page.getByTestId('flow-embedded-collection');
	await expect(embedded).toHaveCount(1);
	await expect(embedded).toContainText(collectionTitle);
	await expect(page.getByTestId('flow-embed-failed')).toHaveCount(0);
	await expect(page.getByTestId('flow-embed-provisional')).toHaveCount(0);
	await expect(embedded.getByRole('grid')).toBeVisible();

	expect(commandBodies).toHaveLength(2);
	for (const body of commandBodies) {
		expect((body.command as { type: string }).type).toBe('create_collection_embed');
	}
	expect(commandBodies[1]).toEqual(commandBodies[0]);
	expect(objectCreates).toHaveLength(0);

	const collectionId = await embedded.getAttribute('data-collection-id');
	expect(collectionId).toBeTruthy();
	const collection = await unwrap<ObjectView>(
		await request.get(`/api/v1/flow/objects/${collectionId}`, {
			headers: f.auth
		})
	);
	expect(collection.object_type).toBe('collection');
	expect(collection.parent_id).toBe(objectId);
	expect(collection.title).toBe(collectionTitle);

	const nav = page.getByRole('navigation', { name: 'Navigator' });
	await expect(nav.getByRole('treeitem').filter({ hasText: collectionTitle })).toBeVisible();

	// The embed is a block in the Page document, not client state.
	await page.reload();
	await expect(page.getByTestId('flow-embedded-collection')).toHaveCount(1);
	await expect(page.getByTestId('flow-embedded-collection')).toContainText(collectionTitle);
	await page.getByTestId('flow-embedded-collection-open').click();
	await expect(page).toHaveURL(new RegExp(`/workspace/${f.workspaceId}/flow/${collectionId}$`));
	await page.screenshot({
		path: 'test-results/flow-slash-bridge-collection.png'
	});
});

test('slash Reference and Embed: cards from the bridge, confirmed removal leaves the record', async ({
	page,
	request
}) => {
	const f = await setup(request);
	const objectId = await newPage(request, f, `Reference host ${runId}`);
	const referenceBodies: Array<Record<string, unknown>> = [];
	const deletes: string[] = [];
	const hiddenReads: string[] = [];
	page.on('request', (req: Request) => {
		const path = new URL(req.url()).pathname;
		if (req.method() === 'POST' && path === `/api/v1/flow/objects/${objectId}/references`)
			referenceBodies.push(req.postDataJSON() as Record<string, unknown>);
		if (req.method() === 'DELETE') deletes.push(path);
		if (path.startsWith(`/api/v1/forms/${f.hiddenFormId}`)) hiddenReads.push(path);
	});

	await signInAs(page, f.ownerLogin);
	await page.goto(`/workspace/${f.workspaceId}/flow/${objectId}`);

	// ---- Reference ----
	await openSlashMenu(page);
	await page.getByTestId('flow-slash-reference').click();
	const dialog = page.getByTestId('flow-slash-forms-dialog');
	await expect(dialog).toHaveAttribute('data-mode', 'reference');
	await expect(page.getByTestId('flow-slash-forms-note')).toContainText(
		'without changing who owns it'
	);
	await page.getByTestId('flow-slash-forms-project').selectOption(f.projectId);
	await page.getByTestId('flow-slash-forms-form').selectOption(f.formId);
	await page.getByTestId('flow-slash-forms-record').selectOption(f.recordId);
	await page.getByTestId('flow-slash-forms-submit').click();

	const card = page.locator('[data-testid="flow-bridge-card"][data-mode="reference"]');
	await expect(card).toHaveCount(1);
	await expect(card.getByTestId('flow-bridge-card-title')).toHaveText(f.recordTitle);
	await expect(card).toHaveAttribute('data-target-type', 'form_record');
	await expect(card.getByTestId('flow-bridge-card-permission')).toHaveText(
		/View only|view and edit/
	);
	await expect(card.getByTestId('flow-bridge-card-source')).toHaveAttribute(
		'href',
		`/workspace/${f.workspaceId}/projects/${f.projectId}/forms/records/${f.recordId}`
	);
	expect(referenceBodies).toHaveLength(1);
	expect(referenceBodies[0]).toEqual({
		target_type: 'form_record',
		target_id: f.recordId,
		display: { mode: 'reference' },
		idempotency_key: expect.any(String)
	});
	const referenceId = await card.getAttribute('data-reference-id');

	// The placeholder block is in the Page document: the card survives a reload.
	await page.reload();
	await expect(page.locator('[data-testid="flow-bridge-card"][data-mode="reference"]')).toHaveCount(
		1
	);

	await page.getByTestId('flow-bridge-card-remove').click();
	const removeDialog = page.getByTestId('flow-bridge-remove-dialog');
	await expect(removeDialog).toContainText('is not deleted or changed');
	expect(deletes).toHaveLength(0);
	await page.getByTestId('flow-bridge-remove-confirm').click();
	await expect(page.getByTestId('flow-bridge-card')).toHaveCount(0);
	expect(deletes).toEqual([`/api/v1/flow/objects/${objectId}/references/${referenceId}`]);
	const live = await unwrap<{ items: unknown[] }>(
		await request.get(`/api/v1/flow/objects/${objectId}/references`, {
			headers: f.auth
		})
	);
	expect(live.items).toHaveLength(0);
	const stillThere = await unwrap<{ id: string; archived_at: string | null }>(
		await request.get(`/api/v1/form-records/${f.recordId}`, {
			headers: f.auth
		})
	);
	expect(stillThere.id).toBe(f.recordId);
	expect(stillThere.archived_at ?? null).toBeNull();

	// ---- Embed ----
	await openSlashMenu(page);
	await page.getByTestId('flow-slash-embed').click();
	await expect(dialog).toHaveAttribute('data-mode', 'embed');
	await page.getByTestId('flow-slash-forms-project').selectOption(f.projectId);
	await page.getByTestId('flow-slash-forms-form').selectOption(f.formId);
	await page.getByTestId('flow-slash-forms-record').selectOption(f.recordId);
	await page.getByTestId('flow-slash-forms-submit').click();
	const embedCard = page.locator('[data-testid="flow-bridge-card"][data-mode="embed"]');
	await expect(embedCard).toHaveCount(1);
	await expect(embedCard.getByTestId('flow-bridge-embed-body')).toContainText('Note');
	await expect(embedCard.getByTestId('flow-bridge-embed-body')).toContainText(
		`Bridge note ${runId}`
	);
	expect(referenceBodies[1]).toMatchObject({ display: { mode: 'embed' } });

	// An Embed whose viewing right is withdrawn afterwards. The bridge then omits the reference
	// from its list (it never discloses a target the caller may not view), so the Page's
	// placeholder renders as unavailable and the browser asks Forms for nothing about it.
	await openSlashMenu(page);
	await page.getByTestId('flow-slash-embed').click();
	await page.getByTestId('flow-slash-forms-project').selectOption(f.projectId);
	await page.getByTestId('flow-slash-forms-form').selectOption(f.hiddenFormId);
	await page.getByTestId('flow-slash-forms-submit').click();
	const formCard = page.locator(
		'[data-testid="flow-bridge-card"][data-mode="embed"][data-target-type="form"]'
	);
	await expect(formCard).toHaveCount(1);
	await expect(formCard.getByTestId('flow-bridge-embed-body')).toContainText('1 fields');
	const hiddenReferenceId = await formCard.getAttribute('data-reference-id');
	await unwrap(
		await request.patch(`/api/v1/forms/${f.hiddenFormId}/permissions`, {
			headers: f.auth,
			data: {
				policies: [
					{
						subject_type: 'role',
						subject_id: 'owner',
						policy: { actions: { 'form.view': false } }
					}
				]
			}
		})
	);
	hiddenReads.length = 0;
	await page.reload();
	await expect(page.locator('[data-testid="flow-bridge-card"][data-mode="embed"]')).toHaveCount(1);
	const unavailable = page.getByTestId('flow-bridge-card-unavailable');
	await expect(unavailable).toHaveCount(1);
	await expect(unavailable).toHaveAttribute('data-reference-id', hiddenReferenceId ?? '');
	await expect(unavailable).toContainText('no longer available');
	expect(hiddenReads).toEqual([]);
	await page.screenshot({
		path: 'test-results/flow-slash-bridge-cards.png',
		fullPage: true
	});
});

test('slash Convert to Forms opens the conversion wizard', async ({ page, request }) => {
	const f = await setup(request);
	const objectId = await newPage(request, f, `Convert host ${runId}`);
	await signInAs(page, f.ownerLogin);
	await page.goto(`/workspace/${f.workspaceId}/flow/${objectId}`);
	await openSlashMenu(page);
	const option = page.getByTestId('flow-slash-convert');
	await expect(option).toContainText('the record belongs to Forms');
	await option.click();
	await expect(page).toHaveURL(new RegExp(`/workspace/${f.workspaceId}/flow/${objectId}/convert$`));
	await expect(page.getByTestId('flow-convert')).toBeVisible();
});
