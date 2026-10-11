import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test, type APIRequestContext, type Page, type Request } from '@playwright/test';

/**
 * The Flow navigator (`/workspace/{id}/flow`) against a real API + PostgreSQL + search worker:
 * Flow search, the projection-lag badge and standalone Collection create
 * (`ui-surface-v1.md` "后续版本 UI 派生" v0.5/v0.6).
 *
 * Over REST an owner creates a workspace with Flow enabled and three Pages whose titles share one
 * unique word. The real worker (`target/release/worker`) copies the accepted projections into
 * `flow_search_index`; the spec waits for REST search to return all three before the browser
 * part starts, so the UI is searched against an index the worker produced, not one the spec wrote.
 *
 * In the browser the owner:
 * - sees the lag badge (aria-live) agree with `GET .../projection-lag`;
 * - types the word: the 200ms debounce turns the burst of keystrokes into ONE search request
 *   (`all_visible=true`, `freshness=allow_stale`); three options appear in a listbox, the
 *   combobox's `aria-activedescendant` follows ArrowDown, and Enter opens that Page (the object
 *   page then GETs the current object);
 * - with the worker paused (SIGSTOP), renames one Page over REST: an `allow_stale` search shows
 *   that hit with the Stale badge, and `require_current` fails closed with `stale_frontier` and
 *   no fallback request;
 * - creates a Collection from the New menu (one default text field, board view): one
 *   `POST .../flow/objects` with `object_type=collection`, the server's collection read has that
 *   field and view, the navigator shows the new object, and the object page renders it with the
 *   registry's Collection renderer (its board view, disabled without a select field).
 *
 * Needs the stack from `scripts/e2e-web-stack.sh up`: BASE_URL serving the built frontend with
 * `/api` proxied to the API on the same origin, and the background worker the script starts on
 * the same configuration (its pid in `<E2E_STACK_DIR>/worker.pid`; the stale/require_current part
 * pauses it with SIGSTOP and resumes it). Registration after the first
 * account needs an instance admin: ADMIN_EMAIL / ADMIN_PASSWORD when both are set, otherwise the
 * run registers the first account.
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = {
	email: `nav-owner-${runId}@e2e.sylvode.test`,
	password: `NavOwner-${runId}-1!`
};
// One lexeme for the `simple` text-search configuration: letters and digits only.
const WORD = `zq${runId.replace(/[^a-z0-9]/g, '')}`;

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
type SearchData = {
	items: Array<{
		object: { id: string; title: string };
		stale: boolean;
		projection_lag: number;
	}>;
	index_frontier: { stale: boolean; lag: number };
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
		email: `nav-admin-${runId}@e2e.sylvode.test`,
		password: `NavAdmin-${runId}-1!`
	};
	await unwrap(
		await request.post('/api/v1/auth/register', {
			data: { ...admin, name: 'Nav Admin' }
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

/** The background worker `scripts/e2e-web-stack.sh up` started for this stack
 * (`<E2E_STACK_DIR>/worker.pid`); the stale/require_current part pauses it. */
function workerPid(): number {
	const stackDir = process.env.E2E_STACK_DIR;
	expect(
		stackDir,
		'E2E_STACK_DIR is set by scripts/e2e-web-stack.sh up (source <state>/env)'
	).toBeTruthy();
	const pid = Number(readFileSync(join(stackDir ?? '', 'worker.pid'), 'utf8').trim());
	expect(Number.isInteger(pid) && pid > 0, 'the stack worker.pid names a process').toBe(true);
	process.kill(pid, 0);
	return pid;
}

test('navigator: Flow search, lag badge and standalone Collection create', async ({
	browser,
	request
}) => {
	test.setTimeout(120_000);
	const pid = workerPid();
	const admin = await instanceAdmin(request);
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...owner, name: 'Nav Owner' }
		})
	);
	const ownerLogin = await login(request, owner);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };

	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `nav-${runId}`, name: `Nav ${runId}` }
		})
	);
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: auth,
			data: {
				enabled: true,
				default_member_level: 'edit',
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	const titles = [`Atlas ${WORD} one`, `Beacon ${WORD} two`, `Comet ${WORD} three`];
	const pages: Array<{ id: string; title: string }> = [];
	for (const title of titles) {
		const created = await unwrap<{ object: { id: string } }>(
			await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
				headers: auth,
				data: {
					object_type: 'page',
					title,
					idempotency_key: crypto.randomUUID()
				}
			})
		);
		pages.push({ id: created.object.id, title });
	}

	const searchUrl = (freshness: string) =>
		`/api/v1/workspaces/${workspace.id}/flow/search?q=${WORD}&all_visible=true&freshness=${freshness}`;
	// The worker ticks every 5s; wait until it has indexed all three accepted projections.
	await expect
		.poll(
			async () => {
				const body = (await (
					await request.get(searchUrl('require_current'), { headers: auth })
				).json()) as Envelope<SearchData>;
				return body.code === 0 ? body.data.items.length : -1;
			},
			{ timeout: 45_000, intervals: [500, 1000, 2000] }
		)
		.toBe(3);

	const lag = await unwrap<{ max_lag: number; p95_lag: number }>(
		await request.get(`/api/v1/workspaces/${workspace.id}/flow/projection-lag`, { headers: auth })
	);

	const context = await browser.newContext();
	const page = await context.newPage();
	await signInAs(page, ownerLogin);
	const searches: string[] = [];
	page.on('request', (req: Request) => {
		const url = new URL(req.url());
		if (url.pathname === `/api/v1/workspaces/${workspace.id}/flow/search`) {
			searches.push(url.search);
		}
	});
	await page.goto(`/workspace/${workspace.id}/flow`);

	// ---- lag badge --------------------------------------------------------------------------
	const badge = page.getByTestId('flow-lag-badge');
	await expect(badge).toHaveAttribute('aria-live', 'polite');
	await expect(badge).toHaveAttribute('data-lag-state', lag.max_lag === 0 ? 'synced' : 'lagging');
	if (lag.max_lag === 0) await expect(badge).toHaveText('Synced');
	else await expect(badge).toContainText(String(lag.max_lag));

	// ---- search: debounce, listbox, keyboard, open ------------------------------------------
	const input = page.getByTestId('flow-search-input');
	await expect(input).toBeEnabled();
	await input.pressSequentially(WORD, { delay: 15 });
	const options = page.getByRole('listbox', { name: 'Search results' }).getByRole('option');
	await expect(options).toHaveCount(3);
	expect(searches, 'one request for the whole burst of keystrokes').toEqual([
		`?q=${WORD}&all_visible=true&freshness=allow_stale&limit=20`
	]);
	const listbox = page.getByRole('listbox', { name: 'Search results' });
	await expect(listbox).toBeVisible();
	await expect(input).toHaveAttribute('aria-expanded', 'true');
	const firstId = await options.nth(0).getAttribute('id');
	await expect(input).toHaveAttribute('aria-activedescendant', firstId ?? '');
	await expect(options.nth(0)).toHaveAttribute('aria-selected', 'true');
	await expect(page.getByTestId('flow-search-stale')).toHaveCount(0);
	const hitTitles = await options.locator('span.font-medium').allTextContents();
	expect([...hitTitles].sort()).toEqual([...titles].sort());
	for (const option of await options.all()) {
		const box = await option.boundingBox();
		expect(box?.height ?? 0, 'option hit area').toBeGreaterThanOrEqual(44);
	}

	await input.press('ArrowDown');
	const secondId = await options.nth(1).getAttribute('id');
	await expect(input).toHaveAttribute('aria-activedescendant', secondId ?? '');
	await expect(options.nth(1)).toHaveAttribute('aria-selected', 'true');
	const secondObjectId = await options.nth(1).getAttribute('data-object-id');
	const opened = pages.find((p) => p.id === secondObjectId);
	expect(opened, 'the active option names a created page').toBeTruthy();
	const objectRead = page.waitForRequest(
		(req) =>
			new URL(req.url()).pathname.startsWith(`/api/v1/flow/objects/${secondObjectId}`) &&
			req.method() === 'GET'
	);
	await input.press('Enter');
	await expect(page).toHaveURL(new RegExp(`/workspace/${workspace.id}/flow/${secondObjectId}$`));
	await objectRead;
	await expect(page.locator('input[aria-label="Page info"]')).toHaveValue(opened?.title ?? '');
	await expect(page.getByTestId('flow-search-panel')).toHaveCount(0);

	// ---- stale hit and require_current fail-closed (worker paused) ---------------------------
	// A page other than the one just opened, so its stale state comes only from the rename.
	const renamed = pages.find((p) => p.id !== secondObjectId) ?? pages[0];
	process.kill(pid, 'SIGSTOP');
	try {
		await unwrap(
			await request.post(`/api/v1/flow/objects/${renamed.id}/commands`, {
				headers: auth,
				data: {
					command: {
						type: 'set_title',
						payload: { title: `Atlas ${WORD} renamed` }
					},
					idempotency_key: crypto.randomUUID()
				}
			})
		);
		const rest = (await (
			await request.get(searchUrl('require_current'), { headers: auth })
		).json()) as Envelope<null>;
		expect(rest.error_code, 'the index is behind while the worker is paused').toBe(
			'stale_frontier'
		);

		searches.length = 0;
		await input.fill('');
		await input.pressSequentially(WORD, { delay: 15 });
		await expect(options).toHaveCount(3);
		const staleOption = page.locator(
			`[data-testid="flow-search-result"][data-object-id="${renamed.id}"]`
		);
		await expect(staleOption.getByTestId('flow-search-stale')).toHaveText('Stale');
		// Every row's badge matches the server's own verdict for that hit (stale flag or lag > 0).
		const serverHits = await unwrap<SearchData>(
			await request.get(searchUrl('allow_stale'), { headers: auth })
		);
		const serverStale = serverHits.items
			.filter((h) => h.stale || h.projection_lag > 0)
			.map((h) => h.object.id)
			.sort();
		expect(serverStale).toContain(renamed.id);
		const uiStale = await page
			.locator('[data-testid="flow-search-result"]:has([data-testid="flow-search-stale"])')
			.evaluateAll((nodes) => nodes.map((node) => node.getAttribute('data-object-id') ?? ''));
		expect(uiStale.sort()).toEqual(serverStale);
		console.log(
			`server hits while paused: ${JSON.stringify(serverHits.items.map((h) => ({ id: h.object.id, stale: h.stale, lag: h.projection_lag })))}; renamed ${renamed.id}, opened ${secondObjectId}`
		);
		await expect(page.getByTestId('flow-search-frontier-stale')).toBeVisible();

		searches.length = 0;
		await page.getByTestId('flow-search-require-current').check();
		const error = page.getByTestId('flow-search-error');
		await expect(error).toHaveAttribute('data-error-code', 'stale_frontier');
		await expect(error).toContainText('not up to date');
		await expect(options).toHaveCount(0);
		await page.waitForTimeout(500);
		expect(searches, 'require_current is not retried as allow_stale').toEqual([
			`?q=${WORD}&all_visible=true&freshness=require_current&limit=20`
		]);
	} finally {
		process.kill(pid, 'SIGCONT');
	}
	await input.fill('');
	await expect(page.getByTestId('flow-search-panel')).toHaveCount(0);

	// ---- standalone Collection create -------------------------------------------------------
	const newButton = page.getByTestId('flow-nav-new');
	const newBox = await newButton.boundingBox();
	expect(newBox?.height ?? 0).toBeGreaterThanOrEqual(44);
	expect(newBox?.width ?? 0).toBeGreaterThanOrEqual(44);
	await newButton.click();
	await expect(newButton).toHaveAttribute('aria-expanded', 'true');
	await expect(page.getByRole('menuitem', { name: 'New Page' })).toBeFocused();
	await page.keyboard.press('ArrowDown');
	const collectionItem = page.getByRole('menuitem', { name: 'New Collection' });
	await expect(collectionItem).toBeFocused();
	await page.keyboard.press('Enter');

	const dialog = page.getByTestId('flow-create-collection');
	await expect(dialog).toBeVisible();
	const collectionTitle = `Roadmap ${runId}`;
	await page.getByTestId('flow-create-collection-name').fill(collectionTitle);
	await expect(page.getByTestId('flow-create-collection-field')).toHaveValue('Name');
	await page.getByTestId('flow-create-collection-view-board').check();
	const createPost = page.waitForResponse(
		(res) =>
			new URL(res.url()).pathname === `/api/v1/workspaces/${workspace.id}/flow/objects` &&
			res.request().method() === 'POST'
	);
	await page.getByTestId('flow-create-collection-submit').click();
	const createResponse = await createPost;
	const created = (await createResponse.json()) as Envelope<{
		object: { id: string; object_type: string };
	}>;
	expect(created.code).toBe(0);
	expect(created.data.object.object_type).toBe('collection');
	const collectionId = created.data.object.id;
	const body = JSON.parse(createResponse.request().postData() ?? '{}') as {
		object_type: string;
		title: string;
		idempotency_key: string;
		initial_fields: Array<{
			field_id: string;
			label: string;
			field_type: string;
			index: number;
		}>;
		initial_view: { view_id: string; view_type: string; name: string };
	};
	expect(body.object_type).toBe('collection');
	expect(body.title).toBe(collectionTitle);
	expect(body.idempotency_key.length).toBeGreaterThan(0);
	expect(body.initial_fields.map((f) => [f.label, f.field_type, f.index])).toEqual([
		['Name', 'text', 0]
	]);
	expect(body.initial_view.view_type).toBe('board');
	expect(body.initial_view.name).toBe('Board');

	await expect(page).toHaveURL(new RegExp(`/workspace/${workspace.id}/flow/${collectionId}$`));
	const tree = page.getByRole('tree');
	const item = tree.getByRole('treeitem').filter({ hasText: collectionTitle });
	await expect(item).toHaveCount(1);
	await expect(item).toHaveAttribute('aria-selected', 'true');
	await expect(item.getByTestId('flow-nav-collection-badge')).toHaveText('Collection');
	// The object page renders it with the registry's Collection renderer: the board
	// view, disabled until the Collection has a select field to group by.
	await expect(page.getByTestId('flow-object-unsupported')).toHaveCount(0);
	await expect(page.getByTestId('collection-renderer')).toBeVisible();
	await expect(page.getByTestId('collection-board-disabled')).toBeVisible();
	await expect(page.getByRole('heading', { level: 1, name: collectionTitle })).toBeVisible();

	const collection = await unwrap<{
		object: { id: string; object_type: string; title: string };
		// `flow/collections.rs::CollectionFieldView` / `CollectionViewView` serialize the type as `type`.
		fields: Array<{ field_id: string; label: string; type: string }>;
		views: Array<{ view_id: string; type: string; name: string }>;
	}>(
		await request.get(`/api/v1/flow/collections/${collectionId}`, {
			headers: auth
		})
	);
	expect(collection.object.object_type).toBe('collection');
	expect(collection.object.title).toBe(collectionTitle);
	expect(collection.fields.map((f) => [f.field_id, f.label, f.type])).toEqual([
		[body.initial_fields[0].field_id, 'Name', 'text']
	]);
	expect(collection.views.map((v) => [v.view_id, v.type, v.name])).toEqual([
		[body.initial_view.view_id, 'board', 'Board']
	]);

	// The new object survives a reload of the navigator (it is in the server's object list).
	await page.reload();
	await expect(
		page.getByRole('tree').getByRole('treeitem').filter({ hasText: collectionTitle })
	).toHaveCount(1);
	await context.close();
});
