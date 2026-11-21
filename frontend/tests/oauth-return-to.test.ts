/**
 * The consent page's address handling: the sign-in `return_to` validator (contract 14.4) and the
 * parsing of the authorization request out of the address bar.
 *
 * Run standalone: `bun tests/oauth-return-to.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	consentReturnTarget,
	isRequestId,
	loginUrlForLocation,
	loginUrlForRequest,
	safeReturnTo
} from '../src/lib/oauth/return-to';
import { apiClient } from '../src/lib/api/client';
import { MAX_QUERY_LENGTH, parseAuthorizeQuery } from '../src/lib/oauth/consent-request';

const suite = new Suite('oauth-return-to');
const ID = '0b9c1b52-6a0e-4f6e-9a55-3a1f6a0d2c11';
const GOOD = `/oauth/authorize?request=${ID}`;

suite.check('the consent route with a request id is accepted', () => {
	assertEqual(safeReturnTo(GOOD), GOOD, 'plain');
	assertEqual(
		safeReturnTo(`/oauth/authorize?request=${ID.toUpperCase()}`),
		`/oauth/authorize?request=${ID.toUpperCase()}`,
		'upper case id'
	);
});

suite.check('missing and empty values are refused', () => {
	assertEqual(safeReturnTo(null), null, 'null');
	assertEqual(safeReturnTo(undefined), null, 'undefined');
	assertEqual(safeReturnTo(''), null, 'empty');
});

const hostile: Array<[string, string]> = [
	['absolute URL', 'https://evil.example/oauth/authorize?request=' + ID],
	['absolute URL to the same path', `http://127.0.0.1:18097${GOOD}`],
	['protocol-relative', `//evil.example${GOOD}`],
	['protocol-relative only', '//evil.example'],
	['backslash host', `/\\evil.example${GOOD}`],
	['backslash path', `/oauth\\authorize?request=${ID}`],
	['leading backslash', `\\oauth\\authorize?request=${ID}`],
	['encoded slash', `/oauth%2fauthorize?request=${ID}`],
	['encoded slash upper', `%2Foauth%2Fauthorize%3Frequest%3D${ID}`],
	['double encoded', `/oauth/authorize?request=${ID.replace('-', '%2d')}`],
	['encoded dot segment', `/oauth/%2e%2e/workspace`],
	['javascript scheme', 'javascript:alert(1)'],
	['javascript with path', `javascript:/oauth/authorize?request=${ID}`],
	['data scheme', 'data:text/html,<script>alert(1)</script>'],
	['control character', `/oauth/authorize?request=${ID}\u0000`],
	['newline', `/oauth/authorize?request=${ID}\n`],
	['carriage return', `/oauth/authorize?request=${ID}\r`],
	['tab', `/oauth/authorize?\trequest=${ID}`],
	['leading space', ` ${GOOD}`],
	['trailing space', `${GOOD} `],
	['delete character', `/oauth/authorize?request=${ID}\u007f`],
	['zero-width space', `/oauth/authorize?request=${ID}​`],
	['extra parameter', `${GOOD}&next=https://evil.example`],
	['parameter first', `/oauth/authorize?next=/x&request=${ID}`],
	['fragment', `${GOOD}#x`],
	['another route', `/workspace?request=${ID}`],
	['sub-path of the consent route', `/oauth/authorize/extra?request=${ID}`],
	['prefix only', '/oauth/authorize'],
	['empty request', '/oauth/authorize?request='],
	['request is not a uuid', '/oauth/authorize?request=abc'],
	['request with path traversal', '/oauth/authorize?request=../../x'],
	['uuid with a suffix', `${GOOD}0`],
	['relative path', `oauth/authorize?request=${ID}`],
	['query only', `?request=${ID}`],
	['over-long value', `/oauth/authorize?request=${ID}${'a'.repeat(5000)}`],
	['over-long padding', `/${'/'.repeat(5000)}`]
];
for (const [label, value] of hostile) {
	suite.check(`refused: ${label}`, () => {
		assertEqual(safeReturnTo(value), null, label);
	});
}

suite.check('request ids and the targets built from them', () => {
	assert(isRequestId(ID), 'uuid');
	assert(!isRequestId(`${ID} `), 'trailing space');
	assert(!isRequestId('not-a-uuid'), 'text');
	assertEqual(consentReturnTarget(ID), GOOD, 'target');
	assertEqual(consentReturnTarget('x'), null, 'bad id');
	assertEqual(
		loginUrlForRequest(ID),
		`/auth/login?return_to=${encodeURIComponent(GOOD)}`,
		'login url'
	);
	assertEqual(loginUrlForRequest('../x'), null, 'login url refuses a bad id');
});

suite.check('a built login URL round-trips through the validator after one decoding', () => {
	const url = new URL(loginUrlForRequest(ID) ?? '', 'http://x.test');
	assertEqual(safeReturnTo(url.searchParams.get('return_to')), GOOD, 'round trip');
});

suite.check('a forced sign-in from the stored consent request comes back to it', () => {
	assertEqual(
		loginUrlForLocation('/oauth/authorize', `?request=${ID}`),
		`/auth/login?return_to=${encodeURIComponent(GOOD)}`,
		'consent request'
	);
	const hostile: Array<[string, string, string]> = [
		['another page', '/workspace', ''],
		['the raw request query', '/oauth/authorize', '?client_id=c&state=s&scope=x'],
		['an extra parameter', '/oauth/authorize', `?request=${ID}&next=//evil.test`],
		['a protocol-relative path', '//evil.test/oauth/authorize', `?request=${ID}`],
		['a backslash', '/oauth/authorize', `?request=${ID}\\evil`],
		['no query', '/oauth/authorize', '']
	];
	for (const [label, pathname, search] of hostile)
		assertEqual(loginUrlForLocation(pathname, search), '/auth/login', label);
});

await suite.checkAsync(
	'an expired session on the consent page signs in again to the same request',
	async () => {
		const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'window');
		const fetchDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'fetch');
		const storageDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
		const location = { href: '', pathname: '/oauth/authorize', search: `?request=${ID}` };
		Object.defineProperty(globalThis, 'window', {
			value: Object.assign(new EventTarget(), { location }),
			configurable: true
		});
		Object.defineProperty(globalThis, 'localStorage', {
			value: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
			configurable: true
		});
		Object.defineProperty(globalThis, 'fetch', {
			value: async () =>
				new Response(JSON.stringify({ code: 401, message: 'expired', data: null }), {
					status: 401
				}),
			configurable: true
		});
		try {
			apiClient.clearAuth();
			await apiClient.request('POST', '/api/v1/oauth/authorization-requests', {}, false);
			assertEqual(location.href, `/auth/login?return_to=${encodeURIComponent(GOOD)}`, 'consent');
			location.href = '';
			location.pathname = '/workspace';
			location.search = '';
			await apiClient.request('GET', '/api/v1/auth/me', undefined, false);
			assertEqual(location.href, '/auth/login', 'any other page');
		} finally {
			if (windowDescriptor) Object.defineProperty(globalThis, 'window', windowDescriptor);
			else Reflect.deleteProperty(globalThis, 'window');
			if (fetchDescriptor) Object.defineProperty(globalThis, 'fetch', fetchDescriptor);
			else Reflect.deleteProperty(globalThis, 'fetch');
			if (storageDescriptor) Object.defineProperty(globalThis, 'localStorage', storageDescriptor);
			else Reflect.deleteProperty(globalThis, 'localStorage');
		}
	}
);

// --- address bar parsing ------------------------------------------------------------------------

const REQUEST =
	'?response_type=code&client_id=ramflux' +
	'&redirect_uri=http%3A%2F%2F127.0.0.1%3A5555%2Foauth%2Fsylvode%2Fcallback' +
	'&scope=projects%3Aread%20offline_access&state=abcdefghijklmnop0123456789' +
	'&code_challenge=abc&code_challenge_method=S256' +
	'&resource=http%3A%2F%2Fa.test%2Fapi&resource=http%3A%2F%2Fa.test%2Fmcp&device_name=Mac';

suite.check('a full request is forwarded with repeated resources as a list', () => {
	const parsed = parseAuthorizeQuery(REQUEST);
	assert(parsed.kind === 'new', 'kind');
	assertEqual(parsed.params.client_id, 'ramflux', 'client');
	assertEqual(parsed.params.redirect_uri, 'http://127.0.0.1:5555/oauth/sylvode/callback', 'uri');
	assertEqual(parsed.params.scope, 'projects:read offline_access', 'scope');
	assertDeepEqual(parsed.params.resource, ['http://a.test/api', 'http://a.test/mcp'], 'resources');
	assertEqual(parsed.params.device_name, 'Mac', 'device');
});

suite.check('unknown parameters are dropped, never forwarded', () => {
	const parsed = parseAuthorizeQuery(`${REQUEST}&return_to=//evil&x=1`);
	assert(parsed.kind === 'new', 'kind');
	assert(!('return_to' in parsed.params) && !('x' in parsed.params), 'dropped');
});

suite.check('a stored request id reloads; a malformed one reads as expired', () => {
	assertDeepEqual(
		parseAuthorizeQuery(`?request=${ID}`),
		{ kind: 'reload', requestId: ID },
		'reload'
	);
	assertDeepEqual(parseAuthorizeQuery('?request=nope'), { kind: 'expired' }, 'not a uuid');
	assertDeepEqual(
		parseAuthorizeQuery(`?request=${ID}&request=${ID}`),
		{ kind: 'invalid' },
		'twice'
	);
});

suite.check('a repeated single-valued parameter is refused locally', () => {
	for (const name of ['redirect_uri', 'client_id', 'state', 'scope', 'code_challenge']) {
		assertDeepEqual(parseAuthorizeQuery(`${REQUEST}&${name}=other`), { kind: 'invalid' }, name);
	}
});

suite.check('nothing to validate against is invalid', () => {
	assertDeepEqual(parseAuthorizeQuery(''), { kind: 'invalid' }, 'empty');
	assertDeepEqual(parseAuthorizeQuery('?client_id=ramflux'), { kind: 'invalid' }, 'no uri');
	assertDeepEqual(parseAuthorizeQuery('?redirect_uri=http://x'), { kind: 'invalid' }, 'no client');
	assertDeepEqual(
		parseAuthorizeQuery(`${REQUEST}&junk=${'x'.repeat(MAX_QUERY_LENGTH)}`),
		{ kind: 'invalid' },
		'too long'
	);
});

finish(suite.result());
