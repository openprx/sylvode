/**
 * The typed client for the authorization server's first-party endpoints, the grant summaries and
 * the locale files of the OAuth screens.
 *
 * Run standalone: `bun tests/oauth-manage.test.ts`
 */
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	createAuthorizationRequest,
	failureKey,
	isNotEnabled,
	oauthApi,
	parseTooManyGrants,
	submitDecision
} from '../src/lib/api/oauth';
import {
	connectionLabel,
	groupScopes,
	relativeTime,
	scopeGroup,
	scopeSummary
} from '../src/lib/oauth/grant-summary';

const suite = new Suite('oauth-manage');
const ROOT = new URL('..', import.meta.url).pathname;

interface Seen {
	url: string;
	method: string;
	headers: Headers;
	body: string;
}
const seen: Seen[] = [];
let next: { status: number; body: unknown } | string = { status: 200, body: {} };

globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
	seen.push({
		url: String(input),
		method: init?.method ?? 'GET',
		headers: new Headers(init?.headers),
		body: typeof init?.body === 'string' ? init.body : ''
	});
	if (typeof next === 'string') return new Response(next, { status: 502 });
	return new Response(JSON.stringify(next.body), {
		status: next.status,
		headers: { 'Content-Type': 'application/json' }
	});
}) as typeof fetch;

function answer(status: number, body: unknown) {
	next = { status, body };
	seen.length = 0;
}
function ok(data: unknown) {
	answer(200, { code: 0, message: 'success', data });
}
function last(): Seen {
	const item = seen.at(-1);
	assert(item !== undefined, 'a request was sent');
	return item;
}

const CONSENT = {
	kind: 'consent',
	request_id: '0b9c1b52-6a0e-4f6e-9a55-3a1f6a0d2c11',
	consent_token: 'a'.repeat(64),
	client: {
		client_id: 'ramflux',
		display_name: 'Ramflux',
		description: 'Ramflux desktop',
		redirect_host: '127.0.0.1',
		device_name: 'MacBook'
	},
	account: { user_id: 'u1', name: 'A. User', email: 'a@example.com' },
	workspaces: [
		{ id: 'w1', name: 'Acme', eligible: true, reason: null },
		{ id: 'w2', name: 'Lab', eligible: false, reason: 'workspace_disallows' }
	],
	preselected_workspace_id: null,
	scopes: [
		{ name: 'projects:read', required: true },
		{ name: 'work_items:write', required: false }
	],
	resources: ['api', 'mcp'],
	active_grant_count: 2,
	max_active_grants: 10
};

const GRANT = {
	grant_id: 'g1',
	client: { client_id: 'ramflux', display_name: 'Ramflux' },
	device_name: 'MacBook',
	workspace: { id: 'w1', name: 'Acme' },
	scopes: ['projects:read'],
	project_mode: 'selected',
	projects: [{ id: 'p1', key: 'AC', name: 'Alpha' }],
	project_count: 3,
	consented_at: '2026-11-20T10:00:00Z',
	last_used_at: null,
	expires_at: '2027-11-20T10:00:00Z'
};

await suite.checkAsync('a consent answer is parsed into typed data', async () => {
	ok(CONSENT);
	const result = await createAuthorizationRequest({
		kind: 'new',
		params: { client_id: 'ramflux', redirect_uri: 'http://127.0.0.1:1/cb' }
	});
	assert(result.ok && result.data.kind === 'consent', 'consent');
	if (!result.ok || result.data.kind !== 'consent') return;
	assertEqual(result.data.consent.client.displayName, 'Ramflux', 'name from the server');
	assertEqual(result.data.consent.client.deviceName, 'MacBook', 'device');
	assertEqual(result.data.consent.workspaces[1]?.eligible, false, 'ineligible workspace');
	assertEqual(result.data.consent.maxActiveGrants, 10, 'limit');
	assertEqual(last().url, '/api/v1/oauth/authorization-requests', 'endpoint');
	assertDeepEqual(
		JSON.parse(last().body),
		{ client_id: 'ramflux', redirect_uri: 'http://127.0.0.1:1/cb' },
		'only the given parameters'
	);
});

await suite.checkAsync('a stored request is reloaded by id', async () => {
	ok({ kind: 'login_required', request_id: 'r1', client: CONSENT.client });
	const result = await createAuthorizationRequest({ kind: 'reload', requestId: 'r1' });
	assert(result.ok && result.data.kind === 'login_required', 'login required');
	assertDeepEqual(JSON.parse(last().body), { request_id: 'r1' }, 'body');
});

await suite.checkAsync(
	'error pages and redirects are told apart; unknown reasons read as unknown client',
	async () => {
		ok({ kind: 'error_page', error: 'redirect_uri_mismatch' });
		const mismatch = await createAuthorizationRequest({ kind: 'reload', requestId: 'r' });
		assert(mismatch.ok && mismatch.data.kind === 'error_page', 'error page');
		if (mismatch.ok && mismatch.data.kind === 'error_page')
			assertEqual(mismatch.data.error, 'redirect_uri_mismatch', 'reason');
		ok({ kind: 'error_page', error: 'something_new' });
		const unknown = await createAuthorizationRequest({ kind: 'reload', requestId: 'r' });
		assert(unknown.ok && unknown.data.kind === 'error_page', 'error page');
		if (unknown.ok && unknown.data.kind === 'error_page')
			assertEqual(unknown.data.error, 'unknown_client', 'fallback');
		ok({ kind: 'redirect', redirect_to: 'http://127.0.0.1:1/cb?error=invalid_scope' });
		const redirect = await createAuthorizationRequest({ kind: 'reload', requestId: 'r' });
		assert(redirect.ok && redirect.data.kind === 'redirect', 'redirect');
	}
);

await suite.checkAsync('a response of the wrong shape is a failure, not a guess', async () => {
	ok({ kind: 'consent', request_id: 'r' });
	const partial = await createAuthorizationRequest({ kind: 'reload', requestId: 'r' });
	assert(!partial.ok && partial.failure.errorCode === 'malformed_response', 'partial consent');
	ok({ kind: 'surprise' });
	const surprise = await createAuthorizationRequest({ kind: 'reload', requestId: 'r' });
	assert(!surprise.ok, 'unknown kind');
	ok({ items: [{ grant_id: 'g1' }], has_more: false });
	const grants = await oauthApi.listOwn();
	assert(!grants.ok, 'incomplete grant');
	next = '<html>bad gateway</html>';
	const html = await oauthApi.listClients();
	assert(!html.ok && html.failure.status === 500, 'non-JSON body');
});

await suite.checkAsync(
	'the decision sends the token in the header and never in the body or URL',
	async () => {
		ok({ redirect_to: 'http://127.0.0.1:1/cb?code=svc_1&state=s&iss=i' });
		const token = 'b'.repeat(64);
		const result = await submitDecision('r1', { decision: 'deny' }, token);
		assert(result.ok, 'ok');
		if (result.ok)
			assertEqual(
				result.data.redirectTo,
				'http://127.0.0.1:1/cb?code=svc_1&state=s&iss=i',
				'redirect'
			);
		assertEqual(last().headers.get('X-Sylvode-Consent'), token, 'header');
		assert(!last().body.includes(token) && !last().url.includes(token), 'not in body or url');
		assertEqual(last().url, '/api/v1/oauth/authorization-requests/r1/decision', 'path');
		await submitDecision('r1', { decision: 'no_account' }, null);
		assertEqual(last().headers.get('X-Sylvode-Consent'), null, 'no header for no_account');
	}
);

await suite.checkAsync('failures carry the status and the machine-readable code', async () => {
	answer(403, { code: 403, message: 'x', error_code: 'consent_token_invalid' });
	const refused = await submitDecision('r1', { decision: 'deny' }, 't');
	assert(!refused.ok, 'failed');
	if (!refused.ok) {
		assertEqual(refused.failure.status, 403, 'status');
		assertEqual(refused.failure.errorCode, 'consent_token_invalid', 'code');
	}
	answer(409, {
		code: 409,
		message: 'x',
		error_code: 'too_many_grants',
		details: {
			max_active_grants: 10,
			grants: [{ grant_id: 'g1', device_name: null, workspace_id: 'w1', consented_at: 't' }]
		}
	});
	const many = await submitDecision('r1', { decision: 'deny' }, 't');
	assert(!many.ok, 'failed');
	if (!many.ok) {
		const parsed = parseTooManyGrants(many.failure.details);
		assertEqual(parsed?.maxActiveGrants, 10, 'limit');
		assertEqual(parsed?.grants[0]?.deviceName, null, 'device');
	}
	assertEqual(parseTooManyGrants({ max_active_grants: 'ten' }), null, 'bad details');
	assertEqual(parseTooManyGrants(null), null, 'no details');
});

suite.check('failure codes map to translation keys that exist', () => {
	const en = JSON.parse(readFileSync(join(ROOT, 'src/lib/oauth/locale.en.json'), 'utf8'));
	for (const [status, code] of [
		[403, 'consent_token_invalid'],
		[403, 'workspace_not_eligible'],
		[400, 'invalid_selection'],
		[403, 'client_not_active'],
		[429, 'rate_limited'],
		[403, 'csrf_origin_mismatch'],
		[401, 'session_required'],
		[404, 'not_found'],
		[404, 'request_not_found'],
		[409, 'already_decided'],
		[409, 'too_many_grants'],
		[403, 'forbidden'],
		[500, 'internal_error'],
		[429, ''],
		[418, 'teapot']
	] as const) {
		const key = failureKey({ status, errorCode: code, details: null });
		const path = key.split('.');
		let node: unknown = en;
		for (const part of path) node = (node as Record<string, unknown> | undefined)?.[part];
		assertEqual(typeof node, 'string', `${status} ${code} -> ${key}`);
	}
});

await suite.checkAsync('management lists are followed page by page and validated', async () => {
	let page = 0;
	globalThis.fetch = (async (input: RequestInfo | URL) => {
		seen.push({ url: String(input), method: 'GET', headers: new Headers(), body: '' });
		page += 1;
		const body =
			page === 1
				? { items: [GRANT], has_more: true, next_offset: 1 }
				: { items: [{ ...GRANT, grant_id: 'g2' }], has_more: false, next_offset: null };
		return new Response(JSON.stringify({ code: 0, message: 'success', data: body }), {
			status: 200,
			headers: { 'Content-Type': 'application/json' }
		});
	}) as typeof fetch;
	seen.length = 0;
	const all = await oauthApi.listOwn();
	assert(all.ok, 'ok');
	if (all.ok) {
		assertDeepEqual(
			all.data.map((grant) => grant.grantId),
			['g1', 'g2'],
			'both pages'
		);
		assertEqual(all.data[0]?.projectCount, 3, 'count of the selection');
		assertEqual(all.data[0]?.user, null, 'no member on the personal list');
	}
	assertEqual(seen[0]?.url, '/api/v1/me/connected-apps?limit=100&offset=0', 'first page');
	assertEqual(seen[1]?.url, '/api/v1/me/connected-apps?limit=100&offset=1', 'second page');
});

await suite.checkAsync(
	'workspace and instance endpoints use the contract paths and bodies',
	async () => {
		let payload: unknown = {};
		globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
			seen.push({
				url: String(input),
				method: init?.method ?? 'GET',
				headers: new Headers(init?.headers),
				body: typeof init?.body === 'string' ? init.body : ''
			});
			return new Response(JSON.stringify(payload), {
				status: 200,
				headers: { 'Content-Type': 'application/json' }
			});
		}) as typeof fetch;
		seen.length = 0;
		payload = {
			code: 0,
			message: 'success',
			data: { allow_connected_apps: false, revoked_grants: 4 }
		};
		const off = await oauthApi.setWorkspaceSetting('w1', false);
		assert(off.ok && off.data.revokedGrants === 4 && !off.data.allow, 'turned off');
		assertEqual(last().method, 'PUT', 'method');
		assertEqual(last().url, '/api/v1/workspaces/w1/connected-apps/settings', 'path');
		assertDeepEqual(JSON.parse(last().body), { allow_connected_apps: false }, 'body');

		payload = {
			code: 0,
			message: 'success',
			data: { client_id: 'ramflux', status: 'disabled', revoked_grants: 7 }
		};
		const disabled = await oauthApi.setClientStatus('ramflux', 'disabled');
		assert(disabled.ok && disabled.data.revokedGrants === 7, 'disabled');
		assertEqual(last().method, 'PATCH', 'method');
		assertEqual(last().url, '/api/v1/admin/oauth-clients/ramflux', 'path');

		payload = { code: 0, message: 'success', data: { grant_id: 'g1', revoked: true } };
		const gone = await oauthApi.disconnectInWorkspace('w1', 'g1');
		assert(gone.ok, 'disconnected');
		assertEqual(last().method, 'DELETE', 'method');
		assertEqual(last().url, '/api/v1/workspaces/w1/connected-apps/g1', 'path');
		const own = await oauthApi.disconnectOwn('g 1');
		assert(own.ok, 'own');
		assertEqual(last().url, '/api/v1/me/connected-apps/g%201', 'ids are encoded');
	}
);

suite.check('scope groups, summaries and labels', () => {
	assertEqual(scopeGroup('work_items:read'), 'read', 'read');
	assertEqual(scopeGroup('forms:write'), 'change', 'change');
	assertEqual(scopeGroup('offline_access'), 'other', 'other');
	assertEqual(scopeGroup('surfaces:embed'), 'other', 'embed');
	assertEqual(scopeSummary(['projects:read', 'offline_access']), 'read', 'read only');
	assertEqual(scopeSummary(['projects:read', 'work_items:write']), 'readWrite', 'with write');
	assertDeepEqual(
		groupScopes([
			{ name: 'offline_access' },
			{ name: 'work_items:write' },
			{ name: 'projects:read' }
		]).map((entry) => [entry.group, entry.scopes.map((scope) => scope.name)]),
		[
			['read', ['projects:read']],
			['change', ['work_items:write']],
			['other', ['offline_access']]
		],
		'ordered groups, empty ones left out'
	);
	assertEqual(connectionLabel('Ramflux', 'MacBook'), 'Ramflux · MacBook', 'with device');
	assertEqual(connectionLabel('Ramflux', null), 'Ramflux', 'without device');
});

suite.check('relative times follow the language', () => {
	const now = new Date('2026-11-21T12:00:00Z');
	assertEqual(relativeTime('2026-11-21T11:50:00Z', now, 'en', 'just now'), '10 minutes ago', 'en');
	assertEqual(relativeTime('2026-11-21T11:50:00Z', now, 'zh', '刚刚'), '10 分钟前', 'zh');
	assertEqual(relativeTime('2026-11-21T11:59:40Z', now, 'en', 'just now'), 'just now', 'seconds');
	assertEqual(relativeTime('2026-11-19T12:00:00Z', now, 'en', 'just now'), '2 days ago', 'days');
	assertEqual(relativeTime('garbage', now, 'en', 'just now'), null, 'unreadable');
});

// --- locale files ---------------------------------------------------------------------------------

function leaves(node: unknown, prefix = ''): string[] {
	if (typeof node !== 'object' || node === null) return [prefix];
	return Object.entries(node).flatMap(([key, value]) =>
		leaves(value, prefix ? `${prefix}.${key}` : key)
	);
}

suite.check('English and Chinese locale files have the same keys (plural groups aside)', () => {
	const read = (lang: string) =>
		leaves(JSON.parse(readFileSync(join(ROOT, `src/lib/oauth/locale.${lang}.json`), 'utf8')));
	const strip = (keys: string[]) =>
		keys.filter((key) => !/\.(one|few|many|two|zero)$/.test(key)).sort();
	assertDeepEqual(strip(read('zh')), strip(read('en')), 'same keys');
	for (const lang of ['en', 'zh']) {
		const keys = read(lang);
		for (const key of keys.filter((k) => /\.one$/.test(k))) {
			assert(keys.includes(key.replace(/\.one$/, '.other')), `${lang}: ${key} has no .other`);
		}
	}
});

await suite.checkAsync(
	'a server without OAuth answers feature_disabled and every list shows the not-enabled state',
	async () => {
		// The API sends typed refusals as HTTP 200 with the status in the envelope `code`.
		globalThis.fetch = (async (input: RequestInfo | URL) => {
			seen.push({ url: String(input), method: 'GET', headers: new Headers(), body: '' });
			return new Response(
				JSON.stringify({
					code: 403,
					message: 'connected applications are not enabled on this server',
					data: null,
					error_code: 'feature_disabled'
				}),
				{ status: 200, headers: { 'Content-Type': 'application/json' } }
			);
		}) as typeof fetch;
		seen.length = 0;
		const own = await oauthApi.listOwn();
		const workspace = await oauthApi.listWorkspace('w1');
		const setting = await oauthApi.getWorkspaceSetting('w1');
		for (const [name, result] of [
			['own list', own],
			['workspace list', workspace],
			['workspace setting', setting]
		] as const) {
			assert(!result.ok, `${name} fails`);
			if (!result.ok) {
				assertEqual(result.failure.status, 403, `${name}: status`);
				assertEqual(result.failure.errorCode, 'feature_disabled', `${name}: error code`);
				assert(isNotEnabled(result.failure), `${name}: shows the not-enabled state`);
			}
		}
		// An ordinary refusal of a person without access is not "not enabled".
		assert(
			!isNotEnabled({ status: 403, errorCode: 'forbidden', details: null }),
			'a plain forbidden is not the not-enabled state'
		);
	}
);

suite.check('the section 14 strings are present in both languages', () => {
	const read = (lang: string) =>
		JSON.parse(readFileSync(join(ROOT, `src/lib/oauth/locale.${lang}.json`), 'utf8'));
	const en = read('en').oauth;
	const zh = read('zh').oauth;
	assertEqual(en.consent.title, 'Connect {client} to Sylvode', 'en title');
	assertEqual(zh.consent.title, '将 {client} 连接到 Sylvode', 'zh title');
	assertEqual(en.consent.login.noAccount, "I don't have a Sylvode account", 'en no account');
	assertEqual(zh.consent.login.noAccount, '我没有 Sylvode 账号', 'zh no account');
	assertEqual(zh.scope['work_items:write'], '创建和更新工作项、添加评论（不能删除）', 'zh scope');
	assertEqual(zh.consent.invalid.title, '此连接请求无效', 'zh invalid');
	assertEqual(zh.manage.workspace.switch, '允许成员连接 Ramflux 等应用', 'zh switch');
});

finish(suite.result());
