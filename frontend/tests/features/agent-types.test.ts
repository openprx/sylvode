import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { AGENT_TYPES, agentTypeKey, isAgentType } from '../../src/lib/admin/agent-types';

const ROOT = new URL('../..', import.meta.url).pathname;
const read = (path: string) => readFileSync(`${ROOT}${path}`, 'utf8');

test('the runner agent types are the ones an administrator can pick', () => {
	assert.deepEqual([...AGENT_TYPES], ['runner', 'openclaw', 'openprx', 'webhook', 'custom']);
	assert.ok(isAgentType('openprx'));
	assert.ok(!isAgentType('mcp'));
	assert.ok(!isAgentType(null));
});

test('bots without a runner agent type have no label key', () => {
	assert.equal(agentTypeKey('runner'), 'agentTypes.runner');
	assert.equal(agentTypeKey('mcp'), null);
	assert.equal(agentTypeKey(undefined), null);
});

test('every agent type has a name in en and zh', () => {
	for (const lang of ['en', 'zh']) {
		const messages = JSON.parse(read(`src/lib/admin/locale.${lang}.json`)) as {
			agentTypes: Record<string, string>;
		};
		for (const type of AGENT_TYPES) assert.ok(messages.agentTypes[type], `${lang} ${type}`);
	}
});

test('the admin user form and the API client no longer carry agent_config', () => {
	for (const file of [
		'src/routes/(app)/admin/users/+page.svelte',
		'src/lib/api/admin.ts',
		'src/lib/api/members.ts',
		'src/lib/api/workspaces.ts'
	]) {
		assert.ok(!/agent_?[cC]onfig/.test(read(file)), `${file} mentions agent config`);
	}
});
