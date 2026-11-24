import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { isNotEnabled } from '../../src/lib/api/oauth';

const LIB = join(new URL('../..', import.meta.url).pathname, 'src/lib');
const failure = (status: number, errorCode: string) => ({ status, errorCode, details: null });

test('feature_disabled means the server has no authorization server', () => {
	assert.equal(isNotEnabled(failure(403, 'feature_disabled'), true), true);
	assert.equal(isNotEnabled(failure(403, 'feature_disabled'), false), true);
});

test('a plain 404 from a list call is treated the same (a server without the answer)', () => {
	assert.equal(isNotEnabled(failure(404, ''), true), true);
	assert.equal(isNotEnabled(failure(404, ''), false), false);
});

test('other failures stay failures', () => {
	assert.equal(isNotEnabled(failure(403, 'forbidden'), true), false);
	assert.equal(isNotEnabled(failure(500, ''), true), false);
	assert.equal(isNotEnabled(failure(429, 'rate_limited'), true), false);
});

test('the three panels show the not-enabled state and the text exists in both languages', () => {
	for (const panel of ['Personal', 'Workspace', 'Admin']) {
		const source = readFileSync(join(LIB, `oauth/ConnectedApps${panel}.svelte`), 'utf8');
		assert.match(source, /isNotEnabled\(/, `${panel} checks for the disabled answer`);
		assert.match(source, /kind="empty"/, `${panel} renders an empty state`);
		assert.match(source, /oauth\.notEnabled/, `${panel} uses the message`);
	}
	const en = JSON.parse(readFileSync(join(LIB, 'oauth/locale.en.json'), 'utf8'));
	const zh = JSON.parse(readFileSync(join(LIB, 'oauth/locale.zh.json'), 'utf8'));
	assert.equal(en.oauth.notEnabled, 'Connected applications are not enabled on this server.');
	assert.equal(zh.oauth.notEnabled, '此服务器未启用已连接的应用。');
});
