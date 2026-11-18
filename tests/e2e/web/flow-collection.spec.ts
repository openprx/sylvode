import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

// The Pages-only browser hides the old table, board, and record editors. This gate verifies
// that the retired UI does not erase or bypass the typed Collection and Record contracts.
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

test('collection: retired browser controls, typed schema and record operations remain available', async ({
	browser,
	request
}) => {
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

	const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
	try {
		const page = await context.newPage();
		await signInAs(page, ownerLogin);
		await page.goto(`/workspace/${workspace.id}/flow`);
		const navigator = page.getByRole('navigation', { name: 'Navigator' });
		await page.getByTestId('flow-nav-new').click();
		await expect(navigator.getByRole('menuitem', { name: 'New Page' })).toBeVisible();
		await expect(navigator.getByRole('menuitem', { name: 'New Collection' })).toHaveCount(0);

		const nameId = crypto.randomUUID();
		const statusId = crypto.randomUUID();
		const tableId = crypto.randomUUID();
		const boardId = crypto.randomUUID();
		const created = await unwrap<{ object: { id: string; document_id: string } }>(
			await context.request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
				headers: auth,
				data: {
					object_type: 'collection',
					title: `Tasks ${runId}`,
					idempotency_key: crypto.randomUUID(),
					initial_fields: [
						{ field_id: nameId, label: 'Name', field_type: 'text', config: {}, index: 0 }
					],
					initial_view: {
						view_id: tableId,
						name: 'Table',
						view_type: 'table',
						config: {},
						index: 0
					}
				}
			})
		);
		const collectionId = created.object.id;
		const schema = async () =>
			unwrap<CollectionData>(
				await context.request.get(`/api/v1/flow/collections/${collectionId}`, { headers: auth })
			);
		const list = async (viewId?: string) =>
			unwrap<RecordsData>(
				await context.request.get(`/api/v1/flow/collections/${collectionId}/records`, {
					headers: auth,
					params: viewId ? { view_id: viewId } : {}
				})
			);
		const command = (type: string, payload: Record<string, unknown>, expectedFrontier?: string) =>
			context.request.post(`/api/v1/flow/objects/${collectionId}/commands`, {
				headers: auth,
				data: {
					command: { type, payload },
					expected_frontier: expectedFrontier,
					idempotency_key: crypto.randomUUID()
				}
			});
		await unwrap(
			await command(
				'field_create',
				{
					field_id: statusId,
					label: 'Status',
					field_type: 'select',
					config: { options: ['Todo', 'Done'] },
					index: 1
				},
				(await schema()).object.frontier
			)
		);
		await unwrap(
			await command(
				'view_create',
				{
					view_id: boardId,
					name: 'Board',
					view_type: 'board',
					config: {},
					index: 1
				},
				(await schema()).object.frontier
			)
		);
		expect((await schema()).views.map(({ name, type }) => [name, type])).toEqual([
			['Table', 'table'],
			['Board', 'board']
		]);
		expect((await schema()).fields.find(({ field_id }) => field_id === statusId)?.config).toEqual({
			options: ['Todo', 'Done']
		});
		const records = [];
		for (const name of ['Alpha', 'Beta']) {
			const record = await unwrap<{ object: { id: string } }>(
				await context.request.post(`/api/v1/flow/collections/${collectionId}/records`, {
					headers: auth,
					data: { values_by_field_id: { [nameId]: name }, idempotency_key: crypto.randomUUID() }
				})
			);
			records.push(record.object.id);
			const row = (await list(tableId)).items.find((item) => item.record.id === record.object.id)!;
			await unwrap(
				await command(
					'record_patch',
					{ record_id: record.object.id, properties: { [statusId]: 'Done' } },
					row.record.frontier
				)
			);
		}
		expect(
			(await list(boardId)).items
				.map(({ values_by_field_id: row }) => [row[nameId], row[statusId]])
				.sort()
		).toEqual([
			['Alpha', 'Done'],
			['Beta', 'Done']
		]);
		const body = `Body for Alpha ${runId}`;
		const first = (await list()).items.find(({ record }) => record.id === records[0])!;
		await unwrap(
			await command('record_patch', { record_id: first.record.id, body }, first.record.frontier)
		);
		const recordObject = () =>
			context.request.get(`/api/v1/flow/objects/${first.record.id}`, { headers: auth });
		const saved = await unwrap<{
			semantic_content: { nodes: Record<string, { properties: { body: string } }> };
		}>(await recordObject());
		expect(saved.semantic_content.nodes['record:body'].properties.body).toBe(body);
		await page.setViewportSize({ width: 390, height: 844 });
		const collabTraffic: string[] = [];
		page.on('request', (req) => {
			const path = new URL(req.url()).pathname;
			if (path.endsWith('/collab/tickets') && req.method() === 'POST') {
				collabTraffic.push(String(req.postDataJSON().document_id));
			}
			if (path.endsWith('/bootstrap')) collabTraffic.push(path.split('/').at(-2) ?? '');
		});
		for (const id of [collectionId, first.record.id]) {
			await page.goto(`/workspace/${workspace.id}/flow/${id}?mode=schema`);
			await expect(page.getByTestId('flow-typed-unavailable')).toBeVisible();
			await expect(page.getByTestId('collection-schema-editor')).toHaveCount(0);
			await expect(page.getByTestId('record-panel-body')).toHaveCount(0);
		}
		const typedIds = [
			collectionId,
			created.object.document_id,
			...(await list()).items.flatMap(({ record }) => [record.id, record.document_id])
		];
		expect(collabTraffic.filter((id) => typedIds.includes(id))).toEqual([]);
		await unwrap(
			await command(
				'field_create',
				{
					field_id: crypto.randomUUID(),
					label: 'Salary',
					field_type: 'number',
					config: { restricted: true },
					index: 2
				},
				(await schema()).object.frontier
			)
		);
		const withheld = await unwrap<{ semantic_content: unknown }>(await recordObject());
		expect(withheld.semantic_content).toEqual({});
		await unwrap(await command('record_archive', { record_id: records[1] }));
		expect((await list()).items.map(({ record }) => record.id)).toEqual([records[0]]);
	} finally {
		await context.close();
	}
});
