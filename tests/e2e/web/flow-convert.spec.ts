import { expect, test, type APIRequestContext, type Page, type Request } from '@playwright/test';

/**
 * Flow Page -> Universal Forms record conversion wizard (`/workspace/{id}/flow/{objectId}/convert`)
 * against a real API + PostgreSQL.
 *
 * Over REST the owner creates a workspace with Flow enabled, a project, a form with a required
 * text field, a decimal field and a single-select field, an explicit owner policy that allows
 * `record.create` (ADR-0019 BR-4: without one the bridge only allows viewing), and one Page. In
 * the browser the owner opens the Page, enters the wizard from the context panel, checks the
 * source step (title, document sequence, frontier equal to a direct GET), picks the project and
 * form, fills the values, previews (schema version, object count, countdown, permission
 * explanation), and goes to confirm. The Page is then renamed over REST, so the commit is
 * rejected with `stale_frontier`; the wizard re-reads the Page, previews again under a new key and
 * the second commit succeeds. The wire log proves both commits copy `preview_id`,
 * `source_frontier` and `target_schema_version` from their own preview response, and the API
 * shows a completed job whose created record carries the mapped values.
 *
 * FP-N5 (the conversion job page) is not built yet: after the commit the wizard navigates to
 * `/workspace/{id}/flow/conversions/{job_id}`, and this spec asserts that URL and checks the job
 * over REST instead of on the page.
 *
 * Needs BASE_URL to serve the built frontend with `/api` proxied to the API on the same origin
 * (as `frontend/nginx.conf` does). Registration after the first account needs an instance admin:
 * ADMIN_EMAIL / ADMIN_PASSWORD when both are set, otherwise the run registers the instance's
 * first account itself (a fresh database).
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = {
	email: `convert-owner-${runId}@e2e.sylvode.test`,
	password: `ConvertOwner-${runId}-1!`
};

type Envelope<T> = { code: number; message: string; data: T; error_code?: string };
type LoginData = {
	tokens: { access_token: string; refresh_token: string };
	user: { id: string };
};
type ObjectView = {
	id: string;
	title: string;
	object_type: string;
	document_seq: number;
	frontier: string;
};
type Preview = {
	preview_id: string;
	source_frontier: string;
	target_schema_version: number;
	estimated_objects: number;
};
type Job = {
	job_id: string;
	status: string;
	created_target_ids: string[];
	source_frontier: string;
	target_schema_version: number;
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
	const admin = {
		email: `convert-admin-${runId}@e2e.sylvode.test`,
		password: `ConvertAdmin-${runId}-1!`
	};
	await unwrap(
		await request.post('/api/v1/auth/register', { data: { ...admin, name: 'Convert Admin' } })
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

test('owner converts a Page into a Forms record through source, mapping, preview and confirm, recovering from a stale frontier', async ({
	page,
	request
}) => {
	const admin = await instanceAdmin(request);
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...owner, name: 'Convert Owner' }
		})
	);
	const ownerLogin = await login(request, owner);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: { slug: `cnv-${runId}`, name: `Convert ${runId}` }
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
			data: { name: `Planning ${runId}`, key: 'PLN' }
		})
	);
	const form = await unwrap<{ id: string; schema_version: number }>(
		await request.post(`/api/v1/projects/${project.id}/forms`, {
			headers: auth,
			data: {
				key: 'plans',
				name: 'Plans',
				schema: {
					version: 'openpr.form.schema.v1',
					fields: [
						{ field_id: 'fld_note', key: 'note', label: 'Note', type: 'text', required: true },
						{ field_id: 'fld_budget', key: 'budget', label: 'Budget', type: 'number' },
						{
							field_id: 'fld_stage',
							key: 'stage',
							label: 'Stage',
							type: 'single_select',
							options: ['draft', 'final']
						}
					]
				}
			}
		})
	);
	await unwrap(
		await request.patch(`/api/v1/forms/${form.id}/permissions`, {
			headers: auth,
			data: {
				policies: [
					{
						subject_type: 'role',
						subject_id: 'owner',
						policy: { actions: { 'form.view': true, 'record.create': true } }
					}
				]
			}
		})
	);
	const pageTitle = `Launch plan ${runId}`;
	const created = await unwrap<{ object: { id: string } }>(
		await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
			headers: auth,
			data: { object_type: 'page', title: pageTitle, idempotency_key: crypto.randomUUID() }
		})
	);
	const objectId = created.object.id;

	const previewBodies: Array<Record<string, unknown>> = [];
	const commitBodies: Array<Record<string, unknown>> = [];
	page.on('request', (req: Request) => {
		if (req.method() !== 'POST') return;
		const path = new URL(req.url()).pathname;
		if (path === '/api/v1/flow/conversions/preview')
			previewBodies.push(req.postDataJSON() as Record<string, unknown>);
		if (path === '/api/v1/flow/conversions')
			commitBodies.push(req.postDataJSON() as Record<string, unknown>);
	});
	const previewResponse = () =>
		page.waitForResponse(
			(res) =>
				res.request().method() === 'POST' &&
				new URL(res.url()).pathname === '/api/v1/flow/conversions/preview'
		);
	const commitResponse = () =>
		page.waitForResponse(
			(res) =>
				res.request().method() === 'POST' &&
				new URL(res.url()).pathname === '/api/v1/flow/conversions'
		);

	await signInAs(page, ownerLogin);
	await page.goto(`/workspace/${workspace.id}/flow/${objectId}`);
	const convertLink = page.getByRole('link', { name: 'Convert to Forms…' });
	await expect(convertLink).toBeVisible();
	await convertLink.click();
	await expect(page).toHaveURL(new RegExp(`/workspace/${workspace.id}/flow/${objectId}/convert$`));
	await expect(page).toHaveTitle('Sylvode - Convert to Forms');

	const wizard = page.getByTestId('flow-convert');
	const step = page.getByTestId('flow-convert-step');
	const next = page.getByTestId('flow-convert-next');

	// ---- 1. source ----
	await expect(step).toHaveText('Step 1 of 4: Source');
	// Read after the object page was open: opening the editor itself can advance the document, so
	// the head the wizard shows is the one after that visit.
	const before = await unwrap<ObjectView>(
		await request.get(`/api/v1/flow/objects/${objectId}`, { headers: auth })
	);
	await expect(page.getByTestId('flow-convert-source-title')).toHaveText(pageTitle);
	await expect(page.getByTestId('flow-convert-source-seq')).toHaveText(String(before.document_seq));
	await expect(page.getByTestId('flow-convert-source-frontier')).toHaveText(before.frontier);
	await expect(wizard).toContainText('This Page stays in Flow and is not changed');
	// Later steps cannot be reached from the step navigation.
	await expect(wizard.getByRole('button', { name: 'Preview', exact: true })).toBeDisabled();
	await expect(wizard.getByRole('button', { name: 'Confirm', exact: true })).toBeDisabled();
	await next.click();

	// ---- 2. target and mapping ----
	await expect(step).toHaveText('Step 2 of 4: Target and mapping');
	await expect(step).toBeFocused();
	await expect(next).toBeDisabled();
	await page.getByLabel('Project').selectOption(project.id);
	const formSelect = page.getByLabel('Form', { exact: true });
	await expect(formSelect).toBeEnabled();
	await formSelect.selectOption(form.id);
	await expect(page.getByLabel('Record title')).toHaveValue(pageTitle);
	await expect(page.locator('#flow-convert-field-note-hint')).toHaveText('This field is required.');
	await expect(next).toBeDisabled();
	await page.locator('#flow-convert-field-budget').fill('1e3');
	await expect(page.locator('#flow-convert-field-budget-hint')).toHaveText(
		'Enter a number, for example 12.5.'
	);
	await page.locator('#flow-convert-field-budget').fill('12.5');
	await page.locator('#flow-convert-field-note').fill('Converted from Flow');
	await page.locator('#flow-convert-field-stage').selectOption('final');
	await expect(next).toBeEnabled();
	expect(previewBodies).toHaveLength(0);

	// ---- 3. preview ----
	const firstPreviewPromise = previewResponse();
	await next.click();
	const firstPreview = ((await (await firstPreviewPromise).json()) as Envelope<Preview>).data;
	await expect(step).toHaveText('Step 3 of 4: Preview');
	await expect(page.getByTestId('flow-convert-schema-version')).toHaveText(
		String(form.schema_version)
	);
	expect(firstPreview.target_schema_version).toBe(form.schema_version);
	await expect(page.getByTestId('flow-convert-estimated')).toHaveText('1');
	await expect(page.getByTestId('flow-convert-countdown')).toHaveText(/\((14|15):\d\d left\)/);
	await expect(page.getByTestId('flow-convert-permission')).toContainText('Create records');
	await expect(page.getByTestId('flow-convert-preview')).toContainText(
		'This is a preview. Nothing has been created yet.'
	);
	expect(previewBodies[0]).toEqual({
		source_object_id: objectId,
		source_frontier: before.frontier,
		target_type: 'form_record',
		mapping: {
			target_form_id: form.id,
			title: pageTitle,
			values: { note: 'Converted from Flow', budget: '12.5', stage: 'final' }
		},
		idempotency_key: expect.any(String)
	});
	await next.click();

	// ---- 4. confirm; the Page changes underneath, so the first commit is stale ----
	await expect(step).toHaveText('Step 4 of 4: Confirm');
	await expect(page.getByTestId('flow-convert-commit-schema-version')).toHaveText(
		String(form.schema_version)
	);
	const acknowledge = page.getByTestId('flow-convert-acknowledge');
	const convert = page.getByTestId('flow-convert-commit-button');
	await expect(acknowledge).not.toBeChecked();
	await expect(convert).toBeDisabled();

	const renamed = `Launch plan renamed ${runId}`;
	await unwrap(
		await request.post(`/api/v1/flow/objects/${objectId}/commands`, {
			headers: auth,
			data: {
				command: { type: 'set_title', payload: { title: renamed } },
				idempotency_key: crypto.randomUUID()
			}
		})
	);

	await acknowledge.check();
	await expect(convert).toBeEnabled();
	const staleCommitPromise = commitResponse();
	await convert.click();
	const staleCommit = (await (await staleCommitPromise).json()) as Envelope<null>;
	expect(staleCommit.error_code).toBe('stale_frontier');
	const commitError = page.getByTestId('flow-convert-commit-error');
	await expect(commitError).toHaveAttribute('data-kind', 'stale_frontier');
	await expect(convert).toBeDisabled();

	const secondPreviewPromise = previewResponse();
	await commitError.getByRole('button', { name: 'Re-read the Page and preview again' }).click();
	const secondPreview = ((await (await secondPreviewPromise).json()) as Envelope<Preview>).data;
	await expect(step).toHaveText('Step 3 of 4: Preview');
	expect(secondPreview.preview_id).not.toBe(firstPreview.preview_id);
	const after = await unwrap<ObjectView>(
		await request.get(`/api/v1/flow/objects/${objectId}`, { headers: auth })
	);
	expect(after.frontier).not.toBe(before.frontier);
	expect(secondPreview.source_frontier).toBe(after.frontier);
	expect(previewBodies).toHaveLength(2);
	expect(previewBodies[1].source_frontier).toBe(after.frontier);
	expect(previewBodies[1].idempotency_key).not.toBe(previewBodies[0].idempotency_key);

	await next.click();
	await expect(step).toHaveText('Step 4 of 4: Confirm');
	await expect(acknowledge).not.toBeChecked();
	await expect(convert).toBeDisabled();
	await acknowledge.check();
	const commitPromise = commitResponse();
	await convert.click();
	const committed = ((await (await commitPromise).json()) as Envelope<Job>).data;
	await expect(page).toHaveURL(
		new RegExp(`/workspace/${workspace.id}/flow/conversions/${committed.job_id}$`)
	);

	// ---- wire: every commit field comes from its own preview response ----
	expect(commitBodies).toHaveLength(2);
	expect(commitBodies[0]).toEqual({
		preview_id: firstPreview.preview_id,
		source_frontier: firstPreview.source_frontier,
		target_schema_version: firstPreview.target_schema_version,
		idempotency_key: expect.any(String),
		confirm: true
	});
	expect(commitBodies[1]).toEqual({
		preview_id: secondPreview.preview_id,
		source_frontier: secondPreview.source_frontier,
		target_schema_version: secondPreview.target_schema_version,
		idempotency_key: expect.any(String),
		confirm: true
	});
	expect(commitBodies[1].idempotency_key).not.toBe(commitBodies[0].idempotency_key);

	// ---- server state ----
	const job = await unwrap<Job>(
		await request.get(`/api/v1/flow/conversions/${committed.job_id}`, { headers: auth })
	);
	expect(job.status).toBe('completed');
	expect(job.created_target_ids).toHaveLength(1);
	expect(job.source_frontier).toBe(after.frontier);
	const record = await unwrap<{
		form_id: string;
		title: string;
		values: Record<string, unknown>;
		source: Record<string, unknown>;
	}>(await request.get(`/api/v1/form-records/${job.created_target_ids[0]}`, { headers: auth }));
	expect(record.form_id).toBe(form.id);
	// The title was left at its default, so it follows the Page title read for the second preview.
	expect(record.title).toBe(renamed);
	expect(record.values.note).toBe('Converted from Flow');
	expect(record.values.stage).toBe('final');
	expect(record.values.budget).toEqual({ type: 'decimal', decimal: '12.5' });
	expect(record.source.source_object_id).toBe(objectId);
	// The source Page is unchanged by the conversion.
	const source = await unwrap<ObjectView>(
		await request.get(`/api/v1/flow/objects/${objectId}`, { headers: auth })
	);
	expect(source.frontier).toBe(after.frontier);
	expect(source.title).toBe(renamed);
});
