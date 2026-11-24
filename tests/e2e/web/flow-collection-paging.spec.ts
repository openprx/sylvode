import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

// The Page-only product mode retires the table and Record renderer. The existing records
// remain readable through the typed paging and record-specific schema APIs.
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

test('collection: typed records page by 50 and the collection and record routes open in the browser', async ({
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
	const reads: string[] = [];
	page.on('request', (req) => reads.push(new URL(req.url()).pathname));
	await page.goto(`/workspace/${workspace.id}/flow/${collectionId}`);
	await expect(page.getByTestId('collection-renderer')).toBeVisible();
	await expect(page.getByTestId('collection-table')).toBeVisible();
	await page.goto(`/workspace/${workspace.id}/flow/${recordIds[3]}`);
	await expect(page.getByTestId('flow-record-route')).toBeVisible();
	await expect(page.getByTestId('record-panel')).toBeVisible();
	expect(reads).not.toContain(`/api/v2/flow/objects/${collectionId}/bootstrap`);
	type RecordsPage = { items: Array<{ record: { id: string } }>; next_cursor?: string | null };
	const url = `/api/v1/flow/collections/${collectionId}/records`;
	const first = await unwrap<RecordsPage>(
		await context.request.get(url, { headers: auth, params: { limit: '50' } })
	);
	expect(first.items).toHaveLength(50);
	expect(first.next_cursor).toBeTruthy();
	const second = await unwrap<RecordsPage>(
		await context.request.get(url, {
			headers: auth,
			params: { limit: '50', cursor: first.next_cursor! }
		})
	);
	expect(second.items).toHaveLength(5);
	expect(second.next_cursor ?? null).toBeNull();
	expect(new Set([...first.items, ...second.items].map(({ record }) => record.id))).toEqual(
		new Set(recordIds)
	);
	const schema = await unwrap<{ fields: Array<{ field_id: string }> }>(
		await context.request.get(`/api/v1/flow/objects/${recordIds[3]}/schema`, { headers: auth })
	);
	expect(schema.fields.map(({ field_id }) => field_id)).toEqual([fieldId]);
	await context.close();
});
