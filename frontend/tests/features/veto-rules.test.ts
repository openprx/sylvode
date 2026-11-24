import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { apiClient, type ApiResult } from '../../src/lib/api/client';
import { vetoApi } from '../../src/lib/api/veto';
import {
	MIN_VETO_REASON,
	normalizeDomainKey,
	proposalVetoDomains,
	reasonLength,
	reasonProblem,
	vetoDomainsFor,
	vetoerFormProblem
} from '../../src/lib/governance/veto-rules';

const ROOT = new URL('../..', import.meta.url).pathname;
const ME = 'u-me';
const rows = [
	{ user_id: ME, domain: 'engineering' },
	{ user_id: ME, domain: 'global' },
	{ user_id: 'u-other', domain: 'design' }
];

test('domain keys are normalised like the server does', () => {
	assert.equal(normalizeDomainKey('  Engineering '), 'engineering');
	assert.equal(normalizeDomainKey('Dev Ops/QA'), 'dev_ops_qa');
	assert.equal(normalizeDomainKey('a-b_c1'), 'a-b_c1');
	assert.equal(normalizeDomainKey('  '), '');
});

test('a proposal without domains is vetoed in global', () => {
	assert.deepEqual(proposalVetoDomains([]), ['global']);
	assert.deepEqual(proposalVetoDomains(['', ' ']), ['global']);
	assert.deepEqual(proposalVetoDomains(['Engineering', 'engineering', 'Design']), [
		'engineering',
		'design'
	]);
});

test('the veto button needs a vetoer seat in a domain of an open or voting proposal', () => {
	assert.deepEqual(vetoDomainsFor('open', ['Engineering', 'Design'], rows, ME), ['engineering']);
	assert.deepEqual(vetoDomainsFor('voting', [], rows, ME), ['global']);
	assert.deepEqual(vetoDomainsFor('draft', ['engineering'], rows, ME), []);
	assert.deepEqual(vetoDomainsFor('vetoed', ['engineering'], rows, ME), []);
	assert.deepEqual(vetoDomainsFor('approved', ['engineering'], rows, ME), []);
	assert.deepEqual(vetoDomainsFor('open', ['design'], rows, ME), [], 'a seat of someone else');
	assert.deepEqual(vetoDomainsFor('open', ['engineering'], rows, ''), [], 'no signed-in user');
});

test('the reason is counted in characters of the trimmed text', () => {
	assert.equal(reasonLength('  abc  '), 3);
	assert.equal(reasonLength('\u{1F600}'.repeat(2)), 2, 'one code point is one character');
	assert.equal(reasonProblem('   '), 'empty');
	assert.equal(reasonProblem('x'.repeat(MIN_VETO_REASON - 1)), 'short');
	assert.equal(reasonProblem(`  ${'x'.repeat(MIN_VETO_REASON - 1)}  `), 'short');
	assert.equal(reasonProblem('x'.repeat(MIN_VETO_REASON)), null);
});

test('a vetoer entry needs a member, a domain and no duplicate', () => {
	assert.equal(vetoerFormProblem({ userId: '', domain: 'x' }, rows), 'member');
	assert.equal(vetoerFormProblem({ userId: ME, domain: ' ' }, rows), 'domain');
	assert.equal(vetoerFormProblem({ userId: ME, domain: 'Engineering' }, rows), 'duplicate');
	assert.equal(vetoerFormProblem({ userId: ME, domain: 'design' }, rows), null);
});

test('the veto client sends the documented routes and bodies', async () => {
	const client = apiClient as unknown as Record<string, unknown>;
	const calls: Array<{ method: string; url: string; body?: unknown }> = [];
	const reply: ApiResult<unknown> = { code: 0, message: '', data: { veto_id: 1 } };
	for (const method of ['post', 'delete']) {
		client[method] = (url: string, body?: unknown) => {
			calls.push({ method, url, body });
			return Promise.resolve(reply);
		};
	}
	try {
		const key = { user_id: 'u1', project_id: 'p1', domain: 'engineering' };
		await vetoApi.exercise('PROP-1', { reason: 'r', domain: 'engineering' });
		await vetoApi.createVetoer({ ...key, granted_by: 'manual_grant' });
		await vetoApi.deleteVetoer(key);
		assert.deepEqual(calls, [
			{
				method: 'post',
				url: '/api/v1/proposals/PROP-1/veto',
				body: { reason: 'r', domain: 'engineering' }
			},
			{
				method: 'post',
				url: '/api/v1/vetoers',
				body: { ...key, granted_by: 'manual_grant' }
			},
			{ method: 'delete', url: '/api/v1/vetoers', body: key }
		]);
	} finally {
		delete client.post;
		delete client.delete;
	}
});

test('every veto text exists in en and zh with the same keys', () => {
	const flat = (node: unknown, prefix = ''): string[] =>
		typeof node === 'object' && node !== null
			? Object.entries(node).flatMap(([k, v]) => flat(v, prefix ? `${prefix}.${k}` : k))
			: [prefix];
	const read = (lang: string) =>
		JSON.parse(readFileSync(`${ROOT}src/lib/governance/locale.${lang}.json`, 'utf8'));
	assert.deepEqual(flat(read('zh')).sort(), flat(read('en')).sort());
});
