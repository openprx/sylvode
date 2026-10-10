import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

/**
 * Workspace package round-trip (`/workspace/{id}/settings/flow/package`) against a real API +
 * PostgreSQL.
 *
 * Workspace A gets two Flow pages over REST, one of them in project "Source roadmap". On A's
 * package page the owner exports the workspace, waits for the job to finish and downloads the
 * `.sylvode-flow.zip` (its SHA-256 must equal the job checksum shown on the page). On B's package
 * page the same file goes through the five-step import wizard: choose, upload, mapping/policy and
 * server preview. The first preview (no mapping) succeeds but reports the source project as
 * `project_unmapped` (`conflict_details`, B2), so the wizard returns to the mapping step with a row
 * titled "Source roadmap"; after mapping it to B's project the preview shows `manifest_summary`
 * (object/document counts, source project, package schema) and the preview hash equals the export
 * checksum; commit happens after ticking the hash acknowledgement. The report page shows
 * `created = 2` and survives a reload; B's Flow object list then holds the two pages, the
 * projected one in B's project.
 *
 * Importing the same package again with `reject_existing`: the preview SUCCEEDS and lists both
 * objects as `lineage_exists` in `conflict_details`; the wizard shows the conflicts, keeps Next
 * and the Confirm step disabled, and a commit of that preview over REST is `policy_rejected` with
 * nothing written. The wizard's "preview again reusing the earlier import" action previews with
 * `reuse_import_lineage`, which can be confirmed and commits with `reused = 2`, `created = 0`. A
 * random import id shows the not-found-safe report state.
 *
 * Needs BASE_URL to serve the built frontend with `/api` proxied to the API on the same origin
 * (as `frontend/nginx.conf` does). Registration after the first account needs an instance admin:
 * ADMIN_EMAIL / ADMIN_PASSWORD when both are set, otherwise the run registers the instance's
 * first account itself (a fresh database).
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = {
	email: `pkg-owner-${runId}@e2e.sylvode.test`,
	password: `PkgOwner-${runId}-1!`
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

async function envelope<T>(
	response: Awaited<ReturnType<APIRequestContext['get']>>
): Promise<Envelope<T>> {
	expect(response.ok(), `${response.url()} answered HTTP ${response.status()}`).toBe(true);
	return (await response.json()) as Envelope<T>;
}

async function unwrap<T>(response: Awaited<ReturnType<APIRequestContext['get']>>): Promise<T> {
	const body = await envelope<T>(response);
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
		email: `pkg-admin-${runId}@e2e.sylvode.test`,
		password: `PkgAdmin-${runId}-1!`
	};
	await unwrap(
		await request.post('/api/v1/auth/register', {
			data: { ...admin, name: 'Package Admin' }
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

/** Steps 1-4 of the wizard on the current page: choose, upload, (default) options, preview. With
 * `mapping`, the first preview is expected to send the wizard back to the mapping step with the
 * source project listed as unmapped; it is then mapped to `mapping.target` and previewed again. */
async function uploadAndPreview(
	page: Page,
	file: string,
	mapping?: { sourceTitle: string; target: string }
): Promise<void> {
	const wizard = page.getByTestId('flow-import');
	const next = wizard.getByRole('button', { name: 'Next', exact: true });
	await expect(wizard.getByTestId('flow-import-step')).toHaveText('Step 1 of 5: Choose file');
	await expect(next).toBeDisabled();
	await wizard.locator('#flow-import-file').setInputFiles(file);
	await expect(wizard.getByTestId('flow-import-file')).toContainText('.sylvode-flow.zip');
	await next.click();
	await expect(wizard.getByTestId('flow-import-step')).toHaveText('Step 2 of 5: Upload');
	await expect(wizard.getByTestId('flow-import-step')).toBeFocused();
	await expect(next).toBeDisabled();
	await wizard.getByRole('button', { name: 'Start upload' }).click();
	await expect(wizard.getByTestId('flow-import-upload-state')).toHaveText(
		'Uploaded and verified by the server.'
	);
	await next.click();
	await expect(wizard.getByTestId('flow-import-step')).toHaveText(
		'Step 3 of 5: Mapping and policy'
	);
	await wizard.getByRole('button', { name: 'Run preview', exact: true }).click();
	if (mapping) {
		// The preview succeeded but found an unmapped source project: back on the mapping step.
		await expect(wizard.getByTestId('flow-import-unmapped')).toBeVisible();
		await expect(wizard.getByTestId('flow-import-step')).toHaveText(
			'Step 3 of 5: Mapping and policy'
		);
		await expect(wizard.getByTestId('flow-import-unmapped-project')).toHaveText([
			mapping.sourceTitle
		]);
		const row = wizard.getByTestId('flow-import-mapping-row');
		await expect(row).toHaveCount(1);
		await expect(row.getByTestId('flow-import-mapping-source')).toContainText(mapping.sourceTitle);
		// No target is guessed: the preview cannot run until one is chosen.
		await expect(wizard.getByRole('button', { name: 'Run preview', exact: true })).toBeDisabled();
		await wizard
			.getByRole('combobox', { name: `Target for ${mapping.sourceTitle}` })
			.selectOption({ label: mapping.target });
		await wizard.getByRole('button', { name: 'Run preview', exact: true }).click();
	}
	await expect(wizard.getByTestId('flow-import-step')).toHaveText('Step 4 of 5: Preview');
}

test('export a workspace package, import it into another workspace with a project mapping, and hold a re-import on its lineage conflicts', async ({
	browser,
	request
}) => {
	const admin = await instanceAdmin(request);
	await unwrap(
		await request.post('/api/v1/auth/register', {
			headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
			data: { ...owner, name: 'Package Owner' }
		})
	);
	const ownerLogin = await login(request, owner);
	const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };
	const createWorkspace = async (tag: string) =>
		unwrap<{ id: string }>(
			await request.post('/api/v1/workspaces', {
				headers: auth,
				data: { slug: `pkg-${tag}-${runId}`, name: `Package ${tag} ${runId}` }
			})
		);
	const workspaceA = await createWorkspace('a');
	const workspaceB = await createWorkspace('b');
	for (const workspace of [workspaceA, workspaceB]) {
		await unwrap(
			await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
				headers: auth,
				data: { enabled: true, idempotency_key: crypto.randomUUID() }
			})
		);
	}
	const sourceProjectTitle = `Source roadmap ${runId}`;
	const sourceProject = await unwrap<{ id: string }>(
		await request.post(`/api/v1/workspaces/${workspaceA.id}/projects`, {
			headers: auth,
			data: { name: sourceProjectTitle, key: 'SRC' }
		})
	);
	const targetProject = await unwrap<{ id: string }>(
		await request.post(`/api/v1/workspaces/${workspaceB.id}/projects`, {
			headers: auth,
			data: { name: `Target roadmap ${runId}`, key: 'TGT' }
		})
	);
	const targetLabel = `TGT - Target roadmap ${runId}`;
	const titles = [`Round trip one ${runId}`, `Round trip two ${runId}`];
	for (const [index, title] of titles.entries()) {
		await unwrap(
			await request.post(`/api/v1/workspaces/${workspaceA.id}/flow/objects`, {
				headers: auth,
				data: {
					object_type: 'page',
					title,
					// The first page lives in a project, so the package carries one source project.
					...(index === 0 ? { project_id: sourceProject.id } : {}),
					idempotency_key: crypto.randomUUID()
				}
			})
		);
	}
	const mapping = { sourceTitle: sourceProjectTitle, target: targetLabel };

	const context = await browser.newContext({ acceptDownloads: true });
	const page = await context.newPage();
	await signInAs(page, ownerLogin);

	// ---- export from A (entered from the Flow settings page) ----
	await page.goto(`/workspace/${workspaceA.id}/settings/flow`);
	await page.getByRole('link', { name: 'Open export and import' }).click();
	await expect(page).toHaveURL(new RegExp(`/workspace/${workspaceA.id}/settings/flow/package$`));
	await expect(page).toHaveTitle('Sylvode - Flow Export and Import');

	const exportCard = page.getByTestId('flow-export');
	await exportCard.getByRole('button', { name: 'Export', exact: true }).click();
	const job = exportCard.getByTestId('flow-export-job').first();
	await expect(job.getByTestId('flow-export-status')).toHaveText('Completed');
	const checksum = (await job.getByTestId('flow-export-checksum').textContent())?.trim() ?? '';
	expect(checksum).toMatch(/^[0-9a-f]{64}$/);

	const downloadPromise = page.waitForEvent('download');
	await job.getByRole('button', { name: 'Download package' }).click();
	const download = await downloadPromise;
	expect(download.suggestedFilename()).toMatch(/\.sylvode-flow\.zip$/);
	const packagePath = join(tmpdir(), `${runId}-${download.suggestedFilename()}`);
	await download.saveAs(packagePath);
	const bytes = readFileSync(packagePath);
	expect(createHash('sha256').update(bytes).digest('hex')).toBe(checksum);

	// The job id is remembered per workspace: a reload re-attaches to it.
	await page.reload();
	await expect(page.getByTestId('flow-export-job').first()).toHaveAttribute(
		'data-job-id',
		(await job.getAttribute('data-job-id')) ?? ''
	);

	// ---- import into B ----
	await page.goto(`/workspace/${workspaceB.id}/settings/flow/package`);
	const wizard = page.getByTestId('flow-import');

	// A file without the package extension is refused locally.
	const wrongPath = join(tmpdir(), `${runId}-not-a-package.zip`);
	writeFileSync(wrongPath, bytes);
	await wizard.locator('#flow-import-file').setInputFiles(wrongPath);
	await expect(wizard.getByRole('alert')).toContainText('.sylvode-flow.zip');
	await expect(wizard.getByRole('button', { name: 'Next', exact: true })).toBeDisabled();

	await uploadAndPreview(page, packagePath, mapping);
	await expect(wizard.getByTestId('flow-import-preview-sha')).toHaveText(checksum);
	await expect(wizard.getByTestId('flow-import-preview-objects')).toHaveText('2');
	const manifest = wizard.getByTestId('flow-import-manifest');
	await expect(manifest.getByTestId('flow-import-manifest-objects')).toHaveText('2');
	await expect(manifest.getByTestId('flow-import-manifest-documents')).toHaveText('2');
	await expect(manifest.getByTestId('flow-import-manifest-schema')).toHaveText(
		'sylvode.flow.export-package.v1'
	);
	await expect(manifest.getByTestId('flow-import-manifest-project')).toHaveText([
		sourceProjectTitle
	]);
	await expect(wizard.getByTestId('flow-import-hold')).toHaveCount(0);
	await expect(wizard.getByTestId('flow-import-conflicts')).toHaveText('None');
	await expect(wizard.getByTestId('flow-import-expiry')).toHaveText(/^(29|30):\d\d$/);
	await wizard.getByRole('button', { name: 'Next', exact: true }).click();
	await expect(wizard.getByTestId('flow-import-step')).toHaveText('Step 5 of 5: Confirm');
	await expect(wizard.getByTestId('flow-import-confirm-hash')).toHaveValue(checksum);
	const importButton = wizard.getByRole('button', {
		name: 'Import',
		exact: true
	});
	const acknowledge = wizard.getByRole('checkbox', {
		name: 'I have checked that this package hash matches the package I intend to import'
	});
	await expect(acknowledge).not.toBeChecked();
	await expect(importButton).toBeDisabled();
	// Going back keeps the preview; coming forward again still requires the acknowledgement.
	await wizard.getByRole('button', { name: 'Back', exact: true }).click();
	await expect(wizard.getByTestId('flow-import-step')).toHaveText('Step 4 of 5: Preview');
	await wizard.getByRole('button', { name: 'Next', exact: true }).click();
	await acknowledge.check();
	await expect(importButton).toBeEnabled();
	await importButton.click();

	// ---- report ----
	await expect(page).toHaveURL(
		new RegExp(`/workspace/${workspaceB.id}/settings/flow/package/imports/[0-9a-f-]{36}$`)
	);
	await expect(page).toHaveTitle('Sylvode - Flow Import Report');
	await expect(page.getByTestId('flow-import-report-status')).toHaveText('Completed');
	await expect(page.getByTestId('flow-import-report-count-created')).toHaveText('2');
	await expect(page.getByTestId('flow-import-report-count-planned')).toHaveText('2');
	await expect(page.getByTestId('flow-import-report-count-failed')).toHaveText('0');
	await page.reload();
	await expect(page.getByTestId('flow-import-report-count-created')).toHaveText('2');
	const firstImportId = new URL(page.url()).pathname.split('/').pop() ?? '';
	const firstReport = await unwrap<{ object_mapping: Record<string, string> }>(
		await request.get(`/api/v1/workspaces/${workspaceB.id}/flow/imports/${firstImportId}`, {
			headers: auth
		})
	);
	expect(Object.keys(firstReport.object_mapping)).toHaveLength(2);

	const listed = await unwrap<{
		items: Array<{ title: string; object_type: string }>;
	}>(
		await request.get(`/api/v1/workspaces/${workspaceB.id}/flow/objects?object_type=page`, {
			headers: auth
		})
	);
	expect(listed.items.map((item) => item.title).sort()).toEqual([...titles].sort());
	const inTarget = await unwrap<{ items: Array<{ title: string }> }>(
		await request.get(
			`/api/v1/workspaces/${workspaceB.id}/flow/objects?object_type=page&project_id=${targetProject.id}`,
			{ headers: auth }
		)
	);
	expect(inTarget.items.map((item) => item.title)).toEqual([titles[0]]);

	// ---- the same package again with reject_existing: the preview succeeds and lists the
	// lineage conflicts, confirm stays closed, and a commit of that preview is policy_rejected ----
	await page.goto(`/workspace/${workspaceB.id}/settings/flow/package`);
	type PreviewWire = {
		preview_id: string;
		package_sha256: string;
		mapping_hash: string;
		conflict_details: Array<{
			kind: string;
			source_object_id?: string;
			existing_target_id?: string;
		}>;
	};
	const previews: Array<Envelope<PreviewWire>> = [];
	page.on('response', async (res) => {
		if (new URL(res.url()).pathname.endsWith('/flow/imports/preview')) {
			previews.push((await res.json()) as Envelope<PreviewWire>);
		}
	});
	await uploadAndPreview(page, packagePath, mapping);
	await expect.poll(() => previews.length).toBe(2);
	const held = previews[1];
	expect(held.code, 'a lineage hit is a successful preview').toBe(0);
	expect(held.data.conflict_details.map((detail) => detail.kind)).toEqual([
		'lineage_exists',
		'lineage_exists'
	]);
	// Each conflict names the object the first import created for that source object.
	expect(
		Object.fromEntries(
			held.data.conflict_details.map((detail) => [detail.source_object_id, detail.existing_target_id])
		)
	).toEqual(firstReport.object_mapping);
	const hold = wizard.getByTestId('flow-import-hold');
	await expect(hold).toHaveAttribute('data-hold', 'lineage_exists');
	await expect(hold).toContainText('These objects were already imported into this workspace');
	await expect(wizard.getByTestId('flow-import-conflict-lineage')).toHaveCount(2);
	for (const detail of held.data.conflict_details) {
		await expect(
			wizard.getByTestId('flow-import-conflict-lineage').filter({
				hasText: `${detail.source_object_id} → existing object ${detail.existing_target_id}`
			})
		).toHaveCount(1);
	}
	await expect(wizard.getByTestId('flow-import-error')).toHaveCount(0);
	await expect(wizard.getByRole('button', { name: 'Next', exact: true })).toBeDisabled();
	await expect(
		wizard
			.getByRole('navigation', { name: 'Import steps' })
			.getByRole('button', { name: 'Confirm' })
	).toBeDisabled();
	// The server refuses the held preview too: commit re-reads the lineage.
	const refused = await envelope<null>(
		await request.post(
			`/api/v1/workspaces/${workspaceB.id}/flow/imports/${held.data.preview_id}/commit`,
			{
				headers: auth,
				data: {
					package_sha256: held.data.package_sha256,
					mapping_hash: held.data.mapping_hash,
					conflict_policy: 'reject_existing',
					confirm: true,
					idempotency_key: crypto.randomUUID()
				}
			}
		)
	);
	expect(refused.code).toBe(403);
	expect(refused.error_code).toBe('policy_rejected');
	const afterRefusal = await unwrap<{ items: unknown[] }>(
		await request.get(`/api/v1/workspaces/${workspaceB.id}/flow/objects?object_type=page`, {
			headers: auth
		})
	);
	expect(afterRefusal.items).toHaveLength(2);

	// ---- switch to reuse_import_lineage from the conflict notice: confirmable, reuses both ----
	await wizard.getByTestId('flow-import-reuse').click();
	await expect.poll(() => previews.length).toBe(3);
	expect(previews[2].code).toBe(0);
	await expect(hold).toHaveCount(0);
	await expect(wizard.getByTestId('flow-import-conflict-lineage')).toHaveCount(2);
	await wizard.getByRole('button', { name: 'Next', exact: true }).click();
	await expect(wizard.getByTestId('flow-import-step')).toHaveText('Step 5 of 5: Confirm');
	await acknowledge.check();
	await importButton.click();
	await expect(page).toHaveURL(
		new RegExp(`/workspace/${workspaceB.id}/settings/flow/package/imports/[0-9a-f-]{36}$`)
	);
	await expect(page.getByTestId('flow-import-report-status')).toHaveText('Completed');
	await expect(page.getByTestId('flow-import-report-count-reused')).toHaveText('2');
	await expect(page.getByTestId('flow-import-report-count-created')).toHaveText('0');
	const afterReuse = await unwrap<{ items: unknown[] }>(
		await request.get(`/api/v1/workspaces/${workspaceB.id}/flow/objects?object_type=page`, {
			headers: auth
		})
	);
	expect(afterReuse.items).toHaveLength(2);

	// ---- unknown import id: not-found-safe ----
	await page.goto(
		`/workspace/${workspaceB.id}/settings/flow/package/imports/${crypto.randomUUID()}`
	);
	await expect(page.getByTestId('flow-import-report-not-found')).toBeVisible();

	await context.close();
});
