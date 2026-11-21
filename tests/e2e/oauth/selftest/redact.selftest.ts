import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Redactor, TOKEN_PREFIXES } from '../lib/redact.ts';

const fake = (prefix: string): string => `${prefix}${'a1b2c3d4'.repeat(8)}`;

test('token shapes, JWTs and bearer values are replaced without leaving a prefix behind', () => {
	const r = new Redactor();
	const jwt = 'eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2lnbmF0dXJl';
	const text = r.text(
		`a ${fake('sva_')} b ${fake('svr_')} c ${fake('svc_')} d ${jwt} e Bearer abcdefghijkl12345 f ${fake('opr_')}`
	);
	for (const prefix of TOKEN_PREFIXES)
		assert.ok(!text.includes(prefix), `${prefix} left in: ${text}`);
	assert.ok(!text.includes(jwt) && !text.includes('abcdefghijkl12345'));
});

test('registered values are replaced wherever they occur, including inside other text', () => {
	const r = new Redactor();
	r.register('hunter2-password', 'password:a');
	r.register('abc', 'x');
	assert.equal(
		r.text('pw=hunter2-password;again hunter2-password'),
		'pw=<redacted:password:a>;again <redacted:password:a>'
	);
	assert.equal(r.text('abc stays'), 'abc stays', 'values under four characters are not registered');
});

test('sensitive JSON keys lose their string values, numbers stay, nesting is walked', () => {
	const r = new Redactor();
	const out = r.json({
		code: 0,
		message: 'ok',
		data: {
			access_token: 'zzzzzzzz',
			refresh_token: 'yyyyyyyy',
			nested: [{ password: 'pw123456', grant_id: 'g1' }],
			code: 'svc-like-value'
		}
	});
	assert.deepEqual(out, {
		code: 0,
		message: 'ok',
		data: {
			access_token: '<redacted>',
			refresh_token: '<redacted>',
			nested: [{ password: '<redacted>', grant_id: 'g1' }],
			code: '<redacted>'
		}
	});
});

test('form bodies, URLs and typed bodies are redacted', () => {
	const r = new Redactor();
	const form = r.body(
		'grant_type=refresh_token&refresh_token=abcdefgh&client_id=ramflux&code_verifier=vvvvvvvv',
		'application/x-www-form-urlencoded'
	);
	assert.ok(form.includes('grant_type=refresh_token') && form.includes('client_id=ramflux'));
	assert.ok(!form.includes('abcdefgh') && !form.includes('vvvvvvvv'));
	const url = r.url(
		'http://127.0.0.1:1234/oauth/sylvode/callback?code=thecode&state=s1&iss=x#frag'
	);
	assert.ok(!url.includes('thecode') && url.includes('state=s1') && !url.includes('frag'));
	assert.equal(
		r.body('{"access_token":"qqqqqqqq","a":1}', 'application/json'),
		'{"access_token":"<redacted>","a":1}'
	);
	assert.equal(r.body('not json {', 'text/plain'), 'not json {');
});

test('placeholders never contain a token prefix followed by an underscore', () => {
	const r = new Redactor();
	for (const prefix of TOKEN_PREFIXES) assert.ok(!r.text(fake(prefix)).includes(prefix));
});
