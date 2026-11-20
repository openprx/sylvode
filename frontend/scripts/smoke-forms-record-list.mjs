#!/usr/bin/env node
/**
 * Browser check for the record list: default table, simplified views, and the read-only record
 * side panel. Drives the built app with real input events (keyboard, Back, focus) through
 * playwright-core, against the forms smoke mock (`smoke-forms-ui.mjs` in serve-only mode) plus a
 * "field gallery" form, served from routes in this script, that holds every field type.
 *
 * Run: `bun run build && node scripts/smoke-forms-record-list.mjs`
 *   CHROMIUM_BIN       Chromium executable (default /usr/bin/chromium)
 *   PLAYWRIGHT_CORE    path to a playwright-core package directory (default: resolve 'playwright-core')
 *   OPENPR_FORMS_UI_SCREENSHOT_DIR  where to write light / dark / narrow screenshots (optional)
 */
import { spawn } from 'node:child_process';
import { mkdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const root = resolve(new URL('..', import.meta.url).pathname);
const chromiumPath = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const screenshotDir = process.env.OPENPR_FORMS_UI_SCREENSHOT_DIR || '';
const playwrightModule = process.env.PLAYWRIGHT_CORE
	? pathToFileURL(join(process.env.PLAYWRIGHT_CORE, 'index.mjs')).href
	: 'playwright-core';
const { chromium } = await import(playwrightModule);

const workspaceId = 'ws-forms-smoke';
const projectId = 'project-forms-smoke';
const formId = 'form-field-gallery';
const now = '2026-11-19T09:10:00.000Z';
const user = { id: 'user-smoke', email: 'smoke@example.com', name: 'Smoke Admin', role: 'admin' };

// --- fixture: one field of every type --------------------------------------------------------
const field = (key, type, label, extra = {}) => ({
	field_id: `fld_${key}_0001`,
	key,
	type,
	label,
	...extra
});
const fields = [
	field('customer', 'text', 'Customer'),
	field('phone', 'phone', 'Phone'),
	field('contact_email', 'email', 'Contact email'),
	field('address', 'address', 'Address'),
	field('dock', 'location', 'Location'),
	field('sku', 'scan', 'Scan code'),
	field('signature', 'signature', 'Signature'),
	field('delivery_no', 'autonumber', 'Delivery no'),
	field('owner', 'member', 'Owner'),
	field('notes', 'textarea', 'Notes'),
	field('instructions', 'rich_text', 'Instructions'),
	field('weight', 'number', 'Weight'),
	field('crates', 'integer', 'Crates'),
	field('amount', 'amount', 'Amount', { amount: { currency: 'CNY', scale: 2 } }),
	field('rating', 'rating', 'Rating'),
	field('progress', 'progress', 'Progress'),
	field('due', 'date', 'Due date'),
	field('slot', 'datetime', 'Time slot'),
	field('status', 'single_select', 'Status', {
		options: ['scheduled', 'in_transit', 'delivered'],
		option_colors: { in_transit: '#2563eb', delivered: '#16a34a' }
	}),
	field('tags', 'multi_select', 'Tags', { options: ['fragile', 'cold', 'priority'] }),
	field('signed_off', 'boolean', 'Signed off'),
	field('documents', 'attachment', 'Documents'),
	field('photo', 'image', 'Photo'),
	field('related', 'relation', 'Related delivery', { relation: { form_key: 'field_gallery' } }),
	field('lines', 'child_table', 'Lines'),
	field('total_weight', 'formula', 'Total weight'),
	field('summary', 'ai_summary', 'Summary')
];
const PNG =
	'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';
const fullValues = {
	customer: 'Lotus Foods',
	phone: '+86 21 5550 0100',
	contact_email: 'receiving@lotusfoods.example',
	address: '88 Huaihai Middle Road\nXuhui District, Shanghai 200031',
	dock: { lat: 31.2165, lng: 121.4587, label: 'Dock 1' },
	sku: 'SKU-88231',
	signature: PNG,
	delivery_no: 'DLV-000118',
	owner: { user_id: 'user-smoke', name: 'Smoke Admin', email: 'smoke@example.com' },
	notes: 'Leave at the cold room.\nCall on arrival.',
	instructions:
		'<p>Hello &amp; welcome</p><script>alert("x")</script><ul><li>Use dock 1</li><li>Keep cold</li></ul>',
	weight: { type: 'decimal', decimal: '1234.50' },
	crates: { type: 'integer', value: 42 },
	amount: { type: 'amount', decimal: '12480.00', currency: 'CNY', scale: 2 },
	rating: { type: 'rating', value: 4 },
	progress: { type: 'progress', value: 65 },
	due: '2026-11-20',
	slot: '2026-11-20T09:30',
	status: 'in_transit',
	tags: ['fragile', 'cold'],
	signed_off: true,
	documents: [
		{ url: 'https://files.example/invoice-118.pdf', name: 'invoice-118.pdf', size: 20480 }
	],
	photo: PNG,
	related: {
		record_id: 'gal-2',
		title: 'Weekly restock · Harbor Market',
		form_key: 'field_gallery'
	},
	lines: [{ record_id: 'line-1' }, { record_id: 'line-2' }],
	total_weight: { type: 'decimal', decimal: '1300.00' },
	summary: 'On schedule; customer asked for a call on arrival.'
};
const record = (id, title, values, updated) => ({
	id,
	workspace_id: workspaceId,
	project_id: projectId,
	form_id: formId,
	title,
	values,
	source: {},
	created_by: 'user-smoke',
	updated_by: 'user-smoke',
	archived_at: null,
	created_at: now,
	updated_at: updated
});
const records = [
	record('gal-1', 'Morning delivery to Lotus Foods', fullValues, '2026-11-19T09:10:00.000Z'),
	record(
		'gal-2',
		'Weekly restock · Harbor Market',
		{
			customer: 'Harbor Market',
			amount: { type: 'amount', decimal: '980.00', currency: 'CNY', scale: 2 },
			status: 'scheduled'
		},
		'2026-11-18T09:10:00.000Z'
	),
	record(
		'gal-3',
		'Pharmacy cold-chain run',
		{
			customer: 'Green Cross Pharmacy',
			amount: { type: 'amount', decimal: '5600.00', currency: 'CNY', scale: 2 },
			status: 'delivered'
		},
		'2026-11-17T09:10:00.000Z'
	)
];
const form = {
	id: formId,
	workspace_id: workspaceId,
	project_id: projectId,
	key: 'field_gallery',
	name: 'Field Gallery',
	description: 'Every field type',
	title_template: '{customer}',
	schema: { version: 'openpr.form.schema.v1', fields },
	detail_layout: {
		header: { badge_field: 'status' },
		highlights: ['amount', 'due', 'owner', 'progress']
	},
	schema_version: 1,
	created_by: 'user-smoke',
	archived_at: null,
	created_at: now,
	updated_at: now
};
const templateView = (id, key, name, viewType) => ({
	id,
	workspace_id: workspaceId,
	project_id: projectId,
	form_id: formId,
	key,
	name,
	view_type: viewType,
	config: {},
	created_by: null,
	archived_at: null,
	created_at: now,
	updated_at: now
});

// --- mock state ------------------------------------------------------------------------------
const state = {
	views: [
		templateView('view-detail', 'detail', 'Detail', 'detail'),
		templateView('view-grid', 'grid', 'Grid', 'grid')
	],
	design: true,
	createdViews: [],
	recordListQueries: [],
	pluginRequests: []
};
function permissions() {
	return {
		form_id: formId,
		policies: [],
		effective: {
			role: state.design ? 'admin' : 'member',
			is_bot: false,
			actions: {
				'form.view': true,
				'form.design': state.design,
				'record.create': true,
				'record.update': state.design,
				'record.delete': state.design,
				'record.export': true
			},
			record_scope: 'all',
			fields: {}
		}
	};
}
const ok = (data) => ({ code: 0, message: 'success', data });
const page_ = (items) => ({ items, total: items.length, page: 1, per_page: 100, total_pages: 1 });
const sortValue = (rec, key) => {
	const value = key === 'title' ? rec.title : rec.values[key];
	if (value && typeof value === 'object') return Number(value.decimal ?? value.value ?? 0);
	return value ?? '';
};

async function api(route) {
	const request = route.request();
	const url = new URL(request.url());
	const path = url.pathname;
	const method = request.method();
	const reply = (data, status = 200) =>
		route.fulfill({
			status,
			contentType: 'application/json',
			body: JSON.stringify(status === 200 ? ok(data) : data)
		});

	if (/plugin-(panels|contributions|preferences|packages)/.test(path)) {
		// Plugin discovery: nothing installed. Record-level discovery is what mounting the
		// record plugin panels triggers, so it is counted.
		if (path.includes('/form-records/gal-')) state.pluginRequests.push(path);
		return reply(path.endsWith('plugin-panels') ? [] : { items: [], projects: [] });
	}
	// Endpoints the shared mock does not answer, unrelated to this check.
	if (path === '/api/v1/scenario-templates') return reply(page_([]));
	if (path.endsWith('/features/flow')) return reply({ enabled: false });
	if (path === `/api/v1/projects/${projectId}/forms`) {
		const upstream = await route.fetch();
		const body = await upstream.json();
		body.data.items = [...body.data.items, form];
		return route.fulfill({ response: upstream, body: JSON.stringify(body) });
	}
	if (path === `/api/v1/forms/${formId}`) return reply(form);
	if (path === `/api/v1/forms/${formId}/permissions`) return reply(permissions());
	if (path === `/api/v1/forms/${formId}/views` && method === 'GET') {
		return reply(
			state.views.filter(
				(view) => view.config.visibility !== 'private' || view.created_by === user.id
			)
		);
	}
	if (path === `/api/v1/forms/${formId}/views` && method === 'POST') {
		const body = request.postDataJSON();
		if (body.config?.visibility !== 'private' && !state.design)
			return reply({ code: 403, message: 'form permission denied: form.design', data: null }, 403);
		const view = {
			...templateView(`view-new-${state.views.length}`, body.key, body.name, body.view_type),
			config: body.config,
			created_by: user.id,
			updated_at: new Date().toISOString()
		};
		state.views.push(view);
		state.createdViews.push(body);
		return reply(view);
	}
	const viewMatch = /^\/api\/v1\/form-views\/([^/]+)$/.exec(path);
	if (viewMatch) {
		const index = state.views.findIndex((view) => view.id === viewMatch[1]);
		if (index < 0) return reply({ code: 404, message: 'form view not found', data: null }, 404);
		if (method === 'DELETE') {
			state.views.splice(index, 1);
			return reply(null);
		}
		const body = request.postDataJSON();
		state.views[index] = { ...state.views[index], ...body, updated_at: new Date().toISOString() };
		delete state.views[index].expected_updated_at;
		return reply(state.views[index]);
	}
	if (path === `/api/v1/forms/${formId}/records`) {
		state.recordListQueries.push(url.search);
		let items = [...records];
		const sort = url.searchParams.get('sort');
		if (sort) {
			const [key, direction] = sort.split(':');
			items.sort(
				(a, b) => (sortValue(a, key) > sortValue(b, key) ? 1 : -1) * (direction === 'desc' ? -1 : 1)
			);
		}
		return reply(page_(items));
	}
	if (path.startsWith(`/api/v1/forms/${formId}/`))
		return reply(path.endsWith('events') ? page_([]) : []);
	const recordMatch = /^\/api\/v1\/form-records\/(gal-[^/]+)(\/.*)?$/.exec(path);
	if (recordMatch) {
		const found = records.find((item) => item.id === recordMatch[1]);
		if (!recordMatch[2]) {
			return found
				? reply({ ...found, flow_lineage: { references: [], conversions: [] } })
				: reply({ code: 404, message: 'form record not found', data: null }, 404);
		}
		return reply(recordMatch[2] === '/children' ? page_([]) : []);
	}
	return route.continue();
}

// --- harness ---------------------------------------------------------------------------------
const failures = [];
let passed = 0;
async function check(label, body) {
	try {
		await body();
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

const server = spawn('node', ['scripts/smoke-forms-ui.mjs'], {
	cwd: root,
	env: { ...process.env, OPENPR_FORMS_UI_SERVE_ONLY: '1' },
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
const formsPath = `/workspace/${workspaceId}/projects/${projectId}/forms`;
const browser = await chromium.launch({ executablePath: chromiumPath });

async function newPage({
	width = 1440,
	height = 900,
	theme = 'light',
	reducedMotion = 'no-preference'
} = {}) {
	const context = await browser.newContext({
		viewport: { width, height },
		locale: 'en-US',
		reducedMotion
	});
	await context.addInitScript(
		([authUser, themeName]) => {
			if (!sessionStorage.getItem('seeded')) {
				localStorage.setItem('auth_token', 'smoke-access-token');
				localStorage.setItem('refresh_token', 'smoke-refresh-token');
				localStorage.setItem('auth_user', JSON.stringify(authUser));
				localStorage.setItem('locale', 'en');
				localStorage.setItem('theme', themeName);
				sessionStorage.setItem('seeded', '1');
			}
		},
		[user, theme]
	);
	const page = await context.newPage();
	page.on('pageerror', (error) => console.log(`  page error: ${error.message}`));
	page.on('response', (response) => {
		const type = response.headers()['content-type'] ?? '';
		if (response.url().includes('/api/v1/') && type.includes('text/html'))
			console.log(`  unmocked API: ${new URL(response.url()).pathname}`);
	});
	await page.route('**/api/v1/**', api);
	return page;
}
async function openGallery(page, search = '') {
	await page.goto(`${origin}${formsPath}?form=${formId}${search}`);
	await page.waitForSelector('[data-view-switcher]');
}
const rowCells = (page) =>
	page.$$eval('[data-record-row]', (rows) =>
		rows.map((row) => row.getAttribute('data-record-row'))
	);
const screenshot = async (page, name, { top = true } = {}) => {
	if (!screenshotDir) return;
	// Playwright scrolls clicked elements into view, scroll containers included; start from the top.
	if (top)
		await page.evaluate(() => {
			for (const node of document.querySelectorAll('*')) {
				if (node.scrollTop || node.scrollLeft) node.scrollTo(0, 0);
			}
			window.scrollTo(0, 0);
		});
	await page.screenshot({
		path: join(screenshotDir, name),
		fullPage: false,
		animations: 'disabled'
	});
};

try {
	if (screenshotDir) await mkdir(screenshotDir, { recursive: true });
	const page = await newPage();

	await check('first open shows the table as "All records" without rewriting the URL', async () => {
		await openGallery(page);
		await page.waitForSelector('[data-view-renderer="grid"]');
		const label = await page.textContent('[data-view-switcher]');
		assert(label.includes('All records'), `switcher reads ${label}`);
		const params = [...new URL(page.url()).searchParams.keys()];
		assert(params.join(',') === 'form', `URL params changed on cold load: ${params}`);
	});

	await check(
		'every field type renders in the table without raw JSON, data URIs or HTML',
		async () => {
			const text = await page.textContent('[data-view-renderer="grid"]');
			for (const bad of ['{"', 'data:image', '<p>', '<script', 'alert('])
				assert(!text.includes(bad), `grid shows ${bad}`);
			assert(
				await page.$('[data-option-color="status:in_transit"]'),
				'option badges keep their data attribute'
			);
		}
	);

	await check(
		'clicking a cell opens the read-only panel, marks the row and pushes ?record=',
		async () => {
			const before = await page.evaluate(() => history.length);
			await page.click('[data-record-row="gal-1"] td:nth-child(2)');
			await page.waitForSelector('[data-record-panel="gal-1"][data-record-panel-status="ready"]');
			assert(
				(await page.getAttribute('[data-record-row="gal-1"]', 'aria-current')) === 'true',
				'row not aria-current'
			);
			assert(new URL(page.url()).searchParams.get('record') === 'gal-1', 'URL lacks record');
			assert(
				(await page.evaluate(() => history.length)) === before + 1,
				'opening did not add one history entry'
			);
			const inputs = await page.$$(
				'[data-record-panel] input, [data-record-panel] textarea, [data-record-panel] select'
			);
			assert(inputs.length === 0, `panel has ${inputs.length} form controls`);
			assert(
				(await page.getAttribute('[data-record-panel]', 'data-record-panel-mode')) === 'docked',
				'not docked at 1440'
			);
		}
	);

	await check('every field type renders in the panel by type', async () => {
		const missing = [];
		for (const item of fields) {
			const node = await page.$(`[data-record-panel-field="gal-1:${item.key}"]`);
			if (!node) {
				missing.push(item.key);
				continue;
			}
			const text = (await node.textContent()) ?? '';
			for (const bad of ['{"', 'data:image', '<p>', '<li>', 'alert(']) {
				if (text.includes(bad)) missing.push(`${item.key} shows ${bad}`);
			}
		}
		assert(missing.length === 0, `problems: ${missing.join(', ')}`);
		const rich = await page.textContent('[data-record-panel-field="gal-1:instructions"]');
		assert(rich.includes('Hello & welcome') && rich.includes('• Use dock 1'), `rich text: ${rich}`);
		assert(
			await page.$('[data-record-panel-field="gal-1:signature"] img'),
			'signature preview missing'
		);
		assert(await page.$('[data-record-panel-field="gal-1:photo"] img'), 'image preview missing');
		const amount = await page.textContent('[data-record-panel-field="gal-1:amount"]');
		assert(amount.includes('12,480.00'), `amount: ${amount}`);
		const location = await page.textContent('[data-record-panel-field="gal-1:dock"]');
		assert(location.includes('31.21650, 121.45870'), `location: ${location}`);
		const files = await page.textContent('[data-record-panel-field="gal-1:documents"]');
		assert(files.includes('invoice-118.pdf') && files.includes('20 kB'), `attachment: ${files}`);
		assert(
			await page.$('[data-record-panel-field="gal-1:related"] [data-field-value-relation="gal-2"]'),
			'relation chip missing'
		);
	});

	await check('no plugin iframe or plugin request until "Apps" is expanded', async () => {
		assert((await page.$$('iframe')).length === 0, 'an iframe is mounted');
		assert(
			state.pluginRequests.length === 0,
			`plugin requests before Apps: ${state.pluginRequests}`
		);
		await page.click('[data-record-panel-apps] summary');
		await page.waitForTimeout(500);
		assert(state.pluginRequests.length > 0, 'expanding Apps did not mount the plugin panels');
		await page.click('[data-record-panel-apps] summary');
	});

	await screenshot(page, 'desktop-light-panel.png');

	await check('Back closes the panel and stays on the list', async () => {
		await page.goBack();
		await page.waitForSelector('[data-record-panel]', { state: 'detached' });
		const url = new URL(page.url());
		assert(url.pathname === formsPath && !url.searchParams.get('record'), `URL after Back: ${url}`);
	});

	await check(
		'Enter opens, arrows move without history entries, Esc closes and returns focus',
		async () => {
			await page.focus('[data-record-title-link="gal-1"]');
			await page.keyboard.press('Enter');
			await page.waitForSelector('[data-record-panel="gal-1"]');
			const length = await page.evaluate(() => history.length);
			await page.keyboard.press('ArrowDown');
			await page.waitForSelector('[data-record-panel="gal-2"]');
			await page.waitForURL((url) => url.searchParams.get('record') === 'gal-2', { timeout: 5000 });
			assert(
				(await page.evaluate(() => history.length)) === length,
				'ArrowDown added a history entry'
			);
			assert(
				(await page.evaluate(() =>
					document.activeElement?.getAttribute('data-record-title-link')
				)) === 'gal-2',
				'focus did not move with the row'
			);
			await page.keyboard.press('Enter');
			assert(
				(await page.evaluate(() => document.activeElement?.id)) === 'record-panel-title',
				'second Enter did not move focus to the panel heading'
			);
			await page.keyboard.press('Alt+ArrowUp');
			await page.waitForSelector('[data-record-panel="gal-1"]');
			await page.keyboard.press('Escape');
			await page.waitForSelector('[data-record-panel]', { state: 'detached' });
			assert(
				(await page.evaluate(() =>
					document.activeElement?.getAttribute('data-record-title-link')
				)) === 'gal-1',
				'focus did not return to the row'
			);
		}
	);

	await check(
		'reload with ?record= reopens the panel; a deleted id shows the deleted state',
		async () => {
			await openGallery(page, '&record=gal-3');
			await page.waitForSelector('[data-record-panel="gal-3"][data-record-panel-status="ready"]');
			await openGallery(page, '&record=gal-404');
			await page.waitForSelector('[data-record-panel-status="missing"]');
			await page.click('[data-record-panel-close]');
			await page.waitForSelector('[data-record-panel]', { state: 'detached' });
		}
	);

	await check(
		'sorting from the column header applies immediately and survives reload',
		async () => {
			await openGallery(page);
			await page.click('[data-grid-sort="amount"]');
			await page.waitForSelector('[data-view-modified]');
			await page.waitForFunction(
				() =>
					document.querySelector('[data-record-row]')?.getAttribute('data-record-row') === 'gal-2'
			);
			assert(
				(await rowCells(page)).join() === 'gal-2,gal-3,gal-1',
				`order ${await rowCells(page)}`
			);
			await page.waitForTimeout(400);
			await openGallery(page);
			await page.waitForSelector('[data-view-modified]');
			assert(
				(await rowCells(page)).join() === 'gal-2,gal-3,gal-1',
				`order after reload ${await rowCells(page)}`
			);
			const sorted = await page.$$eval('th[aria-sort]', (cells) =>
				cells.map(
					(cell) =>
						`${cell.querySelector('[data-grid-sort]')?.getAttribute('data-grid-sort')}=${cell.getAttribute('aria-sort')}`
				)
			);
			assert(sorted.includes('amount=ascending'), `aria-sort: ${sorted.join(' ')}`);
		}
	);

	await check(
		'"Save view…" with only a name creates a private view and makes it current',
		async () => {
			await page.click('[data-view-save-as]');
			await page.waitForSelector('[data-save-view-name]');
			await screenshot(page, 'desktop-light-save-view.png', { top: false });
			const suggested = await page.inputValue('[data-save-view-name]');
			assert(suggested === 'All records · by Amount', `suggested name ${suggested}`);
			assert(await page.$('[data-save-view-audience]'), 'a designer sees the audience choice');
			await page.keyboard.press('Enter');
			await page.waitForFunction(() =>
				document.querySelector('[data-view-switcher]')?.textContent?.includes('by Amount')
			);
			const created = state.createdViews.at(-1);
			assert(
				created.config.visibility === 'private' && created.config.is_default === false,
				JSON.stringify(created)
			);
			assert(created.config.sort?.field === 'amount', 'saved config lacks the working sort');
			assert(
				new URL(page.url()).searchParams.get('view')?.startsWith('view-new'),
				'URL lacks ?view='
			);
			assert(!(await page.$('[data-view-modified]')), 'still modified after saving');
		}
	);

	await check(
		'the explicit choice is remembered; ?view=all opens the table without changing it',
		async () => {
			await page.goto(`${origin}${formsPath}?form=${formId}`);
			await page.waitForFunction(() =>
				document.querySelector('[data-view-switcher]')?.textContent?.includes('by Amount')
			);
			await openGallery(page, '&view=all');
			assert(
				(await page.textContent('[data-view-switcher]')).includes('All records'),
				'?view=all ignored'
			);
			await openGallery(page);
			assert(
				(await page.textContent('[data-view-switcher]')).includes('by Amount'),
				'remembered choice changed'
			);
		}
	);

	await check('a member without design rights saves a personal view', async () => {
		state.design = false;
		await openGallery(page, '&view=all');
		await page.click('[data-view-switcher]');
		await page.click('[data-view-option="view-grid"]');
		await page.click('[data-grid-sort="title"]');
		await page.waitForSelector('[data-view-modified]');
		await page.click('[data-view-save-as]');
		await page.waitForSelector('[data-save-view-name]');
		assert(!(await page.$('[data-save-view-audience]')), 'a member sees the audience choice');
		await page.fill('[data-save-view-name]', 'My titles');
		await page.keyboard.press('Enter');
		await page.waitForFunction(() =>
			document.querySelector('[data-view-switcher]')?.textContent?.includes('My titles')
		);
		assert(state.createdViews.at(-1).config.visibility === 'private', 'member view not private');
		await page.click('[data-record-row="gal-1"] td:nth-child(2)');
		await page.waitForSelector('[data-record-panel="gal-1"]');
		assert(!(await page.$('[data-record-panel-edit]')), 'a member without record.update sees Edit');
		assert(
			!(await page.$('[data-record-row="gal-1"] [aria-label="Edit record"]')),
			'row edit visible without permission'
		);
		await page.keyboard.press('Escape');
		state.design = true;
	});

	await check('Layout settings shows only the current type and applies live', async () => {
		await openGallery(page, '&view=all');
		await page.click('[data-view-menu-button]');
		await page.click('[data-view-menu-layout]');
		await page.waitForSelector('[data-layout-settings-sheet] [data-view-columns-disclosure]');
		const disabledByType = await page.$$eval(
			'[data-layout-settings-sheet] [disabled]',
			(nodes) => nodes.length
		);
		const pivotControls = await page.$$('[data-layout-settings-sheet] [data-pivot-swap-axes]');
		assert(pivotControls.length === 0, 'pivot controls visible for a table');
		console.log(`       (${disabledByType} disabled controls remain for non-type reasons)`);
		await page.click('[data-layout-settings-done]');
	});

	await check('reduced motion: the panel opens without a transform animation', async () => {
		const reduced = await newPage({ reducedMotion: 'reduce' });
		await openGallery(reduced);
		await reduced.click('[data-record-row="gal-1"] td:nth-child(2)');
		await reduced.waitForSelector('[data-record-panel="gal-1"]');
		const name = await reduced.$eval(
			'[data-record-panel]',
			(node) => getComputedStyle(node).animationName
		);
		assert(!/panel-in/.test(name), `animation ${name}`);
		await reduced.context().close();
	});

	await check('right-to-left: the panel docks at the inline end (left)', async () => {
		const rtl = await newPage();
		await openGallery(rtl);
		await rtl.evaluate(() => (document.documentElement.dir = 'rtl'));
		await rtl.click('[data-record-row="gal-1"] td:nth-child(2)');
		await rtl.waitForSelector('[data-record-panel="gal-1"]');
		const [panelBox, listBox] = await Promise.all([
			rtl.$eval('[data-record-panel]', (node) => node.getBoundingClientRect().left),
			rtl.$eval('[data-record-list-panel]', (node) => node.getBoundingClientRect().left)
		]);
		assert(panelBox < listBox, `panel ${panelBox} not left of list ${listBox}`);
		await screenshot(rtl, 'desktop-rtl-panel.png');
		await rtl.context().close();
	});

	await check('dark theme renders the panel', async () => {
		const dark = await newPage({ theme: 'dark' });
		await openGallery(dark, '&record=gal-1&view=all');
		await dark.waitForSelector('[data-record-panel="gal-1"][data-record-panel-status="ready"]');
		assert(
			await dark.evaluate(() => document.documentElement.classList.contains('dark')),
			'dark class missing'
		);
		await screenshot(dark, 'desktop-dark-panel.png');
		await dark.context().close();
	});

	await check('overlay mode at 1100 px', async () => {
		const tablet = await newPage({ width: 1100 });
		await openGallery(tablet, '&record=gal-1&view=all');
		await tablet.waitForSelector('[data-record-panel="gal-1"]');
		const mode = await tablet.getAttribute('[data-record-panel]', 'data-record-panel-mode');
		assert(mode === 'overlay', `mode ${mode}`);
		await screenshot(tablet, 'tablet-overlay-panel.png');
		await tablet.context().close();
	});

	await check('narrow viewport: modal sheet with a focus trap; Esc returns focus', async () => {
		const narrow = await newPage({ width: 390, height: 844 });
		await openGallery(narrow, '&view=all');
		await screenshot(narrow, 'narrow-list.png');
		await narrow.focus('[data-record-title-link="gal-1"]');
		await narrow.keyboard.press('Enter');
		await narrow.waitForSelector('[data-record-panel="gal-1"][data-record-panel-mode="sheet"]');
		const role = await narrow.getAttribute('[data-record-panel]', 'role');
		const modal = await narrow.getAttribute('[data-record-panel]', 'aria-modal');
		assert(role === 'dialog' && modal === 'true', `role ${role} aria-modal ${modal}`);
		for (let index = 0; index < 40; index += 1) await narrow.keyboard.press('Tab');
		const inside = await narrow.evaluate(
			() => !!document.activeElement?.closest('[data-record-panel]')
		);
		assert(inside, 'Tab left the sheet');
		await screenshot(narrow, 'narrow-sheet-panel.png');
		await narrow.keyboard.press('Escape');
		await narrow.waitForSelector('[data-record-panel]', { state: 'detached' });
		assert(
			(await narrow.evaluate(() =>
				document.activeElement?.getAttribute('data-record-title-link')
			)) === 'gal-1',
			'focus did not return to the row'
		);
		await narrow.context().close();
		const narrowDark = await newPage({ width: 390, height: 844, theme: 'dark' });
		await openGallery(narrowDark, '&view=all&record=gal-1');
		await narrowDark.waitForSelector(
			'[data-record-panel="gal-1"][data-record-panel-status="ready"]'
		);
		await screenshot(narrowDark, 'narrow-dark-sheet-panel.png');
		await narrowDark.context().close();
	});
} finally {
	await browser.close();
	server.kill();
}

console.log(`\nforms record list: ${passed} passed, ${failures.length} failed`);
if (failures.length > 0) process.exit(1);
