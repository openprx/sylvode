#!/usr/bin/env node
/**
 * Browser check for the interactions of the forms page. The first-render comparison used while the
 * page was split into a state object, controllers and components cannot see a defect that only
 * shows after the user changes something (a component compiled in the wrong mode that throws on
 * the first edit is the case that led to this script). Every step here changes something with real
 * input events and asserts a visible outcome; any page error or console error fails the run.
 *
 * Covered: import wizard, page modes, layout settings sheet, kanban (drag, bulk move), calendar
 * (drag), pivot (drill-down, export), card and gallery views, timeline (events and records),
 * record editor drawer, record side panel, durable record route (URL restore, reload, record
 * page), detail aside with the child record editor, designer saves (field design schema, detail
 * sections, role permissions, each with its request payload), and no horizontal document overflow
 * at 390 px in the table, kanban, calendar and card views.
 *
 * Not covered (the retired smoke asserted them; they are listed in the Universal Forms gates):
 * the remaining detail-layout builders (header, highlights, composition, widgets), the
 * relation/formula/attachment/image field-designer configuration saves, attachment upload and
 * comment payloads, gantt drag, and the permission list/export filtering checks.
 *
 * Drives the built app through playwright-core against the forms smoke mock
 * (`forms-smoke-server.mjs`), like `smoke-forms-record-list.mjs`.
 *
 * Run: `bun run build && bun run smoke:forms-page-interactions`
 *   CHROMIUM_BIN           Chromium executable (default /usr/bin/chromium)
 *   PLAYWRIGHT_CORE        path to a playwright-core package directory (default: resolve 'playwright-core')
 *   OPENPR_FORMS_UI_PORT   port of the mock server (default: a random free-ish port)
 *   OPENPR_FORMS_UI_SCREENSHOT_DIR  when set, writes forms-ui-desktop.png (1366x900) and
 *                          forms-ui-mobile.png (390x844) of the records page there, the evidence
 *                          `scripts/collect-universal-forms-ui-artifacts.sh` gathers
 */
import { spawn } from 'node:child_process';
import { mkdirSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const root = resolve(new URL('..', import.meta.url).pathname);
const chromiumPath = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const playwrightModule = process.env.PLAYWRIGHT_CORE
	? pathToFileURL(join(process.env.PLAYWRIGHT_CORE, 'index.mjs')).href
	: 'playwright-core';
const { chromium } = await import(playwrightModule);

const user = { id: 'user-smoke', email: 'smoke@example.com', name: 'Smoke Admin', role: 'admin' };
const formsPath = '/workspace/ws-forms-smoke/projects/project-forms-smoke/forms';
const viewUrl = (type) => `${formsPath}?form=form-order-line&view=view-order-line-${type}`;
const recordUrl = `${viewUrl('default')}&record=record-order-line-created`;
const T = 15_000;
const screenshotDir = process.env.OPENPR_FORMS_UI_SCREENSHOT_DIR || '';
const screenshots = [];

// --- harness ---------------------------------------------------------------------------------
const failures = [];
let passed = 0;
const problems = [];
async function check(label, body) {
	const before = problems.length;
	try {
		await body();
		// Give a late error (an effect, a rejected promise) the chance to surface.
		await new Promise((done) => setTimeout(done, 150));
		if (problems.length > before) {
			throw new Error(`page problems:\n${problems.slice(before).join('\n')}`);
		}
		passed += 1;
		console.log(`  ok   ${label}`);
	} catch (error) {
		failures.push(`${label}: ${error.message}`);
		console.log(`  FAIL ${label}\n       ${String(error.message).split('\n').join('\n       ')}`);
	}
}
function assert(condition, message) {
	if (!condition) throw new Error(message);
}

const server = spawn('node', ['scripts/forms-smoke-server.mjs'], {
	cwd: root,
	env: process.env,
	stdio: ['ignore', 'pipe', 'inherit']
});
const port = await new Promise((resolvePort, reject) => {
	server.on('exit', (code) => reject(new Error(`mock server exited with ${code}`)));
	server.stdout.on('data', (chunk) => {
		const match = /SMOKE_SERVER_READY (\d+)/.exec(String(chunk));
		if (match) resolvePort(match[1]);
	});
});
const origin = `http://127.0.0.1:${port}`;
process.on('exit', () => server.kill());

// The mock starts with one record; a POST adds the second one the writes below need (the mock
// accepts writes for these two records only).
const seed = await fetch(`${origin}/api/v1/forms/form-order-line/records`, {
	method: 'POST',
	headers: { 'content-type': 'application/json', authorization: 'Bearer smoke-access-token' },
	body: JSON.stringify({
		title: 'Beef Noodles x2',
		values: { sku_name: 'Beef Noodles', seat_no: '1', status: 'sent_to_kitchen' }
	})
});
assert(seed.ok, `seeding the mock failed with ${seed.status}`);

const browser = await chromium.launch({ executablePath: chromiumPath, headless: true });

// Rewrites of GET responses, for states the stock mock does not hold.
const rewriteJson = (pattern, change) => async (context) => {
	await context.route(pattern, async (route) => {
		if (route.request().method() !== 'GET') return route.continue();
		const upstream = await route.fetch();
		const body = await upstream.json();
		change(body);
		return route.fulfill({ response: upstream, body: JSON.stringify(body) });
	});
};
const withViewConfig = (viewType, config) =>
	rewriteJson('**/api/v1/forms/form-order-line/views', (body) => {
		if (!Array.isArray(body.data)) return;
		for (const view of body.data) {
			if (view.id === `view-order-line-${viewType}`) view.config = { ...view.config, ...config };
		}
	});
const addChildRelation = (field) => {
	if (field.key === 'order_id') {
		field.relation = {
			relation_type: 'parent_child',
			form_key: 'print_job',
			relation_key: 'line_print_jobs'
		};
	}
};
const withChildRelation = async (context) => {
	await rewriteJson('**/api/v1/projects/project-forms-smoke/forms**', (body) => {
		for (const form of body.data.items ?? []) {
			if (form.id === 'form-order-line') form.schema.fields.forEach(addChildRelation);
		}
	})(context);
	await rewriteJson('**/api/v1/forms/form-order-line', (body) => {
		body.data.schema.fields.forEach(addChildRelation);
	})(context);
};

/**
 * One browser context and page per scenario. `writes` lists every non-GET API request the page
 * sent; `downloads` every file it offered.
 */
async function session(name, { mocks = [], width = 1440, height = 1000 } = {}) {
	const context = await browser.newContext({
		viewport: { width, height },
		locale: 'en-US',
		timezoneId: 'UTC',
		reducedMotion: 'reduce',
		acceptDownloads: true
	});
	await context.clock.setFixedTime('2026-05-31T12:00:00.000Z');
	await context.addInitScript(
		({ user }) => {
			localStorage.setItem('auth_token', 'smoke-access-token');
			localStorage.setItem('refresh_token', 'smoke-refresh-token');
			localStorage.setItem('auth_user', JSON.stringify(user));
			localStorage.setItem('locale', 'en');
			localStorage.setItem('theme', 'light');
		},
		{ user }
	);
	for (const mock of mocks) await mock(context);
	const page = await context.newPage();
	page.setDefaultTimeout(T);
	const writes = [];
	const downloads = [];
	const previews = [];
	page.on('pageerror', (error) => problems.push(`${name}: page error: ${error.message}`));
	page.on('console', (message) => {
		if (message.type() === 'error') problems.push(`${name}: console error: ${message.text()}`);
	});
	page.on('request', (request) => {
		const url = new URL(request.url());
		if (request.method() === 'GET' || !url.pathname.startsWith('/api/')) return;
		writes.push({ method: request.method(), path: url.pathname, body: request.postData() ?? '' });
	});
	page.on('download', (download) => downloads.push(download));
	await context.route('**/records/import-preview', async (route) => {
		previews.push(JSON.parse(route.request().postData() ?? '{}'));
		await route.fulfill({
			contentType: 'application/json',
			body: JSON.stringify({
				code: 0,
				message: 'ok',
				data: {
					total_rows: 1,
					valid_rows: 1,
					invalid_rows: 0,
					rows: [{ row_number: 1, valid: true, title: 'x', error: null }]
				}
			})
		});
	});
	const open = async (path, ready) => {
		await page.goto(`${origin}${path}`, { waitUntil: 'load' });
		await page.waitForSelector(ready, { timeout: 20_000 });
		await page.waitForLoadState('networkidle').catch(() => {});
	};
	return { context, page, writes, downloads, previews, open };
}
const button = (page, name, exact = true) => page.getByRole('button', { name, exact }).first();
const writesTo = (s, method, pattern) =>
	s.writes.filter((write) => write.method === method && pattern.test(write.path));
const downloadText = async (download) => readFileSync(await download.path(), 'utf8');

try {
	// --- import wizard -------------------------------------------------------------------------
	console.log('import wizard');
	{
		const s = await session('import');
		const { page } = s;
		const column = (index) => page.locator(`select[data-import-mapping-column="${index}"]`);
		const transform = (index) => page.locator(`select[data-import-mapping-transform="${index}"]`);
		await check('open the wizard and parse a pasted source', async () => {
			await s.open(viewUrl('default'), '[data-record-list-panel]');
			await button(page, 'Import/export').click();
			await button(page, 'Import records').click();
			await page.waitForSelector('[data-import-wizard-step="source"]');
			await page.locator('#form-import-text').fill('sku_name,quantity\nBeef Noodles,2\n');
			await button(page, 'Next').click();
			await page.waitForSelector('[data-import-wizard-step="mapping"]');
			assert((await column(0).count()) === 1, 'the first mapping row is missing');
		});
		await check('change a mapping select and a transform select', async () => {
			await column(0).selectOption('__title');
			await transform(0).selectOption('uppercase');
			assert((await column(0).inputValue()) === '__title', 'the mapping select kept its old value');
			assert(
				(await transform(0).inputValue()) === 'uppercase',
				'the transform select kept its old value'
			);
		});
		await check('preview carries the transformed value', async () => {
			await button(page, 'Preview import').click();
			await page.waitForSelector('[data-import-wizard-step="preview"]');
			assert(s.previews.length > 0, 'no preview request was sent');
			assert(
				JSON.stringify(s.previews.at(-1)).includes('BEEF NOODLES'),
				`the preview request lacks the uppercase value: ${JSON.stringify(s.previews.at(-1))}`
			);
		});
		await check('back to the mapping: values persist, rows stay independent', async () => {
			await button(page, 'Back').click();
			await page.waitForSelector('[data-import-wizard-step="mapping"]');
			assert((await column(0).inputValue()) === '__title', 'the mapping select was reset');
			assert((await transform(0).inputValue()) === 'uppercase', 'the transform select was reset');
			await column(1).selectOption('__skip');
			await transform(1).selectOption('slugify');
			assert((await column(1).inputValue()) === '__skip', 'the second mapping did not change');
			assert(
				(await transform(1).inputValue()) === 'slugify',
				'the second transform did not change'
			);
			assert(
				(await column(0).inputValue()) === '__title',
				'the first mapping changed with the second'
			);
		});
		await s.context.close();
	}

	// --- page modes ----------------------------------------------------------------------------
	console.log('page modes');
	{
		const s = await session('modes');
		const { page } = s;
		await s.open(viewUrl('default'), '[data-record-list-panel]');
		const modes = [
			['Field design', 'field-design'],
			['Detail layout', 'detail-layout'],
			['Permissions', 'permissions'],
			['Import/export', 'import-export']
		];
		for (const [label, mode] of modes) {
			await check(`${label} opens and returns to the records`, async () => {
				await button(page, label).click();
				await page.waitForSelector(`[data-form-mode="${mode}"]`);
				assert(
					(await page.locator('[data-record-list-panel]').count()) === 0,
					'records still shown'
				);
				await button(page, 'Back to records', false).click();
				await page.waitForSelector('[data-record-list-panel]');
				assert(
					(await page.locator(`[data-form-mode="${mode}"]`).count()) === 0,
					`${label} stayed open`
				);
			});
		}
		await check('the mode tabs switch between the configuration modes', async () => {
			await button(page, 'Field design').click();
			await page.waitForSelector('[data-form-mode="field-design"]');
			for (const [label, mode] of [...modes.slice(1), modes[0]]) {
				await page
					.locator('[data-form-config-workbench] button', { hasText: label })
					.first()
					.click();
				await page.waitForSelector(`[data-form-mode="${mode}"]`);
			}
			await button(page, 'Back to records', false).click();
			await page.waitForSelector('[data-record-list-panel]');
		});
		await s.context.close();
	}

	// --- layout settings sheet -----------------------------------------------------------------
	console.log('layout settings');
	{
		const s = await session('layout');
		const { page } = s;
		await s.open(viewUrl('default'), '[data-record-list-panel]');
		await check('open the layout settings sheet', async () => {
			await page.locator('[data-view-menu-button]').first().click();
			await page.locator('[data-view-menu-layout]').first().click();
			await page.waitForSelector('[data-layout-settings-sheet]');
		});
		for (const type of ['kanban', 'calendar', 'pivot', 'grid']) {
			await check(`change the view type to ${type === 'grid' ? 'table' : type}`, async () => {
				await page.locator('[data-view-tools-expanded-panel] select').first().selectOption(type);
				await page.waitForSelector(
					type === 'grid' ? '[data-record-list-panel]' : `[data-view-renderer="${type}"]`
				);
				assert(
					(await page.locator('[data-view-tools-expanded-panel] select').first().inputValue()) ===
						type,
					'the type select did not keep the new value'
				);
			});
		}
		await check('close the sheet', async () => {
			await page.locator('[data-layout-settings-done]').first().click();
			await page.waitForSelector('[data-layout-settings-sheet]', { state: 'detached' });
		});
		await s.context.close();
	}

	// --- kanban --------------------------------------------------------------------------------
	console.log('kanban');
	{
		const s = await session('kanban');
		const { page } = s;
		const card = (id) => page.locator(`[data-kanban-card="${id}"]`);
		const inColumn = (column, id) =>
			page.locator(`[data-kanban-column="${column}"] [data-kanban-card="${id}"]`);
		await s.open(viewUrl('kanban'), '[data-view-renderer="kanban"] [data-kanban-card]');
		await check('drag a card to another column with the pointer', async () => {
			await card('record-order-line-created').waitFor();
			const written = page.waitForRequest(
				(request) =>
					request.method() === 'PATCH' &&
					request.url().includes('/form-records/record-order-line-created')
			);
			await page.dragAndDrop(
				'[data-kanban-card="record-order-line-created"]',
				'[data-kanban-column="draft"]',
				{ sourcePosition: { x: 20, y: 20 }, targetPosition: { x: 40, y: 60 } }
			);
			const request = await written;
			assert(
				String(request.postData()).includes('"status":"draft"'),
				`the PATCH does not carry the new status: ${request.postData()}`
			);
			await inColumn('draft', 'record-order-line-created').waitFor();
		});
		await check('select two cards and move them in bulk', async () => {
			const boxes = page.locator('[data-kanban-card] input[type=checkbox]');
			await boxes.nth(0).check();
			await boxes.nth(1).check();
			await page.getByText('2 selected').first().waitFor();
			await page.getByLabel(/Move selected to/).selectOption('cancelled');
			const before = writesTo(s, 'PATCH', /form-records/).length;
			await button(page, 'Move selected').click();
			await page.waitForFunction(
				() =>
					document.querySelectorAll('[data-kanban-column="cancelled"] [data-kanban-card]').length >=
					2
			);
			assert(
				writesTo(s, 'PATCH', /form-records/).length - before === 2,
				'the bulk move did not send one PATCH per record'
			);
			await inColumn('cancelled', 'record-order-line-created').waitFor();
		});
		await check('clear the selection', async () => {
			await page.locator('[data-kanban-card] input[type=checkbox]').first().check();
			await button(page, 'Clear selection').click();
			await page.waitForFunction(
				() =>
					![...document.querySelectorAll('[data-kanban-card] input[type=checkbox]')].some(
						(box) => box.checked
					)
			);
		});
		await s.context.close();
	}

	// --- calendar ------------------------------------------------------------------------------
	console.log('calendar');
	{
		const s = await session('calendar');
		const { page } = s;
		await s.open(viewUrl('calendar'), '[data-view-renderer="calendar"]');
		await page.waitForSelector('[data-calendar-card][draggable="true"]');
		await check('drag an event to another day with the pointer', async () => {
			const cardLocator = page.locator('[data-calendar-card][draggable="true"]').first();
			const from = await cardLocator.evaluate((element) =>
				element.closest('[data-calendar-date]')?.getAttribute('data-calendar-date')
			);
			assert(from, 'the event is not inside a date cell');
			const target = page
				.locator(`[data-calendar-date]:not([data-calendar-date="${from}"])`)
				.first();
			const targetDate = await target.getAttribute('data-calendar-date');
			const written = page.waitForRequest(
				(request) => request.method() === 'PATCH' && request.url().includes('/form-records/')
			);
			await cardLocator.dragTo(target);
			const request = await written;
			assert(
				String(request.postData()).includes(targetDate),
				`the PATCH does not carry ${targetDate}: ${request.postData()}`
			);
		});
		await check(
			'open an event (DOM click: the button has no width in the mock layout)',
			async () => {
				await page
					.locator('[data-calendar-card] button.text-left')
					.first()
					.evaluate((element) => element.click());
				await page.waitForSelector('[data-record-panel-status]');
			}
		);
		await check('close the record panel', async () => {
			await page.locator('[data-record-panel-close]').first().click();
			await page.waitForSelector('[data-record-panel-status]', { state: 'detached' });
		});
		await s.context.close();
	}

	// --- pivot ---------------------------------------------------------------------------------
	console.log('pivot');
	{
		const s = await session('pivot', {
			mocks: [
				withViewConfig('pivot', {
					pivot_row_field: 'status',
					pivot_column_field: 'seat_no',
					pivot_value_field: 'quantity'
				})
			]
		});
		const { page } = s;
		await s.open(viewUrl('pivot'), '[data-view-renderer="pivot"] table');
		await check('a cell opens its drill-down and the drill-down closes', async () => {
			await page.locator('[data-pivot-cell-button]').first().click();
			await page.waitForSelector('[data-pivot-drilldown]');
			assert(
				Number(await page.locator('[data-pivot-drilldown]').getAttribute('data-record-count')) > 0,
				'the drill-down lists no records'
			);
			await page.locator('[data-pivot-drilldown] button[aria-label="Close"]').click();
			await page.waitForSelector('[data-pivot-drilldown]', { state: 'detached' });
		});
		await check('the export produces a download', async () => {
			const download = page.waitForEvent('download');
			await page.locator('[data-pivot-export-table]').first().click();
			const file = await download;
			assert(
				/\.csv$/i.test(file.suggestedFilename()),
				`unexpected file ${file.suggestedFilename()}`
			);
			assert((await downloadText(file)).length > 0, 'the exported file is empty');
		});
		await s.context.close();
	}

	// --- card and gallery ----------------------------------------------------------------------
	console.log('card and gallery');
	{
		const s = await session('card');
		const { page } = s;
		await s.open(viewUrl('card'), '[data-view-renderer="card"] [data-card-title]');
		const id = await page.locator('[data-card-title]').first().getAttribute('data-card-title');
		await check('open a card', async () => {
			await page.locator(`[data-card-title="${id}"]`).click();
			await page.waitForSelector('[data-record-panel-status]');
			await page.waitForFunction((record) => location.search.includes(`record=${record}`), id);
		});
		await check('the actions menu opens and an action runs', async () => {
			await page.goBack();
			await page.waitForSelector(`[data-card-menu="${id}"]`);
			await page.locator(`[data-card-menu="${id}"] summary`).click();
			assert(
				(await page.locator(`[data-card-menu="${id}"]`).getAttribute('open')) !== null,
				'the menu did not open'
			);
			await page.locator(`[data-card-action="${id}:export-record"]`).click();
			await page.waitForSelector(`[data-card-exported]`);
		});
		await s.context.close();

		const gallery = await session('gallery', {
			mocks: [withViewConfig('card', { card_layout: 'gallery', group_by: 'status' })]
		});
		await gallery.open(viewUrl('card'), '[data-view-renderer="card"] [data-card-title]');
		await check('gallery layout: groups collapse and expand', async () => {
			const { page: view } = gallery;
			assert(
				(await view.locator('[data-view-renderer="card"]').getAttribute('data-card-layout')) ===
					'gallery',
				'the gallery layout is not active'
			);
			const group = await view
				.locator('[data-card-group]:not([data-record-count="0"])')
				.first()
				.getAttribute('data-card-group');
			const toggle = view.locator(`[data-card-group-toggle="${group}"]`);
			await toggle.click();
			await view.waitForSelector(`[data-card-group="${group}"][data-card-group-collapsed="true"]`);
			await toggle.click();
			await view.waitForSelector(
				`[data-card-group="${group}"]:not([data-card-group-collapsed="true"])`
			);
		});
		await gallery.context.close();
	}

	// --- timeline ------------------------------------------------------------------------------
	console.log('timeline');
	{
		const s = await session('timeline-events', {
			mocks: [withViewConfig('timeline', { timeline_source: 'events' })]
		});
		const { page } = s;
		await s.open(viewUrl('timeline'), '[data-view-renderer="timeline"] [data-timeline-event]');
		const eventCount = () => page.locator('[data-timeline-event]').count();
		await check('a filter changes the list', async () => {
			const before = await eventCount();
			assert(before >= 2, `expected at least two events, found ${before}`);
			const menu = page.locator('[data-timeline-event-menu]').first();
			const id = await menu.getAttribute('data-timeline-event-menu');
			await menu.locator('summary').click();
			await page.locator(`[data-timeline-event-action="${id}:filter-type"]`).click();
			await page.waitForFunction(
				(count) => document.querySelectorAll('[data-timeline-event]').length < count,
				before
			);
		});
		await check('the export produces a download', async () => {
			const download = page.waitForEvent('download');
			await page.locator('[data-timeline-export-events-json]').first().click();
			const file = await download;
			assert(
				/\.json$/i.test(file.suggestedFilename()),
				`unexpected file ${file.suggestedFilename()}`
			);
			assert((await downloadText(file)).length > 2, 'the exported file is empty');
		});
		await s.context.close();

		const records = await session('timeline-records', {
			mocks: [withViewConfig('timeline', { timeline_source: 'records' })]
		});
		await records.open(
			viewUrl('timeline'),
			'[data-view-renderer="timeline"] button[aria-label="Delete record"]'
		);
		await check(
			'deleting a record opens the confirmation; cancelling sends no request',
			async () => {
				const view = records.page;
				const rows = await view
					.locator('[data-view-renderer="timeline"] button[aria-label="Delete record"]')
					.count();
				assert(rows > 0, 'the timeline shows no delete button');
				await view
					.locator('[data-view-renderer="timeline"] button[aria-label="Delete record"]')
					.first()
					.click();
				const dialog = view.locator('[role="alertdialog"], [role="dialog"]').first();
				await dialog.waitFor();
				await dialog.getByRole('button', { name: 'Cancel' }).click();
				await dialog.waitFor({ state: 'detached' });
				assert(
					writesTo(records, 'DELETE', /./).length === 0,
					'a DELETE was sent although the dialog was cancelled'
				);
			}
		);
		await records.context.close();
	}

	// --- record editor drawer ------------------------------------------------------------------
	console.log('record editor drawer');
	{
		const s = await session('editor');
		const { page } = s;
		await s.open(viewUrl('default'), '[data-record-list-panel]');
		await check('open, type and cancel', async () => {
			await button(page, 'New').click();
			await page.waitForSelector('[data-record-editor-drawer]');
			const input = page.locator('[data-record-editor-drawer] input').first();
			await input.fill('typed title');
			assert((await input.inputValue()) === 'typed title', 'the typed value was not kept');
			await page
				.locator('[data-record-editor-drawer]')
				.getByRole('button', { name: 'Cancel' })
				.first()
				.click();
			await page.waitForSelector('[data-record-editor-drawer]', { state: 'detached' });
			assert(
				writesTo(s, 'POST', /\/records$/).length === 0,
				'a record was created although the editor was cancelled'
			);
		});
		await s.context.close();
	}

	// --- record side panel ---------------------------------------------------------------------
	console.log('record side panel');
	{
		const s = await session('panel');
		const { page } = s;
		await check('open and close', async () => {
			await s.open(recordUrl, '[data-record-panel-status="ready"]');
			await page.locator('[data-record-panel-close]').first().click();
			await page.waitForSelector('[data-record-panel-status]', { state: 'detached' });
			await page.waitForFunction(() => !location.search.includes('record='));
		});
		await check(
			'the record route restores the panel from the URL, also after a reload',
			async () => {
				for (const step of ['open', 'reload']) {
					if (step === 'open') await s.open(recordUrl, '[data-record-panel-status="ready"]');
					else {
						await page.reload({ waitUntil: 'load' });
						await page.waitForSelector('[data-record-panel-status="ready"]');
					}
					const params = new URL(page.url()).searchParams;
					assert(params.get('form') === 'form-order-line', `${step}: the form is not in the URL`);
					assert(
						params.get('record') === 'record-order-line-created',
						`${step}: the record is not in the URL`
					);
					assert(
						await page
							.locator('[data-record-panel="record-order-line-created"]')
							.getByText('Beef Noodles x2')
							.first()
							.isVisible(),
						`${step}: the panel does not show the record title`
					);
				}
			}
		);
		await check('the durable record page opens from its own route', async () => {
			await s.open(
				`${formsPath}/records/record-order-line-created?form=form-order-line`,
				'[data-permission-state-mode="durable-detail"], main'
			);
			await page.getByText('Beef Noodles x2').first().waitFor();
		});
		await check('the panel Edit opens the editor drawer and cancel returns', async () => {
			await s.open(recordUrl, '[data-record-panel-status="ready"]');
			await page.locator('[data-record-panel-edit]').first().click();
			await page.waitForSelector('[data-record-editor-drawer]');
			await page
				.locator('[data-record-editor-drawer]')
				.getByRole('button', { name: 'Cancel' })
				.first()
				.click();
			await page.waitForSelector('[data-record-editor-drawer]', { state: 'detached' });
			await page.waitForSelector('[data-record-panel-status="ready"]');
		});
		await s.context.close();
	}

	// --- detail aside --------------------------------------------------------------------------
	console.log('detail aside');
	{
		const s = await session('aside', { mocks: [withChildRelation] });
		const { page } = s;
		await s.open(recordUrl, '[data-record-panel-status="ready"]');
		await check('More details opens the aside', async () => {
			await page.locator('[data-record-panel-status="ready"] [data-record-panel-more]').click();
			await page.locator('[data-record-panel-more-details]').click();
			await page.waitForSelector('[data-record-detail-drawer]');
		});
		await check('child editor: open, type and cancel', async () => {
			const drawer = page.locator('[data-record-detail-drawer]');
			await drawer.getByRole('button', { name: 'Edit child record' }).first().click();
			const field = page
				.locator(
					'[data-child-record-edit-field] input, [data-child-record-edit-field] select, [data-child-record-edit-field] textarea'
				)
				.first();
			await field.waitFor();
			if ((await field.evaluate((element) => element.tagName)) === 'SELECT') {
				const values = await field
					.locator('option')
					.evaluateAll((options) => options.map((o) => o.value));
				await field.selectOption(values.at(-1));
			} else {
				await field.fill('typed');
				assert((await field.inputValue()) === 'typed', 'the typed child value was not kept');
			}
			await drawer.getByRole('button', { name: 'Cancel' }).last().click();
			await page.waitForSelector('[data-child-record-edit-field]', { state: 'detached' });
			assert(
				writesTo(s, 'PATCH', /./).length === 0,
				'a child was written although the editor was cancelled'
			);
		});
		await check('child delete is cancelled in the dialog', async () => {
			await page
				.locator('[data-record-detail-drawer]')
				.getByRole('button', { name: 'Delete child record' })
				.first()
				.click();
			const dialog = page.locator('[role="alertdialog"], [role="dialog"]').last();
			await dialog.waitFor();
			await dialog.getByRole('button', { name: 'Cancel' }).click();
			await dialog.waitFor({ state: 'detached' });
			assert(
				writesTo(s, 'DELETE', /./).length === 0,
				'a DELETE was sent although the dialog was cancelled'
			);
		});
		await check('close the aside', async () => {
			await page.locator('[data-record-detail-close]').first().click();
			await page.waitForSelector('[data-record-detail-drawer]', { state: 'detached' });
		});
		await s.context.close();
	}
	// --- designer saves ------------------------------------------------------------------------
	console.log('designer saves');
	{
		const s = await session('designer');
		const { page } = s;
		const formPatches = () => writesTo(s, 'PATCH', /\/forms\/form-order-line$/);
		await s.open(viewUrl('default'), '[data-record-list-panel]');
		await check('field design: add a text field with help text and save the schema', async () => {
			await button(page, 'Field design').click();
			await page.waitForSelector('[data-form-mode="field-design"]');
			const keysBefore = await page
				.locator('[data-designer-field-key]')
				.evaluateAll((nodes) => nodes.map((node) => node.getAttribute('data-designer-field-key')));
			await page.locator('[data-field-library-type="text"]').click();
			await page.waitForFunction(
				(count) => document.querySelectorAll('[data-designer-field-key]').length > count,
				keysBefore.length
			);
			await page.locator('[data-field-inspector-section="input-behavior"] summary').click();
			await page.getByLabel('Help text').fill('Smoke designer help');
			const before = formPatches().length;
			await button(page, 'Save design').click();
			for (let wait = 0; wait < 50 && formPatches().length === before; wait += 1) {
				await page.waitForTimeout(100);
			}
			const patch = formPatches().at(-1);
			assert(formPatches().length > before, 'saving the design sent no PATCH to the form');
			const fields = JSON.parse(patch.body)?.schema?.fields ?? [];
			const added = fields.find((field) => !keysBefore.includes(field.key));
			assert(added, `the payload holds no new field: ${patch.body.slice(0, 300)}`);
			assert(added.type === 'text', `the new field has type ${added.type}`);
			assert(
				JSON.stringify(added).includes('Smoke designer help'),
				`the help text is missing from the new field: ${JSON.stringify(added)}`
			);
			assert(
				fields.length === keysBefore.length + 1,
				'the payload dropped or duplicated existing fields'
			);
		});
		await check('detail layout: a section title and description are saved', async () => {
			await page
				.locator('[data-form-config-workbench] button', { hasText: 'Detail layout' })
				.first()
				.click();
			await page.getByRole('button', { name: 'Detail sections', exact: true }).click();
			await page.waitForSelector('[data-detail-layout-section-builder="form-order-line"]');
			await page.locator('[data-detail-layout-section-title="handoff"]').fill('Service Snapshot');
			await page
				.locator('[data-detail-layout-section-description="handoff"]')
				.fill('Review service notes before closing the line.');
			const before = formPatches().length;
			await page.locator('[data-detail-layout-section-save] button').click();
			await page.getByText('Detail sections saved').first().waitFor();
			assert(formPatches().length > before, 'saving the sections sent no PATCH to the form');
			const sections = JSON.parse(formPatches().at(-1).body)?.detail_layout?.sections ?? [];
			const handoff = sections.find((section) => section.key === 'handoff');
			assert(handoff?.title === 'Service Snapshot', `section title: ${JSON.stringify(handoff)}`);
			assert(
				handoff?.description === 'Review service notes before closing the line.',
				`section description: ${JSON.stringify(handoff)}`
			);
		});
		await check('permissions: a role policy is saved with its scope and field denial', async () => {
			await page
				.locator('[data-form-config-workbench] button', { hasText: 'Permissions' })
				.first()
				.click();
			await page.waitForSelector('[data-field-read-permission="unit_price"]');
			const read = page.locator('[data-field-read-permission="unit_price"]');
			if (await read.isChecked()) await read.uncheck();
			await page.getByLabel('Record scope').selectOption('owned');
			const before = writesTo(s, 'PATCH', /\/permissions$/).length;
			await button(page, 'Save permissions').click();
			await page.getByText('Permissions saved').first().waitFor();
			const patches = writesTo(s, 'PATCH', /\/permissions$/);
			assert(patches.length > before, 'saving the permissions sent no PATCH');
			const policies = JSON.parse(patches.at(-1).body)?.policies ?? [];
			const member = policies.find((p) => p.subject_type === 'role' && p.subject_id === 'member');
			assert(member?.policy?.record_scope === 'owned', `record scope: ${JSON.stringify(member)}`);
			assert(
				JSON.stringify(member?.policy?.fields ?? {}).includes('unit_price'),
				`the unit_price denial is missing: ${JSON.stringify(member)}`
			);
		});
		await s.context.close();
	}

	// --- mobile layout -------------------------------------------------------------------------
	console.log('mobile layout (390 px)');
	{
		const s = await session('mobile', { width: 390, height: 844 });
		const { page } = s;
		// The widest element that sticks out of the viewport, for the failure message.
		const overflowReport = () =>
			page.evaluate(() => {
				const limit = document.documentElement.clientWidth;
				const wide = [];
				for (const element of document.body.querySelectorAll('*')) {
					const box = element.getBoundingClientRect();
					if (box.width > 0 && box.right > limit + 1) {
						wide.push(
							`${element.tagName.toLowerCase()}${
								element.className
									? '.' +
										String(element.className.baseVal ?? element.className)
											.trim()
											.split(/\s+/)
											.slice(0, 3)
											.join('.')
									: ''
							} right=${Math.round(box.right)}`
						);
					}
				}
				return {
					scrollWidth: document.documentElement.scrollWidth,
					clientWidth: limit,
					wide: wide.slice(0, 6)
				};
			});
		const views = [
			['table', viewUrl('default'), '[data-record-list-panel]'],
			['kanban', viewUrl('kanban'), '[data-view-renderer="kanban"] [data-kanban-card]'],
			['calendar', viewUrl('calendar'), '[data-view-renderer="calendar"]'],
			['card', viewUrl('card'), '[data-view-renderer="card"] [data-card-title]']
		];
		const shoot = async (page, name) => {
			if (!screenshotDir) return;
			mkdirSync(screenshotDir, { recursive: true });
			const file = join(screenshotDir, name);
			await page.screenshot({ path: file });
			screenshots.push(file);
		};
		for (const [name, path, ready] of views) {
			await check(`records mode, ${name} view: no horizontal document overflow`, async () => {
				await s.open(path, ready);
				const report = await overflowReport();
				assert(report.clientWidth === 390, `the viewport is ${report.clientWidth} px, not 390`);
				assert(
					report.scrollWidth <= report.clientWidth,
					`horizontal overflow: scrollWidth ${report.scrollWidth} > clientWidth ${report.clientWidth}; ` +
						`elements past the right edge: ${report.wide.join(' | ')}`
				);
				if (name === 'table') await shoot(page, 'forms-ui-mobile.png');
			});
		}
		await s.context.close();

		if (screenshotDir) {
			const desktop = await session('desktop-shot', { width: 1366, height: 900 });
			await desktop.open(viewUrl('default'), '[data-record-list-panel]');
			await shoot(desktop.page, 'forms-ui-desktop.png');
			await desktop.context.close();
		}
	}
} catch (error) {
	failures.push(`script: ${error.stack ?? error.message}`);
	console.log(
		`  FAIL script\n       ${String(error.stack ?? error.message)
			.split('\n')
			.join('\n       ')}`
	);
} finally {
	await browser.close();
	server.kill();
}

console.log(`\nforms page interactions: ${passed} passed, ${failures.length} failed`);
if (failures.length > 0) {
	console.log(failures.map((failure) => `  - ${failure.split('\n')[0]}`).join('\n'));
	process.exitCode = 1;
} else {
	console.log('Forms UI browser smoke passed');
	if (screenshots.length > 0) console.log(`Forms UI screenshots: ${screenshots.join(', ')}`);
}
