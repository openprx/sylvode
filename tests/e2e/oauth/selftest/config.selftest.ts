import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ConfigError, isLoopbackHost, loadConfig, secretsOf, type Env } from '../config.ts';

const base: Env = {
	OAUTH_E2E_ADMIN_EMAIL: 'admin@e2e.test',
	OAUTH_E2E_ADMIN_PASSWORD: 'admin-pw-1234',
	OAUTH_E2E_USER_A_EMAIL: 'a@e2e.test',
	OAUTH_E2E_USER_A_PASSWORD: 'a-pw-12345',
	OAUTH_E2E_USER_B_EMAIL: 'b@e2e.test',
	OAUTH_E2E_USER_B_PASSWORD: 'b-pw-12345',
	OAUTH_E2E_OUT_DIR: '/tmp/out'
};

const messageOf = (env: Env, mode: 'run' | 'setup'): string => {
	try {
		loadConfig(env, mode);
	} catch (error) {
		assert.ok(error instanceof ConfigError);
		return error.message;
	}
	return '';
};

test('defaults match the shared test environment', () => {
	const cfg = loadConfig(base, 'run');
	assert.equal(cfg.uiOrigin, 'http://127.0.0.1:18097');
	assert.equal(cfg.apiOrigin, 'http://127.0.0.1:18083');
	assert.equal(cfg.mcpResource, 'http://127.0.0.1:8090/mcp/rpc');
	assert.equal(cfg.clientId, 'ramflux');
	assert.equal(cfg.workspaceName, 'qa-p4-acceptance');
	assert.equal(cfg.limitMs, 5000);
	assert.deepEqual(
		secretsOf(cfg).map((s) => s.label),
		['password:admin', 'password:userA', 'password:userB']
	);
});

test('every missing variable is named at once', () => {
	const message = messageOf({}, 'run');
	for (const name of [
		'OAUTH_E2E_ADMIN_EMAIL',
		'OAUTH_E2E_ADMIN_PASSWORD',
		'OAUTH_E2E_USER_A_EMAIL',
		'OAUTH_E2E_USER_B_PASSWORD',
		'OAUTH_E2E_OUT_DIR'
	]) {
		assert.ok(message.includes(name), `${name} not named`);
	}
	assert.ok(
		!messageOf({ ...base, OAUTH_E2E_OUT_DIR: undefined }, 'setup'),
		'setup does not need an output directory'
	);
});

test('a non-loopback host is refused unless the override is set', () => {
	assert.match(
		messageOf({ ...base, OAUTH_E2E_API_ORIGIN: 'https://sv.example.com' }, 'setup'),
		/not a loopback host/
	);
	assert.equal(
		messageOf(
			{
				...base,
				OAUTH_E2E_API_ORIGIN: 'https://sv.example.com',
				OAUTH_E2E_ALLOW_NON_LOOPBACK: '1'
			},
			'setup'
		),
		''
	);
	assert.deepEqual(
		['127.0.0.1', '127.9.9.9', 'localhost', '[::1]', 'example.com', '192.0.2.1'].map(isLoopbackHost),
		[true, true, true, true, false, false]
	);
});

test('names must carry the prefix, origins must be plain, accounts must differ', () => {
	assert.match(
		messageOf({ ...base, OAUTH_E2E_WORKSPACE_NAME: 'production' }, 'run'),
		/must start with "qa-p4-"/
	);
	assert.match(
		messageOf({ ...base, OAUTH_E2E_UI_ORIGIN: 'http://127.0.0.1:1/path' }, 'run'),
		/must be an origin/
	);
	assert.match(messageOf({ ...base, OAUTH_E2E_USER_B_EMAIL: 'A@e2e.test' }, 'run'), /must differ/);
});
