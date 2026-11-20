import assert from 'node:assert/strict';
import { test } from 'node:test';
import { CALLBACK_PATH, CallbackListener, parseCallback } from '../lib/callback.ts';

const expected = {
	path: CALLBACK_PATH,
	iss: 'http://127.0.0.1:18097',
	state: 'state-0123456789abcdef'
};
const target = (query: string, path = CALLBACK_PATH): string => `${path}?${query}`;

test('a clean grant yields the code', () => {
	const verdict = parseCallback(
		target(`code=abc&state=${expected.state}&iss=${encodeURIComponent(expected.iss)}`),
		expected
	);
	assert.deepEqual(verdict, { ok: true, code: 'abc' });
});

test('a verified error carries its text', () => {
	const verdict = parseCallback(
		target(
			`error=access_denied&error_description=consent_denied&state=${expected.state}&iss=${encodeURIComponent(expected.iss)}`
		),
		expected
	);
	assert.deepEqual(verdict, {
		ok: false,
		kind: 'error',
		error: 'access_denied',
		description: 'consent_denied'
	});
});

test('the iss parameter is mandatory and must match byte for byte', () => {
	assert.deepEqual(parseCallback(target(`code=abc&state=${expected.state}`), expected), {
		ok: false,
		kind: 'invalid',
		reason: 'iss_missing'
	});
	assert.deepEqual(
		parseCallback(
			target(`code=abc&state=${expected.state}&iss=${encodeURIComponent(`${expected.iss}/`)}`),
			expected
		),
		{ ok: false, kind: 'invalid', reason: 'iss_mismatch' }
	);
});

test('state is checked before the code or an error text is used', () => {
	const iss = encodeURIComponent(expected.iss);
	assert.deepEqual(parseCallback(target(`code=abc&iss=${iss}`), expected), {
		ok: false,
		kind: 'invalid',
		reason: 'state_missing'
	});
	assert.deepEqual(parseCallback(target(`error=access_denied&state=other&iss=${iss}`), expected), {
		ok: false,
		kind: 'invalid',
		reason: 'state_mismatch'
	});
});

test('the path is exact, parameters are not repeated, and a bare redirect is invalid', () => {
	const tail = `state=${expected.state}&iss=${encodeURIComponent(expected.iss)}`;
	assert.deepEqual(
		parseCallback(target(`code=abc&${tail}`, '/oauth/sylvode/callback/'), expected),
		{ ok: false, kind: 'invalid', reason: 'wrong_path' }
	);
	assert.deepEqual(parseCallback(target(`code=a&code=b&${tail}`), expected), {
		ok: false,
		kind: 'invalid',
		reason: 'duplicate_parameter'
	});
	assert.deepEqual(parseCallback(target(tail), expected), {
		ok: false,
		kind: 'invalid',
		reason: 'neither_code_nor_error'
	});
});

test('the listener takes exactly one callback, answers a page without script, and stops', async () => {
	const listener = await CallbackListener.start();
	assert.ok(listener.port >= 1024);
	assert.equal(listener.redirectUri, `http://127.0.0.1:${listener.port}${CALLBACK_PATH}`);
	const stray = await fetch(`http://127.0.0.1:${listener.port}/elsewhere`);
	assert.equal(stray.status, 404);
	assert.deepEqual(listener.strays, ['/elsewhere']);
	assert.equal(await listener.stayedQuiet(50), true);
	const wait = listener.waitForCallback(2000);
	const res = await fetch(`${listener.redirectUri}?code=abc&state=s`);
	assert.equal(res.status, 200);
	assert.equal(res.headers.get('cache-control'), 'no-store');
	assert.ok(!(await res.text()).includes('<script'));
	assert.equal(await wait, `${CALLBACK_PATH}?code=abc&state=s`);
	await assert.rejects(fetch(`${listener.redirectUri}?code=again`));
	await listener.close();
});

test('waiting for a callback that never comes fails after the timeout', async () => {
	const listener = await CallbackListener.start();
	await assert.rejects(listener.waitForCallback(100), /no redirect reached/);
	await listener.close();
});
