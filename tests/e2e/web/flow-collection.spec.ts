import { expect, test, type APIRequestContext, type Page, type Request } from '@playwright/test';

/**
 * Collection renderers on the Flow object route against a real API + PostgreSQL
 * (`versions/v0.6-collections.md` "Web 交互"; `ui-surface-v1.md` "Renderer registry"
 * `collection:table`, `collection:board`, `record:panel`).
 *
 * In the browser the owner:
 * - creates a standalone Collection from the navigator's New menu (table view, default text
 *   field "Name"); the object route renders it with the registry renderer, and never asks for a
 *   collaboration ticket or bootstrap for it;
 * - enters schema design mode (`?mode=schema`), adds a select field (Todo/Done) and a text field,
 *   adds a board view, then leaves the mode. Every `field_*` / `view_*` command is sent while the
 *   mode is on, none outside it;
 * - creates two records from the table ("+ New record"), edits the Name cell of each with the
 *   keyboard (Enter edits, Enter commits): one `record_patch` per cell, one property, with the
 *   record's frontier from the list as `expected_frontier`;
 * - switches to the board and drags one card from "No value" to "Done": one `record_patch` of
 *   the select field; then moves the other card with the keyboard alternative;
 * - reloads: names and statuses come back from the server;
 * - opens a record panel (the first and only read of that record's document), types an
 *   unsaved Notes value, switches views and back (the draft is still there), saves the body;
 *   the server's record body is the typed text;
 * - at 390px the table is a card list and the page has no horizontal overflow (screenshot
 *   attached);
 * - every records read names the active view by `view_id` (no draft query is used here);
 * - after a restricted field turns field secrecy on, the record panel says the body cannot be
 *   read under field secrecy and offers no body editor;
 * - archiving a record from its panel needs an in-place confirmation, sends one `record_archive`,
 *   and the record leaves the list (server too).
 *
 * Needs the stack from `scripts/e2e-web-stack.sh up` (BASE_URL, ADMIN_EMAIL / ADMIN_PASSWORD).
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = {
	email: `col-owner-${runId}@e2e.sylvode.test`,
	password: `ColOwner-${runId}-1!`
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
type CollectionData = {
	object: { id: string; frontier: string; document_id: string };
	fields: Array<{ field_id: string; label: string; type: string; config: Record<string, unknown> }>;
	views: Array<{ view_id: string; name: string; type: string; config: Record<string, unknown> }>;
};
type RecordsData = {
	items: Array<{
		record: { id: string; frontier: string; document_id: string };
		values_by_field_id: Record<string, unknown>;
	}>;
};
type CommandBody = {
	command: { type: string; payload: Record<string, unknown> };
	expected_frontier?: string;
	idempotency_key: string;
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
		email: `col-admin-${runId}@e2e.sylvode.test`,
		password: `ColAdmin-${runId}-1!`
	};
	await unwrap(
		await request.post('/api/v1/auth/register', { data: { ...admin, name: 'Col Admin' } })
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

test('collection: schema mode, table edit, board drag, record panel body, narrow cards', async ({
	browser,
	request
}, testInfo) => {
	test.setTimeout(150_000);
	const admin = await instanceAdmin(request);
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...owner, name: 'Col Owner' }
		})
	);
	const ownerLogin = await login(request, owner);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `col-${runId}`, name: `Col ${runId}` }
		})
	);
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: auth,
			data: { enabled: true, default_member_level: 'edit', idempotency_key: crypto.randomUUID() }
		})
	);

	// Wide enough for the three board columns beside the navigator and the object panel, so the
	// drag source and target are both on screen.
	const context = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
	const page = await context.newPage();
	await signInAs(page, ownerLogin);

	// Every write and every document read the page makes, with the schema mode at send time.
	const commands: Array<{ body: CommandBody; url: string; schemaMode: boolean }> = [];
	const recordPosts: Array<Record<string, unknown>> = [];
	const objectReads: string[] = [];
	// `view_id` of every records list read the page makes.
	const recordReads: Array<string | null> = [];
	// Ticket document ids and bootstrap object ids (the navigator legitimately opens its own
	// document; the Collection and its records must never appear here).
	const collabRequests: string[] = [];
	page.on('request', (req: Request) => {
		const url = new URL(req.url());
		if (
			req.method() === 'POST' &&
			/\/api\/v1\/flow\/objects\/[^/]+\/commands$/.test(url.pathname)
		) {
			commands.push({
				body: JSON.parse(req.postData() ?? '{}') as CommandBody,
				url: url.pathname,
				schemaMode: new URL(page.url()).searchParams.get('mode') === 'schema'
			});
		}
		if (
			req.method() === 'POST' &&
			/\/api\/v1\/flow\/collections\/[^/]+\/records$/.test(url.pathname)
		) {
			recordPosts.push(JSON.parse(req.postData() ?? '{}') as Record<string, unknown>);
		}
		if (
			req.method() === 'GET' &&
			/^\/api\/v1\/flow\/collections\/[^/]+\/records$/.test(url.pathname)
		) {
			recordReads.push(url.searchParams.get('view_id'));
		}
		if (req.method() === 'GET' && /^\/api\/v1\/flow\/objects\/[^/]+$/.test(url.pathname)) {
			objectReads.push(url.pathname.split('/').pop() ?? '');
		}
		if (req.method() === 'POST' && url.pathname === '/api/v1/collab/tickets') {
			collabRequests.push(
				String((JSON.parse(req.postData() ?? '{}') as { document_id?: string }).document_id)
			);
		}
		if (url.pathname.endsWith('/bootstrap')) {
			collabRequests.push(url.pathname.split('/').slice(-2)[0] ?? '');
		}
	});

	// ---- create the Collection through the navigator --------------------------
	await page.goto(`/workspace/${workspace.id}/flow`);
	await page.getByTestId('flow-nav-new').click();
	await page.getByRole('menuitem', { name: 'New Collection' }).click();
	const collectionTitle = `Tasks ${runId}`;
	await page.getByTestId('flow-create-collection-name').fill(collectionTitle);
	await expect(page.getByTestId('flow-create-collection-field')).toHaveValue('Name');
	await page.getByTestId('flow-create-collection-view-table').check();
	const createPost = page.waitForResponse(
		(res) =>
			new URL(res.url()).pathname === `/api/v1/workspaces/${workspace.id}/flow/objects` &&
			res.request().method() === 'POST'
	);
	await page.getByTestId('flow-create-collection-submit').click();
	const created = (await (await createPost).json()) as Envelope<{ object: { id: string } }>;
	expect(created.code).toBe(0);
	const collectionId = created.data.object.id;
	await expect(page).toHaveURL(new RegExp(`/flow/${collectionId}$`));

	const renderer = page.getByTestId('collection-renderer');
	await expect(renderer).toBeVisible();
	await expect(renderer).toHaveAttribute('data-mode', 'entry');
	await expect(page.getByTestId('collection-table')).toHaveAttribute('role', 'grid');
	await expect(page.getByTestId('flow-object-unsupported')).toHaveCount(0);
	// Normal mode has no schema controls.
	await expect(page.getByTestId('collection-schema-editor')).toHaveCount(0);
	await expect(page.getByTestId('collection-save-view')).toHaveCount(0);

	// ---- schema mode: select + text field, board view -----------------------------------------
	const toggle = page.getByTestId('collection-schema-toggle');
	await expect(toggle).toHaveAttribute('aria-pressed', 'false');
	await toggle.click();
	await expect(page).toHaveURL(/[?&]mode=schema/);
	await expect(renderer).toHaveAttribute('data-mode', 'schema');
	await expect(toggle).toHaveAttribute('aria-pressed', 'true');
	const editor = page.getByTestId('collection-schema-editor');
	await expect(editor).toBeVisible();

	await page.getByTestId('collection-new-field-label').fill('Status');
	await page.getByTestId('collection-new-field-type').selectOption('select');
	await page.getByTestId('collection-new-field-options').fill('Todo, Done');
	await page.getByTestId('collection-add-field').click();
	await expect(editor.getByTestId('collection-schema-field')).toHaveCount(2);

	await page.getByTestId('collection-new-field-label').fill('Notes');
	await page.getByTestId('collection-new-field-type').selectOption('text');
	await page.getByTestId('collection-add-field').click();
	await expect(editor.getByTestId('collection-schema-field')).toHaveCount(3);

	await editor.getByLabel('New view name').fill('Board');
	await editor.getByLabel('View type').selectOption('board');
	await editor.getByRole('button', { name: 'Add view' }).click();
	await expect(page.getByTestId('collection-view-tab')).toHaveCount(2);

	const schemaCommands = commands.filter((c) => /^(field|view)_/.test(c.body.command.type));
	expect(schemaCommands.map((c) => c.body.command.type)).toEqual([
		'field_create',
		'field_create',
		'view_create'
	]);
	expect(schemaCommands.every((c) => c.schemaMode)).toBe(true);
	expect(
		schemaCommands.every((c) => c.url === `/api/v1/flow/objects/${collectionId}/commands`)
	).toBe(true);
	const statusCreate = schemaCommands[0].body.command.payload;
	expect(statusCreate.label).toBe('Status');
	expect(statusCreate.field_type).toBe('select');
	expect(statusCreate.config).toEqual({ options: ['Todo', 'Done'] });

	const schema = await unwrap<CollectionData>(
		await request.get(`/api/v1/flow/collections/${collectionId}`, { headers: auth })
	);
	const nameField = schema.fields.find((f) => f.label === 'Name');
	const statusField = schema.fields.find((f) => f.label === 'Status');
	const notesField = schema.fields.find((f) => f.label === 'Notes');
	expect(nameField?.type).toBe('text');
	expect(statusField?.type).toBe('select');
	expect(statusField?.config).toEqual({ options: ['Todo', 'Done'] });
	expect(notesField?.type).toBe('text');
	expect(schema.views.map((v) => [v.name, v.type])).toEqual([
		['Table', 'table'],
		['Board', 'board']
	]);
	const nameId = nameField?.field_id ?? '';
	const statusId = statusField?.field_id ?? '';
	const notesId = notesField?.field_id ?? '';

	await toggle.click();
	await expect(page).not.toHaveURL(/mode=schema/);
	await expect(renderer).toHaveAttribute('data-mode', 'entry');
	await expect(page.getByTestId('collection-schema-editor')).toHaveCount(0);
	const schemaCommandCount = commands.length;

	// ---- two records, table edits ----------------------------------------------------------
	await page.getByTestId('collection-view-tab').filter({ hasText: 'Table' }).click();
	const rows = page.getByTestId('collection-row');
	await page.getByTestId('collection-new-record').click();
	await expect(rows).toHaveCount(1);
	await page.getByTestId('collection-new-record').click();
	await expect(rows).toHaveCount(2);
	expect(recordPosts).toHaveLength(2);
	expect(recordPosts.every((post) => JSON.stringify(post.values_by_field_id) === '{}')).toBe(true);
	expect(new Set(recordPosts.map((post) => post.idempotency_key)).size).toBe(2);

	const listed = await unwrap<RecordsData>(
		await request.get(`/api/v1/flow/collections/${collectionId}/records`, { headers: auth })
	);
	expect(listed.items).toHaveLength(2);
	const frontierById = new Map(listed.items.map((item) => [item.record.id, item.record.frontier]));
	const rowIds = await rows.evaluateAll((elements) =>
		elements.map((element) => element.getAttribute('data-record-id') ?? '')
	);

	const names = ['Alpha', 'Beta'];
	for (const [index, name] of names.entries()) {
		const cell = rows.nth(index).locator(`[role="gridcell"][data-field-id="${nameId}"]`);
		await cell.click();
		await page.keyboard.press('Enter');
		const input = page.locator(`#collection-editor-${rowIds[index]}-${nameId}`);
		await expect(input).toBeFocused();
		await input.fill(name);
		const patched = page.waitForResponse(
			(res) =>
				new URL(res.url()).pathname === `/api/v1/flow/objects/${collectionId}/commands` &&
				res.request().method() === 'POST'
		);
		await page.keyboard.press('Enter');
		expect(((await (await patched).json()) as Envelope<unknown>).code).toBe(0);
		await expect(cell).toHaveText(name);
	}
	const cellPatches = commands.slice(schemaCommandCount);
	expect(cellPatches).toHaveLength(2);
	for (const [index, patch] of cellPatches.entries()) {
		expect(patch.schemaMode).toBe(false);
		expect(patch.body.command.type).toBe('record_patch');
		expect(patch.body.command.payload).toEqual({
			record_id: rowIds[index],
			properties: { [nameId]: names[index] }
		});
		expect(patch.body.expected_frontier).toBe(frontierById.get(rowIds[index]));
	}
	// Arrow keys move the grid focus.
	await page.keyboard.press('ArrowRight');
	await expect(rows.nth(1).locator(`[role="gridcell"][data-field-id="${statusId}"]`)).toBeFocused();

	// ---- board: drag and keyboard move ------------------------------------------------------
	await page.getByTestId('collection-view-tab').filter({ hasText: 'Board' }).click();
	const board = page.getByTestId('collection-board');
	await expect(board).toHaveAttribute('data-group-field-id', statusId);
	const columns = board.getByTestId('collection-board-column');
	await expect(columns).toHaveCount(3);
	await expect(columns.nth(0)).toHaveAttribute('data-option', 'Todo');
	await expect(columns.nth(1)).toHaveAttribute('data-option', 'Done');
	await expect(columns.nth(2)).toHaveAttribute('data-option', '');
	await expect(columns.nth(0).getByRole('list')).toHaveCount(1);
	await expect(columns.nth(2).getByTestId('collection-board-card')).toHaveCount(2);

	const alphaCard = board.locator(
		`[data-testid="collection-board-card"][data-record-id="${rowIds[0]}"]`
	);
	const dragPatch = page.waitForResponse(
		(res) =>
			new URL(res.url()).pathname === `/api/v1/flow/objects/${collectionId}/commands` &&
			res.request().method() === 'POST'
	);
	await alphaCard.getByTestId('collection-board-card-body').dragTo(columns.nth(1));
	expect(((await (await dragPatch).json()) as Envelope<unknown>).code).toBe(0);
	await expect(columns.nth(1).getByTestId('collection-board-card')).toHaveCount(1);
	const drag = commands[commands.length - 1];
	expect(drag.body.command.type).toBe('record_patch');
	expect(drag.body.command.payload).toEqual({
		record_id: rowIds[0],
		properties: { [statusId]: 'Done' }
	});
	expect(drag.schemaMode).toBe(false);

	// Keyboard alternative: Beta from "No value" one column left, to "Done".
	const betaCard = board.locator(
		`[data-testid="collection-board-card"][data-record-id="${rowIds[1]}"]`
	);
	const moveLeft = betaCard.getByTestId('collection-board-move-left');
	const moveBox = await moveLeft.boundingBox();
	expect(moveBox?.height ?? 0).toBeGreaterThanOrEqual(44);
	expect(moveBox?.width ?? 0).toBeGreaterThanOrEqual(44);
	await moveLeft.focus();
	await page.keyboard.press('Enter');
	await expect(columns.nth(1).getByTestId('collection-board-card')).toHaveCount(2);
	expect(commands[commands.length - 1].body.command.payload).toEqual({
		record_id: rowIds[1],
		properties: { [statusId]: 'Done' }
	});

	// ---- persisted after a reload -----------------------------------------------------------
	await page.reload();
	await expect(rows).toHaveCount(2);
	for (const [index, name] of names.entries()) {
		const row = page.locator(`[data-testid="collection-row"][data-record-id="${rowIds[index]}"]`);
		await expect(row.locator(`[data-field-id="${nameId}"]`)).toHaveText(name);
		await expect(row.locator(`[data-field-id="${statusId}"]`)).toHaveText('Done');
	}
	const persisted = await unwrap<RecordsData>(
		await request.get(`/api/v1/flow/collections/${collectionId}/records`, { headers: auth })
	);
	expect(
		persisted.items
			.map((item) => [item.values_by_field_id[nameId], item.values_by_field_id[statusId]])
			.sort()
	).toEqual([
		['Alpha', 'Done'],
		['Beta', 'Done']
	]);

	// ---- record panel: the only record document read; drafts survive a view switch ----------
	// Nothing so far read a record object: loading, editing and dragging used the typed list.
	expect(objectReads.filter((id) => rowIds.includes(id))).toEqual([]);
	const alphaRow = page.locator(`[data-testid="collection-row"][data-record-id="${rowIds[0]}"]`);
	await alphaRow.getByTestId('collection-open-record').click();
	const panel = page.getByTestId('record-panel');
	await expect(panel).toHaveAttribute('data-record-id', rowIds[0]);
	await expect(panel.getByTestId('record-panel-body')).toBeVisible();
	expect(objectReads.filter((id) => rowIds.includes(id))).toEqual([rowIds[0]]);

	const notesInput = page.locator(`#record-panel-${rowIds[0]}-${notesId}`);
	await notesInput.fill('kept draft');
	await page.getByTestId('collection-view-tab').filter({ hasText: 'Board' }).click();
	await expect(page.getByTestId('collection-board')).toBeVisible();
	await page.getByTestId('collection-view-tab').filter({ hasText: 'Table' }).click();
	await expect(notesInput).toHaveValue('kept draft');
	await expect(panel).toHaveAttribute('data-record-id', rowIds[0]);

	const bodyText = `Body for Alpha ${runId}`;
	await panel.getByTestId('record-panel-body').fill(bodyText);
	const bodyPatch = page.waitForResponse(
		(res) =>
			new URL(res.url()).pathname === `/api/v1/flow/objects/${collectionId}/commands` &&
			res.request().method() === 'POST'
	);
	await panel.getByTestId('record-panel-body-save').click();
	expect(((await (await bodyPatch).json()) as Envelope<unknown>).code).toBe(0);
	await expect(panel.getByTestId('record-panel-body-status')).toHaveText('Saved');
	const bodyCommand = commands[commands.length - 1];
	expect(bodyCommand.body.command.payload).toEqual({ record_id: rowIds[0], body: bodyText });
	expect(typeof bodyCommand.body.expected_frontier).toBe('string');

	const recordObject = await unwrap<{
		semantic_content: { nodes: Record<string, { properties: Record<string, string> }> };
	}>(await request.get(`/api/v1/flow/objects/${rowIds[0]}`, { headers: auth }));
	expect(recordObject.semantic_content.nodes['record:body'].properties.body).toBe(bodyText);

	// The panel closes with Escape-free button; reopening after a reload shows the stored body.
	await panel.getByTestId('record-panel-close').click();
	await expect(panel).toHaveCount(0);
	await page.reload();
	await page
		.locator(`[data-testid="collection-row"][data-record-id="${rowIds[0]}"]`)
		.getByTestId('collection-open-record')
		.click();
	await expect(page.getByTestId('record-panel-body')).toHaveValue(bodyText);

	// No collaboration ticket or bootstrap was ever requested for the Collection or its records.
	const serverOnly = new Set([
		collectionId,
		schema.object.document_id,
		...persisted.items.flatMap((item) => [item.record.id, item.record.document_id])
	]);
	expect(serverOnly.size).toBe(6);
	expect(collabRequests.filter((id) => serverOnly.has(id))).toEqual([]);
	// No schema command outside schema mode.
	expect(
		commands.filter((c) => /^(field|view)_/.test(c.body.command.type) && !c.schemaMode)
	).toEqual([]);

	// ---- 390px: cards, no horizontal overflow ----------------------------------------------
	await page.setViewportSize({ width: 390, height: 844 });
	await page.reload();
	const cards = page.getByTestId('collection-card');
	await expect(cards).toHaveCount(2);
	await expect(page.getByTestId('collection-table')).toHaveCount(0);
	const overflow = await page.evaluate(() => ({
		scroll: document.documentElement.scrollWidth,
		client: document.documentElement.clientWidth
	}));
	expect(overflow.scroll).toBeLessThanOrEqual(overflow.client);
	for (const card of await cards.all()) {
		const box = await card.boundingBox();
		expect(box).not.toBeNull();
		expect((box?.x ?? -1) >= 0 && (box?.x ?? 0) + (box?.width ?? 0) <= 390).toBe(true);
		// Not clipped: no ancestor that clips its overflow cuts into the card.
		const clippedBy = await card.evaluate((element) => {
			const rect = element.getBoundingClientRect();
			for (let node = element.parentElement; node; node = node.parentElement) {
				const style = getComputedStyle(node);
				if (`${style.overflowX}${style.overflowY}` === 'visiblevisible') continue;
				const outer = node.getBoundingClientRect();
				if (
					node !== document.documentElement &&
					node !== document.body &&
					(rect.top < outer.top - 0.5 ||
						rect.bottom > outer.bottom + 0.5 ||
						rect.left < outer.left - 0.5 ||
						rect.right > outer.right + 0.5)
				) {
					return `${node.tagName}.${node.className}`;
				}
			}
			return null;
		});
		expect(clippedBy).toBeNull();
	}
	const screenshot = testInfo.outputPath('flow-collection-390.png');
	await page.screenshot({ path: screenshot, fullPage: true });
	await testInfo.attach('flow-collection-390', { path: screenshot, contentType: 'image/png' });

	// ---- records are read by the saved view's id ----------------------------------------------
	const viewIds = new Set(schema.views.map((view) => view.view_id));
	expect(recordReads.length).toBeGreaterThan(0);
	expect(recordReads.every((viewId) => viewId !== null && viewIds.has(viewId))).toBe(true);

	// ---- field secrecy: the body is withheld, not shown as empty ------------------------------
	await page.setViewportSize({ width: 1920, height: 1080 });
	const beforeSecret = await unwrap<CollectionData>(
		await request.get(`/api/v1/flow/collections/${collectionId}`, { headers: auth })
	);
	await unwrap(
		await request.post(`/api/v1/flow/objects/${collectionId}/commands`, {
			headers: auth,
			data: {
				command: {
					type: 'field_create',
					payload: {
						field_id: crypto.randomUUID(),
						label: 'Salary',
						field_type: 'number',
						config: { restricted: true },
						index: 10
					}
				},
				expected_frontier: beforeSecret.object.frontier,
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	await page.reload();
	await page
		.locator(`[data-testid="collection-row"][data-record-id="${rowIds[0]}"]`)
		.getByTestId('collection-open-record')
		.click();
	const secretPanel = page.getByTestId('record-panel');
	await expect(secretPanel).toHaveAttribute('data-record-id', rowIds[0]);
	await expect(secretPanel.getByTestId('record-panel-body-withheld')).toBeVisible();
	await expect(secretPanel.getByTestId('record-panel-body')).toHaveCount(0);
	await expect(secretPanel.getByTestId('record-panel-body-save')).toHaveCount(0);
	const withheld = await unwrap<{ semantic_content: unknown }>(
		await request.get(`/api/v1/flow/objects/${rowIds[0]}`, { headers: auth })
	);
	expect(withheld.semantic_content).toEqual({});

	// ---- archive a record from its panel, after confirmation ----------------------------------
	await secretPanel.getByTestId('record-panel-close').click();
	await page
		.locator(`[data-testid="collection-row"][data-record-id="${rowIds[1]}"]`)
		.getByTestId('collection-open-record')
		.click();
	const archivePanel = page.getByTestId('record-panel');
	await expect(archivePanel).toHaveAttribute('data-record-id', rowIds[1]);
	const commandsBeforeArchive = commands.length;
	await archivePanel.getByTestId('record-panel-archive').click();
	// The first click only asks; nothing is sent until the user confirms.
	await expect(archivePanel.getByTestId('record-panel-archive-confirm')).toBeVisible();
	expect(commands.length).toBe(commandsBeforeArchive);
	const archived = page.waitForResponse(
		(res) =>
			new URL(res.url()).pathname === `/api/v1/flow/objects/${collectionId}/commands` &&
			res.request().method() === 'POST'
	);
	await archivePanel.getByTestId('record-panel-archive-confirm').click();
	expect(((await (await archived).json()) as Envelope<unknown>).code).toBe(0);
	expect(commands.slice(commandsBeforeArchive).map((c) => c.body.command)).toEqual([
		{ type: 'record_archive', payload: { record_id: rowIds[1] } }
	]);
	await expect(page.getByTestId('record-panel')).toHaveCount(0);
	await expect(rows).toHaveCount(1);
	await expect(
		page.locator(`[data-testid="collection-row"][data-record-id="${rowIds[1]}"]`)
	).toHaveCount(0);
	const afterArchive = await unwrap<RecordsData>(
		await request.get(`/api/v1/flow/collections/${collectionId}/records`, { headers: auth })
	);
	expect(afterArchive.items.map((item) => item.record.id)).toEqual([rowIds[0]]);

	await context.close();
});
