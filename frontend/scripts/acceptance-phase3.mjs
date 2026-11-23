#!/usr/bin/env node
// Phase 3 acceptance run against the published UI, driven in headless Chromium.
//
//   node scripts/acceptance-phase3.mjs --out DIR [--only confirmations,flow,plugins,errors,narrow,keyboard,embed]
//                                      [--workspace ID] [--ui URL] [--api URL]
//
// The run creates ONE dedicated workspace (`p3-acceptance-<timestamp>`) and every object it needs
// inside it; nothing outside that workspace is read for its data, changed or deleted. The whole run
// exceeds ten minutes, so it is meant to be run section by section: the first run creates the
// workspace and prints its id, later runs name it with `--workspace ID` (the script refuses a
// workspace whose name does not start with `p3-acceptance-`).
//
// Blind spots it closes (see the section functions):
//   confirmations  every `confirmAction` call site that earlier checks only saw with mocked data
//   flow           the Flow editor with the feature switched on, light and dark at 1440 and 800
//   plugins        a plugin app enabled in the project: Apps page, plugin frame, record plugin section
//   errors         failed requests are shown inline in `main` (role="alert"), never as a toast
//   narrow         800 and 390 wide: top bar, list/content switch, overflow, sheets, dialog buttons
//   keyboard       Tab order, focus rings, Escape and focus return, Cmd/Ctrl+K palette
//   embed          the embed harness at 340, 480, 600 and full width
//
// Browser: the locally installed Chromium (CHROMIUM_BIN overrides the path), headless.
// Credentials: read at run time from the environment or from the env file (UI_SHOTS_ENV_FILE); they
// and the tokens the run creates are never printed or written; the JSON result and the screenshots
// hold no secrets.
//
// Output: one line per check (PASS / FAIL / SKIP and the observed value), a final count, the JSON
// result `acceptance-phase3.json` and a screenshot per failing check in --out. Exit code 1 when any
// check failed.

import { existsSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { LEGACY_PALETTE_SOURCE, contrastProbeSource } from './ui-shots.mjs';

const DEFAULT_UI = 'http://127.0.0.1:18097';
const DEFAULT_API = 'http://127.0.0.1:18083';
const DEFAULT_ENV_FILE =
	'/Users/nn/worker/llama/reports/sv-plugin-analysis-2026-11-14/release-next-local-deploy-20261119T153906Z/acceptance.env';
const DEFAULT_CHROMIUM = '/Applications/Chromium.app/Contents/MacOS/Chromium';
const DEFAULT_PLUGIN_FIXTURE =
	'/Users/nn/worker/llama/ops/sylvode-plugin-v1/p3-extension-acceptance/fixtures/probe-panel.html';
const MARKER = 'p3-acceptance-';
const SECTIONS = ['confirmations', 'flow', 'plugins', 'errors', 'narrow', 'keyboard', 'embed'];
const THEMES = ['light', 'dark'];
const FLOW_WIDTHS = [1440, 800];
const NIL_ID = '00000000-0000-4000-8000-000000000001';

// ---------------------------------------------------------------------------------------------
// Arguments, configuration, results
// ---------------------------------------------------------------------------------------------

function parseArgs(argv) {
	const flags = {};
	for (let i = 0; i < argv.length; i += 1) {
		if (!argv[i].startsWith('--')) continue;
		const next = argv[i + 1];
		if (next === undefined || next.startsWith('--')) flags[argv[i].slice(2)] = true;
		else {
			flags[argv[i].slice(2)] = next;
			i += 1;
		}
	}
	return flags;
}

const flags = parseArgs(process.argv.slice(2));
const UI = String(flags.ui || process.env.P3A_UI || DEFAULT_UI).replace(/\/$/, '');
const API = String(flags.api || process.env.P3A_API || DEFAULT_API).replace(/\/$/, '');
const OUT = resolve(String(flags.out || process.env.P3A_OUT || './acceptance-phase3-out'));
const only = flags.only
	? String(flags.only)
			.split(',')
			.map((s) => s.trim())
	: SECTIONS;
for (const name of only) {
	if (!SECTIONS.includes(name)) {
		console.error(`unknown section "${name}" (sections: ${SECTIONS.join(', ')})`);
		process.exit(2);
	}
}
const STAMP = new Date().toISOString().replace(/[-:]/g, '').replace(/\..*/, 'Z');

const results = [];
let currentSection = '';
let currentPage = null;
let shotCounter = 0;

function slug(text) {
	return text
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-|-$/g, '')
		.slice(0, 70);
}

/** Runs one check; `body` returns the observed value (string) or throws to fail. */
async function check(name, body) {
	const started = Date.now();
	let status = 'PASS';
	let observed = '';
	try {
		const value = await body();
		if (value && typeof value === 'object' && value.skip) {
			status = 'SKIP';
			observed = String(value.skip);
		} else observed = value === undefined ? '' : String(value);
	} catch (error) {
		status = 'FAIL';
		observed = String(error instanceof Error ? error.message : error).split('\n')[0];
	}
	let screenshot = null;
	if (status === 'FAIL' && currentPage && !currentPage.isClosed()) {
		shotCounter += 1;
		screenshot = join(
			OUT,
			`${STAMP}-${String(shotCounter).padStart(3, '0')}-${slug(`${currentSection}-${name}`)}.png`
		);
		await currentPage.screenshot({ path: screenshot, animations: 'disabled' }).catch(() => {
			screenshot = null;
		});
	}
	const entry = {
		section: currentSection,
		name,
		status,
		observed: redact(observed),
		screenshot,
		ms: Date.now() - started
	};
	results.push(entry);
	console.log(`${status} [${currentSection}] ${name} -- ${entry.observed}`);
	return status === 'PASS';
}

function expect(condition, message) {
	if (!condition) throw new Error(message);
}

const secrets = new Set();
function redact(text) {
	let out = String(text);
	for (const secret of secrets)
		if (secret && out.includes(secret)) out = out.split(secret).join('***');
	return out.replace(/opr_[0-9a-f]{8,}/g, 'opr_***').replace(/Bearer\s+[\w.-]+/g, 'Bearer ***');
}

// ---------------------------------------------------------------------------------------------
// Credentials and the API (requests are made from node; tokens never leave this process)
// ---------------------------------------------------------------------------------------------

async function readCredentials(user) {
	const emailKey = `P3X_${user}_EMAIL`;
	const passwordKey = `P3X_${user}_PASSWORD`;
	let email = process.env[emailKey];
	let password = process.env[passwordKey];
	const file = process.env.UI_SHOTS_ENV_FILE || DEFAULT_ENV_FILE;
	if ((!email || !password) && existsSync(file)) {
		for (const line of (await readFile(file, 'utf8')).split('\n')) {
			const match = line.match(/^(?:export\s+)?(\w+)=(.*)$/);
			if (!match) continue;
			const value = match[2].trim().replace(/^['"]|['"]$/g, '');
			if (match[1] === emailKey) email ||= value;
			if (match[1] === passwordKey) password ||= value;
		}
	}
	if (!email || !password) throw new Error(`no credentials for ${user}`);
	secrets.add(password);
	return { email, password };
}

async function signIn(user = 'U1') {
	const response = await fetch(`${API}/api/v1/auth/login`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify(await readCredentials(user))
	});
	const body = await response.json().catch(() => null);
	if (!response.ok || body?.code !== 0 || !body?.data?.tokens?.access_token)
		throw new Error(`sign-in failed with HTTP ${response.status}`);
	secrets.add(body.data.tokens.access_token);
	secrets.add(body.data.tokens.refresh_token);
	return { ...body.data, signedAt: Date.now(), user_key: user };
}

/** API client of one signed-in account. `call` returns the envelope (`code`, `message`, `data`). */
function apiFor(session) {
	async function call(method, path, body) {
		const response = await fetch(API + path, {
			method,
			headers: {
				authorization: `Bearer ${session.tokens.access_token}`,
				'content-type': 'application/json'
			},
			body: body === undefined ? undefined : JSON.stringify(body)
		});
		const text = await response.text();
		let envelope;
		try {
			envelope = JSON.parse(text);
		} catch {
			envelope = { code: response.status, message: text.slice(0, 200), data: null };
		}
		return { http: response.status, ...envelope };
	}
	/** Same call, but a non-zero code throws (used while creating test data). */
	async function must(method, path, body) {
		const result = await call(method, path, body);
		if (result.code !== 0)
			throw new Error(`${method} ${path.split('?')[0]} -> ${result.code} ${result.message}`);
		return result.data;
	}
	return { call, must, session };
}

const uuid = () => globalThis.crypto.randomUUID();
const items = (data) => data?.items ?? (Array.isArray(data) ? data : []);

// ---------------------------------------------------------------------------------------------
// Fixture: the dedicated workspace
// ---------------------------------------------------------------------------------------------

const world = {};

async function ensureWorld(api) {
	const reuse = typeof flags.workspace === 'string' ? flags.workspace : null;
	if (reuse) {
		const workspace = await api.must('GET', `/api/v1/workspaces/${reuse}`);
		if (!String(workspace.name).startsWith(MARKER))
			throw new Error(
				`workspace ${reuse} is not an acceptance workspace (name must start with ${MARKER})`
			);
		world.ws = workspace.id;
		world.wsName = workspace.name;
		const projects = items(await api.must('GET', `/api/v1/workspaces/${reuse}/projects`));
		const project = projects.find((p) => p.key === 'ACC');
		if (!project) throw new Error('the workspace has no project ACC');
		world.project = project.id;
	} else {
		const name = `${MARKER}${STAMP.toLowerCase()}`;
		const workspace = await api.must('POST', '/api/v1/workspaces', { slug: name, name });
		world.ws = workspace.id;
		world.wsName = name;
		const project = await api.must('POST', `/api/v1/workspaces/${world.ws}/projects`, {
			name: 'Acceptance',
			key: 'ACC'
		});
		world.project = project.id;
	}
	// Flow is switched on per workspace (workspace admin setting); the runtime capabilities of the
	// environment are already on.
	const flow = await api.must('PUT', `/api/v1/workspaces/${world.ws}/features/flow`, {
		enabled: true,
		idempotency_key: uuid()
	});
	world.flowEnabled = flow.flow_enabled === true;
	console.log(
		`workspace: ${world.ws} (${world.wsName}), project: ${world.project}, flow_enabled: ${world.flowEnabled}`
	);
}

// ---------------------------------------------------------------------------------------------
// Browser helpers
// ---------------------------------------------------------------------------------------------

let browser = null;
let sessions = {};

async function launch() {
	const { chromium } = await import('playwright-core');
	const executablePath = process.env.CHROMIUM_BIN || DEFAULT_CHROMIUM;
	if (!existsSync(executablePath)) throw new Error(`Chromium not found at ${executablePath}`);
	browser = await chromium.launch({ executablePath, headless: true });
}

/** A session younger than ten minutes, or a fresh sign-in. */
async function sessionOf(user = 'U1') {
	const cached = sessions[user];
	if (cached && Date.now() - cached.signedAt < 10 * 60_000) return cached;
	sessions[user] = await signIn(user);
	return sessions[user];
}

const FREEZE_CSS = `*, *::before, *::after { animation: none !important; transition: none !important; caret-color: transparent !important; scroll-behavior: auto !important; }`;

/**
 * Opens `path` in a fresh context. `window.confirm`, `alert` and `prompt` are stubbed to record their
 * calls (`window.__native`). Page errors and console errors are collected on `page.__problems`.
 */
async function openPage(
	path,
	{
		width = 1440,
		height = 900,
		theme = 'light',
		user = 'U1',
		auth = true,
		freeze = false,
		settle = true
	} = {}
) {
	const session = await sessionOf(user);
	const context = await browser.newContext({
		viewport: { width, height },
		deviceScaleFactor: 1,
		colorScheme: theme,
		reducedMotion: 'reduce',
		locale: 'en-US',
		timezoneId: 'UTC'
	});
	await context.addInitScript(
		({ theme: themeValue, tokens, user: sessionUser, authenticated }) => {
			try {
				localStorage.setItem('locale', 'en');
				localStorage.setItem('theme', themeValue);
				if (authenticated) {
					localStorage.setItem('auth_token', tokens.access_token);
					localStorage.setItem('refresh_token', tokens.refresh_token);
					localStorage.setItem('auth_user', JSON.stringify(sessionUser));
				}
			} catch {
				// sandboxed frames have no storage
			}
			try {
				window.__native = [];
				window.confirm = (message) => {
					window.__native.push(`confirm:${message}`);
					return false;
				};
				window.alert = (message) => {
					window.__native.push(`alert:${message}`);
				};
				window.prompt = (message) => {
					window.__native.push(`prompt:${message}`);
					return null;
				};
			} catch {
				// not writable
			}
		},
		{ theme, tokens: session.tokens, user: session.user, authenticated: auth }
	);
	const page = await context.newPage();
	page.setDefaultTimeout(10_000);
	page.__problems = [];
	page.__width = width;
	page.__theme = theme;
	page.on('pageerror', (error) =>
		page.__problems.push(`pageerror: ${error.message.slice(0, 160)}`)
	);
	page.on('console', (message) => {
		if (message.type() === 'error')
			page.__problems.push(`console: ${message.text().slice(0, 160)}`);
	});
	currentPage = page;
	await page.goto(UI + path, { waitUntil: 'load', timeout: 30_000 });
	if (settle) await settled(page);
	if (freeze) await page.addStyleTag({ content: FREEZE_CSS });
	return page;
}

async function settled(page, extra = 300) {
	await page.waitForLoadState('networkidle', { timeout: 8000 }).catch(() => {});
	await page.waitForTimeout(extra);
}

async function closePage(page) {
	if (currentPage === page) currentPage = null;
	await page
		.context()
		.close()
		.catch(() => {});
}

/** Problems collected since `mark` (an index into page.__problems). */
const problemsSince = (page, mark = 0) => page.__problems.slice(mark);

/** The legacy-style scan of ui-shots.mjs, plus its contrast probe, over the page and the plugin host frame. */
async function scan(page) {
	const legacyScan = (source) => {
		const palette = new RegExp(source);
		let count = 0;
		const sample = [];
		for (const element of document.querySelectorAll('[class]')) {
			const value = element.getAttribute('class') || '';
			if (palette.test(value) || value.includes('dark:')) {
				count += 1;
				if (sample.length < 3)
					sample.push(`${element.tagName.toLowerCase()}.${value.slice(0, 60)}`);
			}
		}
		return { count, sample };
	};
	const legacy = await page.evaluate(legacyScan, LEGACY_PALETTE_SOURCE);
	const contrast = await page.evaluate(contrastProbeSource());
	// The plugin host frame is a product document of its own; the plugin's content inside it is not.
	for (const frame of page.frames()) {
		if (frame === page.mainFrame() || !frame.url().includes('/plugin-host/')) continue;
		const inner = await frame.evaluate(legacyScan, LEGACY_PALETTE_SOURCE).catch(() => null);
		const measured = await frame.evaluate(contrastProbeSource()).catch(() => null);
		if (inner) {
			legacy.count += inner.count;
			legacy.sample.push(...inner.sample.map((x) => `[host frame] ${x}`));
		}
		if (measured) {
			contrast.measured += measured.measured;
			contrast.below45 += measured.below45;
			contrast.worst.push(...measured.worst);
			if (measured.min !== null && (contrast.min === null || measured.min < contrast.min))
				contrast.min = measured.min;
		}
	}
	return { legacy, contrast };
}

/**
 * Tabs through the page from `start` (a selector to focus first, or null to continue where focus is) and
 * reports controls whose focus ring is missing. A ring is a computed outline of at least 2px in a visible
 * style or a box-shadow ring. A focusable splitter (`role="separator"`) draws its own 3px bar and a field
 * whose box carries the ring (`:focus-within`) counts as having its ring. A native control that stops several times (the parts of a date field) is judged once.
 * The walk ends when focus enters a text-editing surface of the editor (a further Tab there edits).
 */
async function ringAudit(page, { start = null, tabs = 14 } = {}) {
	if (start) {
		await page.evaluate((selector) => {
			const element = document.querySelector(selector);
			if (element) element.focus({ preventScroll: true });
		}, start);
	}
	await page.evaluate(() =>
		document.querySelectorAll('[data-acc-ring]').forEach((e) => e.removeAttribute('data-acc-ring'))
	);
	const seen = [];
	for (let i = 0; i < tabs; i += 1) {
		await page.keyboard.press('Tab');
		const info = await page.evaluate(() => {
			const el = document.activeElement;
			if (!el || el === document.body) return null;
			const repeated = el.hasAttribute('data-acc-ring');
			el.setAttribute('data-acc-ring', '1');
			const style = getComputedStyle(el);
			const width = parseFloat(style.outlineWidth) || 0;
			const ringOutline =
				width >= 2 && style.outlineStyle !== 'none' && style.outlineColor !== 'rgba(0, 0, 0, 0)';
			const ringShadow =
				style.boxShadow !== 'none' &&
				/\b[2-9]px\b/.test(style.boxShadow.replace(/rgba?\([^)]*\)/g, ''));
			// A field's ring may sit on the component's box (`:focus-within`), as the text field does.
			let boxed = false;
			for (
				let node = el.parentElement, depth = 0;
				node && depth < 3;
				node = node.parentElement, depth += 1
			) {
				if (!node.matches(':focus-within')) continue;
				const outer = getComputedStyle(node);
				if ((parseFloat(outer.outlineWidth) || 0) >= 2 && outer.outlineStyle !== 'none')
					boxed = true;
			}
			const own = el.getAttribute('role') === 'separator' || boxed;
			const label = el.labels && el.labels[0] ? el.labels[0].textContent : '';
			const name = (
				el.getAttribute('aria-label') ||
				label ||
				el.textContent ||
				el.getAttribute('placeholder') ||
				el.type ||
				''
			)
				.trim()
				.replace(/\s+/g, ' ')
				.slice(0, 30);
			return {
				repeated,
				tag: el.tagName.toLowerCase(),
				name,
				ring: ringOutline || ringShadow || own,
				outline: `${style.outlineWidth} ${style.outlineStyle}`,
				editing: el.isContentEditable
			};
		});
		if (!info) continue;
		if (!info.repeated) seen.push(info);
		if (info.editing) break;
	}
	const bad = seen.filter((entry) => !entry.ring);
	return { checked: seen.length, bad };
}

/** The content checks of one visible state: no page problem since `mark`, no legacy style, text contrast. */
async function contentChecks(page, label, { mark = 0 } = {}) {
	await check(`${label}: no page error, no legacy style, contrast >= 4.5`, async () => {
		const problems = problemsSince(page, mark);
		const { legacy, contrast } = await scan(page);
		const summary = `errors=${problems.length} legacy=${legacy.count} contrast.min=${contrast.min} below4.5=${contrast.below45} (measured ${contrast.measured}; disabled min ${contrast.minDisabled})`;
		const failures = [];
		if (problems.length) failures.push(`page problems: ${problems.slice(0, 2).join(' ; ')}`);
		if (legacy.count) failures.push(`legacy styles: ${legacy.sample.join(' ; ')}`);
		if (contrast.below45)
			failures.push(
				`contrast below 4.5 (${contrast.below45}): ${JSON.stringify(contrast.worst.slice(0, 2))}`
			);
		if (failures.length) throw new Error(`${summary} -- ${failures.join(' | ')}`);
		return summary;
	});
}

/** The focus-ring check of one visible state. */
async function ringChecks(page, label, options = {}) {
	await check(`${label}: keyboard focus ring of 2px`, async () => {
		const result = await ringAudit(page, options);
		if (result.checked === 0) return { skip: 'the state has no keyboard-focusable control' };
		if (result.bad.length)
			throw new Error(
				`${result.checked - result.bad.length}/${result.checked} controls have a ring; none on ${result.bad
					.slice(0, 3)
					.map((b) => `${b.tag} "${b.name}" (outline ${b.outline})`)
					.join(', ')}`
			);
		return `${result.checked}/${result.checked} focused controls show a ring`;
	});
}

/** Marks the smallest container that holds `text` and a button matching `buttonPattern`; clicks it. */
async function clickRowButton(page, text, buttonPattern, { root = 'main' } = {}) {
	const found = await page.evaluate(
		({ text: needle, pattern, root: rootSelector }) => {
			const matcher = new RegExp(pattern, 'i');
			const scope = document.querySelector(rootSelector) || document.body;
			const visible = (element) => {
				const box = element.getBoundingClientRect();
				return box.width > 0 && box.height > 0 && !element.closest('[inert], [hidden]');
			};
			const holders = [...scope.querySelectorAll('*')].filter(
				(element) =>
					[...element.childNodes].some(
						(node) => node.nodeType === 3 && node.nodeValue.includes(needle)
					) && visible(element)
			);
			for (const holder of holders) {
				let node = holder;
				for (
					let depth = 0;
					node && depth < 10 && node !== scope.parentElement;
					depth += 1, node = node.parentElement
				) {
					const buttons = [...node.querySelectorAll('button, [role="button"], a')].filter(
						(button) =>
							matcher.test(
								(button.getAttribute('aria-label') || button.textContent || '').trim()
							) &&
							!button.disabled &&
							visible(button)
					);
					if (buttons.length) {
						document
							.querySelectorAll('[data-acc-target]')
							.forEach((e) => e.removeAttribute('data-acc-target'));
						buttons[0].setAttribute('data-acc-target', '1');
						return buttons.length;
					}
				}
			}
			return 0;
		},
		{ text, pattern: buttonPattern.source, root }
	);
	if (!found) throw new Error(`no ${buttonPattern} button next to "${text}"`);
	await page
		.locator('[data-acc-target="1"]')
		.first()
		.scrollIntoViewIfNeeded()
		.catch(() => {});
	await page.locator('[data-acc-target="1"]').first().click();
}

// ---------------------------------------------------------------------------------------------
// Section 1: confirmations
// ---------------------------------------------------------------------------------------------

const alertState = (page) =>
	page.evaluate(() => {
		const dialog = document.querySelector('[role="alertdialog"]');
		if (!dialog) return null;
		const buttons = [...dialog.querySelectorAll('footer button')];
		const active = document.activeElement;
		const label = (button) =>
			(button.getAttribute('aria-label') || button.textContent || '').trim();
		return {
			title: (dialog.querySelector('h2')?.textContent || '').trim(),
			cancel: buttons[0] ? label(buttons[0]) : '',
			confirm: buttons[1] ? label(buttons[1]) : '',
			focusIsCancel: !!active && buttons[0] === active,
			focusText: active ? (active.textContent || '').trim().slice(0, 30) : '',
			clipped: buttons.map((button) => {
				const box = button.getBoundingClientRect();
				const frame = dialog.getBoundingClientRect();
				return (
					box.left < frame.left - 1 || box.right > frame.right + 1 || box.right > innerWidth + 1
				);
			}),
			width: Math.round(dialog.getBoundingClientRect().width)
		};
	});

/** Errors on the page after a confirmed action: an inline error tip, an alert in main, or an error toast. */
const errorsShown = (page) =>
	page.evaluate(() => ({
		tip: [
			...document.querySelectorAll(
				'[data-ui="tip-bar"][data-tone="danger"], [data-ui="tip-bar"][data-tone="warning"]'
			)
		].map((e) => e.textContent.trim().slice(0, 80)),
		alerts: [...document.querySelectorAll('main [role="alert"]')].map((e) =>
			e.textContent.trim().slice(0, 80)
		),
		toasts: [
			...document.querySelectorAll(
				'[data-toast][data-tone="warning"], [data-toast][data-tone="danger"]'
			)
		].map((e) => e.textContent.trim().slice(0, 80))
	}));

/**
 * Drives one destructive confirmation: native confirm never called, an alertdialog with focus on Cancel and a
 * confirm label that names verb and object, Cancel keeps the object, Confirm removes it and shows no error.
 */
async function driveConfirm(name, page, { open, verb, object, exists }) {
	const mark = page.__problems.length;
	await check(`${name}: confirmation dialog`, async () => {
		await page.evaluate(() => (window.__native = []));
		await open();
		await page.waitForSelector('[role="alertdialog"]', { timeout: 5000 });
		await page.waitForTimeout(150);
		const native = await page.evaluate(() => window.__native.length);
		expect(native === 0, `${native} native dialog call(s)`);
		const state = await alertState(page);
		expect(state, 'no alertdialog');
		expect(state.focusIsCancel, `focus starts on "${state.focusText}", not Cancel`);
		const label = state.confirm;
		expect(
			new RegExp(verb, 'i').test(label),
			`confirm label "${label}" does not name the verb ${verb}`
		);
		expect(object.test(label), `confirm label "${label}" does not name the object (${object})`);
		return `native=0 role=alertdialog focus=Cancel confirm="${label}" title="${state.title.slice(0, 60)}"`;
	});
	await check(`${name}: Cancel keeps the object`, async () => {
		if (!(await page.$('[role="alertdialog"]'))) throw new Error('dialog not open');
		await page.locator('[role="alertdialog"] footer button').first().click();
		await page.waitForSelector('[role="alertdialog"]', { state: 'detached', timeout: 5000 });
		await page.waitForTimeout(400);
		const still = await exists();
		expect(still, 'the object is gone after Cancel');
		return 'object still present (API), dialog closed';
	});
	await check(`${name}: Confirm removes the object, no error`, async () => {
		await page.evaluate(() => (window.__native = []));
		await open();
		await page.waitForSelector('[role="alertdialog"]', { timeout: 5000 });
		await page.locator('[role="alertdialog"] footer button').nth(1).click();
		await page.waitForSelector('[role="alertdialog"]', { state: 'detached', timeout: 5000 });
		let gone = false;
		for (let i = 0; i < 20 && !gone; i += 1) {
			gone = !(await exists());
			if (!gone) await page.waitForTimeout(250);
		}
		expect(gone, 'the object is still present (API) after Confirm');
		await page.waitForTimeout(500);
		const shown = await errorsShown(page);
		const total = shown.tip.length + shown.alerts.length + shown.toasts.length;
		expect(total === 0, `error shown after delete: ${JSON.stringify(shown)}`);
		const native = await page.evaluate(() => window.__native.length);
		expect(native === 0, `${native} native dialog call(s)`);
		const problems = problemsSince(page, mark);
		expect(problems.length === 0, `page problems: ${problems.slice(0, 2).join(' ; ')}`);
		return 'object gone (API), no error tip, alert or toast, no page error';
	});
}

async function sectionConfirmations(api) {
	const stamp = Date.now().toString(36);
	const ws = world.ws;
	const project = world.project;

	// --- access token ----------------------------------------------------------------------
	{
		const tokenName = `acc-token-${stamp}`;
		const bot = await api.must('POST', `/api/v1/workspaces/${ws}/bots`, {
			name: tokenName,
			permissions: ['read'],
			transport_surface: 'rest'
		});
		if (bot.token) secrets.add(bot.token);
		const page = await openPage(`/workspace/${ws}/members`);
		await driveConfirm('revoke access token', page, {
			open: () => clickRowButton(page, tokenName, /^revoke/),
			verb: 'revoke',
			object: new RegExp(tokenName),
			exists: async () => {
				const list = (await api.call('GET', `/api/v1/workspaces/${ws}/bots`)).data ?? [];
				const row = items(list).find((b) => b.id === bot.id);
				return !!row && row.is_active !== false;
			}
		});
		await closePage(page);
	}

	// --- webhook ---------------------------------------------------------------------------
	{
		const url = `https://example.com/acc-${stamp}`;
		const hook = await api.must('POST', `/api/v1/workspaces/${ws}/webhooks`, {
			name: `acc-${stamp}`,
			url,
			events: ['issue.created']
		});
		const page = await openPage(`/workspace/${ws}/webhooks`);
		await driveConfirm('delete webhook', page, {
			open: () => clickRowButton(page, url, /^delete$/),
			verb: 'delete',
			object: /webhook|example\.com/i,
			exists: async () =>
				(await api.call('GET', `/api/v1/workspaces/${ws}/webhooks/${hook.id}`)).code === 0
		});
		await closePage(page);
	}

	// --- sprint / cycle --------------------------------------------------------------------
	{
		const name = `acc-sprint-${stamp}`;
		const sprint = await api.must('POST', `/api/v1/projects/${project}/sprints`, {
			name,
			start_date: '2026-11-20',
			end_date: '2026-12-04'
		});
		const page = await openPage(`/workspace/${ws}/projects/${project}/cycles`);
		await driveConfirm('delete sprint', page, {
			open: () => clickRowButton(page, name, /^delete$/),
			verb: 'delete',
			object: new RegExp(name),
			exists: async () => sprintExists(api, project, sprint.id)
		});
		await closePage(page);
	}

	// --- bot connection ---------------------------------------------------------------------
	// The Execution section's page arrives later; the connection API is what is accepted here:
	// connect, read, and remove (the last connection releases the bot's token and webhook).
	await check(
		'remove bot connection: the connection goes, the token is revoked, the webhook is deleted',
		async () => {
			const name = `Acc Bot ${stamp}`;
			const path = `/api/v1/projects/${project}/bot-connection`;
			const connected = await api.must('POST', path, {
				bot: { name },
				runner_url: 'http://93.184.216.34:9090',
				cli: 'claude',
				working_directory: '/tmp/acceptance'
			});
			expect(
				connected.config_block?.text?.includes('token = "opr_'),
				'the connect answer has no configuration block'
			);
			secrets.add(connected.config_block.text);
			const botId = connected.connection.bot.id;
			const read = await api.must('GET', path);
			expect(read.connection?.bot?.id === botId, 'the connection is not readable after connect');
			expect(
				read.config_block === undefined && !JSON.stringify(read).includes('opr_'),
				'a read returned a secret'
			);
			const hooks = items(await api.must('GET', `/api/v1/workspaces/${ws}/webhooks`));
			expect(
				hooks.some(
					(hook) => hook.bot_user_id === botId && hook.managed_by?.kind === 'project_connection'
				),
				'the managed webhook is not reported as managed'
			);
			await api.must('DELETE', `${path}?expected_version=${read.connection.version}`);
			expect(
				(await api.must('GET', path)).connection === null,
				'the connection is still there after removal'
			);
			const hooksAfter = items(await api.must('GET', `/api/v1/workspaces/${ws}/webhooks`));
			expect(!hooksAfter.some((hook) => hook.bot_user_id === botId), 'the webhook was not deleted');
			const bots = items(await api.must('GET', `/api/v1/workspaces/${ws}/bots`));
			const left = bots.find((bot) => bot.id === botId);
			expect(left !== undefined && left.is_active === false, 'the token was not revoked');
			return 'connected, read, removed; token revoked; webhook deleted';
		}
	);

	// --- workflow and workflow state -------------------------------------------------------
	{
		const name = `acc-workflow-${stamp}`;
		const workflow = await api.must('POST', `/api/v1/workspaces/${ws}/workflows`, {
			name,
			description: 'acceptance'
		});
		const stateKey = `acc_state_${stamp}`.replace(/[^a-z0-9_]/g, '_');
		const stateName = `Acc State ${stamp}`;
		const state = await api.must('POST', `/api/v1/workflows/${workflow.id}/states`, {
			key: stateKey,
			display_name: stateName,
			category: 'active'
		});
		const page = await openPage(`/workspace/${ws}/workflows`);
		await check('open the states dialog of the new workflow', async () => {
			await clickRowButton(page, name, /manage states/);
			await page.waitForSelector('[data-ui-layer="dialog"] table, [data-ui-layer="dialog"] tbody');
			await page.waitForSelector(`text=${stateName}`);
			return `dialog lists "${stateName}"`;
		});
		await driveConfirm('delete workflow state', page, {
			open: () => clickRowButton(page, stateName, /^delete$/, { root: '[data-ui-layer="dialog"]' }),
			verb: 'delete',
			object: /state|Acc State/i,
			exists: async () => {
				const detail = await api.call('GET', `/api/v1/workflows/${workflow.id}`);
				return (detail.data?.states ?? []).some((s) => s.id === state.id);
			}
		});
		await page.keyboard.press('Escape');
		await page.waitForTimeout(300);
		await driveConfirm('delete workflow', page, {
			open: () => clickRowButton(page, name, /^delete$/),
			verb: 'delete',
			object: new RegExp(name),
			exists: async () => (await api.call('GET', `/api/v1/workflows/${workflow.id}`)).code === 0
		});
		await closePage(page);
	}

	// --- issue comment ---------------------------------------------------------------------
	{
		const issue = await api.must('POST', `/api/v1/projects/${project}/issues`, {
			title: `acc issue ${stamp}`,
			description: 'acceptance'
		});
		const text = `acc comment ${stamp}`;
		const comment = await api.must('POST', `/api/v1/issues/${issue.id}/comments`, {
			content: text
		});
		const page = await openPage(`/workspace/${ws}/projects/${project}/issues/${issue.id}`);
		await driveConfirm('delete issue comment', page, {
			open: () => clickRowButton(page, text, /^delete$/),
			verb: 'delete',
			object: /comment/i,
			exists: async () => {
				const list = items((await api.call('GET', `/api/v1/issues/${issue.id}/comments`)).data);
				return list.some((c) => c.id === comment.id);
			}
		});
		await closePage(page);
	}

	// --- proposal template (real data) -----------------------------------------------------
	{
		const name = `acc-template-${stamp}`;
		const template = await api.must('POST', '/api/v1/proposal-templates', {
			project_id: project,
			name,
			content: { body: 'acceptance template body' }
		});
		const page = await openPage('/proposal-templates');
		await check('proposal templates: select the acceptance project', async () => {
			const options = await page.$$eval('main select option', (nodes) =>
				nodes.map((n) => ({ value: n.value, text: n.textContent.trim() }))
			);
			const option = options.find((o) => o.value === project);
			expect(option, `no option for the project among ${options.length} options`);
			await page.selectOption('main select', project);
			await page.waitForSelector(`text=${name}`, { timeout: 8000 });
			return `template "${name}" listed`;
		});
		await driveConfirm('delete proposal template', page, {
			open: () => clickRowButton(page, name, /^delete$/),
			verb: 'delete',
			object: new RegExp(name),
			exists: async () => {
				const list = items(
					(await api.call('GET', `/api/v1/proposal-templates?project_id=${project}`)).data
				);
				return list.some((t) => t.id === template.id);
			}
		});
		await closePage(page);
	}

	// --- trust-score appeal (needs a trust score log of the signed-in user) -----------------
	{
		const me = api.session.user.id;
		const history = items((await api.call('GET', `/api/v1/trust-scores/${me}/history`)).data);
		const log = history[0];
		await check('delete trust-score appeal', async () => {
			if (!log)
				return {
					skip: 'no trust score log exists for the account (logs are written only by the worker when a proposal is decided); an appeal needs one and the API cannot create it'
				};
			const created = await api.must('POST', '/api/v1/trust-scores/appeals', {
				log_id: log.id,
				reason: `acceptance appeal ${stamp} with a reason of sufficient length`
			});
			const page = await openPage('/trust-scores/appeals');
			await driveConfirm('delete trust-score appeal', page, {
				open: () => clickRowButton(page, `acceptance appeal ${stamp}`, /^delete$/),
				verb: 'delete',
				object: /appeal/i,
				exists: async () => {
					const list = items((await api.call('GET', '/api/v1/trust-scores/appeals')).data);
					return list.some((a) => a.id === created.id);
				}
			});
			await closePage(page);
			return 'appeal confirmation driven';
		});
	}

	// --- draft proposal (real data) --------------------------------------------------------
	{
		const draft = await api.must('POST', '/api/v1/proposals', {
			workspace_id: ws,
			title: `acc draft ${stamp}`,
			proposal_type: 'feature',
			content: `Acceptance draft ${stamp}: a draft proposal that exists only to be deleted through the interface.`,
			domains: ['technical']
		});
		const page = await openPage(`/proposals/${draft.id}`);
		await driveConfirm('delete draft proposal', page, {
			open: async () => {
				await page
					.locator('main button', { hasText: /delete draft/i })
					.first()
					.click();
			},
			verb: 'delete',
			object: /draft|proposal/i,
			exists: async () => (await api.call('GET', `/api/v1/proposals/${draft.id}`)).code === 0
		});
		await closePage(page);
	}

	// --- forms: child record ---------------------------------------------------------------
	{
		const form = await formsFixture(api);
		const lineTitle = `Acc Line ${stamp}`;
		const line = await api.must('POST', `/api/v1/forms/${form.lines}/records`, {
			title: lineTitle,
			values: { title: lineTitle, order_ref: form.order }
		});
		await api.must('POST', `/api/v1/form-records/${form.order}/links`, {
			target_type: 'form_record',
			target_id: line.id,
			relation_key: 'order_lines',
			relation_type: 'parent_child'
		});
		const page = await openPage(
			`/workspace/${ws}/projects/${project}/forms?form=${form.orders}&record=${form.order}`
		);
		await driveConfirm('delete forms child record', page, {
			open: async () => {
				const trash = page.locator('button[aria-label="Delete child record"]').first();
				if (!(await trash.isVisible().catch(() => false))) {
					await page.locator('[data-record-panel-more]').click();
					await page.locator('[data-record-panel-more-details]').click();
				}
				await trash.click();
			},
			verb: 'delete',
			object: new RegExp(lineTitle),
			exists: async () => {
				const list = items(
					(await api.call('GET', `/api/v1/form-records/${form.order}/children`)).data
				);
				return list.some((c) => c.record?.id === line.id);
			}
		});
		await closePage(page);
	}
}

/** Whether the sprint can still be read by id. */
async function sprintExists(api, _project, id) {
	return (await api.call('GET', `/api/v1/sprints/${id}`)).code === 0;
}

/** Two forms (orders, lines) linked parent-child through the relation field `order_ref`, and one order. */
async function formsFixture(api) {
	if (world.forms) return world.forms;
	const project = world.project;
	const existing = items(await api.must('GET', `/api/v1/projects/${project}/forms`));
	let orders = existing.find((f) => f.key === 'orders');
	let lines = existing.find((f) => f.key === 'lines');
	if (!orders)
		orders = await api.must('POST', `/api/v1/projects/${project}/forms`, {
			key: 'orders',
			name: 'Orders',
			schema: { fields: [{ key: 'title', label: 'Title', type: 'text' }] }
		});
	if (!lines)
		lines = await api.must('POST', `/api/v1/projects/${project}/forms`, {
			key: 'lines',
			name: 'Lines',
			schema: {
				fields: [
					{ key: 'title', label: 'Title', type: 'text' },
					{
						key: 'order_ref',
						label: 'Order',
						type: 'relation',
						relation: {
							target_form_key: 'orders',
							relation_type: 'parent_child',
							relation_key: 'order_lines',
							display_field: 'title'
						}
					}
				]
			}
		});
	const records = items(await api.must('GET', `/api/v1/forms/${orders.id}/records`));
	let order = records[0];
	if (!order)
		order = await api.must('POST', `/api/v1/forms/${orders.id}/records`, {
			title: 'Order one',
			values: { title: 'Order one' }
		});
	world.forms = { orders: orders.id, lines: lines.id, order: order.id };
	return world.forms;
}

// ---------------------------------------------------------------------------------------------
// Section 2: Flow with the feature on
// ---------------------------------------------------------------------------------------------

const comboName = (theme, width) => `${theme} ${width}`;

async function sectionFlow(api) {
	const ws = world.ws;
	await check('Flow is switched on for the workspace (API)', async () => {
		const flags = await api.must('GET', `/api/v1/workspaces/${ws}/features/flow`);
		expect(flags.flow_enabled === true, `flow_enabled=${flags.flow_enabled}`);
		const capabilities = await api.must('GET', '/api/v2/flow/capabilities');
		return `flow_enabled=true block_transfers_supported=${capabilities.block_transfers_supported} block_shares_supported=${capabilities.block_shares_supported}`;
	});

	// --- create a page in the UI and type into it ------------------------------------------
	let pageId = null;
	let publicPath = null;
	{
		const page = await openPage(`/workspace/${ws}/flow`);
		await check('Flow: empty navigator offers to create the first page', async () => {
			const empty = await page.locator('button', { hasText: /create your first page/i }).count();
			const tree = await page.locator('[data-testid="flow-nav-new"]').count();
			expect(empty + tree > 0, 'neither "Create your first page" nor the New control is present');
			return `create-first=${empty} new-control=${tree}`;
		});
		await check('Flow: create a page from the navigator', async () => {
			if (await page.locator('button', { hasText: /create your first page/i }).count())
				await page
					.locator('button', { hasText: /create your first page/i })
					.first()
					.click();
			else {
				await page.click('[data-testid="flow-nav-new"]');
				await page
					.locator('[role="menuitem"]', { hasText: /new page/i })
					.first()
					.click();
			}
			await page.waitForURL(/\/flow\/[0-9a-f-]{36}/, { timeout: 15_000 });
			pageId = page.url().match(/\/flow\/([0-9a-f-]{36})/)?.[1] ?? null;
			await page.waitForSelector('[data-testid="flow-page-editor"]');
			return `page ${pageId}`;
		});
		await check(
			'Flow: typing in the editor shows the text and the sync indicator reaches "saved"',
			async () => {
				await page.click('[data-testid="flow-add-content"]');
				await page
					.locator('button', { hasText: /^Text$/ })
					.first()
					.click();
				await page.waitForTimeout(400);
				await page.keyboard.type('Acceptance text typed in the Flow editor');
				await page.waitForFunction(
					() =>
						document
							.querySelector('[data-testid="flow-page-editor"]')
							?.innerText.includes('Acceptance text typed'),
					null,
					{ timeout: 8000 }
				);
				await page.waitForFunction(
					() =>
						document
							.querySelector('[data-testid="flow-sync-indicator"]')
							?.getAttribute('data-state') === 'saved',
					null,
					{ timeout: 15_000 }
				);
				const state = await page.getAttribute('[data-testid="flow-sync-indicator"]', 'data-state');
				return `text visible, sync=${state}`;
			}
		);
		await check('Flow: the page is stored on the server', async () => {
			await page.waitForTimeout(1500);
			const object = await api.call('GET', `/api/v2/flow/objects/${pageId}`);
			expect(object.code === 0, `object read -> ${object.code} ${object.message}`);
			return `object read code 0`;
		});
		await closePage(page);
	}
	if (!pageId) return;

	// --- the public link (created once in the share panel) ----------------------------------
	{
		const page = await openPage(`/workspace/${ws}/flow/${pageId}`);
		await check('Flow: create a public link in the share panel', async () => {
			await page.click('[data-testid="flow-share-open"]');
			await page.waitForSelector('[data-testid="flow-public-links"]');
			await page
				.locator('[data-testid="flow-public-links"] button', { hasText: /create/i })
				.last()
				.click();
			const input = page.locator('[data-testid="flow-public-links"] input[readonly]');
			await input.waitFor({ timeout: 10_000 });
			const value = await input.inputValue();
			const token = value.split('/s/')[1];
			expect(token, 'no /s/<token> link was shown');
			secrets.add(token);
			publicPath = `/s/${token}`;
			return `link created (${value.split('/s/')[0]}/s/***)`;
		});
		await closePage(page);
	}

	// --- every state in light and dark at 1440 and 800 -------------------------------------
	for (const theme of THEMES) {
		for (const width of FLOW_WIDTHS) {
			const combo = comboName(theme, width);

			// navigator tree (at the narrow width it sits in the navigator drawer)
			{
				const label = `Flow navigator tree [${combo}]`;
				const page = await openPage(`/workspace/${ws}/flow/${pageId}`, {
					theme,
					width,
					freeze: true
				});
				await check(`${label}: renders`, async () => {
					if (
						!(await page
							.locator('[role="tree"]')
							.first()
							.isVisible()
							.catch(() => false))
					) {
						await page.click('[data-testid="flow-nav-toggle"]');
						await page.waitForTimeout(400);
					}
					await page.waitForSelector('[role="tree"]', { state: 'visible', timeout: 8000 });
					const rows = await page.locator('[role="tree"] [role="treeitem"]').count();
					expect(rows >= 1, `the tree has ${rows} items`);
					return `treeitems=${rows}`;
				});
				await contentChecks(page, label);
				await ringChecks(page, label, { start: '[role="tree"]' });
				await closePage(page);
			}

			// page: header and editor, typing
			{
				const label = `Flow page header and editor [${combo}]`;
				const page = await openPage(`/workspace/${ws}/flow/${pageId}`, {
					theme,
					width,
					freeze: true
				});
				await check(`${label}: renders`, async () => {
					await page.waitForSelector('[data-testid="flow-page-header"]');
					await page.waitForSelector('[data-testid="flow-page-editor"]');
					const text = await page.evaluate(
						() => document.querySelector('[data-testid="flow-page-editor"]')?.innerText || ''
					);
					expect(text.includes('Acceptance text typed'), 'the saved text is not shown');
					const title = await page.locator('textarea[aria-label="Page title"]').count();
					expect(title === 1, 'the page title field is missing');
					return 'header, title and the saved text are shown';
				});
				await contentChecks(page, label);
				await check(`Flow typing and sync indicator [${combo}]`, async () => {
					const mark = page.__problems.length;
					await page
						.locator('[data-testid="flow-page-editor"] p[data-block-id]', {
							hasText: /Acceptance text/
						})
						.first()
						.click();
					await page.keyboard.press('End');
					const word = ` w${theme[0]}${width}`;
					await page.keyboard.type(word);
					await page.waitForFunction(
						(w) =>
							document.querySelector('[data-testid="flow-page-editor"]')?.innerText.includes(w),
						word.trim(),
						{ timeout: 8000 }
					);
					await page.waitForFunction(
						() =>
							document
								.querySelector('[data-testid="flow-sync-indicator"]')
								?.getAttribute('data-state') === 'saved',
						null,
						{ timeout: 15_000 }
					);
					const problems = problemsSince(page, mark);
					expect(problems.length === 0, `page problems: ${problems.join(' ; ')}`);
					return `typed "${word.trim()}", sync=saved`;
				});
				await closePage(page);
				const again = await openPage(`/workspace/${ws}/flow/${pageId}`, {
					theme,
					width,
					freeze: true
				});
				await ringChecks(again, label, { start: '#sv-main' });
				await closePage(again);
			}

			// context panel
			{
				const label = `Flow context panel [${combo}]`;
				const page = await openPage(`/workspace/${ws}/flow/${pageId}`, {
					theme,
					width,
					freeze: true
				});
				await check(`${label}: renders`, async () => {
					await page.click('[data-testid="flow-more"]');
					await page.click('[data-testid="flow-open-info"]');
					await page.waitForSelector('[data-testid="flow-tool-panel"]', { state: 'visible' });
					await page.waitForTimeout(500);
					return 'the Info panel is open';
				});
				await contentChecks(page, label);
				await ringChecks(page, label, { start: '[data-testid="flow-tool-panel"]' });
				await closePage(page);
			}

			// share panel
			{
				const label = `Flow share panel [${combo}]`;
				const page = await openPage(`/workspace/${ws}/flow/${pageId}`, {
					theme,
					width,
					freeze: true
				});
				await check(`${label}: renders`, async () => {
					await page.click('[data-testid="flow-share-open"]');
					await page.waitForSelector('[data-testid="flow-share-panel"]', { state: 'visible' });
					await page.waitForTimeout(500);
					return 'the share panel is open';
				});
				await contentChecks(page, label);
				await ringChecks(page, label, { start: '[data-testid="flow-share-panel"]' });
				await closePage(page);
			}

			// block transfer dialog (open only)
			{
				const label = `Flow block transfer dialog [${combo}]`;
				const page = await openPage(`/workspace/${ws}/flow/${pageId}`, {
					theme,
					width,
					freeze: true
				});
				await check(`${label}: renders`, async () => {
					await page.waitForSelector('[data-testid="flow-page-editor"]');
					const block = page
						.locator('[data-testid="flow-page-editor"] p[data-block-id]', {
							hasText: /Acceptance text/
						})
						.first();
					await block.hover();
					await page.waitForSelector('[data-flow-block-controls]', { timeout: 5000 });
					await page.locator('[data-flow-block-controls] button[aria-haspopup="menu"]').click();
					await page.locator('[role="menuitem"]', { hasText: /copy to page/i }).click();
					await page.waitForSelector('[data-testid="flow-block-transfer"]', {
						state: 'visible',
						timeout: 8000
					});
					await page.waitForTimeout(500);
					return 'the block transfer dialog is open';
				});
				await contentChecks(page, label);
				await ringChecks(page, label, { start: '[data-testid="flow-block-transfer"]' });
				await closePage(page);
			}

			// public share page (no session)
			{
				const label = `Flow public share page [${combo}]`;
				const page = await openPage(publicPath, { theme, width, auth: false, freeze: true });
				await check(`${label}: renders`, async () => {
					await page.waitForSelector('[data-testid="flow-public-page"]', { timeout: 10_000 });
					const text = await page.evaluate(() => document.body.innerText);
					expect(/Acceptance text typed/.test(text), 'the shared page text is not shown');
					return 'the shared text is shown without a session';
				});
				await contentChecks(page, label);
				await ringChecks(page, label, { start: null });
				await closePage(page);
			}
		}
	}

	// --- collection view -------------------------------------------------------------------
	{
		const page = await openPage(`/workspace/${ws}/flow`);
		await check('Flow: collection view', async () => {
			await page.click('[data-testid="flow-nav-new"]');
			const entries = await page.locator('[role="menu"] [role="menuitem"]').allTextContents();
			const offered = entries.some((entry) => /collection/i.test(entry));
			if (!offered)
				return {
					skip: `the UI offers no collection: the New menu holds only ${JSON.stringify(entries.map((e) => e.trim()))}`
				};
			return `New menu offers a collection: ${JSON.stringify(entries.map((e) => e.trim()))}`;
		});
		await closePage(page);
	}
}

// ---------------------------------------------------------------------------------------------
// Section 3: plugin surfaces with an app enabled
// ---------------------------------------------------------------------------------------------

async function pluginPackage() {
	const fixture = process.env.P3A_PLUGIN_FIXTURE || DEFAULT_PLUGIN_FIXTURE;
	if (!existsSync(fixture)) return null;
	const { createHash } = await import('node:crypto');
	const html = await readFile(fixture);
	const contribution = (id, kind, slot, label, view, route, formKey) => ({
		id,
		kind,
		slot,
		label,
		view,
		route,
		form_key: formKey
	});
	const form = 'order_line';
	const manifest = {
		schema_version: 'openpr.plugin.v3',
		key: 'order_board',
		name: 'Order board',
		version: '1.0.0',
		publisher: 'P3 acceptance',
		host_protocol: { min: 3, max: 3 },
		profile: 'ui',
		bridge_version: 2,
		entry: {
			path: 'panel.html',
			size: html.length,
			sha256: createHash('sha256').update(html).digest('hex')
		},
		wasm: null,
		contributions: [
			contribution('board', 'view', 'project.view', 'Order board', 'board', 'board', null),
			contribution(
				'record_summary',
				'panel',
				'form.record.detail',
				'Order summary',
				'summary',
				null,
				form
			)
		],
		reads: [{ id: 'orders', form_key: form, fields: ['quantity', 'unit_price', 'amount'] }],
		capabilities: []
	};
	return { manifest, web_base64: html.toString('base64') };
}

async function pluginFixture(api) {
	if (world.plugin) return world.plugin;
	const project = world.project;
	const existing = items(await api.must('GET', `/api/v1/projects/${project}/forms`));
	let form = existing.find((f) => f.key === 'order_line');
	if (!form)
		form = await api.must('POST', `/api/v1/projects/${project}/forms`, {
			key: 'order_line',
			name: 'Order line',
			schema: {
				fields: [
					{ key: 'quantity', label: 'Quantity', type: 'number' },
					{ key: 'unit_price', label: 'Unit price', type: 'number' },
					{ key: 'note', label: 'Note', type: 'text' },
					{ key: 'amount', label: 'Amount', type: 'number' }
				]
			}
		});
	let record = items(await api.must('GET', `/api/v1/forms/${form.id}/records`))[0];
	if (!record)
		record = await api.must('POST', `/api/v1/forms/${form.id}/records`, {
			title: 'Acceptance order line',
			values: { quantity: '2', unit_price: '12.50', amount: '0', note: 'seed' }
		});
	world.plugin = { form: form.id, record: record.id };
	return world.plugin;
}

async function sectionPlugins(api) {
	const project = world.project;
	const ws = world.ws;
	let ready = false;
	await check('plugin: install and enable a test app in the project', async () => {
		const upload = await pluginPackage();
		if (!upload)
			return {
				skip: `fixture panel not found (${process.env.P3A_PLUGIN_FIXTURE || DEFAULT_PLUGIN_FIXTURE})`
			};
		const fixture = await pluginFixture(api);
		const base = `/api/v1/projects/${project}/plugin-packages`;
		const catalog = await api.must('GET', base);
		let binding = (catalog.bindings ?? []).find((b) => b.key === 'order_board');
		if (!binding || !binding.package_id) {
			const installed = await api.must('POST', base, upload);
			const generation = binding?.generation ?? 0;
			await api.must('POST', `${base}/order_board/binding`, {
				package_id: installed.id,
				expected_generation: generation,
				grants: ['ui.display', 'read:orders'],
				request_id: uuid()
			});
		}
		const view = items(
			await api.must('GET', `/api/v1/projects/${project}/plugin-contributions?surface=project`)
		);
		expect(view.length >= 1, 'discovery lists no project view after the bind');
		ready = true;
		return `order_board bound, ${view.length} project view(s), record ${fixture.record}`;
	});
	if (!ready) return;
	const fixture = world.plugin;

	for (const theme of THEMES) {
		for (const width of FLOW_WIDTHS) {
			const combo = comboName(theme, width);
			const frameOf = (page) => {
				const host = page.frameLocator('iframe[src="/plugin-host/frame.html"]').first();
				return host.frameLocator('iframe[title="Plugin content"]');
			};

			{
				const label = `plugin: project Apps page [${combo}]`;
				const page = await openPage(`/workspace/${ws}/projects/${project}/apps`, {
					theme,
					width,
					freeze: true
				});
				await check(`${label}: lists the app`, async () => {
					const link = page.locator('main a[data-plugin-contribution="board"]');
					expect(await link.count(), 'no "Order board" entry on the Apps page');
					return 'entry "Order board" shown';
				});
				await contentChecks(page, label);
				await ringChecks(page, label, { start: '#sv-main' });
				await closePage(page);
			}

			{
				const label = `plugin: app frame [${combo}]`;
				const page = await openPage(`/workspace/${ws}/projects/${project}/apps`, {
					theme,
					width,
					freeze: true
				});
				await check(`${label}: opening the app renders the plugin frame`, async () => {
					await page.locator('main a[data-plugin-contribution="board"]').first().click();
					await page.waitForURL(/\/apps\/order_board\/board/);
					const inner = frameOf(page);
					await inner.locator('#context:not(:empty)').waitFor({ timeout: 15_000 });
					const heading = await inner.locator('#heading').textContent();
					await page.waitForTimeout(500);
					return `frame heading "${heading}"`;
				});
				await contentChecks(page, label);
				await ringChecks(page, label, { start: '#sv-main' });
				await closePage(page);
			}

			{
				const label = `plugin: record side panel [${combo}]`;
				const page = await openPage(
					`/workspace/${ws}/projects/${project}/forms?form=${fixture.form}&record=${fixture.record}`,
					{ theme, width, freeze: true }
				);
				await check(`${label}: shows the plugin section`, async () => {
					const panel = page.locator('[data-record-panel]').first();
					await panel.waitFor({ timeout: 10_000 });
					const apps = page.locator('[data-record-panel-apps]');
					expect(await apps.count(), 'the panel has no Apps section (data-record-panel-apps)');
					const open = await apps.evaluate((element) => element.hasAttribute('open'));
					if (!open) await apps.locator('summary').first().click();
					const section = page.locator(
						'[data-record-panel] [data-plugin-slot="form.record.detail"] [data-plugin-contribution="record_summary"]'
					);
					await section.first().waitFor({ timeout: 10_000 });
					return `mode=${await panel.getAttribute('data-record-panel-mode')}, plugin section "record_summary" shown`;
				});
				await contentChecks(page, label);
				await ringChecks(page, label, { start: '[data-record-panel]' });
				await closePage(page);
			}

			{
				const label = `plugin: record page [${combo}]`;
				const page = await openPage(
					`/workspace/${ws}/projects/${project}/forms/records/${fixture.record}`,
					{
						theme,
						width,
						freeze: true
					}
				);
				await check(`${label}: the plugin section opens its frame`, async () => {
					const section = page.locator(
						'[data-plugin-slot="form.record.detail"] [data-plugin-contribution="record_summary"]'
					);
					await section.first().waitFor({ timeout: 10_000 });
					await section.first().click();
					await frameOf(page).locator('#context:not(:empty)').waitFor({ timeout: 15_000 });
					await page.waitForTimeout(400);
					return 'the frame is connected';
				});
				await contentChecks(page, label);
				await ringChecks(page, label, { start: '#sv-main' });
				await closePage(page);
			}
		}
	}
}

// ---------------------------------------------------------------------------------------------
// Section 4: inline errors instead of toasts
// ---------------------------------------------------------------------------------------------

async function inlineErrorOf(page) {
	return page.evaluate(() => {
		const main = document.querySelector('main');
		const alerts = main ? [...main.querySelectorAll('[role="alert"]')] : [];
		const first = alerts[0];
		return {
			alerts: alerts.length,
			text: first ? first.textContent.trim().replace(/\s+/g, ' ').slice(0, 90) : '',
			retry: first
				? !![...first.querySelectorAll('button')].find((b) => /retry/i.test(b.textContent))
				: false,
			retryAnywhere: !![...(main?.querySelectorAll('button') ?? [])].find((b) =>
				/^\s*retry\s*$/i.test(b.textContent)
			),
			toasts: [...document.querySelectorAll('[data-toast]')].map(
				(e) => `${e.getAttribute('data-tone')}:${e.textContent.trim().slice(0, 50)}`
			)
		};
	});
}

async function sectionErrors(api) {
	const ws = world.ws;
	const project = world.project;
	const issue = await api.must('POST', `/api/v1/projects/${project}/issues`, {
		title: 'acc issue to be deleted',
		description: 'acceptance'
	});
	await api.must('DELETE', `/api/v1/issues/${issue.id}`);
	const cases = [
		{ name: 'deleted issue', path: `/workspace/${ws}/projects/${project}/issues/${issue.id}` },
		{ name: 'non-existent project', path: `/workspace/${ws}/projects/${NIL_ID}` },
		{ name: 'non-existent workspace members', path: `/workspace/${NIL_ID}/members` },
		{ name: 'non-existent user trust profile', path: `/users/${NIL_ID}/trust` },
		{
			name: 'non-existent audit report',
			path: `/workspace/${ws}/projects/${project}/audit-reports/${NIL_ID}`,
			extra: true
		},
		{
			name: 'non-existent form record',
			path: `/workspace/${ws}/projects/${project}/forms/records/${NIL_ID}`,
			extra: true
		},
		{ name: 'non-existent proposal', path: `/proposals/${NIL_ID}`, extra: true },
		{
			name: 'workspace home of a non-existent workspace',
			path: `/workspace/${NIL_ID}`,
			retry: true
		},
		{
			name: 'cycles of a non-existent project',
			path: `/workspace/${ws}/projects/${NIL_ID}/cycles`,
			retry: true
		}
	];
	for (const entry of cases) {
		const page = await openPage(entry.path);
		await check(`inline error: ${entry.name}`, async () => {
			await page.waitForTimeout(1200);
			const state = await inlineErrorOf(page);
			expect(
				state.alerts > 0,
				`no role="alert" element inside main; Retry button present=${state.retryAnywhere}; url=${new URL(page.url()).pathname.replace(/[0-9a-f-]{36}/g, '<id>')}; main shows: ${(await page.locator('main').innerText()).replace(/\s+/g, ' ').slice(-110)}`
			);
			const errorToasts = state.toasts.filter((t) => /^(warning|danger):/.test(t));
			expect(errorToasts.length === 0, `error toast(s): ${errorToasts.join(' ; ')}`);
			if (entry.retry)
				expect(state.retry || state.retryAnywhere, `no Retry action (alert: "${state.text}")`);
			return `alert in main: "${state.text}" retry=${state.retry || state.retryAnywhere} toasts=${state.toasts.length}`;
		});
		await closePage(page);
	}

	// A form submit the server rejects: a webhook URL that points at a blocked (loopback) address.
	{
		const page = await openPage(`/workspace/${ws}/webhooks`);
		await check('inline error: webhook form rejected by the server', async () => {
			await page
				.locator('main button', { hasText: /create webhook/i })
				.first()
				.click();
			const dialog = page.locator('[data-ui-layer="dialog"]').last();
			await dialog.waitFor();
			const url = dialog
				.locator('input[type="url"], input[name="url"], input[placeholder*="http"]')
				.first();
			await url.fill('http://127.0.0.1:9/acceptance');
			const events = dialog.locator('input[type="checkbox"]');
			if (await events.count())
				await events
					.first()
					.check()
					.catch(() => {});
			await dialog
				.locator('button', { hasText: /^(create|save|add)/i })
				.last()
				.click();
			await page.waitForTimeout(1500);
			const state = await inlineErrorOf(page);
			const dialogError = await dialog
				.locator('[role="alert"]')
				.count()
				.catch(() => 0);
			expect(
				state.alerts > 0 || dialogError > 0,
				'the rejected submit shows no role="alert" element'
			);
			const errorToasts = state.toasts.filter((t) => /^(warning|danger):/.test(t));
			expect(errorToasts.length === 0, `error toast(s): ${errorToasts.join(' ; ')}`);
			expect(state.alerts > 0, `the alert is not inside main (dialog alerts: ${dialogError})`);
			return `alert in main: "${state.text}" toasts=${state.toasts.length}`;
		});
		await closePage(page);
	}
}

// ---------------------------------------------------------------------------------------------
// Section 5: narrow layout
// ---------------------------------------------------------------------------------------------

async function narrowPages(api) {
	const fixture = await pluginFixtureOrForms(api);
	const ws = world.ws;
	const project = world.project;
	return [
		{ name: 'home', path: '/' },
		{ name: 'workspace members', path: `/workspace/${ws}/members` },
		{ name: 'project issues', path: `/workspace/${ws}/projects/${project}/issues` },
		{
			name: 'forms',
			path: `/workspace/${ws}/projects/${project}/forms?form=${fixture.orders}`,
			sheetPath: `/workspace/${ws}/projects/${project}/forms?form=${fixture.orders}&record=${fixture.order}`
		},
		{ name: 'proposal templates', path: '/proposal-templates' },
		{ name: 'settings', path: '/settings' },
		{ name: 'flow page', path: `/workspace/${ws}/flow` }
	];
}

async function pluginFixtureOrForms(api) {
	return formsFixture(api);
}

async function sectionNarrow(api) {
	const pages = await narrowPages(api);
	for (const width of [800, 390]) {
		for (const entry of pages) {
			const page = await openPage(entry.path, { width, freeze: true });
			const label = `${entry.name} [${width}]`;
			await check(`narrow: top bar is 64px high, no horizontal overflow -- ${label}`, async () => {
				const bar = await page.evaluate(() => {
					const element = document.querySelector('[data-shell="rail"][data-layout="topbar"]');
					return element ? Math.round(element.getBoundingClientRect().height * 100) / 100 : null;
				});
				expect(bar !== null, 'no top bar (data-layout="topbar")');
				expect(bar === 64, `top bar is ${bar}px high`);
				const overflow = await page.evaluate(() => ({
					scrollWidth: document.documentElement.scrollWidth,
					clientWidth: document.documentElement.clientWidth
				}));
				expect(
					overflow.scrollWidth <= overflow.clientWidth,
					`scrollWidth ${overflow.scrollWidth} > clientWidth ${overflow.clientWidth}`
				);
				return `topbar=${bar}px scrollWidth=${overflow.scrollWidth} clientWidth=${overflow.clientWidth}`;
			});
			await check(`narrow: list/content switch -- ${label}`, async () => {
				const view = () =>
					page.evaluate(() => ({
						column: [
							...document.querySelectorAll('[data-shell="column"], [data-shell="settings-column"]')
						].some((e) => e.getBoundingClientRect().width > 0),
						content:
							!!document.querySelector('[data-shell="content"]') &&
							document.querySelector('[data-shell="content"]').getBoundingClientRect().width > 0,
						back:
							!!document.querySelector('[data-shell="back-to-list"]') &&
							document.querySelector('[data-shell="back-to-list"]').getBoundingClientRect().width >
								0
					}));
				let state = await view();
				if (!state.content) {
					// The list is showing: pick the first row to reach the content.
					const row = page.locator('[data-shell-list] a, [data-shell-list] button').first();
					if (await row.count()) await row.click().catch(() => {});
					await page.waitForTimeout(500);
					state = await view();
				}
				expect(
					state.content && state.back,
					`content=${state.content} back-to-list row=${state.back}`
				);
				await page.locator('[data-shell="back-to-list"]').first().click();
				await page.waitForTimeout(400);
				const after = await view();
				expect(
					after.column && !after.content,
					`after the back row: column=${after.column} content=${after.content}`
				);
				const overflow = await page.evaluate(
					() => document.documentElement.scrollWidth <= document.documentElement.clientWidth
				);
				expect(overflow, 'horizontal overflow in the list view');
				return `content->list works (column shown, content hidden)`;
			});
			if (entry.sheetPath) {
				await closePage(page);
				const sheetPage = await openPage(entry.sheetPath, { width, freeze: true });
				await check(
					`narrow: record panel is a sheet where the design says so -- ${label}`,
					async () => {
						const panel = sheetPage.locator('[data-record-panel]').first();
						await panel.waitFor({ timeout: 10_000 });
						const info = await panel.evaluate((element) => {
							const box = element.getBoundingClientRect();
							return {
								mode: element.getAttribute('data-record-panel-mode'),
								left: Math.round(box.left),
								right: Math.round(box.right),
								width: Math.round(box.width),
								viewport: innerWidth
							};
						});
						if (width < 768)
							expect(
								info.mode === 'sheet' && info.width === info.viewport,
								`mode=${info.mode} width=${info.width} viewport=${info.viewport}`
							);
						else
							expect(
								info.right <= info.viewport + 1 && info.left >= -1,
								`panel spans ${info.left}..${info.right} of ${info.viewport}`
							);
						const overflow = await sheetPage.evaluate(
							() => document.documentElement.scrollWidth <= document.documentElement.clientWidth
						);
						expect(overflow, 'horizontal overflow with the panel open');
						return `mode=${info.mode} width=${info.width}/${info.viewport}`;
					}
				);
				await closePage(sheetPage);
			} else await closePage(page);
		}

		// dialogs and the confirm dialog at this width
		{
			const stamp = Date.now().toString(36);
			const name = `acc-narrow-${stamp}`;
			const sprint = await api.must('POST', `/api/v1/projects/${world.project}/sprints`, {
				name,
				start_date: '2026-11-20',
				end_date: '2026-12-04'
			});
			const page = await openPage(`/workspace/${world.ws}/projects/${world.project}/cycles`, {
				width,
				freeze: true
			});
			if (await page.locator('[data-shell="back-to-list"]').count()) {
				const visible = await page
					.locator('[data-shell="content"]')
					.isVisible()
					.catch(() => false);
				if (!visible)
					await page
						.locator('[data-shell="back-to-list"]')
						.click()
						.catch(() => {});
			}
			await check(
				`narrow: confirm dialog fits and its buttons are not clipped [${width}]`,
				async () => {
					await clickRowButton(page, name, /^delete$/);
					await page.waitForSelector('[role="alertdialog"]');
					const state = await alertState(page);
					const frame = await page.evaluate(() => {
						const box = document.querySelector('[role="alertdialog"]').getBoundingClientRect();
						return {
							left: Math.round(box.left),
							right: Math.round(box.right),
							viewport: innerWidth
						};
					});
					expect(
						state.clipped.every((c) => !c),
						`button clipped: ${JSON.stringify(state.clipped)}`
					);
					expect(
						frame.left >= 0 && frame.right <= frame.viewport,
						`dialog spans ${frame.left}..${frame.right} of ${frame.viewport}`
					);
					const overflow = await page.evaluate(
						() => document.documentElement.scrollWidth <= document.documentElement.clientWidth
					);
					expect(overflow, 'horizontal overflow with the dialog open');
					await page.locator('[role="alertdialog"] footer button').first().click();
					return `dialog ${frame.left}..${frame.right}/${frame.viewport}, buttons inside`;
				}
			);
			await check(
				`narrow: the hand-made Create sprint modal fits the viewport [${width}]`,
				async () => {
					await page
						.locator('main button', { hasText: /create sprint/i })
						.first()
						.click();
					await page.waitForTimeout(500);
					const box = await page.evaluate(() => {
						const heading = [...document.querySelectorAll('h2')].find((h) =>
							/create sprint/i.test(h.textContent)
						);
						if (!heading) return null;
						let node = heading;
						while (
							node &&
							!(node.parentElement && getComputedStyle(node.parentElement).position === 'fixed')
						)
							node = node.parentElement;
						const element = node || heading;
						const rect = element.getBoundingClientRect();
						const frame = element.closest('[role="dialog"], [aria-modal="true"]');
						return {
							left: Math.round(rect.left),
							right: Math.round(rect.right),
							width: Math.round(rect.width),
							viewport: innerWidth,
							role: frame ? frame.getAttribute('role') : null
						};
					});
					expect(box, 'the Create Sprint modal did not open');
					expect(
						box.left >= 0 && box.right <= box.viewport,
						`modal spans ${box.left}..${box.right} of ${box.viewport}`
					);
					const overflow = await page.evaluate(
						() => document.documentElement.scrollWidth <= document.documentElement.clientWidth
					);
					expect(overflow, 'horizontal overflow with the modal open');
					return `modal width ${box.width}/${box.viewport}`;
				}
			);
			await check(
				`narrow: the Create sprint modal is exposed as a dialog (role=dialog, aria-modal) [${width}]`,
				async () => {
					const info = await page.evaluate(() => {
						const heading = [...document.querySelectorAll('h2')].find((h) =>
							/create sprint/i.test(h.textContent)
						);
						const frame = heading?.closest('[role="dialog"], [role="alertdialog"]');
						return {
							found: !!heading,
							role: frame ? frame.getAttribute('role') : null,
							modal: frame ? frame.getAttribute('aria-modal') : null
						};
					});
					expect(info.found, 'the Create Sprint modal is not open');
					expect(
						info.role !== null && info.modal === 'true',
						`role=${info.role} aria-modal=${info.modal}`
					);
					return `role=${info.role} aria-modal=${info.modal}`;
				}
			);
			await closePage(page);
			await api.call('DELETE', `/api/v1/sprints/${sprint.id}`);
			const hooks = await openPage(`/workspace/${world.ws}/webhooks`, { width, freeze: true });
			await check(`narrow: the Create webhook dialog fits the viewport [${width}]`, async () => {
				await hooks
					.locator('main button', { hasText: /create webhook/i })
					.first()
					.click();
				const dialog = hooks.locator('[data-ui-layer="dialog"] [role="dialog"]').last();
				await dialog.waitFor({ timeout: 5000 });
				const box = await dialog.evaluate((element) => {
					const rect = element.getBoundingClientRect();
					return {
						left: Math.round(rect.left),
						right: Math.round(rect.right),
						width: Math.round(rect.width),
						viewport: innerWidth
					};
				});
				expect(
					box.left >= 0 && box.right <= box.viewport,
					`dialog spans ${box.left}..${box.right} of ${box.viewport}`
				);
				const overflow = await hooks.evaluate(
					() => document.documentElement.scrollWidth <= document.documentElement.clientWidth
				);
				expect(overflow, 'horizontal overflow with the dialog open');
				return `dialog ${box.left}..${box.right}/${box.viewport} (width ${box.width})`;
			});
			await closePage(hooks);
		}
	}
}

// ---------------------------------------------------------------------------------------------
// Section 6: keyboard-only pass
// ---------------------------------------------------------------------------------------------

const INTERACTIVE =
	'a[href], button, input:not([type="hidden"]), select, textarea, summary, [tabindex], [contenteditable="true"]';

async function enumerateControls(page) {
	return page.evaluate((selector) => {
		const out = [];
		const radios = new Set();
		for (const element of document.querySelectorAll(selector)) {
			if (element.disabled || element.tabIndex < 0) continue;
			if (element.closest('[inert], [hidden], [aria-hidden="true"]')) continue;
			const details = element.closest('details:not([open])');
			if (details && element.tagName !== 'SUMMARY' && !(element.parentElement === details))
				continue;
			const box = element.getBoundingClientRect();
			const style = getComputedStyle(element);
			if (style.visibility === 'hidden' || style.display === 'none') continue;
			if (box.width < 1 || box.height < 1) continue;
			if (element.type === 'radio') {
				if (radios.has(element.name)) continue;
				radios.add(element.name);
			}
			out.push({
				id: out.length,
				name: (
					element.getAttribute('aria-label') ||
					element.textContent ||
					element.getAttribute('name') ||
					element.tagName
				)
					.trim()
					.replace(/\s+/g, ' ')
					.slice(0, 40)
			});
			element.setAttribute('data-acc-kb', String(out.length - 1));
		}
		return out;
	}, INTERACTIVE);
}

async function sectionKeyboard(api) {
	const ws = world.ws;
	const project = world.project;
	const pages = [
		{ name: 'workspace members', path: `/workspace/${ws}/members` },
		{ name: 'workspace webhooks', path: `/workspace/${ws}/webhooks` },
		{ name: 'project issues', path: `/workspace/${ws}/projects/${project}/issues` },
		{ name: 'settings', path: '/settings' }
	];
	for (const entry of pages) {
		const page = await openPage(entry.path, { freeze: true });
		await check(
			`keyboard: Tab reaches every control, in document order, with a ring -- ${entry.name}`,
			async () => {
				const controls = await enumerateControls(page);
				await page.evaluate(() => {
					document.activeElement?.blur();
					window.scrollTo(0, 0);
				});
				const visited = [];
				const trace = [];
				let stuck = 0;
				let previous = null;
				let outOfOrder = 0;
				let ringMissing = [];
				for (let i = 0; i < controls.length + 40; i += 1) {
					await page.keyboard.press('Tab');
					const info = await page.evaluate(() => {
						const el = document.activeElement;
						if (!el || el === document.body) return { body: true };
						const style = getComputedStyle(el);
						const kb = el.getAttribute('data-acc-kb');
						let boxed = false;
						for (
							let node = el.parentElement, depth = 0;
							node && depth < 3;
							node = node.parentElement, depth += 1
						) {
							if (!node.matches(':focus-within')) continue;
							const outer = getComputedStyle(node);
							if ((parseFloat(outer.outlineWidth) || 0) >= 2 && outer.outlineStyle !== 'none')
								boxed = true;
						}
						return {
							boxed,
							kb: kb === null ? null : Number(kb),
							tag: el.tagName.toLowerCase(),
							name: (
								el.getAttribute('aria-label') ||
								(el.labels && el.labels[0] ? el.labels[0].textContent : '') ||
								el.textContent ||
								el.type ||
								''
							)
								.trim()
								.replace(/\s+/g, ' ')
								.slice(0, 30),
							outline: parseFloat(style.outlineWidth) || 0,
							outlineStyle: style.outlineStyle,
							shadow: style.boxShadow !== 'none',
							token: (() => {
								el.setAttribute('data-acc-cur', '1');
								return true;
							})()
						};
					});
					if (info.body) {
						trace.push('<body>');
						break;
					}
					const ring =
						(info.outline >= 2 && info.outlineStyle !== 'none') || info.shadow || info.boxed;
					if (!ring) ringMissing.push(`${info.tag}"${info.name}"`);
					if (info.kb !== null) visited.push(info.kb);
					if (previous !== null && info.kb !== null) {
						const order = await page.evaluate(
							([a, b]) => {
								const first = document.querySelector(`[data-acc-kb="${a}"]`);
								const second = document.querySelector(`[data-acc-kb="${b}"]`);
								if (!first || !second || first === second) return 0;
								return first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING
									? 1
									: -1;
							},
							[previous, info.kb]
						);
						if (order === -1) outOfOrder += 1;
					}
					if (info.kb !== null) previous = info.kb;
					await page.evaluate(() =>
						document
							.querySelectorAll('[data-acc-cur]')
							.forEach((e) => e.removeAttribute('data-acc-cur'))
					);
					trace.push(info.name || info.tag);
					if (visited.length >= 3 && visited.slice(-3).every((v) => v === visited.at(-1))) {
						stuck += 1;
						break;
					}
					if (trace.length === 1)
						expect(/skip/i.test(info.name), `first stop is "${info.name}", not the skip link`);
				}
				const reached = new Set(visited);
				const missed = controls.filter((c) => !reached.has(c.id));
				expect(stuck === 0, `focus trap near "${trace.at(-1)}"`);
				expect(
					missed.length === 0,
					`${missed.length} of ${controls.length} controls not reached: ${missed
						.slice(0, 4)
						.map((m) => m.name)
						.join(', ')}`
				);
				expect(outOfOrder === 0, `${outOfOrder} step(s) went backwards in document order`);
				expect(ringMissing.length === 0, `no 2px ring on ${ringMissing.slice(0, 4).join(', ')}`);
				return `${controls.length} controls, ${reached.size} reached, first=skip link, order ok, rings ok`;
			}
		);
		await check(
			`keyboard: Cmd/Ctrl+K opens the palette and a query navigates -- ${entry.name}`,
			async () => {
				await page.evaluate(() => document.activeElement?.blur());
				await page.keyboard.press('Control+k');
				await page.waitForSelector('[data-palette-input]', { timeout: 5000 });
				await page.keyboard.type('Acceptance');
				await page.waitForTimeout(500);
				const before = page.url();
				await page.waitForSelector('[data-palette-kind]', { timeout: 5000 });
				await page.keyboard.press('Enter');
				await page.waitForTimeout(1000);
				const opened = await page.locator('[data-palette-input]').count();
				expect(opened === 0, 'the palette stays open after Enter');
				expect(page.url() !== before, `url unchanged: ${page.url().replace(ws, 'W')}`);
				return `navigated to ${new URL(page.url()).pathname.replace(ws, 'W').replace(project, 'P')}`;
			}
		);
		await closePage(page);
	}

	// Escape closes menus, popovers and dialogs and returns focus to the opener.
	{
		const page = await openPage(`/workspace/${ws}/webhooks`, { freeze: true });
		const escapeCase = async (name, openerSelector, openedSelector, openAction) => {
			await check(`keyboard: Escape closes ${name} and returns focus to the opener`, async () => {
				const opener = page.locator(openerSelector).first();
				await opener.focus();
				await (openAction ? openAction(opener) : page.keyboard.press('Enter'));
				await page.waitForSelector(openedSelector, { timeout: 5000 });
				await page.keyboard.press('Escape');
				await page
					.waitForSelector(openedSelector, { state: 'detached', timeout: 5000 })
					.catch(async () => {
						const hidden = await page.locator(openedSelector).first().isHidden();
						expect(hidden, `${name} is still open after Escape`);
					});
				await page.waitForTimeout(250);
				const back = await opener.evaluate((element) => document.activeElement === element);
				expect(back, 'focus did not return to the opener');
				return 'closed, focus back on the opener';
			});
		};
		await escapeCase('the account menu', '[data-shell="account"]', '[role="menu"]');
		await escapeCase(
			'a dialog',
			'main button:has-text("Create Webhook")',
			'[data-ui-layer="dialog"]:not([hidden])'
		);
		await closePage(page);
	}
	{
		const stamp = Date.now().toString(36);
		const name = `acc-kb-${stamp}`;
		const sprint = await api.must('POST', `/api/v1/projects/${project}/sprints`, {
			name,
			start_date: '2026-11-20',
			end_date: '2026-12-04'
		});
		const page = await openPage(`/workspace/${ws}/projects/${project}/cycles`, { freeze: true });
		await check(
			'keyboard: Escape closes the confirm dialog and returns focus to the opener',
			async () => {
				await clickRowButton(page, name, /^delete$/);
				await page.waitForSelector('[role="alertdialog"]');
				await page.keyboard.press('Escape');
				await page.waitForSelector('[role="alertdialog"]', { state: 'detached', timeout: 5000 });
				await page.waitForTimeout(250);
				const focus = await page.evaluate(() => {
					const el = document.activeElement;
					return el
						? `${el.tagName.toLowerCase()}:${(el.textContent || '').trim().slice(0, 20)}`
						: 'none';
				});
				expect(/button:delete/i.test(focus), `focus is on ${focus}`);
				const still = await sprintExists(api, project, sprint.id);
				expect(still, 'Escape removed the object');
				return `focus on ${focus}, object kept`;
			}
		);
		await closePage(page);
		await api.call('DELETE', `/api/v1/sprints/${sprint.id}`);
	}
	{
		const fixture = await formsFixture(api);
		const page = await openPage(
			`/workspace/${ws}/projects/${project}/forms?form=${fixture.orders}&record=${fixture.order}`,
			{ freeze: true }
		);
		const popover = async (name, triggerSelector, openedSelector) => {
			await check(`keyboard: Escape closes ${name} and returns focus to its trigger`, async () => {
				const trigger = page.locator(triggerSelector).first();
				await trigger.waitFor();
				await trigger.focus();
				await page.keyboard.press('Enter');
				await page.waitForSelector(openedSelector, { state: 'visible', timeout: 5000 });
				await page.keyboard.press('Escape');
				await page.waitForSelector(openedSelector, { state: 'hidden', timeout: 5000 });
				await page.waitForTimeout(250);
				const back = await trigger.evaluate((element) => document.activeElement === element);
				expect(back, 'focus did not return to the trigger');
				return 'closed, focus back on the trigger';
			});
		};
		await popover('the filter popover', 'main button:has-text("Filter")', 'main [role="dialog"]');
		await popover('the fields popover', 'main button:has-text("Fields")', 'main [role="dialog"]');
		await popover('the record panel menu', '[data-record-panel-more]', '[data-record-panel-menu]');
		await closePage(page);
	}
}

// ---------------------------------------------------------------------------------------------
// Section 7: embed pilot
// ---------------------------------------------------------------------------------------------

async function sectionEmbed(api) {
	const fixture = await formsFixture(api);
	const project = world.project;
	const ws = world.ws;
	const path = `/embed/harness?surface=record&w=${ws}&p=${project}&r=${fixture.order}`;
	const page = await openPage(path, { width: 1500, height: 1000 });
	await check('embed: the harness loads four frames', async () => {
		await page.waitForSelector('[data-testid="harness-frame"]');
		const count = await page.locator('[data-testid="harness-frame"]').count();
		expect(count === 4, `${count} frames`);
		await page.waitForTimeout(2500);
		return `${count} frames`;
	});
	for (const [id, width] of [
		['harness-340', 340],
		['harness-480', 480],
		['harness-600', 600],
		['harness-full', null]
	]) {
		await check(`embed: frame ${width ?? 'full width'} -- no overflow, no shell`, async () => {
			const handle = await page.locator(`iframe[data-frame-id="${id}"]`).elementHandle();
			const frame = await handle.contentFrame();
			expect(frame, 'no frame content');
			await frame.waitForLoadState('load');
			await frame.waitForSelector('[data-embed-frame]', { timeout: 15_000 });
			await page.waitForTimeout(800);
			const info = await frame.evaluate(() => ({
				scrollWidth: document.documentElement.scrollWidth,
				clientWidth: document.documentElement.clientWidth,
				shell: document.querySelectorAll('[data-shell], nav[data-shell], [data-layout="topbar"]')
					.length,
				rail: document.querySelectorAll(
					'[data-shell="rail"], [data-shell="column"], [data-shell="header"], [role="tablist"]'
				).length,
				skip: !!document.querySelector('a[href="#sv-main"]'),
				text: (document.body.innerText || '').length
			}));
			expect(
				info.scrollWidth <= info.clientWidth,
				`scrollWidth ${info.scrollWidth} > clientWidth ${info.clientWidth}`
			);
			expect(
				info.shell === 0 && info.rail === 0 && !info.skip,
				`${info.shell} shell element(s), ${info.rail} rail/column/header/tab element(s), skip link=${info.skip}`
			);
			expect(info.text > 0, 'the frame is empty');
			const outer = await page.evaluate((frameId) => {
				const element = document.querySelector(`iframe[data-frame-id="${frameId}"]`);
				return {
					frame: Math.round(element.getBoundingClientRect().width),
					doc: document.documentElement.scrollWidth <= document.documentElement.clientWidth
				};
			}, id);
			if (width) expect(outer.frame === width, `frame is ${outer.frame}px wide, not ${width}`);
			return `frame ${outer.frame}px, scrollWidth ${info.scrollWidth}<=${info.clientWidth}, shell elements 0`;
		});
	}
	await check('embed: the host theme change is applied in every frame', async () => {
		const read = () =>
			page.evaluate(() =>
				[...document.querySelectorAll('iframe[data-testid="harness-frame"]')].map((element) => {
					const doc = element.contentDocument?.documentElement;
					return `${doc?.dataset.theme ?? '?'}/${doc?.classList.contains('dark') ? 'dark' : 'light'}`;
				})
			);
		await page.click('[data-testid="harness-theme-dark"]');
		await page.waitForTimeout(800);
		const dark = await read();
		expect(
			dark.every((v) => v === 'dark/dark'),
			`after "dark": ${dark.join(', ')}`
		);
		await page.click('[data-testid="harness-theme-light"]');
		await page.waitForTimeout(800);
		const light = await read();
		expect(
			light.every((v) => v === 'light/light'),
			`after "light": ${light.join(', ')}`
		);
		return `dark -> ${dark.join(',')}; light -> ${light.join(',')}`;
	});
	await check('embed: no overflow of the harness page and no page error', async () => {
		const overflow = await page.evaluate(
			() => document.documentElement.scrollWidth <= document.documentElement.clientWidth
		);
		expect(overflow, 'the harness page overflows horizontally');
		expect(
			page.__problems.length === 0,
			`page problems: ${page.__problems.slice(0, 2).join(' ; ')}`
		);
		return 'ok';
	});
	await closePage(page);
}

// ---------------------------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------------------------

const RUNNERS = {
	confirmations: sectionConfirmations,
	flow: sectionFlow,
	plugins: sectionPlugins,
	errors: sectionErrors,
	narrow: sectionNarrow,
	keyboard: sectionKeyboard,
	embed: sectionEmbed
};

async function main() {
	await mkdir(OUT, { recursive: true });
	await launch();
	let fatal = null;
	try {
		const session = await sessionOf('U1');
		const api = apiFor(session);
		await ensureWorld(api);
		for (const name of only) {
			currentSection = name;
			console.log(`--- section ${name}`);
			try {
				// A long section outlives one access token: the API client follows the cached session.
				const fresh = apiFor(await sessionOf('U1'));
				await RUNNERS[name](fresh);
			} catch (error) {
				fatal = error;
				await check(`section ${name} ran to its end`, async () => {
					throw error;
				});
			}
		}
	} catch (error) {
		fatal = error;
		console.error(`FATAL ${redact(error instanceof Error ? error.message : error)}`);
	} finally {
		if (browser) await browser.close().catch(() => {});
	}
	const count = (status) => results.filter((r) => r.status === status).length;
	const summary = {
		run: STAMP,
		ui: UI,
		api: API,
		workspace_id: world.ws ?? null,
		workspace_name: world.wsName ?? null,
		project_id: world.project ?? null,
		sections: only,
		pass: count('PASS'),
		fail: count('FAIL'),
		skip: count('SKIP'),
		fatal: fatal ? redact(String(fatal.message ?? fatal)) : null,
		results
	};
	const file = join(OUT, `acceptance-phase3-${only.join('+')}-${STAMP}.json`);
	await writeFile(file, redact(JSON.stringify(summary, null, 2)) + '\n');
	console.log(`workspace id: ${world.ws ?? 'none'}`);
	console.log(
		`TOTAL ${results.length}: ${summary.pass} PASS, ${summary.fail} FAIL, ${summary.skip} SKIP -- ${file}`
	);
	process.exitCode = summary.fail || fatal ? 1 : 0;
}

main().catch((error) => {
	console.error(`acceptance-phase3: ${redact(error instanceof Error ? error.message : error)}`);
	process.exit(1);
});
