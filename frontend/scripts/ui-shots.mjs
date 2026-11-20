#!/usr/bin/env node
// Browser harness for the interface redesign: screenshots, token probe, LEGACY-SCAN and pixel diff.
//
//   node scripts/ui-shots.mjs list
//   node scripts/ui-shots.mjs capture --out DIR [--base-url URL] [--widths 1440,800]
//                                     [--themes light,dark] [--routes name,name] [--now ISO]
//   node scripts/ui-shots.mjs diff DIR_A DIR_B [--report FILE] [--tolerance N] [--diff-dir DIR]
//
// Capture is read-only. It signs in once through the API (one session is created), then only
// navigates. Every non-GET request the pages try to make is aborted and listed in scan.json
// under `blockedWrites`; the sign-in refresh endpoint is the only exception.
//
// The project-issue route needs an issue. The harness never creates data: it takes the issue from
// UI_SHOTS_ISSUE (`workspaceId/projectId/issueId`) or finds the first existing issue through
// read-only API calls. When the environment has none, the route is recorded as
// `skipped: no issue in the environment` in scan.json and counted in the summary line.
//
// Browser: the locally installed open-source Chromium, headless (CHROMIUM_BIN overrides the path).
// Credentials: read at run time from the environment (P3X_U1_EMAIL, P3X_U1_PASSWORD) or from the
// env file named by UI_SHOTS_ENV_FILE. They are never printed or written anywhere.
//
// LEGACY-SCAN: per screen, the number of elements whose class attribute holds a raw palette
// utility (slate, blue, red, amber, emerald, green, cyan, yellow, gray with a 2-3 digit shade) or
// a `dark:` variant. A migrated screen scans to 0.

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { existsSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { PNG } from 'pngjs';

const DEFAULT_BASE_URL = 'http://127.0.0.1:18097';
const DEFAULT_API_ORIGIN = 'http://127.0.0.1:18083';
const DEFAULT_ENV_FILE =
	'/Users/nn/worker/llama/reports/sv-plugin-analysis-2026-11-14/release-next-local-deploy-20261119T153906Z/acceptance.env';
const DEFAULT_CHROMIUM = '/Applications/Chromium.app/Contents/MacOS/Chromium';
const VIEWPORT_HEIGHT = 900;

/**
 * Route list. `{ws}` and `{project}` are resolved at run time from read-only API calls. `{issue}`
 * comes with its own workspace and project (`{issue_ws}`, `{issue_project}`), because the first
 * issue found may live in another project. A route whose placeholder cannot be resolved is
 * skipped and reported with its reason (SKIP_REASONS).
 * `auth: false` routes are captured without a session.
 * Not listed on purpose: the Flow page editor (`flow/[objectId]`), which opens a collaboration
 * WebSocket that the API only accepts from the origin of the deployed UI.
 */
export const ROUTES = [
	{ name: 'auth-login', path: '/auth/login', auth: false },
	{ name: 'home', path: '/' },
	{ name: 'inbox', path: '/inbox' },
	{ name: 'my-work', path: '/my-work' },
	{ name: 'search', path: '/search' },
	{ name: 'settings', path: '/settings' },
	{ name: 'plugins', path: '/plugins' },
	{ name: 'plugins-creator', path: '/plugins/creator' },
	{ name: 'agents', path: '/agents' },
	{ name: 'flow-package', path: '/flow/package' },
	{ name: 'flow-list', path: '/flow' },
	{ name: 'governance', path: '/governance' },
	{ name: 'governance-config', path: '/governance/config' },
	{ name: 'governance-audit-logs', path: '/governance/audit-logs' },
	{ name: 'proposals', path: '/proposals' },
	{ name: 'proposals-new', path: '/proposals/new' },
	{ name: 'proposal-templates', path: '/proposal-templates' },
	{ name: 'impact-reviews', path: '/impact-reviews' },
	{ name: 'trust-board', path: '/trust-board' },
	{ name: 'decisions-analytics', path: '/decisions/analytics' },
	{ name: 'admin', path: '/admin' },
	{ name: 'admin-users', path: '/admin/users' },
	{ name: 'admin-settings', path: '/admin/settings' },
	{ name: 'admin-ui', path: '/admin/ui' },
	{ name: 'workspace-list', path: '/workspace' },
	{ name: 'workspace-home', path: '/workspace/{ws}' },
	{ name: 'workspace-projects', path: '/workspace/{ws}/projects' },
	{ name: 'workspace-members', path: '/workspace/{ws}/members' },
	{ name: 'workspace-connections', path: '/workspace/{ws}/connections' },
	{ name: 'workspace-webhooks', path: '/workspace/{ws}/webhooks' },
	{ name: 'workspace-workflows', path: '/workspace/{ws}/workflows' },
	{ name: 'workspace-settings', path: '/workspace/{ws}/settings' },
	{ name: 'workspace-settings-flow', path: '/workspace/{ws}/settings/flow' },
	{ name: 'workspace-settings-flow-operations', path: '/workspace/{ws}/settings/flow/operations' },
	{ name: 'workspace-settings-flow-package', path: '/workspace/{ws}/settings/flow/package' },
	{ name: 'workspace-flow', path: '/workspace/{ws}/flow' },
	{ name: 'project-home', path: '/workspace/{ws}/projects/{project}' },
	{ name: 'project-board', path: '/workspace/{ws}/projects/{project}/board' },
	{ name: 'project-cycles', path: '/workspace/{ws}/projects/{project}/cycles' },
	{ name: 'project-issues', path: '/workspace/{ws}/projects/{project}/issues' },
	{
		name: 'project-issue',
		path: '/workspace/{issue_ws}/projects/{issue_project}/issues/{issue}'
	},
	{ name: 'project-forms', path: '/workspace/{ws}/projects/{project}/forms' },
	{ name: 'project-apps', path: '/workspace/{ws}/projects/{project}/apps' },
	{ name: 'project-ai-agents', path: '/workspace/{ws}/projects/{project}/ai-agents' },
	{ name: 'project-audit-reports', path: '/workspace/{ws}/projects/{project}/audit-reports' },
	{
		name: 'project-settings-plugins',
		path: '/workspace/{ws}/projects/{project}/settings/plugins'
	}
];

/** Why a route is skipped, by the placeholder that could not be resolved. */
export const SKIP_REASONS = {
	issue: 'no issue in the environment',
	issue_ws: 'no issue in the environment',
	issue_project: 'no issue in the environment',
	ws: 'no workspace in the environment',
	project: 'no project in the environment'
};

// The raw palette utilities and `dark:` variants of the legacy language (plan section 4).
export const LEGACY_PALETTE_SOURCE =
	'\\b(slate|blue|red|amber|emerald|green|cyan|yellow|gray)-\\d{2,3}\\b';

function parseArgs(argv) {
	const positional = [];
	const flags = {};
	for (let i = 0; i < argv.length; i += 1) {
		const arg = argv[i];
		if (arg.startsWith('--')) {
			const key = arg.slice(2);
			const next = argv[i + 1];
			if (next === undefined || next.startsWith('--')) flags[key] = true;
			else {
				flags[key] = next;
				i += 1;
			}
		} else positional.push(arg);
	}
	return { positional, flags };
}

async function readCredentials(user) {
	const emailKey = `P3X_${user}_EMAIL`;
	const passwordKey = `P3X_${user}_PASSWORD`;
	let email = process.env[emailKey];
	let password = process.env[passwordKey];
	if (!email || !password) {
		const file = process.env.UI_SHOTS_ENV_FILE || DEFAULT_ENV_FILE;
		if (existsSync(file)) {
			for (const line of (await readFile(file, 'utf8')).split('\n')) {
				const match = line.match(/^(?:export\s+)?(\w+)=(.*)$/);
				if (!match) continue;
				const value = match[2].trim().replace(/^['"]|['"]$/g, '');
				if (match[1] === emailKey) email = email || value;
				if (match[1] === passwordKey) password = password || value;
			}
		}
	}
	if (!email || !password)
		throw new Error(`no credentials for ${user} (${emailKey}, ${passwordKey})`);
	return { email, password };
}

async function apiJson(api, token, path) {
	const response = await fetch(api + path, {
		headers: token ? { authorization: `Bearer ${token}` } : {}
	});
	if (!response.ok) return null;
	const body = await response.json();
	return body?.data ?? null;
}

async function signIn(api, user) {
	const credentials = await readCredentials(user);
	const response = await fetch(`${api}/api/v1/auth/login`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify(credentials)
	});
	const body = await response.json().catch(() => null);
	if (!response.ok || body?.code !== 0 || !body?.data?.tokens?.access_token)
		throw new Error(`sign-in failed with HTTP ${response.status}`);
	return body.data;
}

/** `workspaceId/projectId/issueId` from UI_SHOTS_ISSUE, or null. */
export function parseIssueRef(value) {
	const parts = String(value ?? '')
		.trim()
		.split('/');
	return parts.length === 3 && parts.every(Boolean)
		? { issue_ws: parts[0], issue_project: parts[1], issue: parts[2] }
		: null;
}

/** First existing issue over the user's workspaces and projects (read-only, bounded). */
async function findIssue(api, token, workspaces) {
	for (const workspace of workspaces.slice(0, 40)) {
		const projects = await apiJson(api, token, `/api/v1/workspaces/${workspace.id}/projects`);
		for (const project of (projects?.items ?? projects ?? []).slice(0, 40)) {
			const issues = await apiJson(api, token, `/api/v1/projects/${project.id}/issues`);
			const issue = (issues?.items ?? issues ?? [])[0];
			if (issue) return { issue_ws: workspace.id, issue_project: project.id, issue: issue.id };
		}
	}
	return null;
}

async function resolvePlaceholders(api, token) {
	const found = {};
	const workspaces = await apiJson(api, token, '/api/v1/workspaces');
	const workspaceList = workspaces?.items ?? workspaces ?? [];
	const workspace = workspaceList[0];
	if (workspace) {
		found.ws = workspace.id;
		const projects = await apiJson(api, token, `/api/v1/workspaces/${workspace.id}/projects`);
		const list = projects?.items ?? projects ?? [];
		const project = list.find((p) => / A$/.test(p.name)) ?? list[0];
		if (project) found.project = project.id;
	}
	Object.assign(
		found,
		parseIssueRef(process.env.UI_SHOTS_ISSUE) ?? (await findIssue(api, token, workspaceList)) ?? {}
	);
	return found;
}

/** Returns the filled path, or `{ missing: key }` for the first placeholder without a value. */
export function fillPath(template, values) {
	let missing = null;
	const path = template.replace(/\{(\w+)\}/g, (_, key) => {
		if (!values[key]) missing = missing ?? key;
		return values[key] ?? '';
	});
	return missing ? { missing } : { path };
}

const FREEZE_CSS = `*, *::before, *::after {
	animation: none !important;
	transition: none !important;
	caret-color: transparent !important;
	scroll-behavior: auto !important;
}`;

async function capture(flags) {
	const out = resolve(flags.out || '');
	if (!flags.out) throw new Error('capture needs --out DIR');
	const baseUrl = String(flags['base-url'] || DEFAULT_BASE_URL).replace(/\/$/, '');
	const api = String(flags.api || DEFAULT_API_ORIGIN).replace(/\/$/, '');
	const widths = String(flags.widths || '1440,800')
		.split(',')
		.map(Number);
	const themes = String(flags.themes || 'light,dark').split(',');
	const only = flags.routes ? new Set(String(flags.routes).split(',')) : null;
	const now = new Date(
		flags.now || new Date(Math.floor(Date.now() / 3_600_000) * 3_600_000).toISOString()
	);
	const user = String(flags.user || 'U1');

	const { chromium } = await import('playwright-core');
	const executablePath = process.env.CHROMIUM_BIN || DEFAULT_CHROMIUM;
	if (!existsSync(executablePath)) throw new Error(`Chromium not found at ${executablePath}`);

	const session = await signIn(api, user);
	const values = await resolvePlaceholders(api, session.tokens.access_token);
	await mkdir(out, { recursive: true });

	const browser = await chromium.launch({ executablePath, headless: true });
	const results = [];
	const blockedWrites = [];
	const skipped = [];
	try {
		for (const theme of themes) {
			for (const width of widths) {
				for (const route of ROUTES) {
					if (only && !only.has(route.name)) continue;
					const filled = fillPath(route.path, values);
					if (!filled.path) {
						const reason = SKIP_REASONS[filled.missing] ?? `unresolved {${filled.missing}}`;
						if (!skipped.some((entry) => entry.route === route.name))
							skipped.push({ route: route.name, reason });
						results.push({
							route: route.name,
							theme,
							width,
							status: 'skipped',
							reason: `skipped: ${reason}`
						});
						continue;
					}
					const path = filled.path;
					const context = await browser.newContext({
						viewport: { width, height: VIEWPORT_HEIGHT },
						deviceScaleFactor: 1,
						colorScheme: 'light',
						reducedMotion: 'reduce',
						locale: 'en-US',
						timezoneId: 'UTC'
					});
					await context.clock.setFixedTime(now);
					await context.addInitScript(
						({ theme: themeValue, tokens, user: sessionUser, authenticated }) => {
							try {
								localStorage.setItem('theme', themeValue);
								if (authenticated) {
									localStorage.setItem('auth_token', tokens.access_token);
									localStorage.setItem('refresh_token', tokens.refresh_token);
									localStorage.setItem('auth_user', JSON.stringify(sessionUser));
								}
							} catch {
								// storage unavailable: the page then renders its signed-out state
							}
						},
						{
							theme,
							tokens: session.tokens,
							user: session.user,
							authenticated: route.auth !== false
						}
					);
					await context.route('**/api/**', (request) => {
						const req = request.request();
						const method = req.method();
						const url = new URL(req.url());
						if (
							method === 'GET' ||
							method === 'HEAD' ||
							method === 'OPTIONS' ||
							url.pathname === '/api/v1/auth/refresh'
						)
							return request.continue();
						blockedWrites.push(`${method} ${url.pathname}`);
						return request.abort('blockedbyclient');
					});
					const page = await context.newPage();
					const problems = [];
					page.on('pageerror', (error) => problems.push(`pageerror: ${error.message}`));
					page.on('console', (message) => {
						if (message.type() === 'error')
							problems.push(`console: ${message.text().slice(0, 160)}`);
					});
					const started = Date.now();
					let status = 'ok';
					try {
						await page.goto(baseUrl + path, { waitUntil: 'load', timeout: 30_000 });
						await page.waitForLoadState('networkidle', { timeout: 15_000 }).catch(() => {
							status = 'busy';
						});
						await page.addStyleTag({ content: FREEZE_CSS });
						await page.evaluate(() => document.fonts.ready);
						await page.waitForTimeout(500);
						const probe = await page.evaluate((paletteSource) => {
							const palette = new RegExp(paletteSource);
							let legacy = 0;
							for (const element of document.querySelectorAll('[class]')) {
								const value = element.getAttribute('class') || '';
								if (palette.test(value) || value.includes('dark:')) legacy += 1;
							}
							const style = getComputedStyle(document.documentElement);
							return {
								legacy,
								dark: document.documentElement.classList.contains('dark'),
								rfBg: style.getPropertyValue('--rf-bg').trim(),
								url: location.pathname + location.search,
								textLength: (document.body.innerText || '').length
							};
						}, LEGACY_PALETTE_SOURCE);
						const file = `${route.name}.${theme}.${width}.png`;
						await page.screenshot({
							path: join(out, file),
							fullPage: true,
							animations: 'disabled'
						});
						results.push({
							route: route.name,
							theme,
							width,
							file,
							status,
							...probe,
							problems: problems.slice(0, 5),
							ms: Date.now() - started
						});
					} catch (error) {
						results.push({
							route: route.name,
							theme,
							width,
							status: 'error',
							error: String(error.message).split('\n')[0]
						});
					} finally {
						await context.close();
					}
				}
			}
		}
	} finally {
		await browser.close();
	}

	const summary = {
		baseUrl,
		now: now.toISOString(),
		widths,
		themes,
		resolved: Object.keys(values),
		skipped,
		blockedWrites: [...new Set(blockedWrites)],
		results
	};
	await writeFile(join(out, 'scan.json'), JSON.stringify(summary, null, 2) + '\n');
	const failed = results.filter((r) => r.status === 'error');
	const captured = results.filter((r) => r.status !== 'error' && r.status !== 'skipped');
	console.log(
		`captured ${captured.length} screens into ${out} (${failed.length} failed, ${skipped.length} routes skipped)`
	);
	for (const r of failed) console.log(`  FAILED ${r.route} ${r.theme} ${r.width}: ${r.error}`);
	for (const entry of skipped) console.log(`  SKIPPED ${entry.route}: ${entry.reason}`);
	if (summary.blockedWrites.length)
		console.log(`  blocked write requests: ${summary.blockedWrites.join(', ')}`);
	process.exitCode = failed.length ? 1 : 0;
}

/** Exact pixel comparison of two PNGs. A size mismatch counts every pixel of the larger as different. */
export function comparePng(bufferA, bufferB, tolerance = 0) {
	const a = PNG.sync.read(bufferA);
	const b = PNG.sync.read(bufferB);
	if (a.width !== b.width || a.height !== b.height) {
		return {
			differing: Math.max(a.width * a.height, b.width * b.height),
			sizeMismatch: `${a.width}x${a.height} vs ${b.width}x${b.height}`,
			image: null
		};
	}
	const image = new PNG({ width: a.width, height: a.height });
	let differing = 0;
	for (let i = 0; i < a.data.length; i += 4) {
		let different = false;
		for (let c = 0; c < 4; c += 1) {
			if (Math.abs(a.data[i + c] - b.data[i + c]) > tolerance) different = true;
		}
		if (different) {
			differing += 1;
			image.data[i] = 255;
			image.data[i + 1] = 0;
			image.data[i + 2] = 0;
			image.data[i + 3] = 255;
		} else {
			image.data[i] = a.data[i];
			image.data[i + 1] = a.data[i + 1];
			image.data[i + 2] = a.data[i + 2];
			image.data[i + 3] = 40;
		}
	}
	return { differing, sizeMismatch: null, image };
}

async function diff(positional, flags) {
	const [dirA, dirB] = positional.slice(1).map((p) => resolve(p));
	if (!dirA || !dirB) throw new Error('diff needs DIR_A DIR_B');
	const tolerance = Number(flags.tolerance || 0);
	const diffDir = flags['diff-dir'] ? resolve(String(flags['diff-dir'])) : null;
	if (diffDir) await mkdir(diffDir, { recursive: true });
	const filesA = readdirSync(dirA).filter((f) => f.endsWith('.png'));
	const filesB = new Set(readdirSync(dirB).filter((f) => f.endsWith('.png')));
	const lines = ['| Screen | Differing pixels | Note |', '|---|---:|---|'];
	let total = 0;
	let different = 0;
	for (const file of filesA.sort()) {
		const name = file.replace(/\.png$/, '');
		if (!filesB.has(file)) {
			lines.push(`| ${name} | n/a | missing in B |`);
			different += 1;
			continue;
		}
		const result = comparePng(
			await readFile(join(dirA, file)),
			await readFile(join(dirB, file)),
			tolerance
		);
		total += result.differing;
		if (result.differing > 0) different += 1;
		if (result.image && result.differing > 0 && diffDir)
			await writeFile(join(diffDir, `${name}.diff.png`), PNG.sync.write(result.image));
		lines.push(`| ${name} | ${result.differing} | ${result.sizeMismatch ?? ''} |`);
	}
	for (const file of filesB)
		if (!filesA.includes(file))
			lines.push(`| ${file.replace(/\.png$/, '')} | n/a | missing in A |`);
	lines.push(
		'',
		`${filesA.length} screens compared, ${different} differ, ${total} differing pixels in total.`
	);
	const text = lines.join('\n') + '\n';
	console.log(text);
	if (flags.report) await writeFile(resolve(String(flags.report)), text);
	process.exitCode = different ? 1 : 0;
}

async function main() {
	const { positional, flags } = parseArgs(process.argv.slice(2));
	const command = positional[0];
	if (command === 'list') {
		for (const route of ROUTES)
			console.log(`${route.name}\t${route.path}${route.auth === false ? '\t(no session)' : ''}`);
	} else if (command === 'capture') await capture(flags);
	else if (command === 'diff') await diff(positional, flags);
	else {
		console.error('usage: ui-shots.mjs list | capture --out DIR [...] | diff DIR_A DIR_B [...]');
		process.exitCode = 2;
	}
}

if (import.meta.url === `file://${process.argv[1]}`) {
	main().catch((error) => {
		console.error(`ui-shots: ${error.message}`);
		process.exit(1);
	});
}
