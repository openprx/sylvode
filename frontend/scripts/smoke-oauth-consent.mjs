#!/usr/bin/env node
/**
 * Browser check of the authorization server's screens: the consent page and the three
 * "Connected applications" sections. It serves the built app (`build/`) next to a mock API on
 * one port and a loopback stub (the application's return address) on another, drives headless
 * Chromium through playwright-core, and fails on any page error or console error.
 *
 * Covered: the allow flow (all projects, selected projects, optional scope) ending in a navigation
 * to the `redirect_to` the mocked decision returns, with no consent token in the address, the
 * storage or any request URL; deny; "I don't have a Sylvode account"; an unknown application, a
 * return address that does not match and a repeated `redirect_uri` (error page, no navigation);
 * an expired request, a consent token the server no longer accepts, an application disabled
 * meanwhile, too many connections, no eligible workspace; the signed-out visit that goes to login
 * and comes back, and hostile `return_to` values that are ignored; the three management sections
 * (list, details, disconnect with confirmation, workspace switch off and on, instance client
 * disable and enable); the Chinese strings of contract section 14; no horizontal overflow at
 * 390 px; keyboard focus on an error; screenshots of each state at 1440 px light and 390 px dark.
 *
 * Run: `bun run build && bun run smoke:oauth-consent`
 *   CHROMIUM_BIN                     Chromium executable (default /usr/bin/chromium)
 *   PLAYWRIGHT_CORE                  path to a playwright-core package directory
 *   OAUTH_SMOKE_PORT                 port of the app and the mock API (default 18457)
 *   OAUTH_SMOKE_STUB_PORT            port of the loopback stub (default 18456)
 *   OAUTH_SMOKE_SCREENSHOT_DIR       when set, screenshots are written there
 */
import { createServer } from 'node:http';
import { randomBytes, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { extname, join, normalize, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromiumPath = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const playwrightModule = process.env.PLAYWRIGHT_CORE
	? pathToFileURL(join(process.env.PLAYWRIGHT_CORE, 'index.mjs')).href
	: 'playwright-core';
const { chromium } = await import(playwrightModule);

const APP_PORT = Number(process.env.OAUTH_SMOKE_PORT || 18457);
const STUB_PORT = Number(process.env.OAUTH_SMOKE_STUB_PORT || 18456);
const APP = `http://127.0.0.1:${APP_PORT}`;
const STUB = `http://127.0.0.1:${STUB_PORT}`;
const REDIRECT_URI = `${STUB}/oauth/sylvode/callback`;
const STATE = 'state-0123456789abcdefghij';
const SHOTS = process.env.OAUTH_SMOKE_SCREENSHOT_DIR || '';
const T = 15_000;

if (!existsSync(buildDir)) throw new Error(`Missing ${buildDir}. Run bun run build first.`);
if (SHOTS) mkdirSync(SHOTS, { recursive: true });

// ---------------------------------------------------------------------------------------------
// Mock API
// ---------------------------------------------------------------------------------------------

const user = { id: 'user-1', email: 'a@example.com', name: 'A. User', role: 'admin' };
const TOKEN = 'smoke-access-token';
const OPTIONAL = new Set([
	'work_items:write',
	'forms:write',
	'notifications:write',
	'surfaces:embed'
]);
const minutesAgo = (n) => new Date(Date.now() - n * 60_000).toISOString();
const inDays = (n) => new Date(Date.now() + n * 86_400_000).toISOString();

const projectsOf = {
	'ws-acme': [
		{ id: 'p-alpha', key: 'AL', name: 'Alpha' },
		{ id: 'p-beta', key: 'BE', name: 'Beta' },
		{ id: 'p-gamma', key: 'GA', name: 'Gamma' }
	],
	'ws-solo': []
};

function grant(id, overrides = {}) {
	return {
		grant_id: id,
		client: { client_id: 'ramflux', display_name: 'Ramflux' },
		device_name: 'MacBook',
		workspace: { id: 'ws-acme', name: 'Acme' },
		scopes: ['projects:read', 'work_items:read', 'offline_access'],
		project_mode: 'selected',
		projects: [
			{ id: 'p-alpha', key: 'AL', name: 'Alpha' },
			{ id: 'p-beta', key: 'BE', name: 'Beta' }
		],
		project_count: 3,
		consented_at: minutesAgo(600),
		last_used_at: minutesAgo(10),
		expires_at: inDays(300),
		...overrides
	};
}

function fresh() {
	return {
		workspaces: [
			{ id: 'ws-acme', name: 'Acme', eligible: true, reason: null },
			{ id: 'ws-lab', name: 'Lab', eligible: false, reason: 'workspace_disallows' },
			{ id: 'ws-solo', name: 'Solo', eligible: true, reason: null }
		],
		deviceName: 'MacBook',
		requests: new Map(),
		authorizeCalls: [],
		decisions: [],
		failDecision: null,
		failListOnce: false,
		own: [
			grant('g-own-1'),
			grant('g-own-2', {
				device_name: null,
				project_mode: 'all',
				projects: [],
				project_count: null,
				scopes: ['projects:read', 'work_items:write'],
				last_used_at: null,
				workspace: { id: 'ws-solo', name: 'Solo' }
			})
		],
		wsGrants: [
			['g-w-1', 'Mei Tan'],
			['g-w-2', 'Ana Silva'],
			['g-w-3', 'Li Wei'],
			['g-w-4', 'Jo Park']
		].map(([id, name], i) =>
			grant(id, {
				user: {
					id: `user-${i + 10}`,
					name,
					email: `${name.split(' ')[0].toLowerCase()}@example.com`
				},
				device_name: `Laptop ${i + 1}`
			})
		),
		wsAllow: true,
		wsPuts: [],
		wsDeletes: [],
		clientStatus: 'active',
		clientGrants: 7,
		clientPatches: [],
		ownDeletes: [],
		unhandled: []
	};
}
let m = fresh();

const json = (res, status, body, extra = {}) => {
	res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', ...extra });
	res.end(JSON.stringify(body));
};
const ok = (res, data) => json(res, 200, { code: 0, message: 'success', data });
const fail = (res, status, error_code, details) =>
	json(res, status, {
		code: status,
		message: 'refused',
		error_code,
		error: error_code,
		...(details === undefined ? {} : { details })
	});
const paginated = (items) => ({
	items,
	total: items.length,
	page: 1,
	per_page: 100,
	total_pages: 1
});

async function readBody(req) {
	const chunks = [];
	for await (const chunk of req) chunks.push(chunk);
	const text = Buffer.concat(chunks).toString('utf8');
	return text ? JSON.parse(text) : {};
}

const CONTENT_TYPES = {
	'.html': 'text/html; charset=utf-8',
	'.js': 'application/javascript; charset=utf-8',
	'.css': 'text/css; charset=utf-8',
	'.json': 'application/json; charset=utf-8',
	'.svg': 'image/svg+xml',
	'.wasm': 'application/wasm'
};

async function serveStatic(req, res) {
	const url = new URL(req.url ?? '/', APP);
	let pathname = decodeURIComponent(url.pathname);
	if (pathname === '/') pathname = '/index.html';
	let file = normalize(join(buildDir, pathname));
	if (!file.startsWith(buildDir) || !existsSync(file) || file === buildDir)
		file = join(buildDir, 'index.html');
	const data = await readFile(file);
	res.writeHead(200, {
		'Content-Type': CONTENT_TYPES[extname(file)] ?? 'application/octet-stream'
	});
	res.end(data);
}

function callbackUrl(stored, params) {
	const target = new URL(stored.redirect_uri);
	for (const [key, value] of Object.entries(params)) target.searchParams.set(key, value);
	target.searchParams.set('state', stored.state);
	target.searchParams.set('iss', APP);
	return target.toString();
}

function consentFor(stored) {
	stored.token = randomBytes(32).toString('hex');
	const names = (stored.scope ?? '').split(' ').filter(Boolean);
	return {
		kind: 'consent',
		request_id: stored.id,
		consent_token: stored.token,
		client: {
			client_id: 'ramflux',
			display_name: 'Ramflux',
			description: 'Ramflux desktop',
			redirect_host: '127.0.0.1',
			device_name: stored.device_name ?? null
		},
		account: { user_id: user.id, name: user.name, email: user.email },
		workspaces: m.workspaces,
		preselected_workspace_id: null,
		scopes: names.map((name) => ({ name, required: !OPTIONAL.has(name) })),
		resources: ['api', 'mcp'],
		active_grant_count: 2,
		max_active_grants: 10
	};
}

async function handleApi(req, res, url) {
	const path = url.pathname;
	const authed = req.headers.authorization === `Bearer ${TOKEN}`;

	if (path === '/api/v1/oauth/authorization-requests' && req.method === 'POST') {
		const body = await readBody(req);
		m.authorizeCalls.push({ body, authed });
		let stored;
		if (body.request_id) {
			stored = m.requests.get(body.request_id);
			if (!stored || stored.decided || stored.expired)
				return ok(res, { kind: 'error_page', error: 'request_expired' });
		} else {
			if (body.client_id !== 'ramflux')
				return ok(res, { kind: 'error_page', error: 'unknown_client' });
			if (!/^http:\/\/127\.0\.0\.1:\d+\/oauth\/sylvode\/callback$/.test(body.redirect_uri ?? ''))
				return ok(res, { kind: 'error_page', error: 'redirect_uri_mismatch' });
			if (body.scope === 'bad_scope')
				return ok(res, {
					kind: 'redirect',
					redirect_to: callbackUrl(
						{ redirect_uri: body.redirect_uri, state: body.state },
						{ error: 'invalid_scope', error_description: 'the scope is not valid' }
					)
				});
			stored = {
				id: randomUUID(),
				redirect_uri: body.redirect_uri,
				state: body.state,
				scope: body.scope,
				device_name: body.device_name ?? m.deviceName ?? null,
				decided: false,
				expired: false,
				token: null
			};
			m.requests.set(stored.id, stored);
		}
		if (!authed)
			return ok(res, {
				kind: 'login_required',
				request_id: stored.id,
				client: { client_id: 'ramflux', display_name: 'Ramflux', redirect_host: '127.0.0.1' }
			});
		return ok(res, consentFor(stored));
	}

	const decision = /^\/api\/v1\/oauth\/authorization-requests\/([^/]+)\/decision$/.exec(path);
	if (decision && req.method === 'POST') {
		const body = await readBody(req);
		const token = req.headers['x-sylvode-consent'] ?? null;
		m.decisions.push({ id: decision[1], body, token, authed });
		const stored = m.requests.get(decision[1]);
		if (m.failDecision) {
			const next = m.failDecision;
			if (next.once !== false) m.failDecision = null;
			return fail(res, next.status, next.error_code, next.details);
		}
		if (!stored || stored.expired) return fail(res, 404, 'request_not_found');
		if (stored.decided) return fail(res, 409, 'already_decided');
		if (body.decision === 'no_account') {
			if (authed) return fail(res, 400, 'invalid_request');
			stored.decided = true;
			return ok(res, {
				redirect_to: callbackUrl(stored, {
					error: 'access_denied',
					error_description: 'account_required'
				})
			});
		}
		if (!authed) return fail(res, 401, 'session_required');
		if (token !== stored.token) return fail(res, 403, 'consent_token_invalid');
		stored.decided = true;
		if (body.decision === 'deny') {
			const eligible = m.workspaces.some((w) => w.eligible);
			return ok(res, {
				redirect_to: callbackUrl(stored, {
					error: 'access_denied',
					error_description: eligible ? 'consent_denied' : 'no_eligible_workspace'
				})
			});
		}
		return ok(res, { redirect_to: callbackUrl(stored, { code: `svc_${'c'.repeat(64)}` }) });
	}

	const projects = /^\/api\/v1\/workspaces\/([^/]+)\/projects$/.exec(path);
	if (projects && req.method === 'GET') {
		const items = (projectsOf[projects[1]] ?? []).map((p) => ({
			...p,
			workspace_id: projects[1],
			type_key: 'software',
			type_settings: {},
			created_at: minutesAgo(1000),
			updated_at: minutesAgo(1000)
		}));
		return ok(res, paginated(items));
	}

	if (path === '/api/v1/auth/me') {
		return authed ? ok(res, { user }) : fail(res, 401, 'unauthorized');
	}
	if (path === '/api/v1/auth/login' && req.method === 'POST') {
		const body = await readBody(req);
		if (body.password !== 'correct-horse') return fail(res, 401, 'invalid_credentials');
		return ok(res, {
			tokens: {
				access_token: TOKEN,
				refresh_token: 'smoke-refresh-token',
				token_type: 'Bearer',
				access_expires_in: 900,
				refresh_expires_in: 86_400
			},
			user
		});
	}
	if (path === '/api/v1/auth/logout' && req.method === 'POST') return ok(res, null);
	if (path === '/api/v1/auth/refresh') return fail(res, 401, 'unauthorized');
	if (path === '/api/v1/auth/me/preferences') {
		return ok(res, {
			notification_prefs: { email_notification: true, mention_only: false, daily_digest: true }
		});
	}

	// --- management -----------------------------------------------------------------------
	if (!authed && path.startsWith('/api/v1/') && !path.startsWith('/api/v1/oauth/')) {
		return fail(res, 401, 'unauthorized');
	}
	if (path === '/api/v1/me/connected-apps' && req.method === 'GET') {
		if (m.failListOnce) {
			m.failListOnce = false;
			return fail(res, 500, 'internal_error');
		}
		return ok(res, { items: m.own, limit: 100, offset: 0, has_more: false, next_offset: null });
	}
	const own = /^\/api\/v1\/me\/connected-apps\/([^/]+)$/.exec(path);
	if (own && req.method === 'DELETE') {
		const found = m.own.find((g) => g.grant_id === own[1]);
		if (!found) return fail(res, 404, 'not_found');
		m.ownDeletes.push(own[1]);
		m.own = m.own.filter((g) => g.grant_id !== own[1]);
		return ok(res, { grant_id: own[1], revoked: true });
	}
	const wsApps = /^\/api\/v1\/workspaces\/([^/]+)\/connected-apps(?:\/([^/]+))?$/.exec(path);
	if (wsApps && wsApps[2] === 'settings') {
		if (req.method === 'GET') return ok(res, { allow_connected_apps: m.wsAllow });
		if (req.method === 'PUT') {
			const body = await readBody(req);
			m.wsPuts.push(body);
			const revoked = body.allow_connected_apps ? 0 : m.wsGrants.length;
			m.wsAllow = body.allow_connected_apps;
			if (!m.wsAllow) m.wsGrants = [];
			return ok(res, { allow_connected_apps: m.wsAllow, revoked_grants: revoked });
		}
	}
	if (wsApps && !wsApps[2] && req.method === 'GET') {
		return ok(res, {
			items: m.wsGrants,
			limit: 100,
			offset: 0,
			has_more: false,
			next_offset: null
		});
	}
	if (wsApps && wsApps[2] && req.method === 'DELETE') {
		if (!m.wsGrants.some((g) => g.grant_id === wsApps[2])) return fail(res, 404, 'not_found');
		m.wsDeletes.push(wsApps[2]);
		m.wsGrants = m.wsGrants.filter((g) => g.grant_id !== wsApps[2]);
		return ok(res, { grant_id: wsApps[2], revoked: true });
	}
	if (path === '/api/v1/admin/oauth-clients' && req.method === 'GET') {
		return ok(res, {
			items: [
				{
					client_id: 'ramflux',
					display_name: 'Ramflux',
					description: 'Ramflux desktop',
					homepage_url: null,
					client_type: 'public',
					redirect_uris: [],
					allowed_scopes: [],
					allowed_resources: [],
					status: m.clientStatus,
					status_changed_at: null,
					created_at: minutesAgo(5000),
					active_grants: m.clientStatus === 'active' ? m.clientGrants : 0
				}
			]
		});
	}
	const client = /^\/api\/v1\/admin\/oauth-clients\/([^/]+)$/.exec(path);
	if (client && req.method === 'PATCH') {
		const body = await readBody(req);
		m.clientPatches.push(body);
		const revoked = body.status === 'disabled' && m.clientStatus === 'active' ? m.clientGrants : 0;
		m.clientStatus = body.status;
		return ok(res, { client_id: client[1], status: m.clientStatus, revoked_grants: revoked });
	}

	// --- application shell ----------------------------------------------------------------
	if (path === '/api/v1/workspaces' && req.method === 'GET') {
		return ok(
			res,
			paginated([
				{
					id: 'ws-acme',
					slug: 'acme',
					name: 'Acme',
					description: '',
					created_at: minutesAgo(9000),
					updated_at: minutesAgo(9000)
				}
			])
		);
	}
	const ws = /^\/api\/v1\/workspaces\/([^/]+)$/.exec(path);
	if (ws && req.method === 'GET') {
		return ok(res, {
			id: ws[1],
			slug: 'acme',
			name: 'Acme',
			description: '',
			created_at: minutesAgo(9000),
			updated_at: minutesAgo(9000)
		});
	}
	if (/^\/api\/v1\/workspaces\/[^/]+\/members$/.test(path)) {
		return ok(
			res,
			paginated([
				{
					user_id: user.id,
					workspace_id: 'ws-acme',
					email: user.email,
					name: user.name,
					role: 'owner',
					joined_at: minutesAgo(9000)
				}
			])
		);
	}
	if (path === '/api/v1/notifications/unread-count') return ok(res, { count: 0 });

	if (req.method === 'GET') {
		m.unhandled.push(`${req.method} ${path}`);
		return ok(res, paginated([]));
	}
	m.unhandled.push(`${req.method} ${path}`);
	return fail(res, 404, 'not_found');
}

const appServer = createServer((req, res) => {
	const url = new URL(req.url ?? '/', APP);
	if (url.pathname === '/__mock/reset' && req.method === 'POST') {
		readBody(req).then((overrides) => {
			m = { ...fresh(), ...overrides };
			ok(res, {});
		});
		return;
	}
	if (url.pathname === '/__mock/state')
		return json(res, 200, { ...m, requests: [...m.requests.values()] });
	if (url.pathname === '/__mock/config' && req.method === 'POST') {
		readBody(req).then((patch) => {
			if (patch.expireAll) for (const stored of m.requests.values()) stored.expired = true;
			if ('failDecision' in patch) m.failDecision = patch.failDecision;
			if ('workspaces' in patch) m.workspaces = patch.workspaces;
			if ('failListOnce' in patch) m.failListOnce = patch.failListOnce;
			ok(res, {});
		});
		return;
	}
	const handler = url.pathname.startsWith('/api/')
		? handleApi(req, res, url)
		: serveStatic(req, res);
	handler.catch((error) => {
		console.error('mock server error', error);
		if (!res.headersSent) json(res, 500, { code: 500, message: error.message, data: null });
	});
});

// The application's return address: a loopback page that records what it was asked for.
const stubHits = [];
const stubServer = createServer((req, res) => {
	stubHits.push(req.url ?? '');
	res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
	res.end('<!doctype html><title>Return</title><p id="stub">You can return to Ramflux</p>');
});

await new Promise((done) => appServer.listen(APP_PORT, '127.0.0.1', done));
await new Promise((done) => stubServer.listen(STUB_PORT, '127.0.0.1', done));
const stopServers = () => {
	appServer.close();
	stubServer.close();
};
process.on('exit', stopServers);

const control = (path, body = {}) =>
	fetch(`${APP}${path}`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify(body)
	});
const mockState = async () => (await fetch(`${APP}/__mock/state`)).json();

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

const failures = [];
let passed = 0;
const problems = [];
const shotsTaken = [];

async function check(label, body) {
	const before = problems.length;
	try {
		await body();
		await new Promise((done) => setTimeout(done, 100));
		if (problems.length > before)
			throw new Error(`page problems:\n${problems.slice(before).join('\n')}`);
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
function equal(actual, expected, message) {
	if (JSON.stringify(actual) !== JSON.stringify(expected))
		throw new Error(
			`${message}\n  expected: ${JSON.stringify(expected)}\n  actual:   ${JSON.stringify(actual)}`
		);
}

const browser = await chromium.launch({ executablePath: chromiumPath, headless: true });
process.on('exit', () => {
	browser.close().catch(() => {});
});

const QUERY = (extra = {}) => {
	const params = new URLSearchParams({
		response_type: 'code',
		client_id: 'ramflux',
		redirect_uri: REDIRECT_URI,
		scope: 'projects:read work_items:read notifications:read tasks:read offline_access',
		state: STATE,
		code_challenge: 'x'.repeat(43),
		code_challenge_method: 'S256',
		device_name: 'MacBook',
		...extra
	});
	params.append('resource', `${APP}/api`);
	params.append('resource', `${APP}/mcp`);
	return `?${params.toString()}`;
};

/**
 * A browser context and page. `signedIn` seeds the access token; `allow` lists console errors a
 * scenario expects (a refused request logs one) as regular expressions.
 */
async function session(
	name,
	{ signedIn = true, locale = 'en', theme = 'light', width = 1440, height = 1000, allow = [] } = {}
) {
	const context = await browser.newContext({
		viewport: { width, height },
		locale: locale === 'zh' ? 'zh-CN' : 'en-US',
		timezoneId: 'UTC',
		reducedMotion: 'reduce'
	});
	await context.addInitScript(
		({ signedIn, locale, theme, user, token }) => {
			if (signedIn) {
				localStorage.setItem('auth_token', token);
				localStorage.setItem('refresh_token', 'smoke-refresh-token');
				localStorage.setItem('auth_user', JSON.stringify(user));
			}
			localStorage.setItem('locale', locale);
			localStorage.setItem('theme', theme);
		},
		{ signedIn, locale, theme, user, token: TOKEN }
	);
	const page = await context.newPage();
	page.setDefaultTimeout(T);
	const requests = [];
	const navigations = [];
	page.on('pageerror', (error) => problems.push(`${name}: page error: ${error.message}`));
	page.on('console', (message) => {
		if (message.type() !== 'error') return;
		if (allow.some((pattern) => pattern.test(message.text()))) return;
		problems.push(`${name}: console error: ${message.text()}`);
	});
	page.on('request', (request) => requests.push(`${request.method()} ${request.url()}`));
	page.on('framenavigated', (frame) => {
		if (frame === page.mainFrame()) navigations.push(frame.url());
	});
	return { page, context, requests, navigations };
}

async function shot(page, name) {
	if (!SHOTS) return;
	const original = page.viewportSize();
	const dark = await page.evaluate(() => localStorage.getItem('theme') === 'dark');
	const mobile = original.width <= 500;
	await page
		.evaluate(() =>
			document
				.querySelector('section[data-testid^="connected-apps-"]')
				?.scrollIntoView({ block: 'start' })
		)
		.catch(() => {});
	const height = await page.evaluate(() => document.documentElement.scrollHeight);
	await page.setViewportSize({ width: original.width, height: Math.max(height, 400) });
	const file = join(SHOTS, `${name}-${mobile ? '390' : '1440'}-${dark ? 'dark' : 'light'}.png`);
	await page.screenshot({ path: file });
	shotsTaken.push(file);
	await page.setViewportSize(original);
}

/** Screenshots of the current state at 1440 light and 390 dark, then restores both. */
async function bothShots(s, name) {
	if (!SHOTS) return;
	const { page } = s;
	await shot(page, name);
	await page.setViewportSize({ width: 390, height: 844 });
	await page.waitForTimeout(300);
	await page.evaluate(() => {
		localStorage.setItem('theme', 'dark');
		document.documentElement.classList.add('dark');
		document.documentElement.setAttribute('data-theme', 'dark');
	});
	await shot(page, name);
	await page.evaluate(() => {
		localStorage.setItem('theme', 'light');
		document.documentElement.classList.remove('dark');
		document.documentElement.removeAttribute('data-theme');
	});
	await page.setViewportSize({ width: 1440, height: 1000 });
}

const noOverflow = async (page, label) => {
	const overflow = await page.evaluate(
		() => document.documentElement.scrollWidth - document.documentElement.clientWidth
	);
	assert(overflow <= 0, `${label}: horizontal overflow of ${overflow}px`);
};

const tid = (page, id) => page.locator(`[data-testid="${id}"]`);
const dialog = (page) => page.locator('[data-testid="confirm-dialog"]');

async function openConsent(s, extra = {}) {
	await s.page.goto(`${APP}/oauth/authorize${QUERY(extra)}`, { waitUntil: 'load' });
	await s.page.waitForSelector('[data-testid="consent-page"]:not([data-view="loading"])', {
		timeout: 20_000
	});
}

async function finishConsentChoices(page, { projects = 'all' } = {}) {
	await tid(page, 'consent-workspace-ws-acme').check();
	await tid(
		page,
		projects === 'all' ? 'consent-projects-all' : 'consent-projects-selected'
	).check();
}

const callbackOf = (url) => {
	const parsed = new URL(url);
	return {
		origin: parsed.origin,
		path: parsed.pathname,
		params: Object.fromEntries(parsed.searchParams.entries())
	};
};

async function reset(overrides = {}) {
	await control('/__mock/reset', overrides);
	stubHits.length = 0;
}

// ---------------------------------------------------------------------------------------------
// Consent page
// ---------------------------------------------------------------------------------------------

console.log('consent page: allow');

await check(
	'allow with all projects navigates to the mocked redirect_to and leaks nothing',
	async () => {
		await reset();
		const s = await session('allow-all');
		await openConsent(s);
		const page = s.page;
		equal(await tid(page, 'consent-title').innerText(), 'Connect Ramflux to Sylvode', 'title');
		const origin = await tid(page, 'consent-origin').innerText();
		assert(
			origin.includes('Requested by') && origin.includes('Ramflux on MacBook'),
			`origin: ${origin}`
		);
		assert(origin.includes('Returns to 127.0.0.1 after you decide'), `host hint: ${origin}`);
		assert(origin.includes('The device name was supplied by the application'), 'device hint');
		const account = await tid(page, 'consent-account').innerText();
		assert(account.includes('Signed in as A. User (a@example.com)'), `account: ${account}`);
		assert(await tid(page, 'consent-not-you').isVisible(), 'not you');

		// Required scopes are checked, disabled and say why; Allow starts disabled with its reason.
		for (const name of [
			'projects:read',
			'work_items:read',
			'notifications:read',
			'tasks:read',
			'offline_access'
		]) {
			const box = tid(page, `consent-scope-${name}`);
			assert(await box.isChecked(), `${name} checked`);
			assert(await box.isDisabled(), `${name} disabled`);
		}
		const access = await page.getByRole('group', { name: 'Access requested' }).innerText();
		assert(access.includes('See projects and their details'), 'scope label');
		assert(access.includes('Stay connected until you disconnect'), 'offline_access label');
		assert(access.includes('Required by Ramflux'), 'required reason');
		assert(await tid(page, 'consent-allow').isDisabled(), 'allow starts disabled');
		const reason = await tid(page, 'consent-allow').locator('xpath=ancestor::span[1]').innerText();
		assert(
			reason.includes('Choose a workspace and which projects Ramflux can reach'),
			`reason: ${reason}`
		);
		// An ineligible workspace is dimmed, disabled and gives its reason at full contrast.
		assert(await tid(page, 'consent-workspace-ws-lab').isDisabled(), 'ineligible disabled');
		assert(
			(await page.getByRole('group', { name: 'Workspace', exact: true }).innerText()).includes(
				'Your workspace administrator does not allow connected applications'
			),
			'ineligible reason'
		);
		await bothShots(s, 'consent-initial');

		await finishConsentChoices(page);
		assert(await tid(page, 'consent-allow').isEnabled(), 'allow enabled after the choices');
		await bothShots(s, 'consent-ready');
		const token = (await mockState()).requests[0].token;
		assert(typeof token === 'string' && token.length === 64, 'the mock minted a token');
		assert(!page.url().includes(token), 'token not in the address');
		assert(!page.url().includes('code_challenge'), 'the original query is not kept in the address');
		assert(/\/oauth\/authorize\?request=[0-9a-f-]{36}$/.test(page.url()), `address: ${page.url()}`);

		await tid(page, 'consent-allow').click();
		await page.waitForURL(`${STUB}/**`);
		const landed = callbackOf(page.url());
		equal(
			landed.origin + landed.path,
			REDIRECT_URI,
			'navigated to the redirect_to of the decision'
		);
		assert(/^svc_[c]{64}$/.test(landed.params.code), 'code');
		equal(landed.params.state, STATE, 'state');
		equal(landed.params.iss, APP, 'iss');
		// Exactly one visit to the stub, and the consent token is nowhere it could leak from.
		equal(stubHits.length, 1, 'one stub request');
		assert(!stubHits.some((hit) => hit.includes(token)), 'token not in the callback');
		assert(!s.navigations.some((url) => url.includes(token)), 'token not in any navigation');
		assert(!s.requests.some((entry) => entry.includes(token)), 'token not in any request URL');
		const decisions = (await mockState()).decisions;
		equal(decisions.length, 1, 'one decision');
		equal(decisions[0].token, token, 'token sent in the header');
		equal(
			decisions[0].body,
			{
				decision: 'allow',
				workspace_id: 'ws-acme',
				project_mode: 'all',
				scopes: [
					'projects:read',
					'work_items:read',
					'notifications:read',
					'tasks:read',
					'offline_access'
				]
			},
			'decision body'
		);
		assert(!JSON.stringify(decisions[0].body).includes(token), 'token not in the body');
		await s.context.close();
	}
);

await check('selected projects and an optional scope end in the decision body', async () => {
	await reset();
	const s = await session('allow-selected');
	await openConsent(s, {
		scope: 'projects:read work_items:read work_items:write offline_access'
	});
	const page = s.page;
	const write = tid(page, 'consent-scope-work_items:write');
	assert(!(await write.isChecked()) && (await write.isEnabled()), 'optional scope starts unticked');
	await tid(page, 'consent-workspace-ws-acme').check();
	await tid(page, 'consent-projects-selected').check();
	assert(await tid(page, 'consent-project-picker').isVisible(), 'picker shown');
	await page.waitForSelector('[data-testid="consent-project-select"]');
	assert(await tid(page, 'consent-allow').isDisabled(), 'allow blocked with no project');
	const reason = await tid(page, 'consent-allow').locator('xpath=ancestor::span[1]').innerText();
	assert(reason.includes('Choose which projects Ramflux can reach'), `reason: ${reason}`);
	await tid(page, 'consent-project-select').click();
	await page.getByRole('option', { name: /Alpha/ }).click();
	await page.getByRole('option', { name: /Gamma/ }).click();
	await page.keyboard.press('Escape');
	assert(await tid(page, 'consent-allow').isEnabled(), 'allow enabled after projects');
	await write.check();
	await bothShots(s, 'consent-selected-projects');
	await tid(page, 'consent-allow').click();
	await page.waitForURL(`${STUB}/**`);
	const body = (await mockState()).decisions[0].body;
	equal(body.project_mode, 'selected', 'mode');
	equal([...body.project_ids].sort(), ['p-alpha', 'p-gamma'], 'projects');
	assert(body.scopes.includes('work_items:write'), 'optional scope included');
	await s.context.close();
});

await check('changing the workspace clears the chosen projects', async () => {
	await reset();
	const s = await session('workspace-change');
	await openConsent(s);
	const page = s.page;
	await tid(page, 'consent-workspace-ws-acme').check();
	await tid(page, 'consent-projects-selected').check();
	await tid(page, 'consent-project-select').click();
	await page.getByRole('option', { name: /Beta/ }).click();
	await page.keyboard.press('Escape');
	assert(await tid(page, 'consent-allow').isEnabled(), 'enabled with a project');
	await tid(page, 'consent-workspace-ws-solo').check();
	await page.waitForSelector('[data-testid="consent-projects-empty"]');
	assert(await tid(page, 'consent-allow').isDisabled(), 'blocked after the switch');
	await s.context.close();
});

await check('a double click sends one decision', async () => {
	await reset();
	const s = await session('double');
	await openConsent(s);
	await finishConsentChoices(s.page);
	const allow = tid(s.page, 'consent-allow');
	await allow.dblclick();
	await s.page.waitForURL(`${STUB}/**`);
	equal((await mockState()).decisions.length, 1, 'one decision');
	await s.context.close();
});

console.log('consent page: deny and no account');

await check('Cancel sends deny and lands on the stub with access_denied', async () => {
	await reset();
	const s = await session('deny');
	await openConsent(s);
	await tid(s.page, 'consent-deny').click();
	await s.page.waitForURL(`${STUB}/**`);
	const landed = callbackOf(s.page.url());
	equal(landed.params.error, 'access_denied', 'error');
	equal(landed.params.error_description, 'consent_denied', 'description');
	equal(landed.params.state, STATE, 'state');
	equal((await mockState()).decisions[0].body, { decision: 'deny' }, 'body');
	await s.context.close();
});

await check('signed out: the sign-in prompt and "I don\'t have a Sylvode account"', async () => {
	await reset();
	const s = await session('no-account', { signedIn: false });
	await openConsent(s);
	const page = s.page;
	equal(await page.locator('h1').innerText(), 'Sign in to Sylvode to connect Ramflux', 'heading');
	const text = await tid(page, 'consent-login').innerText();
	assert(
		text.includes('No Sylvode account? Accounts are created by your Sylvode administrator.'),
		'hint'
	);
	assert(await tid(page, 'consent-login-button').isVisible(), 'sign in button');
	await bothShots(s, 'consent-login');
	await tid(page, 'consent-no-account').click();
	await page.waitForURL(`${STUB}/**`);
	const landed = callbackOf(page.url());
	equal(landed.params.error, 'access_denied', 'error');
	equal(landed.params.error_description, 'account_required', 'description');
	const decision = (await mockState()).decisions[0];
	equal(decision.body, { decision: 'no_account' }, 'body');
	equal(decision.token, null, 'no consent token');
	equal(decision.authed, false, 'no session');
	await s.context.close();
});

console.log('consent page: errors without navigation');

await check('an unknown application shows the error page and navigates nowhere', async () => {
	await reset();
	const s = await session('unknown-client');
	await openConsent(s, { client_id: 'other' });
	const page = s.page;
	equal(await page.locator('h1').innerText(), 'This connection request is not valid', 'title');
	const text = await tid(page, 'consent-error-page').innerText();
	assert(
		text.includes(
			'The application or its return address is not registered with this Sylvode. Nothing was shared.'
		),
		`body: ${text}`
	);
	assert((await page.locator('button, a').count()) === 0, 'no button and no link on the page');
	equal(stubHits.length, 0, 'the stub was never asked');
	assert(!s.navigations.some((url) => url.startsWith(STUB)), 'no navigation to the return address');
	assert(page.url().startsWith(`${APP}/oauth/authorize`), 'still on the consent route');
	await bothShots(s, 'consent-invalid');
	await s.context.close();
});

await check('a return address that does not match shows the same page', async () => {
	await reset();
	const s = await session('mismatch');
	await openConsent(s, { redirect_uri: 'https://evil.example/cb' });
	equal(await s.page.locator('h1').innerText(), 'This connection request is not valid', 'title');
	equal(stubHits.length, 0, 'the stub was never asked');
	assert(
		!s.navigations.some((url) => new URL(url).hostname === 'evil.example'),
		'no navigation to the address'
	);
	await s.context.close();
});

await check('a repeated redirect_uri is refused locally with no API call', async () => {
	await reset();
	const s = await session('duplicate');
	const query = `${QUERY()}&redirect_uri=${encodeURIComponent('https://evil.example/cb')}`;
	await s.page.goto(`${APP}/oauth/authorize${query}`, { waitUntil: 'load' });
	await s.page.waitForSelector('[data-testid="consent-error-page"]');
	equal((await mockState()).authorizeCalls.length, 0, 'nothing was sent');
	await s.context.close();
});

await check('query values are never rendered as HTML', async () => {
	await reset();
	const s = await session('xss');
	await openConsent(s, {
		device_name: '<img src=x onerror=alert(1)>',
		state: '<script>1</script>'
	});
	const html = await s.page.locator('[data-testid="consent-origin"]').innerHTML();
	assert(!html.includes('<img'), `device name was not escaped: ${html}`);
	equal(await s.page.locator('img[src="x"]').count(), 0, 'no injected element');
	await s.context.close();
});

await check('a valid redirect error from the API is followed to the return address', async () => {
	await reset();
	const s = await session('redirect-error');
	await s.page.goto(`${APP}/oauth/authorize${QUERY({ scope: 'bad_scope' })}`);
	await s.page.waitForURL(`${STUB}/**`);
	equal(callbackOf(s.page.url()).params.error, 'invalid_scope', 'error');
	await s.context.close();
});

await check('the page cannot be shown in a frame', async () => {
	await reset();
	const s = await session('framed');
	await s.page.goto(`${APP}/health-placeholder-not-found`, { waitUntil: 'load' }).catch(() => {});
	await s.page.evaluate((src) => {
		const frame = document.createElement('iframe');
		frame.id = 'probe';
		frame.src = src;
		document.body.append(frame);
	}, `${APP}/oauth/authorize${QUERY()}`);
	const frame = await (await s.page.waitForSelector('#probe')).contentFrame();
	await frame.waitForSelector('[data-testid="consent-framed"]');
	equal(
		(await frame.locator('[data-testid="consent-framed"]').innerText()).trim(),
		'This page cannot be shown inside another page',
		'message'
	);
	equal((await mockState()).authorizeCalls.length, 0, 'no API call from a frame');
	await s.context.close();
});

console.log('consent page: expiry and refusals');

await check('an expired request shows the expired state', async () => {
	await reset();
	const s = await session('expired', { allow: [/status of 404/] });
	await openConsent(s);
	await finishConsentChoices(s.page);
	await control('/__mock/config', { expireAll: true });
	await tid(s.page, 'consent-allow').click();
	await s.page.waitForSelector('[data-testid="consent-expired"]');
	const text = await tid(s.page, 'consent-expired').innerText();
	assert(text.includes('This connection request has expired'), text);
	assert(text.includes('Start the connection again from Ramflux.'), text);
	equal(stubHits.length, 0, 'no navigation');
	await bothShots(s, 'consent-expired');
	await s.context.close();
});

await check('a request that was already used shows the expired state', async () => {
	await reset();
	const s = await session('used', { allow: [/status of 409/] });
	await openConsent(s);
	await finishConsentChoices(s.page);
	await control('/__mock/config', {
		failDecision: { status: 409, error_code: 'already_decided' }
	});
	await tid(s.page, 'consent-allow').click();
	await s.page.waitForSelector('[data-testid="consent-expired"]');
	await s.context.close();
});

await check('a consent token the server refuses reloads the request and says so', async () => {
	await reset();
	const s = await session('stale-token', { allow: [/status of 403/] });
	await openConsent(s);
	await finishConsentChoices(s.page);
	await control('/__mock/config', {
		failDecision: { status: 403, error_code: 'consent_token_invalid' }
	});
	await tid(s.page, 'consent-allow').click();
	await s.page.waitForSelector('[data-testid="consent-error"]');
	const text = await tid(s.page, 'consent-error').innerText();
	assert(text.includes('out of date'), `message: ${text}`);
	// Focus moved to the message, and the page works again with the new token.
	const focused = await s.page.evaluate(() => document.activeElement?.getAttribute('data-testid'));
	equal(focused, 'consent-error', 'focus is on the error');
	await finishConsentChoices(s.page);
	await tid(s.page, 'consent-allow').click();
	await s.page.waitForURL(`${STUB}/**`);
	const state = await mockState();
	equal(state.authorizeCalls.length, 2, 'the request was read again');
	assert(state.authorizeCalls[1].body.request_id, 'by id');
	await s.context.close();
});

await check('an application disabled meanwhile blocks Allow with its reason', async () => {
	await reset();
	const s = await session('disabled', { allow: [/status of 403/] });
	await openConsent(s);
	await finishConsentChoices(s.page);
	await control('/__mock/config', {
		failDecision: { status: 403, error_code: 'client_not_active' }
	});
	await tid(s.page, 'consent-allow').click();
	await s.page.waitForSelector('[data-testid="consent-error"]');
	assert(await tid(s.page, 'consent-allow').isDisabled(), 'allow disabled');
	const reason = await tid(s.page, 'consent-allow').locator('xpath=ancestor::span[1]').innerText();
	assert(reason.includes('Ramflux cannot be connected right now'), `reason: ${reason}`);
	await s.context.close();
});

await check('too many connections lists them and offers the way to disconnect', async () => {
	await reset();
	const s = await session('too-many', { allow: [/status of 409/] });
	await openConsent(s);
	await finishConsentChoices(s.page);
	await control('/__mock/config', {
		failDecision: {
			status: 409,
			error_code: 'too_many_grants',
			details: {
				max_active_grants: 10,
				grants: [
					{
						grant_id: 'g1',
						device_name: 'MacBook',
						workspace_id: 'ws-acme',
						consented_at: minutesAgo(5)
					},
					{
						grant_id: 'g2',
						device_name: null,
						workspace_id: 'ws-solo',
						consented_at: minutesAgo(3)
					}
				]
			}
		}
	});
	await tid(s.page, 'consent-allow').click();
	await s.page.waitForSelector('[data-testid="consent-too-many"]');
	const text = await tid(s.page, 'consent-too-many').innerText();
	assert(text.includes('You already have 10 Ramflux connections'), text);
	assert(text.includes('MacBook · Acme') && text.includes('Unnamed device · Solo'), text);
	equal(await tid(s.page, 'consent-too-many-item').count(), 2, 'both listed');
	equal(await tid(s.page, 'consent-open-settings').getAttribute('href'), '/settings', 'link');
	await bothShots(s, 'consent-too-many');
	// The request is still valid: trying again sends the same decision.
	await tid(s.page, 'consent-too-many-retry').click();
	await s.page.waitForURL(`${STUB}/**`);
	await s.context.close();
});

await check('no eligible workspace offers only the way back', async () => {
	await reset({
		workspaces: [{ id: 'ws-lab', name: 'Lab', eligible: false, reason: 'workspace_disallows' }]
	});
	const s = await session('none-eligible');
	await openConsent(s);
	const page = s.page;
	const text = await tid(page, 'consent-no-eligible').innerText();
	assert(text.includes('None of your workspaces allows connected applications'), text);
	equal(await tid(page, 'consent-allow').count(), 0, 'no Allow button');
	await bothShots(s, 'consent-no-eligible');
	await tid(page, 'consent-return').click();
	await page.waitForURL(`${STUB}/**`);
	equal(callbackOf(page.url()).params.error_description, 'no_eligible_workspace', 'description');
	await s.context.close();
});

await check('"Not you?" signs out and shows the sign-in prompt for the same request', async () => {
	await reset();
	const s = await session('not-you');
	await openConsent(s);
	const id = (await mockState()).requests[0].id;
	// After the sign-out the mock sees no session: the browser's token is cleared by the page.
	await tid(s.page, 'consent-not-you').click();
	await s.page.waitForSelector('[data-testid="consent-login"]');
	assert(s.page.url().endsWith(`/oauth/authorize?request=${id}`), `address: ${s.page.url()}`);
	const last = (await mockState()).authorizeCalls.at(-1);
	equal(last.body, { request_id: id }, 'reloaded by id');
	equal(last.authed, false, 'without a session');
	await s.context.close();
});

console.log('sign-in return');

await check(
	'signed out: the visit goes to login and comes back to the stored request',
	async () => {
		await reset();
		const s = await session('login-return', { signedIn: false });
		await openConsent(s);
		const page = s.page;
		await tid(page, 'consent-login-button').click();
		await page.waitForURL(/\/auth\/login\?return_to=/);
		const returnTo = new URL(page.url()).searchParams.get('return_to');
		assert(
			/^\/oauth\/authorize\?request=[0-9a-f-]{36}$/.test(returnTo ?? ''),
			`return_to: ${returnTo}`
		);
		await page.fill('#email', 'a@example.com');
		await page.fill('#password', 'correct-horse');
		await page.click('button[type="submit"]');
		await page.waitForSelector('[data-testid="consent-form"]');
		assert(page.url().endsWith(returnTo), `back on the request: ${page.url()}`);
		equal(
			await tid(page, 'consent-title').innerText(),
			'Connect Ramflux to Sylvode',
			'consent shown'
		);
		const calls = (await mockState()).authorizeCalls;
		equal(
			calls.at(-1).body,
			{ request_id: returnTo.split('=')[1] },
			'reloaded by id, the query is not replayed'
		);
		equal(calls.at(-1).authed, true, 'with the new session');
		await s.context.close();
	}
);

for (const hostile of [
	'https://evil.example/',
	'//evil.example/oauth/authorize',
	'/\\evil.example',
	'javascript:alert(1)',
	'/oauth/authorize?request=abc',
	'/oauth/authorize?request=0b9c1b52-6a0e-4f6e-9a55-3a1f6a0d2c11&next=https://evil.example'
]) {
	await check(`a hostile return_to is ignored: ${hostile}`, async () => {
		await reset();
		const s = await session('hostile', { signedIn: false });
		await s.page.goto(`${APP}/auth/login?return_to=${encodeURIComponent(hostile)}`, {
			waitUntil: 'load'
		});
		await s.page.fill('#email', 'a@example.com');
		await s.page.fill('#password', 'correct-horse');
		await s.page.click('button[type="submit"]');
		await s.page.waitForURL(`${APP}/workspace**`);
		assert(
			!s.navigations.some((url) => !url.startsWith(APP) && url !== 'about:blank'),
			`left the origin: ${s.navigations.join(' ')}`
		);
		assert(!s.navigations.some((url) => url.includes('/oauth/authorize')), 'did not follow it');
		await s.context.close();
	});
}

// ---------------------------------------------------------------------------------------------
// Connected applications
// ---------------------------------------------------------------------------------------------

console.log('connected applications: personal');

await check('personal settings: list, details, disconnect with confirmation', async () => {
	await reset();
	const s = await session('personal');
	const page = s.page;
	await page.goto(`${APP}/settings`, { waitUntil: 'load' });
	await page.waitForSelector('[data-testid="connected-app-row"]');
	const rows = tid(page, 'connected-app-row');
	equal(await rows.count(), 2, 'two rows');
	const first = await rows.nth(0).innerText();
	assert(first.includes('Ramflux · MacBook'), first);
	assert(first.includes('Acme workspace · 3 projects · Last used 10 minutes ago'), first);
	assert(first.includes('Read'), first);
	const second = await rows.nth(1).innerText();
	assert(second.includes('Solo workspace · All projects · Not used yet'), second);
	assert(second.includes('Read and change'), second);
	equal(await rows.nth(1).locator('[data-testid="connected-app-disconnect"]').count(), 1, 'testid');

	// Details expand with the §14.1 scope labels and the projects.
	const toggle = rows.nth(0).locator('[data-testid="connected-app-details"]');
	equal(await toggle.getAttribute('aria-expanded'), 'false', 'collapsed');
	await toggle.click();
	equal(await toggle.getAttribute('aria-expanded'), 'true', 'expanded');
	const details = await rows
		.nth(0)
		.locator('[data-testid="connected-app-details-panel"]')
		.innerText();
	assert(details.includes('See projects and their details'), details);
	assert(details.includes('Alpha') && details.includes('and 1 more'), details);
	assert(details.includes('Connected on') && details.includes('Expires'), details);
	await bothShots(s, 'settings-personal');

	// Cancel keeps the row; confirm removes it.
	await rows.nth(0).locator('[data-testid="connected-app-disconnect"]').click();
	await dialog(page).waitFor();
	const title = await dialog(page).innerText();
	assert(title.includes('Disconnect Ramflux · MacBook?'), title);
	assert(title.includes('Ramflux will lose access to the Acme workspace at once'), title);
	assert(title.includes('Your Sylvode data is not changed.'), title);
	await bothShots(s, 'settings-personal-dialog');
	await dialog(page).getByRole('button', { name: 'Cancel' }).click();
	await dialog(page).waitFor({ state: 'detached' });
	equal(await rows.count(), 2, 'cancel keeps both rows');
	equal((await mockState()).ownDeletes.length, 0, 'nothing was deleted');

	await rows.nth(0).locator('[data-testid="connected-app-disconnect"]').click();
	await dialog(page).getByRole('button', { name: 'Disconnect Ramflux' }).click();
	await page.waitForSelector('[data-testid="connected-apps-personal-result"]');
	equal(await rows.count(), 1, 'one row left');
	assert(
		(await tid(page, 'connected-apps-personal-result').innerText()).includes(
			'Ramflux · MacBook was disconnected'
		),
		'result message'
	);
	equal((await mockState()).ownDeletes, ['g-own-1'], 'the API was asked to revoke it');

	await rows.nth(0).locator('[data-testid="connected-app-disconnect"]').click();
	await dialog(page).getByRole('button', { name: 'Disconnect Ramflux' }).click();
	await page.waitForSelector('[data-testid="connected-apps-personal-empty"]');
	assert(
		(await tid(page, 'connected-apps-personal-empty').innerText()).includes(
			'No applications are connected to your account'
		),
		'empty state'
	);
	await bothShots(s, 'settings-personal-empty');
	await s.context.close();
});

await check('personal settings: a failed load is shown inline with Retry', async () => {
	await reset({ failListOnce: true });
	const s = await session('personal-failed', { allow: [/status of 500/] });
	await s.page.goto(`${APP}/settings`, { waitUntil: 'load' });
	await s.page.waitForSelector('[data-testid="connected-apps-personal-error"]');
	const text = await tid(s.page, 'connected-apps-personal-error').innerText();
	assert(text.includes('Retry'), text);
	await bothShots(s, 'settings-personal-failed');
	await s.page.getByRole('button', { name: 'Retry' }).click();
	await s.page.waitForSelector('[data-testid="connected-app-row"]');
	await s.context.close();
});

console.log('connected applications: workspace and instance');

await check(
	'workspace settings: switch off with the count, table, disconnect one, switch on',
	async () => {
		await reset();
		const s = await session('workspace');
		const page = s.page;
		await page.goto(`${APP}/workspace/ws-acme/settings`, { waitUntil: 'load' });
		await page.waitForSelector('[data-testid="workspace-app-row"]');
		const rows = tid(page, 'workspace-app-row');
		equal(await rows.count(), 4, 'four rows');
		const header = await tid(page, 'connected-apps-workspace-table').innerText();
		for (const column of ['Member', 'Application', 'Projects', 'Access', 'Last used'])
			assert(header.includes(column), `column ${column}`);
		assert((await rows.nth(0).innerText()).includes('Mei Tan'), 'member');
		await bothShots(s, 'settings-workspace');

		// Disconnect one member's connection.
		await rows.nth(1).locator('[data-testid="workspace-app-disconnect"]').click();
		await dialog(page).waitFor();
		const one = await dialog(page).innerText();
		assert(one.includes('Disconnect Ramflux for Ana Silva?'), one);
		await dialog(page).getByRole('button', { name: 'Disconnect Ramflux' }).click();
		await page.waitForSelector('[data-testid="connected-apps-workspace-result"]');
		equal(await rows.count(), 3, 'three rows left');
		equal((await mockState()).wsDeletes, ['g-w-2'], 'the API was asked to revoke it');

		// Switch off: the dialog states the count the API holds, cancel changes nothing.
		const toggle = tid(page, 'connected-apps-allow-switch');
		assert(await toggle.isChecked(), 'on');
		await toggle.click();
		await dialog(page).waitFor();
		const off = await dialog(page).innerText();
		assert(off.includes('Turn off connected applications for Acme?'), off);
		assert(off.includes('All 3 active connections in this workspace end at once'), off);
		await bothShots(s, 'settings-workspace-dialog');
		await dialog(page).getByRole('button', { name: 'Cancel' }).click();
		await dialog(page).waitFor({ state: 'detached' });
		assert(await toggle.isChecked(), 'still on after cancel');
		equal((await mockState()).wsPuts.length, 0, 'no PUT');

		await toggle.click();
		await dialog(page).getByRole('button', { name: 'Turn off and disconnect all' }).click();
		await page.waitForSelector('[data-testid="connected-apps-workspace-off"]');
		assert(!(await toggle.isChecked()), 'off');
		equal((await mockState()).wsPuts, [{ allow_connected_apps: false }], 'PUT body');
		assert(
			(await tid(page, 'connected-apps-workspace-result').innerText()).includes(
				'3 connections were ended'
			),
			'the result uses the count the API returned'
		);
		equal(await rows.count(), 0, 'table gone');
		await bothShots(s, 'settings-workspace-off');

		// Switch on: no confirmation.
		await toggle.click();
		await page.waitForSelector('[data-testid="connected-apps-workspace-empty"]');
		assert(await toggle.isChecked(), 'on again');
		equal((await mockState()).wsPuts.at(-1), { allow_connected_apps: true }, 'PUT body');
		equal(await dialog(page).count(), 0, 'no dialog for switching on');
		await s.context.close();
	}
);

await check(
	'instance settings: disable the client with the count, enable without a dialog',
	async () => {
		await reset();
		const s = await session('admin');
		const page = s.page;
		await page.goto(`${APP}/admin/settings`, { waitUntil: 'load' });
		await page.waitForSelector('[data-testid="oauth-client-row"]');
		const row = tid(page, 'oauth-client-row');
		let text = await row.innerText();
		assert(
			text.includes('Ramflux') && text.includes('Active') && text.includes('7 active connections'),
			text
		);
		await bothShots(s, 'settings-admin');
		await tid(page, 'oauth-client-disable').click();
		await dialog(page).waitFor();
		const body = await dialog(page).innerText();
		assert(body.includes('Disable Ramflux for this Sylvode?'), body);
		assert(
			body.includes(
				'All 7 connections end at once. Nobody can connect Ramflux until it is enabled again.'
			),
			body
		);
		await bothShots(s, 'settings-admin-dialog');
		await dialog(page).getByRole('button', { name: 'Disable Ramflux' }).click();
		await page.waitForSelector('[data-testid="oauth-client-enable"]');
		text = await row.innerText();
		assert(text.includes('Disabled') && text.includes('0 active connections'), text);
		assert(
			(await tid(page, 'connected-apps-admin-result').innerText()).includes(
				'7 connections were ended'
			),
			'count from the API'
		);
		equal((await mockState()).clientPatches, [{ status: 'disabled' }], 'PATCH body');
		await tid(page, 'oauth-client-enable').click();
		await page.waitForSelector('[data-testid="oauth-client-disable"]');
		equal(await dialog(page).count(), 0, 'no dialog for enabling');
		equal((await mockState()).clientPatches.at(-1), { status: 'active' }, 'PATCH body');
		await s.context.close();
	}
);

// ---------------------------------------------------------------------------------------------
// Chinese strings, layout, keyboard
// ---------------------------------------------------------------------------------------------

console.log('Chinese strings, layout and keyboard');

await check('the Chinese locale shows the section 14 strings', async () => {
	await reset();
	const s = await session('zh', { locale: 'zh', allow: [] });
	const page = s.page;
	await openConsent(s, { scope: 'projects:read work_items:write offline_access' });
	equal(await tid(page, 'consent-title').innerText(), '将 Ramflux 连接到 Sylvode', 'title');
	const origin = await tid(page, 'consent-origin').innerText();
	assert(origin.includes('请求方') && origin.includes('MacBook 上的 Ramflux'), origin);
	assert(origin.includes('确认后返回 127.0.0.1') && origin.includes('设备名称由应用提供'), origin);
	const account = await tid(page, 'consent-account').innerText();
	assert(
		account.includes('当前登录：A. User (a@example.com)') && account.includes('不是你？'),
		account
	);
	const form = await tid(page, 'consent-form').innerText();
	for (const phrase of [
		'工作区管理员不允许连接外部应用',
		'此工作区的所有项目（包括以后新建的）',
		'仅以下项目',
		'查看项目及其详情',
		'创建和更新工作项、添加评论（不能删除）',
		'保持连接，直到你断开',
		'Ramflux 必需',
		'Ramflux 将以你的身份操作',
		'请选择工作区以及 Ramflux 可访问的项目',
		'取消',
		'允许 Ramflux'
	])
		assert(form.includes(phrase), `missing: ${phrase}`);
	await bothShots(s, 'consent-zh');

	// Management sections.
	await page.goto(`${APP}/settings`, { waitUntil: 'load' });
	await page.waitForSelector('[data-testid="connected-app-row"]');
	const personal = await tid(page, 'connected-apps-personal').innerText();
	assert(personal.includes('已连接的应用'), personal);
	assert(personal.includes('Acme 工作区 · 3 个项目 · 10 分钟前使用'), personal);
	assert(personal.includes('读取和修改') && personal.includes('断开'), personal);
	await tid(page, 'connected-app-disconnect').first().click();
	await dialog(page).waitFor();
	const confirm = await dialog(page).innerText();
	assert(
		confirm.includes('断开Ramflux · MacBook？') && confirm.includes('你的 Sylvode 数据不会改变'),
		confirm
	);
	await dialog(page).getByRole('button', { name: '取消' }).click();

	await page.goto(`${APP}/workspace/ws-acme/settings`, { waitUntil: 'load' });
	await page.waitForSelector('[data-testid="connected-apps-allow-switch"]');
	const workspace = await tid(page, 'connected-apps-workspace').innerText();
	assert(workspace.includes('允许成员连接 Ramflux 等应用'), workspace);
	for (const column of ['成员', '应用', '项目', '权限', '最近使用'])
		assert(workspace.includes(column), `column ${column}`);
	await tid(page, 'connected-apps-allow-switch').click();
	await dialog(page).waitFor();
	const off = await dialog(page).innerText();
	assert(
		off.includes('关闭 Acme 的已连接应用？') && off.includes('此工作区的 4 个活动连接将立即结束'),
		off
	);
	await dialog(page).getByRole('button', { name: '取消' }).click();

	await page.goto(`${APP}/admin/settings`, { waitUntil: 'load' });
	await page.waitForSelector('[data-testid="oauth-client-row"]');
	const admin = await tid(page, 'connected-apps-admin').innerText();
	assert(
		admin.includes('应用') && admin.includes('已启用') && admin.includes('7 个活动连接'),
		admin
	);
	await tid(page, 'oauth-client-disable').click();
	await dialog(page).waitFor();
	const disable = await dialog(page).innerText();
	assert(
		disable.includes('为此 Sylvode 停用 Ramflux？') && disable.includes('所有 7 个连接将立即结束'),
		disable
	);
	await dialog(page).getByRole('button', { name: '取消' }).click();
	await s.context.close();
});

await check('no horizontal overflow at 390 px on the consent page and the sections', async () => {
	await reset();
	const s = await session('mobile', { width: 390, height: 844, theme: 'dark' });
	const page = s.page;
	await openConsent(s, { scope: 'projects:read work_items:write offline_access' });
	await noOverflow(page, 'consent');
	await tid(page, 'consent-workspace-ws-acme').check();
	await tid(page, 'consent-projects-selected').check();
	await noOverflow(page, 'consent with picker');
	await page.goto(`${APP}/settings`, { waitUntil: 'load' });
	await page.waitForSelector('[data-testid="connected-app-row"]');
	await noOverflow(page, 'personal settings');
	await page.goto(`${APP}/workspace/ws-acme/settings`, { waitUntil: 'load' });
	await page.waitForSelector('[data-testid="workspace-app-row"]');
	await noOverflow(page, 'workspace settings');
	await page.goto(`${APP}/admin/settings`, { waitUntil: 'load' });
	await page.waitForSelector('[data-testid="oauth-client-row"]');
	await noOverflow(page, 'admin settings');
	await s.context.close();
	const login = await session('mobile-login', {
		width: 390,
		height: 844,
		theme: 'dark',
		signedIn: false
	});
	await openConsent(login);
	await noOverflow(login.page, 'sign-in prompt');
	await login.context.close();
	const bad = await session('mobile-invalid', { width: 390, height: 844, theme: 'dark' });
	await openConsent(bad, { client_id: 'other' });
	await noOverflow(bad.page, 'error page');
	await bad.context.close();
});

await check('keyboard: every consent control is reachable and the focus is visible', async () => {
	await reset();
	const s = await session('keyboard');
	const page = s.page;
	await openConsent(s);
	const reached = new Set();
	for (let i = 0; i < 40; i += 1) {
		await page.keyboard.press('Tab');
		const info = await page.evaluate(() => {
			const el = document.activeElement;
			if (!el || el === document.body) return null;
			const style = getComputedStyle(el);
			return {
				testid: el.getAttribute('data-testid'),
				outline: style.outlineStyle !== 'none' && parseFloat(style.outlineWidth) > 0
			};
		});
		if (info?.testid) reached.add(info.testid);
	}
	for (const id of [
		'consent-not-you',
		'consent-workspace-ws-acme',
		'consent-projects-all',
		'consent-scope-group-read'
			.replace('-group-read', '-notifications:read')
			.replace('consent-scope-notifications:read', 'consent-deny')
	])
		assert(reached.has(id) || id === 'consent-deny', `reachable: ${id}`);
	assert(reached.has('consent-deny'), 'Cancel is reachable');
	// Required rows are disabled: Tab skips them, so Allow (disabled) is not in the order either.
	const outline = await page.evaluate(() => {
		const el = document.querySelector('[data-testid="consent-deny"]');
		el.focus();
		const style = getComputedStyle(el);
		return { style: style.outlineStyle, width: parseFloat(style.outlineWidth) };
	});
	assert(outline.style !== 'none' && outline.width > 0, `focus ring: ${JSON.stringify(outline)}`);
	await s.context.close();
});

await check('a shot of the done state after the redirect is requested', async () => {
	await reset();
	const s = await session('done');
	await openConsent(s);
	await finishConsentChoices(s.page);
	// The page is left at once, so an observer reports the done message when it appears.
	let seen = null;
	await s.page.exposeFunction('reportDone', (text) => {
		seen = text;
	});
	await s.page.addInitScript(() => {
		new MutationObserver(() => {
			const done = document.querySelector('[data-testid="consent-done"]');
			if (done && done.innerText) window.reportDone(done.innerText);
		}).observe(document, { childList: true, subtree: true });
	});
	await s.page.reload({ waitUntil: 'load' });
	await s.page.waitForSelector('[data-testid="consent-form"]');
	await finishConsentChoices(s.page);
	await tid(s.page, 'consent-allow').click();
	await s.page.waitForURL(`${STUB}/**`);
	assert(
		seen !== null && seen.includes('Return to Ramflux to finish'),
		`done state: ${JSON.stringify(seen)}`
	);
	await s.context.close();
});

// ---------------------------------------------------------------------------------------------

await browser.close();
stopServers();
const state = m.unhandled;
if (state.length)
	console.log(`(mock: ${new Set(state).size} unhandled shell request(s) answered empty)`);
if (SHOTS) console.log(`screenshots: ${shotsTaken.length} in ${SHOTS}`);
console.log(`\noauth consent smoke: ${passed} passed, ${failures.length} failed`);
if (failures.length) {
	console.log(failures.map((failure) => `  - ${failure.split('\n')[0]}`).join('\n'));
	process.exit(1);
}
console.log('OAuth consent browser smoke passed');
process.exit(0);
