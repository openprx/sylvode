import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

/**
 * Collection records paging and table virtualisation against a real API + PostgreSQL
 * (`versions/v0.6-collections.md` "大集合只打开 viewport"; `rest-api-v1.md` v0.6 records list,
 * `limit` default 50), and the standalone Record route's schema read.
 *
 * The owner creates a Collection and 55 records through REST, then in the browser:
 * - the first records read asks for `limit=50` and the table reports 50 loaded, while far fewer
 *   than 50 rows are in the DOM (only the virtual window);
 * - scrolling to the end loads the next page (`cursor` + `limit=50`): 55 loaded, the grid's
 *   `aria-rowcount` covers all 55, and the DOM still holds fewer than 55 rows;
 * - opening one record on its own route reads the owning Collection's schema through
 *   `GET /flow/objects/{record_id}/schema` and shows that record's panel.
 *
 * Needs the stack from `scripts/e2e-web-stack.sh up` (BASE_URL, ADMIN_EMAIL / ADMIN_PASSWORD).
 */

const RECORDS = 55;
const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = {
	email: `page-owner-${runId}@e2e.sylvode.test`,
	password: `PageOwner-${runId}-1!`
};

type Envelope<T> = { code: number; message: string; data: T; error_code?: string };
type LoginData = {
	tokens: { access_token: string; refresh_token: string };
	user: { id: string };
};
type Accepted = { object: { id: string } };

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
		email: `page-admin-${runId}@e2e.sylvode.test`,
		password: `PageAdmin-${runId}-1!`
	};
	await unwrap(
		await request.post('/api/v1/auth/register', { data: { ...admin, name: 'Page Admin' } })
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

test('collection: 55 records page by 50, the table renders only its window, a record route reads its schema', async ({
	browser,
	request
}) => {
	test.setTimeout(120_000);
	const admin = await instanceAdmin(request);
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...owner, name: 'Page Owner' }
		})
	);
	const ownerLogin = await login(request, owner);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `pg-${runId}`, name: `Paging ${runId}` }
		})
	);
	await unwrap(
		await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
			headers: auth,
			data: { enabled: true, default_member_level: 'edit', idempotency_key: crypto.randomUUID() }
		})
	);
	const fieldId = crypto.randomUUID();
	const collection = await unwrap<Accepted>(
		await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
			headers: auth,
			data: {
				object_type: 'collection',
				title: `Many ${runId}`,
				initial_fields: [
					{ field_id: fieldId, label: 'Name', field_type: 'text', config: {}, index: 0 }
				],
				initial_view: {
					view_id: crypto.randomUUID(),
					name: 'Table',
					view_type: 'table',
					config: {},
					index: 0
				},
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	const collectionId = collection.object.id;
	const recordIds: string[] = [];
	for (let i = 0; i < RECORDS; i += 1) {
		const created = await unwrap<Accepted>(
			await request.post(`/api/v1/flow/collections/${collectionId}/records`, {
				headers: auth,
				data: {
					values_by_field_id: { [fieldId]: `Row ${String(i).padStart(2, '0')}` },
					idempotency_key: crypto.randomUUID()
				}
			})
		);
		recordIds.push(created.object.id);
	}

	const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
	const page = await context.newPage();
	await signInAs(page, ownerLogin);
	const recordReads: Array<{ limit: string | null; cursor: string | null }> = [];
	const schemaReads: string[] = [];
	page.on('request', (req) => {
		const url = new URL(req.url());
		if (
			req.method() === 'GET' &&
			url.pathname === `/api/v1/flow/collections/${collectionId}/records`
		) {
			recordReads.push({
				limit: url.searchParams.get('limit'),
				cursor: url.searchParams.get('cursor')
			});
		}
		const schema = /^\/api\/v1\/flow\/objects\/([^/]+)\/schema$/.exec(url.pathname);
		if (req.method() === 'GET' && schema) schemaReads.push(schema[1]);
	});

	await page.goto(`/workspace/${workspace.id}/flow/${collectionId}`);
	const table = page.getByTestId('collection-table');
	const rows = page.getByTestId('collection-row');
	await expect(table).toBeVisible();
	await expect(page.getByText('50 records loaded')).toBeVisible();
	expect(recordReads[0], 'the first page').toEqual({ limit: '50', cursor: null });
	const firstPaint = await rows.count();
	expect(firstPaint, 'some rows are rendered').toBeGreaterThan(0);
	expect(firstPaint, `only the window is in the DOM (${firstPaint} of 50)`).toBeLessThan(50);

	// Scroll to the end: the next page is requested with the cursor.
	const scroller = page.getByTestId('collection-table-scroller');
	await scroller.evaluate((element) => {
		element.scrollTop = element.scrollHeight;
		element.dispatchEvent(new Event('scroll'));
	});
	await expect(page.getByText(`${RECORDS} records loaded`)).toBeVisible();
	expect(recordReads.length, 'two pages were read').toBe(2);
	expect(recordReads[1].limit, 'the second page asks for 50 again').toBe('50');
	expect(recordReads[1].cursor, 'the second page continues from the cursor').not.toBeNull();
	await expect(table).toHaveAttribute('aria-rowcount', String(RECORDS + 1));
	const afterMore = await rows.count();
	expect(afterMore, `the DOM still holds only a window (${afterMore} of ${RECORDS})`).toBeLessThan(
		RECORDS
	);
	// Scrolled to the end again, the 55th row (grid row index 56, after the header) is rendered
	// while the first rows have left the DOM.
	await scroller.evaluate((element) => {
		element.scrollTop = element.scrollHeight;
		element.dispatchEvent(new Event('scroll'));
	});
	await expect(
		page.locator(`[data-testid="collection-row"][aria-rowindex="${RECORDS + 1}"]`),
		'the last record is reachable at the end of the list'
	).toBeVisible();
	await expect(page.locator('[data-testid="collection-row"][aria-rowindex="2"]')).toHaveCount(0);
	expect(await rows.count(), 'still only a window at the end').toBeLessThan(RECORDS);

	// The standalone Record route reads the owning Collection's schema through the record.
	const recordId = recordIds[3];
	await page.goto(`/workspace/${workspace.id}/flow/${recordId}`);
	await expect(page.getByTestId('flow-record-route')).toBeVisible();
	await expect.poll(() => schemaReads.includes(recordId)).toBe(true);
	expect(schemaReads, 'no Collection-scoped schema read is needed for it').not.toContain(
		collectionId
	);
	await context.close();
});
