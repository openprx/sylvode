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
 * Failed jobs come from the production path (B4): the target form's `record.create` is revoked
 * between preview and commit, the run re-checks it after the job row exists, so the job is stored
 * `failed` and the typed error carries `details.job_id`. The wizard then hands over to the job
 * page. The retry is confirmed, reuses one key across a synthetic 503, and succeeds although the
 * job's preview expired 20 minutes ago (retry does not depend on the preview TTL). A failed job
 * whose Page moved on gets a permanent `stale_frontier`, and one whose target form schema changed
 * gets `policy_rejected` / `target_schema_changed`; both withdraw retry with their own wording.
 * A job left `started` past the 15-minute threshold is reclaimed by the API's periodic reaper
 * (failed, `server_rejected`, `stale_started`), shown as an interrupted run and retried.
 * Moving the preview TTL and a stuck job's start into the past are written in SQL, so it needs:
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

/** Sets whether the owner role may create records in the setup form (production API path). */
async function setRecordCreate(
	request: APIRequestContext,
	setup: ConversionSetup,
	allowed: boolean
): Promise<void> {
	await unwrap(
		await request.patch(`/api/v1/forms/${setup.formId}/permissions`, {
			headers: setup.auth,
			data: {
				policies: [
					{
						subject_type: 'role',
						subject_id: 'owner',
						policy: { actions: { 'form.view': true, 'record.create': allowed } }
					}
				]
			}
		})
	);
}

type FailedCommit = {
	code: number;
	error_code?: string;
	data: null;
	details?: { job_id?: string; job_status?: string; reason?: string };
};

/** Production-path failed job: preview, then revoke `record.create`, then commit. The commit's
 * run re-checks the target permission after the job row exists, so the job is stored `failed`
 * (error `policy_rejected`) and the typed error names it (`details.job_id`, B4). The permission
 * is granted again afterwards, so a retry can succeed. */
async function failedJobOverRest(
	request: APIRequestContext,
	setup: ConversionSetup
): Promise<{ jobId: string; previewId: string }> {
	const source = await unwrap<ObjectView>(
		await request.get(`/api/v1/flow/objects/${setup.objectId}`, { headers: setup.auth })
	);
	const preview = await unwrap<Preview>(
		await request.post('/api/v1/flow/conversions/preview', {
			headers: setup.auth,
			data: {
				source_object_id: setup.objectId,
				source_frontier: source.frontier,
				target_type: 'form_record',
				mapping: { target_form_id: setup.formId, values: { note: 'from REST' } },
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	await setRecordCreate(request, setup, false);
	const response = await request.post('/api/v1/flow/conversions', {
		headers: setup.auth,
		data: {
			preview_id: preview.preview_id,
			source_frontier: preview.source_frontier,
			target_schema_version: preview.target_schema_version,
			idempotency_key: crypto.randomUUID(),
			confirm: true
		}
	});
	const refused = (await response.json()) as FailedCommit;
	expect(refused.error_code).toBe('policy_rejected');
	expect(refused.details?.job_status).toBe('failed');
	expect(refused.details?.job_id).toMatch(UUID);
	await setRecordCreate(request, setup, true);
	return { jobId: refused.details?.job_id ?? '', previewId: preview.preview_id };
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

test('a failed job is retried only after confirmation, with one key per job, independent of the preview TTL; stale or schema-changed retries withdraw retry', async ({
	page,
	request
}) => {
	const missingDatabase = !process.env.E2E_DATABASE_URL;
	const skipReason =
		'E2E_DATABASE_URL is not set: the preview TTL is moved into the past in SQL to prove a retry does not depend on it';
	if (missingDatabase) {
		expect(
			process.env.E2E_REQUIRE_DATABASE,
			`${skipReason}; E2E_REQUIRE_DATABASE=1 forbids skipping the retry test`
		).not.toBe('1');
		console.warn(`[flow-convert] SKIPPED retry test: ${skipReason}`);
	}
	test.skip(missingDatabase, skipReason);
	const setup = await conversionSetup(request, 'retry');

	// ---- 1. the wizard commit fails after its job exists: the wizard hands over to the job ----
	await signInAs(page, setup.ownerLogin);
	await page.goto(`/workspace/${setup.workspaceId}/flow/${setup.objectId}/convert`);
	const step = page.getByTestId('flow-convert-step');
	const next = page.getByTestId('flow-convert-next');
	await expect(step).toHaveText('Step 1 of 4: Source');
	await next.click();
	await page.getByLabel('Project').selectOption(setup.projectId);
	const formSelect = page.getByLabel('Form', { exact: true });
	await expect(formSelect).toBeEnabled();
	await formSelect.selectOption(setup.formId);
	// `note` is left empty on purpose: the preview warns about it (`<code>:<field_key>`).
	await next.click();
	await expect(step).toHaveText('Step 3 of 4: Preview');
	const warning = page.getByTestId('flow-convert-warning');
	await expect(warning).toHaveCount(1);
	await expect(warning).toHaveAttribute('data-code', 'target_value_missing');
	await expect(warning).toHaveText('No value for field "Note"; it will be left empty.');
	await next.click();
	await expect(step).toHaveText('Step 4 of 4: Confirm');
	// The target permission is revoked between preview and commit.
	await setRecordCreate(request, setup, false);
	await page.getByTestId('flow-convert-acknowledge').check();
	const wizardCommit = page.waitForResponse(
		(res) =>
			res.request().method() === 'POST' &&
			new URL(res.url()).pathname === '/api/v1/flow/conversions'
	);
	await page.getByTestId('flow-convert-commit-button').click();
	const refused = (await (await wizardCommit).json()) as FailedCommit;
	expect(refused.error_code).toBe('policy_rejected');
	expect(refused.details?.job_status).toBe('failed');
	const firstJobId = refused.details?.job_id ?? '';
	expect(firstJobId).toMatch(UUID);
	await expect(page).toHaveURL(
		new RegExp(`/workspace/${setup.workspaceId}/flow/conversions/${firstJobId}$`)
	);
	await setRecordCreate(request, setup, true);

	const status = page.getByTestId('flow-conversion-status');
	await expect(status).toHaveAttribute('data-status', 'failed');
	await expect(status).toHaveText('Failed');
	const storedError = page.getByTestId('flow-conversion-error');
	await expect(storedError).toHaveAttribute('data-code', 'policy_rejected');
	await expect(storedError).toHaveText('That action was rejected by policy.');
	await expect(page.getByTestId('flow-conversion-target-type')).toHaveText('A Forms record');
	await expect(page.getByTestId('flow-conversion-targets-empty')).toBeVisible();
	await expect(page.getByTestId('flow-conversion-lineage')).toHaveText('No lineage recorded yet.');
	const firstFailed = await unwrap<Job>(
		await request.get(`/api/v1/flow/conversions/${firstJobId}`, { headers: setup.auth })
	);
	expect(firstFailed.status).toBe('failed');
	expect(firstFailed.error).toBe('policy_rejected');
	expect(firstFailed.created_target_ids).toEqual([]);

	// The job's preview is long expired: the retry runs against the job's own frontier and
	// schema version, not the preview TTL (`rest-api-v1.md` v0.7, 2026-10-10).
	sql(
		"UPDATE flow_conversion_previews SET expires_at = now() - interval '20 minutes' WHERE id = (SELECT preview_id FROM flow_conversion_jobs WHERE id = :'id');",
		firstJobId
	);

	const retryPath = `/api/v1/flow/conversions/${firstJobId}/retry`;
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
	expect(retried.job_id).toBe(firstJobId);
	expect(retried.status).toBe('completed');
	expect(retried.error).toBeNull();
	expect(retried.created_target_ids).toHaveLength(1);

	await expect(status).toHaveAttribute('data-status', 'completed');
	await expect(retry).toHaveCount(0);
	await expect(retryError).toHaveCount(0);
	await expect(storedError).toHaveCount(0);
	const target = page.getByTestId('flow-conversion-target');
	await expect(target).toHaveAttribute('data-state', 'record');
	await expect(page.getByTestId('flow-conversion-target-link')).toHaveAttribute(
		'href',
		`/workspace/${setup.workspaceId}/projects/${setup.projectId}/forms/records/${retried.created_target_ids[0]}`
	);
	const afterRetry = await unwrap<Job>(
		await request.get(`/api/v1/flow/conversions/${firstJobId}`, { headers: setup.auth })
	);
	expect(afterRetry.status).toBe('completed');
	expect(afterRetry.created_target_ids).toEqual(retried.created_target_ids);

	// ---- 2. the Page moved on after the failure: the retry is a permanent `stale_frontier` ----
	const second = await failedJobOverRest(request, setup);
	await unwrap(
		await request.post(`/api/v1/flow/objects/${setup.objectId}/commands`, {
			headers: setup.auth,
			data: {
				command: { type: 'set_title', payload: { title: `Retry plan moved ${runId}` } },
				idempotency_key: crypto.randomUUID()
			}
		})
	);
	await page.goto(`/workspace/${setup.workspaceId}/flow/conversions/${second.jobId}`);
	await expect(status).toHaveAttribute('data-status', 'failed');
	const staleResponse = page.waitForResponse(
		(res) => new URL(res.url()).pathname === `/api/v1/flow/conversions/${second.jobId}/retry`
	);
	await retry.click();
	await page.getByTestId('flow-conversion-retry-confirm').click();
	const stale = (await (await staleResponse).json()) as FailedCommit;
	expect(stale.error_code).toBe('stale_frontier');
	await expect(retryError).toHaveAttribute('data-kind', 'stale_frontier');
	await expect(retryError).toHaveText(
		'The Page has changed since the preview, so this job cannot be retried. Start a new conversion from the Page.'
	);
	await expect(retry).toHaveCount(0);
	await expect(page.getByTestId('flow-conversion-new')).toHaveAttribute(
		'href',
		`/workspace/${setup.workspaceId}/flow/${setup.objectId}/convert`
	);
	// The refused retry re-reads the job: its stored error is now the stale frontier.
	await expect(storedError).toHaveAttribute('data-code', 'stale_frontier');
	const stillStale = await unwrap<Job>(
		await request.get(`/api/v1/flow/conversions/${second.jobId}`, { headers: setup.auth })
	);
	expect(stillStale.status).toBe('failed');
	expect(stillStale.error).toBe('stale_frontier');

	// ---- 3. the target form schema changed after the failure: `target_schema_changed` ----
	const third = await failedJobOverRest(request, setup);
	await unwrap(
		await request.patch(`/api/v1/forms/${setup.formId}`, {
			headers: setup.auth,
			data: {
				schema: {
					version: 'openpr.form.schema.v1',
					fields: [
						{ field_id: 'fld_note', key: 'note', label: 'Note', type: 'text' },
						{ field_id: 'fld_extra', key: 'extra', label: 'Extra', type: 'text' }
					]
				}
			}
		})
	);
	await page.goto(`/workspace/${setup.workspaceId}/flow/conversions/${third.jobId}`);
	await expect(status).toHaveAttribute('data-status', 'failed');
	const schemaResponse = page.waitForResponse(
		(res) => new URL(res.url()).pathname === `/api/v1/flow/conversions/${third.jobId}/retry`
	);
	await retry.click();
	await page.getByTestId('flow-conversion-retry-confirm').click();
	const schema = (await (await schemaResponse).json()) as FailedCommit;
	expect(schema.error_code).toBe('policy_rejected');
	expect(schema.details?.reason).toBe('target_schema_changed');
	await expect(retryError).toHaveAttribute('data-kind', 'policy_rejected');
	await expect(retryError).toHaveAttribute('data-reason', 'target_schema_changed');
	await expect(retryError).toHaveText(
		"The target form's fields changed since this job's preview, so this job cannot be retried. Start a new conversion from the Page."
	);
	await expect(retry).toHaveCount(0);
	const stillSchema = await unwrap<Job>(
		await request.get(`/api/v1/flow/conversions/${third.jobId}`, { headers: setup.auth })
	);
	expect(stillSchema.status).toBe('failed');
	expect(stillSchema.created_target_ids).toEqual([]);

	// ---- 4. a job the server reclaimed (`error_reason = "stale_started"`) is an interrupted run:
	// worded as such and retryable, while a plain `server_rejected` job stays permanent ----
	// The fixture only builds the reaper's INPUT: a job left `started` whose start lies 16 minutes
	// back (`started_at`, past the 15-minute threshold; `updated_at` stays fresh and does not
	// count). The result -- `failed`, `server_rejected`, `stale_started` -- is written by the
	// API's own periodic reaper (`[flow] conversion_reap_interval_secs = 2` in the stack config).
	const reaped = await failedJobOverRest(request, setup);
	sql(
		"UPDATE flow_conversion_jobs SET status = 'started', error_code = NULL, error_reason = NULL, started_at = now() - interval '16 minutes', updated_at = now() WHERE id = :'id';",
		reaped.jobId
	);
	await expect
		.poll(
			async () => {
				const job = await unwrap<Job & { error_reason?: string }>(
					await request.get(`/api/v1/flow/conversions/${reaped.jobId}`, { headers: setup.auth })
				);
				return [job.status, job.error, job.error_reason ?? null];
			},
			{ message: 'the periodic reaper fails the stale started job', timeout: 20_000 }
		)
		.toEqual(['failed', 'server_rejected', 'stale_started']);
	await page.goto(`/workspace/${setup.workspaceId}/flow/conversions/${reaped.jobId}`);
	await expect(status).toHaveAttribute('data-status', 'failed');
	await expect(storedError).toHaveAttribute('data-code', 'server_rejected');
	await expect(storedError).toHaveAttribute('data-reason', 'stale_started');
	await expect(storedError).toHaveText(
		'Interrupted (reclaimed by the server): the conversion ran too long and the server reclaimed it. Nothing was created by the interrupted run; you can retry it.'
	);
	await expect(retry).toBeEnabled();
	const reapedRetry = page.waitForResponse(
		(res) => new URL(res.url()).pathname === `/api/v1/flow/conversions/${reaped.jobId}/retry`
	);
	await retry.click();
	await page.getByTestId('flow-conversion-retry-confirm').click();
	const reapedRetried = ((await (await reapedRetry).json()) as Envelope<Job>).data;
	expect(reapedRetried.status).toBe('completed');
	await expect(status).toHaveAttribute('data-status', 'completed');

	const refusedForGood = await failedJobOverRest(request, setup);
	sql(
		"UPDATE flow_conversion_jobs SET error_code = 'server_rejected' WHERE id = :'id';",
		refusedForGood.jobId
	);
	await page.goto(`/workspace/${setup.workspaceId}/flow/conversions/${refusedForGood.jobId}`);
	await expect(status).toHaveAttribute('data-status', 'failed');
	await expect(storedError).toHaveAttribute('data-code', 'server_rejected');
	await expect(retry).toHaveCount(0);
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
