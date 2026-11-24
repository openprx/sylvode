import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { apiClient, type ApiResult } from '../../src/lib/api/client';
import { authApi } from '../../src/lib/api/auth';
import { setupBody, validateSetup, type SetupForm } from '../../src/lib/setup/rules';

const ROOT = new URL('../..', import.meta.url).pathname;
const good: SetupForm = {
	name: ' Ada ',
	email: ' ada@example.com ',
	password: 'correct horse',
	confirm: 'correct horse'
};

test('a complete form passes and the body is trimmed', () => {
	assert.equal(validateSetup(good), null);
	assert.deepEqual(setupBody(good), {
		name: 'Ada',
		email: 'ada@example.com',
		password: 'correct horse'
	});
});

test('each problem is reported by its own key, in field order', () => {
	assert.equal(validateSetup({ ...good, name: '  ' }), 'setup.errors.nameRequired');
	assert.equal(validateSetup({ ...good, email: 'ada' }), 'setup.errors.emailInvalid');
	assert.equal(validateSetup({ ...good, email: 'a b@c.d' }), 'setup.errors.emailInvalid');
	assert.equal(
		validateSetup({ ...good, password: 'short', confirm: 'short' }),
		'setup.errors.passwordShort'
	);
	assert.equal(validateSetup({ ...good, confirm: 'other horse' }), 'setup.errors.passwordMismatch');
	assert.equal(
		validateSetup({ ...good, password: '12345678', confirm: '12345678' }),
		null,
		'eight characters is enough'
	);
});

test('the status check is an unauthenticated GET of setup-status', async () => {
	const client = apiClient as unknown as Record<string, unknown>;
	const calls: string[] = [];
	const reply: ApiResult<{ needs_setup: boolean }> = {
		code: 0,
		message: '',
		data: { needs_setup: true }
	};
	client.get = (url: string) => {
		calls.push(url);
		return Promise.resolve(reply);
	};
	try {
		const result = await authApi.setupStatus();
		assert.deepEqual(calls, ['/api/v1/auth/setup-status']);
		assert.equal(result.data?.needs_setup, true);
	} finally {
		delete client.get;
	}
});

test('every setup text exists in en and zh with the same keys', () => {
	const read = (lang: string) =>
		JSON.parse(readFileSync(`${ROOT}src/lib/setup/locale.${lang}.json`, 'utf8')) as {
			setup: Record<string, unknown>;
		};
	const en = read('en').setup;
	const zh = read('zh').setup;
	assert.deepEqual(Object.keys(zh).sort(), Object.keys(en).sort());
	assert.deepEqual(
		Object.keys(zh.errors as object).sort(),
		Object.keys(en.errors as object).sort()
	);
});
