import { execFileSync } from 'node:child_process';
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
 * After the commit the wizard lands on the conversion job page
 * (`/workspace/{id}/flow/conversions/{job_id}`, FP-N5). The page must reach the `completed`
 * terminal status, show the source Page (linked, with its title), frontier, schema version and
 * lineage exactly as the API reports them, and link the created record to its Forms record page,
 * which the spec then opens. A random job id, another user's job id and a malformed id all get
 * the same safe empty state.
 *
 * The production API never stores a `failed` job (its only producer, the conversion fault hook in
 * `apps/api/src/flow/bridge.rs`, is compiled for tests only), so the retry path is driven against
 * a completed job that the spec marks `failed` directly in PostgreSQL. That needs:
 *
 *   E2E_DATABASE_URL      libpq URL of the database the API under test uses, e.g.
 *                         `postgres://user:pass@127.0.0.1:5432/sylvode`; `psql` must be on PATH.
 *   E2E_REQUIRE_DATABASE  set to `1` where the retry test must run (a release gate, CI): a missing
 *                         E2E_DATABASE_URL then FAILS the test instead of skipping it.
 *
 * Without E2E_DATABASE_URL (and without E2E_REQUIRE_DATABASE=1) the retry test is skipped through
 * `test.skip(condition, reason)`: the reason is printed to the run output and recorded as the
 * test's `skip` annotation, and the summary counts it as skipped -- "2 passed, 1 skipped" is not
 * a pass of the retry path.
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
	lineage_id: string | null;
	warnings: unknown[];
	error: string | null;
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
	const jobReadPromise = page.waitForResponse(
		(res) =>
			res.request().method() === 'GET' &&
			/^\/api\/v1\/flow\/conversions\/[^/]+$/.test(new URL(res.url()).pathname)
	);
	await convert.click();
	const committed = ((await (await commitPromise).json()) as Envelope<Job>).data;
	await expect(page).toHaveURL(
		new RegExp(`/workspace/${workspace.id}/flow/conversions/${committed.job_id}$`)
	);
	// The landing page reads the job itself (FP-N5), through the contract path.
	const jobRead = await jobReadPromise;
	expect(new URL(jobRead.url()).pathname).toBe(`/api/v1/flow/conversions/${committed.job_id}`);
	expect(((await jobRead.json()) as Envelope<Job>).data.status).toBe('completed');

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

	// ---- the conversion job page (FP-N5) shows the job exactly as the API reports it ----
	await expect(page).toHaveTitle('Sylvode - Conversion Job');
	const status = page.getByTestId('flow-conversion-status');
	await expect(status).toHaveAttribute('data-status', 'completed');
	await expect(status).toHaveText('Completed');
	await expect(page.getByTestId('flow-conversion-polling')).toHaveCount(0);
	await expect(page.getByTestId('flow-conversion-job-id')).toHaveText(committed.job_id);
	const sourceLink = page.getByTestId('flow-conversion-source-link');
	await expect(sourceLink).toHaveText(renamed);
	await expect(sourceLink).toHaveAttribute('href', `/workspace/${workspace.id}/flow/${objectId}`);
	await expect(page.getByTestId('flow-conversion-frontier')).toHaveText(job.source_frontier);
	await expect(page.getByTestId('flow-conversion-schema-version')).toHaveText(
		String(job.target_schema_version)
	);
	expect(job.lineage_id).toBeTruthy();
	await expect(page.getByTestId('flow-conversion-lineage')).toHaveText(job.lineage_id ?? '');
	await expect(page.getByTestId('flow-conversion-warnings')).toHaveAttribute(
		'data-count',
		String(job.warnings.length)
	);
	await expect(page.getByTestId('flow-conversion-error')).toHaveCount(0);
	await expect(page.getByTestId('flow-conversion-retry')).toHaveCount(0);
	const targets = page.getByTestId('flow-conversion-target');
	await expect(targets).toHaveCount(1);
	await expect(targets.first()).toHaveAttribute('data-target-id', job.created_target_ids[0]);
	await expect(targets.first()).toHaveAttribute('data-state', 'record');
	const recordHref = `/workspace/${workspace.id}/projects/${project.id}/forms/records/${job.created_target_ids[0]}`;
	const recordLink = page.getByTestId('flow-conversion-target-link');
	await expect(recordLink).toHaveText(renamed);
	await expect(recordLink).toHaveAttribute('href', recordHref);

	// A reload of the deep link reads the job again and lands on the same terminal state.
	await page.reload();
	await expect(status).toHaveAttribute('data-status', 'completed');
	await expect(recordLink).toHaveAttribute('href', recordHref);
	await recordLink.click();
	await expect(page).toHaveURL(new RegExp(`${recordHref}$`));
	await expect(page.getByRole('heading', { level: 1, name: renamed })).toBeVisible();
});

type ConversionSetup = {
	ownerLogin: LoginData;
	auth: { Authorization: string };
	workspaceId: string;
	projectId: string;
	formId: string;
	objectId: string;
};

/** Over REST: an owner with a Flow-enabled workspace, a project, a form whose owner role may
 * create records, and one Page. */
async function conversionSetup(
	request: APIRequestContext,
	label: string
): Promise<ConversionSetup> {
	const admin = await instanceAdmin(request);
	const who = {
		email: `convert-${label}-${runId}@e2e.sylvode.test`,
		password: `Convert-${label}-${runId}-1!`
	};
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...who, name: `Convert ${label}` }
		})
	);
	const ownerLogin = await login(request, who);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const workspace = await unwrap<{ id: string }>(
		await request.post('/api/v1/workspaces', {
			headers: auth,
			data: {
				slug: `cnv-${label}-${runId}`,
				name: `Convert ${label} ${runId}`
			}
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
			data: { name: `Planning ${label} ${runId}`, key: 'PLN' }
		})
	);
	const form = await unwrap<{ id: string }>(
		await request.post(`/api/v1/projects/${project.id}/forms`, {
			headers: auth,
			data: {
				key: 'plans',
				name: 'Plans',
				schema: {
					version: 'openpr.form.schema.v1',
					fields: [{ field_id: 'fld_note', key: 'note', label: 'Note', type: 'text' }]
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
	const created = await unwrap<{ object: { id: string } }>(
		await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
			headers: auth,
			data: {
				object_type: 'page',
				title: `Retry plan ${label} ${runId}`,
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	return {
		ownerLogin,
		auth,
		workspaceId: workspace.id,
		projectId: project.id,
		formId: form.id,
		objectId: created.object.id
	};
}

/** Preview + commit over REST; returns the committed job and its preview id. */
async function convertOverRest(
	request: APIRequestContext,
	setup: ConversionSetup
): Promise<{ job: Job; previewId: string }> {
	const source = await unwrap<ObjectView>(
		await request.get(`/api/v1/flow/objects/${setup.objectId}`, {
			headers: setup.auth
		})
	);
	const preview = await unwrap<Preview>(
		await request.post('/api/v1/flow/conversions/preview', {
			headers: setup.auth,
			data: {
				source_object_id: setup.objectId,
				source_frontier: source.frontier,
				target_type: 'form_record',
				mapping: {
					target_form_id: setup.formId,
					values: { note: 'from REST' }
				},
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	const job = await unwrap<Job>(
		await request.post('/api/v1/flow/conversions', {
			headers: setup.auth,
			data: {
				preview_id: preview.preview_id,
				source_frontier: preview.source_frontier,
				target_schema_version: preview.target_schema_version,
				idempotency_key: crypto.randomUUID(),
				confirm: true
			}
		})
	);
	expect(job.status).toBe('completed');
	return { job, previewId: preview.preview_id };
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** Runs one statement against the API's database with `:'id'` bound by psql. */
function sql(statement: string, id: string): void {
	expect(id).toMatch(UUID);
	execFileSync(
		'psql',
		[process.env.E2E_DATABASE_URL ?? '', '-X', '-q', '-v', 'ON_ERROR_STOP=1', '-v', `id=${id}`],
		{ input: statement, stdio: ['pipe', 'ignore', 'inherit'] }
	);
}

/** The fixture the production API cannot produce: a job in the backend's `failed` state with no
 * created target, as `record_failed_conversion` leaves it. */
function markJobFailed(jobId: string): void {
	sql(
		"UPDATE flow_conversion_jobs SET status = 'failed', error_code = 'injected_before_target_create', lineage_id = NULL, created_target_ids = ARRAY[]::uuid[] WHERE id = :'id';",
		jobId
	);
}

test('a random, malformed or foreign conversion job id gets the same safe empty state', async ({
	page,
	request
}) => {
	const owner = await conversionSetup(request, 'safe');
	const other = await conversionSetup(request, 'other');
	const { job: foreignJob } = await convertOverRest(request, other);

	const reads: string[] = [];
	page.on('request', (req: Request) => {
		const path = new URL(req.url()).pathname;
		if (path.startsWith('/api/v1/flow/conversions/') || path.startsWith('/api/v1/flow/objects/'))
			reads.push(path);
	});
	await signInAs(page, owner.ownerLogin);
	const notFound = page.getByTestId('flow-conversion-not-found');
	const empty = 'This conversion job does not exist, or you do not have access to it.';

	for (const jobId of [crypto.randomUUID(), foreignJob.job_id]) {
		const readPromise = page.waitForResponse(
			(res) => new URL(res.url()).pathname === `/api/v1/flow/conversions/${jobId}`
		);
		await page.goto(`/workspace/${owner.workspaceId}/flow/conversions/${jobId}`);
		const read = (await (await readPromise).json()) as Envelope<null>;
		expect(read.code).toBe(404);
		await expect(page).toHaveTitle('Sylvode - Conversion Job');
		await expect(notFound).toBeVisible();
		await expect(notFound).toContainText(empty);
		await expect(page.getByTestId('flow-conversion-status')).toHaveCount(0);
		await expect(page.getByTestId('flow-conversion-retry')).toHaveCount(0);
		await expect(page.getByTestId('flow-conversion-source')).toHaveCount(0);
		// The static `conversions` segment wins over the sibling `[objectId]` routes.
		await expect(page.getByTestId('flow-convert')).toHaveCount(0);
	}

	await page.goto(`/workspace/${owner.workspaceId}/flow/conversions/not-a-job-id`);
	await expect(notFound).toBeVisible();
	await expect(notFound).toContainText(empty);

	// The navigator reads the owner's own workspace objects. Neither the `conversions` segment nor
	// the foreign job's source Page is ever read as an object.
	const objectReads = reads.filter((path) => path.startsWith('/api/v1/flow/objects/'));
	expect(objectReads.filter((path) => path.startsWith('/api/v1/flow/objects/conversions'))).toEqual(
		[]
	);
	expect(objectReads.filter((path) => path.includes(other.objectId))).toEqual([]);
	expect(reads.filter((path) => path.includes('not-a-job-id'))).toEqual([]);
});

test('a failed job is retried only after confirmation, with one key per job; a permanent rejection withdraws retry', async ({
	page,
	request
}) => {
	const missingDatabase = !process.env.E2E_DATABASE_URL;
	const skipReason =
		'E2E_DATABASE_URL is not set: the production API never stores a failed job, so the retry fixture is written in SQL';
	if (missingDatabase) {
		expect(
			process.env.E2E_REQUIRE_DATABASE,
			`${skipReason}; E2E_REQUIRE_DATABASE=1 forbids skipping the retry test`
		).not.toBe('1');
		console.warn(`[flow-convert] SKIPPED retry test: ${skipReason}`);
	}
	test.skip(missingDatabase, skipReason);
	const setup = await conversionSetup(request, 'retry');
	const first = await convertOverRest(request, setup);
	markJobFailed(first.job.job_id);

	const retryPath = `/api/v1/flow/conversions/${first.job.job_id}/retry`;
	const retryBodies: Array<Record<string, unknown>> = [];
	page.on('request', (req: Request) => {
		if (req.method() === 'POST' && new URL(req.url()).pathname.endsWith('/retry'))
			retryBodies.push(req.postDataJSON() as Record<string, unknown>);
	});
	// The first retry answer is a synthetic 503, so the second click must resend the same key.
	let interceptedOnce = false;
	await page.route(`**${retryPath}`, async (route) => {
		if (interceptedOnce) return route.continue();
		interceptedOnce = true;
		return route.fulfill({
			contentType: 'application/json',
			body: JSON.stringify({ code: 503, message: 'unavailable', data: null })
		});
	});

	await signInAs(page, setup.ownerLogin);
	await page.goto(`/workspace/${setup.workspaceId}/flow/conversions/${first.job.job_id}`);
	const status = page.getByTestId('flow-conversion-status');
	await expect(status).toHaveAttribute('data-status', 'failed');
	await expect(status).toHaveText('Failed');
	await expect(page.getByTestId('flow-conversion-error')).toHaveText(
		'The conversion failed. Nothing was created by the failed attempt.'
	);
	await expect(page.getByTestId('flow-conversion-targets-empty')).toBeVisible();
	await expect(page.getByTestId('flow-conversion-lineage')).toHaveText('No lineage recorded yet.');

	const retry = page.getByTestId('flow-conversion-retry');
	const dialog = page.getByTestId('flow-conversion-retry-dialog');
	await expect(retry).toBeEnabled();
	await retry.click();
	await expect(dialog).toBeVisible();
	await expect(page.getByTestId('flow-conversion-retry-cancel')).toBeFocused();
	await page.getByTestId('flow-conversion-retry-cancel').click();
	await expect(dialog).toHaveCount(0);
	expect(retryBodies).toHaveLength(0);

	await retry.click();
	await page.getByTestId('flow-conversion-retry-confirm').click();
	const retryError = page.getByTestId('flow-conversion-retry-error');
	await expect(retryError).toHaveAttribute('data-kind', 'unavailable');
	await expect(retry).toBeEnabled();
	expect(retryBodies).toHaveLength(1);

	const retryResponse = page.waitForResponse(
		(res) => new URL(res.url()).pathname === retryPath && res.request().method() === 'POST'
	);
	await retry.click();
	await page.getByTestId('flow-conversion-retry-confirm').click();
	const retried = ((await (await retryResponse).json()) as Envelope<Job>).data;
	expect(retryBodies).toHaveLength(2);
	expect(retryBodies[0]).toEqual({
		idempotency_key: expect.any(String),
		confirm: true
	});
	expect(retryBodies[1]).toEqual(retryBodies[0]);
	expect(retried.status).toBe('completed');
	expect(retried.created_target_ids).toHaveLength(1);

	await expect(status).toHaveAttribute('data-status', 'completed');
	await expect(retry).toHaveCount(0);
	await expect(retryError).toHaveCount(0);
	const target = page.getByTestId('flow-conversion-target');
	await expect(target).toHaveAttribute('data-state', 'record');
	await expect(page.getByTestId('flow-conversion-target-link')).toHaveAttribute(
		'href',
		`/workspace/${setup.workspaceId}/projects/${setup.projectId}/forms/records/${retried.created_target_ids[0]}`
	);
	const afterRetry = await unwrap<Job>(
		await request.get(`/api/v1/flow/conversions/${first.job.job_id}`, {
			headers: setup.auth
		})
	);
	expect(afterRetry.status).toBe('completed');
	expect(afterRetry.created_target_ids).toEqual(retried.created_target_ids);

	// A failed job whose preview has expired: the retry is a permanent `policy_rejected`, so the
	// action is withdrawn and the page offers a new conversion from the Page instead.
	const second = await convertOverRest(request, setup);
	markJobFailed(second.job.job_id);
	sql(
		"UPDATE flow_conversion_previews SET expires_at = now() - interval '1 second' WHERE id = :'id';",
		second.previewId
	);
	await page.goto(`/workspace/${setup.workspaceId}/flow/conversions/${second.job.job_id}`);
	await expect(status).toHaveAttribute('data-status', 'failed');
	const rejectedResponse = page.waitForResponse(
		(res) => new URL(res.url()).pathname === `/api/v1/flow/conversions/${second.job.job_id}/retry`
	);
	await retry.click();
	await page.getByTestId('flow-conversion-retry-confirm').click();
	const rejected = (await (await rejectedResponse).json()) as Envelope<null>;
	expect(rejected.error_code).toBe('policy_rejected');
	await expect(retryError).toHaveAttribute('data-kind', 'policy_rejected');
	await expect(retry).toHaveCount(0);
	await expect(page.getByTestId('flow-conversion-new')).toHaveAttribute(
		'href',
		`/workspace/${setup.workspaceId}/flow/${setup.objectId}/convert`
	);
	const stillFailed = await unwrap<Job>(
		await request.get(`/api/v1/flow/conversions/${second.job.job_id}`, {
			headers: setup.auth
		})
	);
	expect(stillFailed.status).toBe('failed');
});

test('390x844: the navigator is a closed drawer and the object, convert and job pages use the full width', async ({
	page,
	request
}, testInfo) => {
	const setup = await conversionSetup(request, 'narrow');
	const { job } = await convertOverRest(request, setup);
	await page.setViewportSize({ width: 390, height: 844 });
	await signInAs(page, setup.ownerLogin);

	const toggle = page.getByTestId('flow-nav-toggle');
	const drawer = page.getByTestId('flow-nav-drawer');
	const content = page.getByTestId('flow-content');

	async function expectTarget(locator: ReturnType<Page['locator']>, label: string): Promise<void> {
		const box = await locator.boundingBox();
		expect(box, `${label} has a box`).not.toBeNull();
		expect(box!.width, `${label} width`).toBeGreaterThanOrEqual(44);
		expect(box!.height, `${label} height`).toBeGreaterThanOrEqual(44);
	}

	async function expectFullWidthLayout(name: string): Promise<void> {
		await expect(toggle).toBeVisible();
		await expect(toggle).toHaveAttribute('aria-expanded', 'false');
		await expect(drawer).toBeHidden();
		await expectTarget(toggle, `${name}: navigator toggle`);
		const box = await content.boundingBox();
		expect(box, `${name}: content box`).not.toBeNull();
		expect(box!.width, `${name}: content width`).toBeGreaterThanOrEqual(300);
		expect(box!.x + box!.width, `${name}: content right edge inside the viewport`).toBeLessThanOrEqual(390);
		const overflow = await page.evaluate(
			() => document.documentElement.scrollWidth - document.documentElement.clientWidth
		);
		expect(overflow, `${name}: no horizontal page scroll`).toBeLessThanOrEqual(0);
		await page.screenshot({ path: testInfo.outputPath(`${name}-390x844.png`), fullPage: true });
	}

	// ---- object page ----
	await page.goto(`/workspace/${setup.workspaceId}/flow/${setup.objectId}`);
	await expect(page.getByRole('link', { name: /Convert to Forms/ })).toBeVisible();
	await expectFullWidthLayout('object');
	// The context panel stacks under the canvas instead of being clipped at the right edge.
	const convertLink = page.getByRole('link', { name: /Convert to Forms/ });
	await convertLink.scrollIntoViewIfNeeded();
	const linkBox = await convertLink.boundingBox();
	expect(linkBox, 'object: convert link box').not.toBeNull();
	expect(linkBox!.x, 'object: convert link left edge').toBeGreaterThanOrEqual(0);
	expect(linkBox!.x + linkBox!.width, 'object: convert link right edge').toBeLessThanOrEqual(390);
	await expectTarget(convertLink, 'object: convert link');
	const panelBox = await content.locator('aside').boundingBox();
	expect(panelBox, 'object: context panel box').not.toBeNull();
	expect(panelBox!.width, 'object: context panel width').toBeGreaterThanOrEqual(300);
	expect(panelBox!.x + panelBox!.width, 'object: context panel right edge').toBeLessThanOrEqual(390);
	// The drawer opens from the toggle, shows the navigator over the content, and Escape closes it.
	await toggle.click();
	await expect(toggle).toHaveAttribute('aria-expanded', 'true');
	await expect(drawer).toBeVisible();
	await expect(drawer.getByRole('navigation')).toBeVisible();
	await page.keyboard.press('Escape');
	await expect(toggle).toHaveAttribute('aria-expanded', 'false');
	await expect(drawer).toBeHidden();

	// ---- convert wizard ----
	await page.goto(`/workspace/${setup.workspaceId}/flow/${setup.objectId}/convert`);
	const next = page.getByTestId('flow-convert-next');
	await expect(next).toBeVisible();
	await expectFullWidthLayout('convert');
	await expectTarget(next, 'convert: Next');
	await expectTarget(page.getByRole('button', { name: 'Back', exact: true }), 'convert: Back');
	for (const [index, step] of (await content.getByRole('listitem').getByRole('button').all()).entries()) {
		await expectTarget(step, `convert: step ${index + 1}`);
	}

	// ---- conversion job page ----
	await page.goto(`/workspace/${setup.workspaceId}/flow/conversions/${job.job_id}`);
	await expect(page.getByTestId('flow-conversion-status')).toHaveAttribute('data-status', 'completed');
	await expectFullWidthLayout('job');
	for (const [index, button] of (await content.getByRole('button').all()).entries()) {
		if (await button.isVisible()) await expectTarget(button, `job: button ${index + 1}`);
	}
});
