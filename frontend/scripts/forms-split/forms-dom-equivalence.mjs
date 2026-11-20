#!/usr/bin/env node
// DOM-equivalence harness for the forms page split.
//
//   node scripts/forms-split/forms-dom-equivalence.mjs list
//   node scripts/forms-split/forms-dom-equivalence.mjs capture --out DIR [--states S01,S03]
//   node scripts/forms-split/forms-dom-equivalence.mjs compare DIR_A DIR_B [--diff-dir DIR]
//
// Capture serves `frontend/build` with the in-process mock API of `scripts/smoke-forms-ui.mjs`
// (serve-only mode) and drives it with `playwright-core` and the locally installed Chromium.
// No shared environment and no real data are touched. Every request that is not GET, HEAD or
// OPTIONS is aborted, so each state is read-only even against the mock.
//
// Per state it writes, into DIR:
//   <state>-<theme>-<width>.html       normalised body HTML (comments and scripts removed,
//                                      whitespace collapsed, attributes sorted, uid() ids renumbered)
//   <state>-<theme>-<width>.png        full-page screenshot
//   <state>-<theme>-<width>.requests.json
//                                      the /api GET requests issued from navigation until the state
//                                      settled (sorted), their count, and whether the page reached
//                                      network idle and stayed quiet for one more second
// A state whose ready selector never appears is written as <state>-...skipped.txt and listed.
//
// Environment:
//   CHROMIUM_BIN            default /Applications/Chromium.app/Contents/MacOS/Chromium
//   FORMS_DOM_PORT          mock server port, must be within 18400-18449 (default 18401)
//   FORMS_DOM_IDLE_MS       bound for reaching network idle per state (default 10000)
//
// The server is started inside this process and stopped by its recorded PID (written to
// DIR/server.pid while it runs); nothing is killed by name.
import { spawn } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';

const root = resolve(new URL('../../', import.meta.url).pathname);
const CHROMIUM_BIN =
	process.env.CHROMIUM_BIN || '/Applications/Chromium.app/Contents/MacOS/Chromium';
const PORT = Number(process.env.FORMS_DOM_PORT || 18401);
const IDLE_BOUND_MS = Number(process.env.FORMS_DOM_IDLE_MS || 10_000);
const FIXED_TIME = '2026-05-31T12:00:00.000Z';
const HEIGHT = 900;
const READY_TIMEOUT_MS = 15_000;

const FREEZE_CSS = `*, *::before, *::after {
	animation: none !important;
	transition: none !important;
	caret-color: transparent !important;
	scroll-behavior: auto !important;
}`;

const USER = { id: 'user-smoke', email: 'smoke@example.com', name: 'Smoke Admin', role: 'admin' };
const P = '/workspace/ws-forms-smoke/projects/project-forms-smoke/forms';
const F = `${P}?form=form-order-line`;
const viewUrl = (type) => `${F}&view=view-order-line-${type}`;
const RECORD_URL = `${viewUrl('default')}&record=record-order-line-created`;

// --- state table -----------------------------------------------------------------------------
// Each state: id, theme, width, url, steps (functions over the playwright page), ready selector.
// A state replays its whole path from navigation, so states are independent of each other.

const click = (selector) => async (page) => {
	await page.locator(selector).first().click({ timeout: READY_TIMEOUT_MS });
};
const clickButton =
	(name, exact = true) =>
	async (page) => {
		await page.getByRole('button', { name, exact }).first().click({ timeout: READY_TIMEOUT_MS });
	};
const waitFor = (selector) => async (page) => {
	await page.waitForSelector(selector, { timeout: READY_TIMEOUT_MS });
};
const openLayoutSettings = [click('[data-view-menu-button]'), click('[data-view-menu-layout]')];
const openRecordDetails = [
	waitFor('[data-record-panel-status="ready"]'),
	click('[data-record-panel-status="ready"] [data-record-panel-more]'),
	click('[data-record-panel-more-details]')
];

// The 44 states, numbered S01-S44 in the order of the contract (section 7.2).
// Read-only response rewrites for states the stock mock cannot reach. They only change GET
// responses (the data a real project would hold), never the page.
const rewriteJson = (pattern, change) => async (context) => {
	await context.route(pattern, async (route) => {
		if (route.request().method() !== 'GET') return route.continue();
		const upstream = await route.fetch();
		const body = await upstream.json();
		change(body);
		return route.fulfill({ response: upstream, body: JSON.stringify(body) });
	});
};
const withPivotConfig = rewriteJson('**/api/v1/forms/form-order-line/views', (body) => {
	for (const view of body.data) {
		if (view.id === 'view-order-line-pivot') {
			view.config = {
				...view.config,
				pivot_row_field: 'status',
				pivot_column_field: 'seat_no',
				pivot_value_field: 'quantity'
			};
		}
	}
});
const addChildRelation = (field) => {
	if (field.key === 'order_id') {
		field.relation = { relation_type: 'parent_child', form_key: 'print_job' };
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

function states44() {
	const viewTypes = ['detail', 'kanban', 'pivot', 'gantt', 'calendar', 'card', 'timeline'];
	const list = [];
	const add = (def) => list.push({ theme: 'light', width: 1440, steps: [], ...def });

	add({
		name: 'index',
		url: P,
		ready: '[data-record-live-region]',
		finalWait: async (page) => {
			await page.waitForFunction(
				() => !document.body.textContent?.includes('Loading forms...'),
				null,
				{ timeout: READY_TIMEOUT_MS }
			);
		}
	});
	add({
		name: 'create-panel',
		url: P,
		steps: [clickButton('New form')],
		ready: 'h2:has-text("Define a form type")'
	});
	add({ name: 'grid', url: viewUrl('default'), ready: '[data-record-list-panel]' });
	for (const type of viewTypes) {
		add({ name: `view-${type}`, url: viewUrl(type), ready: `[data-view-renderer="${type}"]` });
	}
	add({ name: 'private-view', url: viewUrl('private-other'), ready: '[data-record-list-panel]' });
	add({
		name: 'pivot-drilldown',
		url: viewUrl('pivot'),
		mock: withPivotConfig,
		steps: [waitFor('[data-view-renderer="pivot"]'), click('[data-pivot-cell-button]')],
		ready: '[data-pivot-drilldown]'
	});
	for (const [type, label] of [['default', 'grid'], ...viewTypes.map((t) => [t, `view-${t}`])]) {
		add({
			name: `layout-settings-${label}`,
			url: viewUrl(type),
			steps: [waitFor('[data-view-menu-button]'), ...openLayoutSettings],
			ready: '[data-layout-settings-sheet]'
		});
	}
	const panel = {
		name: 'record-panel',
		url: RECORD_URL,
		ready: '[data-record-panel-status="ready"]'
	};
	const aside = {
		name: 'detail-aside',
		url: RECORD_URL,
		steps: openRecordDetails,
		ready: '[data-record-detail-drawer]'
	};
	add(panel);
	add(aside);
	add({
		name: 'child-editor',
		url: RECORD_URL,
		mock: withChildRelation,
		steps: [
			...openRecordDetails,
			waitFor('[data-record-detail-drawer]'),
			async (page) => {
				await page
					.locator('[data-record-detail-drawer]')
					.getByRole('button', { name: /^Add child record · / })
					.first()
					.click({ timeout: READY_TIMEOUT_MS });
			}
		],
		ready: '[data-child-record-edit-field]'
	});
	add({
		name: 'editor-drawer-edit',
		url: RECORD_URL,
		steps: [waitFor('[data-record-panel-status="ready"]'), click('[data-record-panel-edit]')],
		ready: '[data-record-editor-drawer]'
	});
	add({
		name: 'editor-drawer-create',
		url: viewUrl('default'),
		steps: [waitFor('[data-record-list-panel]'), clickButton('New')],
		ready: '[data-record-editor-drawer]'
	});
	add({
		name: 'field-design',
		url: viewUrl('default'),
		steps: [waitFor('[data-record-list-panel]'), clickButton('Field design')],
		ready: '[data-form-mode="field-design"]'
	});
	const layoutTab = (tab, builder) => ({
		name: `detail-layout-${tab}`,
		url: viewUrl('default'),
		steps: [
			waitFor('[data-record-list-panel]'),
			clickButton('Detail layout'),
			waitFor('[data-form-mode="detail-layout"]'),
			click(`[data-detail-layout-tab="${tab}"]`)
		],
		ready: `[data-form-mode="detail-layout"] [data-detail-${builder}-builder]`
	});
	add(layoutTab('header', 'header'));
	add(layoutTab('highlights', 'highlights'));
	add(layoutTab('composition', 'page-composition'));
	add(layoutTab('sections', 'layout-section'));
	add(layoutTab('widgets', 'page-widget'));
	add({
		name: 'permissions',
		url: viewUrl('default'),
		steps: [waitFor('[data-record-list-panel]'), clickButton('Permissions')],
		ready: '[data-form-mode="permissions"]'
	});
	const importExport = [waitFor('[data-record-list-panel]'), clickButton('Import/export')];
	add({
		name: 'import-export',
		url: viewUrl('default'),
		steps: importExport,
		ready: '[data-form-mode="import-export"]'
	});
	const wizardSource = [
		...importExport,
		waitFor('[data-form-mode="import-export"]'),
		clickButton('Import records')
	];
	add({
		name: 'import-wizard-source',
		url: viewUrl('default'),
		steps: wizardSource,
		ready: '[data-import-wizard-step="source"]'
	});
	add({
		name: 'import-wizard-mapping',
		url: viewUrl('default'),
		steps: [
			...wizardSource,
			waitFor('[data-import-wizard-step="source"]'),
			async (page) => {
				await page.locator('#form-import-text').fill('sku_name,quantity\nBeef Noodles,2\n');
			},
			clickButton('Next')
		],
		ready: '[data-import-wizard-step="mapping"]'
	});
	const gridState = { name: 'grid', url: viewUrl('default'), ready: '[data-record-list-panel]' };
	for (const [width, label] of [
		[800, 'narrow'],
		[390, 'phone']
	]) {
		add({ ...gridState, name: `${label}-grid`, width });
		add({ ...panel, name: `${label}-record-panel`, width });
		add({ ...aside, name: `${label}-detail-aside`, width });
	}
	const dark = { theme: 'dark' };
	add({ ...gridState, name: 'dark-grid', ...dark });
	add({
		name: 'dark-view-pivot',
		url: viewUrl('pivot'),
		ready: '[data-view-renderer="pivot"]',
		...dark
	});
	add({ ...panel, name: 'dark-record-panel', ...dark });
	add({ ...aside, name: 'dark-detail-aside', ...dark });

	return list.map((state, index) => ({ ...state, id: `S${String(index + 1).padStart(2, '0')}` }));
}

export const STATES = states44();

function fileStem(state) {
	return `${state.id}-${state.name}-${state.theme}-${state.width}`;
}

// --- normalisation (runs in the page) --------------------------------------------------------

function normalisePage() {
	const ID_ATTRIBUTES = new Set([
		'id',
		'for',
		'list',
		'aria-labelledby',
		'aria-describedby',
		'aria-controls',
		'aria-owns',
		'aria-activedescendant'
	]);
	const UID = /^ui-.+-\d+$/;
	const VOID = new Set([
		'area',
		'base',
		'br',
		'col',
		'embed',
		'hr',
		'img',
		'input',
		'link',
		'meta',
		'source',
		'track',
		'wbr'
	]);
	const ids = new Map();
	const renumber = (token) => {
		if (!UID.test(token)) return token;
		if (!ids.has(token)) {
			const prefix = token.replace(/^ui-/, '').replace(/-\d+$/, '');
			ids.set(token, `ui-${prefix}-#${ids.size + 1}`);
		}
		return ids.get(token);
	};
	const escapeAttribute = (value) => value.replace(/&/g, '&amp;').replace(/"/g, '&quot;');
	const escapeText = (value) =>
		value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
	const out = [];
	const walk = (node, depth) => {
		const pad = '  '.repeat(depth);
		if (node.nodeType === Node.TEXT_NODE) {
			const text = (node.nodeValue ?? '').replace(/\s+/g, ' ').trim();
			if (text) out.push(`${pad}${escapeText(text)}`);
			return;
		}
		if (node.nodeType !== Node.ELEMENT_NODE) return;
		const tag = node.localName;
		if (tag === 'script') return;
		const attributes = [...node.attributes]
			.map((attribute) => {
				let value = attribute.value;
				if (ID_ATTRIBUTES.has(attribute.name)) {
					value = value.split(/\s+/).filter(Boolean).map(renumber).join(' ');
				}
				return [attribute.name, value];
			})
			.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))
			.map(([name, value]) => (value === '' ? name : `${name}="${escapeAttribute(value)}"`));
		out.push(`${pad}<${tag}${attributes.length ? ` ${attributes.join(' ')}` : ''}>`);
		if (VOID.has(tag)) return;
		for (const child of node.childNodes) walk(child, depth + 1);
		out.push(`${pad}</${tag}>`);
	};
	for (const child of document.body.childNodes) walk(child, 0);
	return out.join('\n') + '\n';
}

// --- server ----------------------------------------------------------------------------------

async function startServer(outDir) {
	if (!(PORT >= 18400 && PORT <= 18449)) {
		throw new Error(`FORMS_DOM_PORT ${PORT} is outside 18400-18449`);
	}
	if (!existsSync(join(root, 'build'))) {
		throw new Error('frontend/build is missing: run `bun run build` first');
	}
	const server = spawn('node', ['scripts/smoke-forms-ui.mjs'], {
		cwd: root,
		env: {
			...process.env,
			OPENPR_FORMS_UI_SERVE_ONLY: '1',
			OPENPR_FORMS_UI_PORT: String(PORT)
		},
		stdio: ['ignore', 'pipe', 'inherit']
	});
	await writeFile(join(outDir, 'server.pid'), `${server.pid}\n`);
	const port = await new Promise((resolvePort, reject) => {
		const timer = setTimeout(() => reject(new Error('mock server did not start')), 30_000);
		server.once('exit', (code) => reject(new Error(`mock server exited early (${code})`)));
		server.stdout.on('data', (chunk) => {
			const match = /SMOKE_SERVER_READY (\d+)/.exec(String(chunk));
			if (match) {
				clearTimeout(timer);
				resolvePort(Number(match[1]));
			}
		});
	});
	return { server, port };
}

function stopServer(server) {
	try {
		process.kill(server.pid);
	} catch {
		// already gone
	}
}

// --- capture ---------------------------------------------------------------------------------

async function captureState(browser, baseUrl, state, outDir) {
	const stem = fileStem(state);
	const context = await browser.newContext({
		viewport: { width: state.width, height: HEIGHT },
		deviceScaleFactor: 1,
		locale: 'en-US',
		timezoneId: 'UTC',
		reducedMotion: 'reduce'
	});
	try {
		await context.clock.setFixedTime(FIXED_TIME);
		await context.addInitScript(
			({ user, theme }) => {
				localStorage.setItem('auth_token', 'smoke-access-token');
				localStorage.setItem('refresh_token', 'smoke-refresh-token');
				localStorage.setItem('auth_user', JSON.stringify(user));
				localStorage.setItem('locale', 'en');
				localStorage.setItem('theme', theme);
			},
			{ user: USER, theme: state.theme }
		);
		const blockedWrites = [];
		await context.route('**/*', (route) => {
			const method = route.request().method();
			if (method === 'GET' || method === 'HEAD' || method === 'OPTIONS') return route.continue();
			blockedWrites.push(`${method} ${new URL(route.request().url()).pathname}`);
			return route.abort('blockedbyclient');
		});
		if (state.mock) await state.mock(context);
		const page = await context.newPage();
		const requests = [];
		page.on('request', (request) => {
			const url = new URL(request.url());
			if (request.method() === 'GET' && url.pathname.startsWith('/api/')) {
				requests.push(`GET ${url.pathname}${url.search}`);
			}
		});
		const problems = [];
		page.on('pageerror', (error) => problems.push(`pageerror: ${error.message}`));

		try {
			await page.goto(baseUrl + state.url, { waitUntil: 'load', timeout: 30_000 });
			for (const step of state.steps) await step(page);
			await page.waitForSelector(state.ready, { timeout: READY_TIMEOUT_MS });
			if (state.finalWait) await state.finalWait(page);
		} catch (error) {
			await writeFile(
				join(outDir, `${stem}.skipped.txt`),
				`ready selector or step failed: ${String(error.message).split('\n')[0]}\n`
			);
			return { stem, skipped: String(error.message).split('\n')[0] };
		}

		const idleStarted = Date.now();
		let idle = true;
		await page.waitForLoadState('networkidle', { timeout: IDLE_BOUND_MS }).catch(() => {
			idle = false;
		});
		const idleMs = Date.now() - idleStarted;
		await page.addStyleTag({ content: FREEZE_CSS });
		await page.evaluate(() => document.fonts.ready);
		await page.waitForTimeout(500);
		const before = requests.length;
		// A request loop (an effect that keeps re-running) shows up as growth in this quiet window.
		await page.waitForTimeout(1000);
		const quiet = requests.length === before;

		const html = await page.evaluate(normalisePage);
		await writeFile(join(outDir, `${stem}.html`), html);
		await page.screenshot({ path: join(outDir, `${stem}.png`), fullPage: true });
		const sorted = [...requests].sort();
		await writeFile(
			join(outDir, `${stem}.requests.json`),
			JSON.stringify(
				{ count: sorted.length, idle, quiet, blockedWrites, problems, requests: sorted },
				null,
				2
			) + '\n'
		);
		return { stem, idle, idleMs, quiet, count: sorted.length, problems };
	} finally {
		await context.close();
	}
}

async function capture(flags) {
	if (!flags.out) throw new Error('capture needs --out DIR');
	const outDir = resolve(String(flags.out));
	await rm(outDir, { recursive: true, force: true });
	await mkdir(outDir, { recursive: true });
	const wanted = flags.states ? new Set(String(flags.states).split(',')) : null;
	const states = STATES.filter((state) => !wanted || wanted.has(state.id));
	const playwright = await import('playwright-core');
	const { server, port } = await startServer(outDir);
	const baseUrl = `http://127.0.0.1:${port}`;
	let failed = 0;
	let browser;
	try {
		browser = await playwright.chromium.launch({ executablePath: CHROMIUM_BIN, headless: true });
		for (const state of states) {
			const result = await captureState(browser, baseUrl, state, outDir);
			if (result.skipped) {
				failed += 1;
				console.log(`SKIPPED ${result.stem}: ${result.skipped}`);
			} else {
				const flagsText = [
					result.idle ? '' : 'NOT-IDLE',
					result.quiet ? '' : 'NOT-QUIET',
					result.problems.length ? `${result.problems.length} page errors` : ''
				]
					.filter(Boolean)
					.join(' ');
				if (flagsText) failed += 1;
				console.log(
					`ok ${result.stem} requests=${result.count} idle=${result.idleMs}ms ${flagsText}`.trimEnd()
				);
			}
		}
	} finally {
		if (browser) await browser.close();
		stopServer(server);
		await rm(join(outDir, 'server.pid'), { force: true });
	}
	console.log(`\n${states.length} states captured into ${outDir}, ${failed} skipped or unsettled`);
	process.exitCode = failed ? 1 : 0;
}

// --- compare ---------------------------------------------------------------------------------

function unifiedDiff(a, b) {
	const x = a.split('\n');
	const y = b.split('\n');
	let start = 0;
	while (start < x.length && start < y.length && x[start] === y[start]) start += 1;
	let endX = x.length;
	let endY = y.length;
	while (endX > start && endY > start && x[endX - 1] === y[endY - 1]) {
		endX -= 1;
		endY -= 1;
	}
	const from = Math.max(0, start - 3);
	const lines = [`@@ line ${start + 1} @@`];
	for (let i = from; i < start; i += 1) lines.push(` ${x[i]}`);
	for (let i = start; i < endX; i += 1) lines.push(`-${x[i]}`);
	for (let i = start; i < endY; i += 1) lines.push(`+${y[i]}`);
	for (let i = endX; i < Math.min(x.length, endX + 3); i += 1) lines.push(` ${x[i]}`);
	return lines.join('\n') + '\n';
}

async function compare(positional, flags) {
	const [dirA, dirB] = positional.slice(1).map((path) => resolve(path));
	if (!dirA || !dirB) throw new Error('compare needs DIR_A DIR_B');
	const diffDir = flags['diff-dir'] ? resolve(String(flags['diff-dir'])) : null;
	if (diffDir) await mkdir(diffDir, { recursive: true });
	const stems = (dir) =>
		new Set(
			readdirSync(dir)
				.filter((name) => name.endsWith('.html') || name.endsWith('.skipped.txt'))
				.map((name) => name.replace(/\.(html|skipped\.txt)$/, ''))
		);
	const a = stems(dirA);
	const b = stems(dirB);
	let differing = 0;
	let compared = 0;
	for (const stem of [...new Set([...a, ...b])].sort()) {
		if (!a.has(stem) || !b.has(stem)) {
			console.log(`DIFF ${stem}: present only in ${a.has(stem) ? 'A' : 'B'}`);
			differing += 1;
			continue;
		}
		const skippedA = existsSync(join(dirA, `${stem}.skipped.txt`));
		const skippedB = existsSync(join(dirB, `${stem}.skipped.txt`));
		if (skippedA || skippedB) {
			console.log(
				`DIFF ${stem}: skipped in ${skippedA && skippedB ? 'both' : skippedA ? 'A' : 'B'}`
			);
			differing += 1;
			continue;
		}
		compared += 1;
		const htmlA = readFileSync(join(dirA, `${stem}.html`), 'utf8');
		const htmlB = readFileSync(join(dirB, `${stem}.html`), 'utf8');
		if (htmlA !== htmlB) {
			differing += 1;
			console.log(`DIFF ${stem}: normalised HTML differs`);
			if (diffDir) await writeFile(join(diffDir, `${stem}.diff`), unifiedDiff(htmlA, htmlB));
			else console.log(unifiedDiff(htmlA, htmlB));
		}
		const requestsA = JSON.parse(readFileSync(join(dirA, `${stem}.requests.json`), 'utf8'));
		const requestsB = JSON.parse(readFileSync(join(dirB, `${stem}.requests.json`), 'utf8'));
		if (JSON.stringify(requestsA.requests) !== JSON.stringify(requestsB.requests)) {
			differing += 1;
			console.log(`DIFF ${stem}: GET requests differ (A ${requestsA.count}, B ${requestsB.count})`);
			const setA = new Set(requestsA.requests);
			const setB = new Set(requestsB.requests);
			for (const request of setA) if (!setB.has(request)) console.log(`  only in A: ${request}`);
			for (const request of setB) if (!setA.has(request)) console.log(`  only in B: ${request}`);
		}
		if (!requestsB.idle || !requestsB.quiet) {
			differing += 1;
			console.log(
				`DIFF ${stem}: B did not settle (idle=${requestsB.idle}, quiet=${requestsB.quiet})`
			);
		}
	}
	console.log(`\n${compared} states compared, ${differing} differences`);
	process.exitCode = differing ? 1 : 0;
}

// --- main ------------------------------------------------------------------------------------

function parseArgs(argv) {
	const positional = [];
	const flags = {};
	for (let i = 0; i < argv.length; i += 1) {
		if (argv[i].startsWith('--'))
			flags[argv[i].slice(2)] = argv[i + 1]?.startsWith('--') ? true : argv[++i];
		else positional.push(argv[i]);
	}
	return { positional, flags };
}

async function main() {
	const { positional, flags } = parseArgs(process.argv.slice(2));
	const command = positional[0];
	if (command === 'list') {
		for (const state of STATES) console.log(`${fileStem(state)}\t${state.url}`);
	} else if (command === 'capture') await capture(flags);
	else if (command === 'compare') await compare(positional, flags);
	else {
		console.error('usage: forms-dom-equivalence.mjs list | capture --out DIR | compare A B');
		process.exitCode = 2;
	}
}

if (import.meta.url === `file://${process.argv[1]}`) {
	main().catch((error) => {
		console.error(`forms-dom-equivalence: ${error.message}`);
		process.exit(1);
	});
}
