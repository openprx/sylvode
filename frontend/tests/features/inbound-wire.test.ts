/**
 * The inbound API client and its boundary parsers: every parser accepts the documented shapes
 * and refuses wrong types, a stale rule version becomes the typed conflict result, the request
 * shapes are what the rules document specifies, and the secrets appear only in the create and
 * rotate answers.
 *
 * Run standalone: `bun test tests/features/inbound-wire.test.ts`
 */
import { test } from 'node:test';
import nodeAssert from 'node:assert/strict';
import { Suite, assert, assertDeepEqual, assertEqual } from '../support/harness';
import { apiClient, type ApiResult } from '../../src/lib/api/client';
import { inboundApi } from '../../src/lib/api/inbound';
import {
	parseEvent,
	parseEvents,
	parseRule,
	parseRuleWrite,
	parseRules,
	parseSecretsAnswer,
	parseSource,
	parseSourceList,
	parseTestResult
} from '../../src/lib/project/inbound/wire';

const suite = new Suite('inbound-wire');

const TOKEN = `svi_${'a1'.repeat(32)}`;
const SECRET = `svs_${'b2'.repeat(32)}`;

const sourceJson = {
	id: 's1',
	workspace_id: 'w1',
	project_id: 'p1',
	name: 'prx-waf edge-01',
	format: 'native_v1',
	token_prefix: 'svi_1a2b3c4d',
	require_signature: true,
	has_signing_secret: true,
	ip_allowlist: ['192.0.2.0/24'],
	acting_user_id: 'u1',
	enabled: true,
	rate_per_minute: 120,
	daily_cap: 20000,
	last_used_at: '2026-11-21T10:00:00Z',
	last_used_ip: '192.0.2.7',
	created_by: 'u1',
	created_at: '2026-11-20T10:00:00Z',
	updated_at: '2026-11-21T10:00:00Z'
};

const eventJson = {
	id: 'e1',
	event_id: 'evt-1',
	kind: 'waf.attack_detected',
	severity_rank: 3,
	dedup_key: 'k',
	occurred_at: '2026-11-21T10:00:00Z',
	received_at: '2026-11-21T10:00:01Z',
	title: '<img src=x onerror=alert(1)>',
	summary: 'text',
	subject: { host: 'h' },
	payload: { a: 1 },
	signed: true,
	client_ip: '192.0.2.7',
	outcome: 'alert_opened',
	rule_id: 'r1',
	alert_id: 'a1',
	task_id: null,
	work_item: { id: 'i1', identifier: 'OPS-12' }
};

const ruleJson = {
	id: 'r1',
	workspace_id: 'w1',
	project_id: 'p1',
	name: 'WAF high',
	position: 10,
	enabled: true,
	source_id: null,
	kind_patterns: ['waf.*'],
	min_severity: 'high',
	action: 'work_item_and_task',
	bot_user_id: 'b1',
	instructions: '',
	priority_map: { critical: 'urgent', high: 'high', medium: 'medium', low: 'low', info: 'low' },
	label_ids: [],
	notify_user_ids: ['u1'],
	max_tasks_per_hour: 6,
	max_active_tasks: 2,
	dedup_window_secs: 3600,
	rearm_after_secs: 900,
	task_max_attempts: 3,
	version: 4
};

const verdict = (extra: object = {}) => ({
	id: 'r1',
	name: 'WAF high',
	position: 10,
	matched: true,
	reason: 'matched',
	...extra
});

const testJson = {
	matched: true,
	rule: verdict(),
	rules_evaluated: [verdict(), verdict({ id: 'r2', matched: false, reason: 'disabled' })],
	alert: 'would_open',
	task: 'would_queue',
	queue_cause: 'task_cap',
	would_escalate: false,
	caps: {
		tasks_last_hour: 6,
		active_for_rule: 1,
		active_in_workspace: 1,
		max_tasks_per_hour: 6,
		max_active_tasks: 2,
		max_active_tasks_per_workspace: 5,
		alerts_opened_last_hour: 0,
		max_alerts_per_rule_per_hour: 20
	},
	notify_recipients: 1,
	title: '[HIGH] prx-waf · waf.attack_detected · h'
};

suite.check('a source: documented shape, G3 fields optional', () => {
	const source = parseSource(sourceJson);
	assert(source !== null, 'parsed');
	assertEqual(source.suspended, null, 'no verdict without G3');
	assertEqual(source.acting_user_name, null, 'no name without G3');
	const withG3 = parseSource({ ...sourceJson, suspended: true, acting_user_name: 'Ada' });
	assertEqual(withG3?.suspended, true, 'G3 verdict');
	assertEqual(withG3?.acting_user_name, 'Ada', 'G3 name');
	assertEqual(
		parseSource({ ...sourceJson, acting_user_id: null, last_used_at: null })?.acting_user_id,
		null,
		'nulls'
	);
});

suite.check('a source: wrong types are refused', () => {
	assertEqual(parseSource({ ...sourceJson, enabled: 'yes' }), null, 'enabled');
	assertEqual(parseSource({ ...sourceJson, rate_per_minute: '120' }), null, 'rate');
	assertEqual(parseSource({ ...sourceJson, ip_allowlist: [1] }), null, 'allow-list');
	assertEqual(parseSource({ ...sourceJson, suspended: 'x' }), null, 'suspended');
	assertEqual(parseSource({ ...sourceJson, token_prefix: undefined }), null, 'prefix');
	assertEqual(parseSource(null), null, 'null');
	assertEqual(parseSource([]), null, 'array');
});

suite.check('the source list has the G1 shape', () => {
	const list = parseSourceList({
		sources: [sourceJson],
		inbound_enabled: false,
		endpoint_url: null
	});
	assertEqual(list?.sources.length, 1, 'sources');
	assertEqual(list?.inbound_enabled, false, 'switch');
	assertEqual(list?.endpoint_url, null, 'address');
	assertEqual(
		parseSourceList({
			sources: [],
			inbound_enabled: true,
			endpoint_url: 'https://sv.example/api/v1/inbound/events'
		})?.endpoint_url,
		'https://sv.example/api/v1/inbound/events',
		'address kept'
	);
	assertEqual(parseSourceList([sourceJson]), null, 'the old bare array is not understood');
	assertEqual(parseSourceList({ sources: [], endpoint_url: null }), null, 'switch is required');
	assertEqual(
		parseSourceList({ sources: [{}], inbound_enabled: true, endpoint_url: null }),
		null,
		'bad row'
	);
});

suite.check('create and rotate answers carry the secrets; nothing else does', () => {
	const answer = parseSecretsAnswer({
		source: sourceJson,
		token: TOKEN,
		signing_secret: SECRET,
		endpoint_url: null
	});
	assertEqual(answer?.token, TOKEN, 'token');
	assertEqual(answer?.signing_secret, SECRET, 'secret');
	assertEqual(
		parseSecretsAnswer({ source: sourceJson, token: TOKEN, endpoint_url: null }),
		null,
		'no secret'
	);
	assertEqual(
		parseSecretsAnswer({
			source: sourceJson,
			token: '',
			signing_secret: SECRET,
			endpoint_url: null
		}),
		null,
		'empty token'
	);
	const text = JSON.stringify([
		parseSource(sourceJson),
		parseSourceList({ sources: [sourceJson], inbound_enabled: true, endpoint_url: null }),
		parseEvents([eventJson]),
		parseRules([ruleJson])
	]);
	assertEqual(text.includes('svi_' + 'a1'), false, 'no token in any other parser output');
});

suite.check('events: documented shape and G4 work item', () => {
	const event = parseEvent(eventJson);
	assertEqual(event?.work_item?.identifier, 'OPS-12', 'work item');
	assertEqual(parseEvent({ ...eventJson, work_item: null })?.work_item, null, 'no work item');
	assertEqual(parseEvent({ ...eventJson, work_item: { id: 'i' } }), null, 'half a work item');
	assertEqual(parseEvent({ ...eventJson, signed: 1 }), null, 'signed');
	assertEqual(parseEvent({ ...eventJson, severity_rank: 'high' }), null, 'rank');
	assertEqual(parseEvents({}), null, 'not a list');
	assertEqual(parseEvents([eventJson, 5]), null, 'bad row');
	assertEqual(
		parseEvent({ ...eventJson, outcome: 'brand_new' })?.outcome,
		'brand_new',
		'new codes pass'
	);
	assertEqual(parseEvent({ ...eventJson, title: 7 }), null, 'title');
});

suite.check('rules: documented shape', () => {
	assertEqual(parseRule(ruleJson)?.version, 4, 'version');
	assertEqual(parseRules([ruleJson])?.length, 1, 'list');
	assertEqual(parseRule({ ...ruleJson, min_severity: 'severe' }), null, 'severity');
	assertEqual(parseRule({ ...ruleJson, action: 'x' }), null, 'action');
	assertEqual(
		parseRule({ ...ruleJson, priority_map: { high: 'urgent' } }),
		null,
		'map needs all five'
	);
	assertEqual(parseRule({ ...ruleJson, kind_patterns: 'waf.*' }), null, 'patterns');
	assertEqual(parseRule({ ...ruleJson, version: '4' }), null, 'version type');
	const write = parseRuleWrite({ rule: ruleJson, warnings: ['bot_not_connected_to_project'] });
	assertDeepEqual(write?.warnings, ['bot_not_connected_to_project'], 'warnings');
	assertEqual(parseRuleWrite({ rule: ruleJson }), null, 'warnings required');
});

suite.check('rule test answers, including the G5 draft mark and unknown codes', () => {
	const result = parseTestResult(testJson);
	assertEqual(result?.rules_evaluated.length, 2, 'rows');
	assertEqual(result?.caps?.max_alerts_per_rule_per_hour, 20, 'caps');
	assertEqual(result?.rules_evaluated[0].draft, false, 'not a draft');
	const draft = parseTestResult({ ...testJson, rule: verdict({ draft: true }) });
	assertEqual(draft?.rule?.draft, true, 'draft');
	assertEqual(
		parseTestResult({ ...testJson, caps: null, rule: null, matched: false, queue_cause: null })
			?.caps,
		null,
		'no caps'
	);
	assertEqual(parseTestResult({ ...testJson, alert: 'brand_new' })?.alert, 'brand_new', 'new code');
	assertEqual(parseTestResult({ ...testJson, caps: { tasks_last_hour: 1 } }), null, 'half caps');
	assertEqual(parseTestResult({ ...testJson, would_escalate: 'no' }), null, 'escalate');
	assertEqual(parseTestResult({ ...testJson, rules_evaluated: [{}] }), null, 'bad verdict');
});

// --- the client ------------------------------------------------------------------------------

type Call = { method: string; url: string; body?: unknown };
const calls: Call[] = [];
let reply: ApiResult<unknown> = { code: 0, message: 'ok', data: null };
const client = apiClient as unknown as Record<string, unknown>;
for (const method of ['get', 'post', 'put', 'patch', 'delete']) {
	client[method] = (url: string, body?: unknown) => {
		calls.push({ method, url, body });
		return Promise.resolve(reply);
	};
}
const ok = (data: unknown): ApiResult<unknown> => ({ code: 0, message: 'ok', data });

await suite.checkAsync('requests: routes and bodies follow the rules document', async () => {
	calls.length = 0;
	reply = ok({ sources: [], inbound_enabled: true, endpoint_url: null });
	await inboundApi.listSources('p 1');
	reply = ok({ source: sourceJson, token: TOKEN, signing_secret: SECRET, endpoint_url: null });
	await inboundApi.createSource('p1', { name: 'n', require_signature: true });
	await inboundApi.rotateSource('s1');
	reply = ok(sourceJson);
	await inboundApi.updateSource('s1', { take_over: true });
	reply = ok({ deleted: true });
	await inboundApi.deleteSource('s1');
	await inboundApi.deleteRule('r1');
	reply = ok([eventJson]);
	await inboundApi.listEvents('s1');
	await inboundApi.listEvents('s1', { before: '2026-11-21T10:00:01Z', limit: 10 });
	reply = ok([ruleJson]);
	await inboundApi.listRules('p1');
	await inboundApi.reorderRules('p1', ['r2', 'r1']);
	assertDeepEqual(
		calls.map((c) => `${c.method} ${c.url}`),
		[
			'get /api/v1/projects/p%201/inbound-sources',
			'post /api/v1/projects/p1/inbound-sources',
			'post /api/v1/inbound-sources/s1/rotate',
			'patch /api/v1/inbound-sources/s1',
			'delete /api/v1/inbound-sources/s1',
			'delete /api/v1/alert-rules/r1',
			'get /api/v1/inbound-sources/s1/events?limit=50',
			'get /api/v1/inbound-sources/s1/events?limit=10&before=2026-11-21T10%3A00%3A01Z',
			'get /api/v1/projects/p1/alert-rules',
			'post /api/v1/projects/p1/alert-rules/reorder'
		],
		'routes'
	);
	assertDeepEqual(calls[3].body, { take_over: true }, 'only the changed field is sent');
	assertDeepEqual(calls[9].body, { rule_ids: ['r2', 'r1'] }, 'the full order');
});

await suite.checkAsync(
	'rule write and test bodies are sent as given (draft_rule included)',
	async () => {
		calls.length = 0;
		const body = {
			name: 'n',
			enabled: true,
			source_id: null,
			kind_patterns: [],
			min_severity: 'high' as const,
			action: 'work_item' as const,
			instructions: '',
			priority_map: {
				critical: 'urgent',
				high: 'high',
				medium: 'medium',
				low: 'low',
				info: 'low'
			} as const,
			label_ids: [],
			notify_user_ids: [],
			max_tasks_per_hour: 6,
			max_active_tasks: 2,
			dedup_window_secs: 3600,
			rearm_after_secs: 900,
			task_max_attempts: 3
		};
		reply = ok({ rule: ruleJson, warnings: [] });
		await inboundApi.createRule('p1', body);
		await inboundApi.replaceRule('r1', { ...body, expected_version: 4 });
		reply = ok(testJson);
		await inboundApi.testRules('p1', {
			source_id: 's1',
			inbound_event_id: 'e1',
			draft_rule: body,
			replaces: 'r1'
		});
		await inboundApi.testRules('p1', { source_id: 's1', envelope: { kind: 'x' } });
		assertEqual(calls[0].url, '/api/v1/projects/p1/alert-rules', 'create route');
		assertEqual('expected_version' in (calls[0].body as object), false, 'no version on create');
		assertEqual(calls[1].method, 'put', 'replace is a PUT');
		assertEqual((calls[1].body as { expected_version: number }).expected_version, 4, 'version');
		assertEqual('bot_user_id' in (calls[1].body as object), false, 'no bot without hand-to-bot');
		assertEqual(calls[2].url, '/api/v1/projects/p1/alert-rules/test', 'test route');
		assertEqual((calls[2].body as { draft_rule: unknown }).draft_rule, body, 'draft rule');
		assertEqual((calls[2].body as { replaces: string }).replaces, 'r1', 'replaces');
		assertEqual('draft_rule' in (calls[3].body as object), false, 'no draft by default');
	}
);

await suite.checkAsync('results: success, conflict, errors and unreadable answers', async () => {
	reply = ok({ rule: ruleJson, warnings: [] });
	const good = await inboundApi.replaceRule('r1', {} as never);
	assertEqual(good.ok, true, 'success');
	reply = { code: 409, message: 'stale', data: null, error_code: 'version_conflict' };
	const conflict = await inboundApi.replaceRule('r1', {} as never);
	assert(!conflict.ok && conflict.kind === 'conflict', 'typed conflict');
	reply = { code: 409, message: 'other', data: null };
	const other409 = await inboundApi.replaceRule('r1', {} as never);
	assert(
		!other409.ok && other409.kind === 'error' && other409.code === 409,
		'a 409 without the code is an error'
	);
	reply = { code: 403, message: 'only admins', data: null };
	const forbidden = await inboundApi.listSources('p1');
	assert(!forbidden.ok && forbidden.kind === 'error' && forbidden.code === 403, '403 kept');
	reply = { code: 400, message: 'name must be 1-100 characters', data: null, error_code: 'bad' };
	const bad = await inboundApi.createSource('p1', { name: '' });
	assert(
		!bad.ok && bad.kind === 'error' && bad.message.startsWith('name must'),
		'server message kept'
	);
	reply = ok({ unexpected: true });
	const garbled = await inboundApi.createSource('p1', { name: 'n' });
	assert(!garbled.ok && garbled.code === 500, 'an answer that is not understood fails');
});

test(suite.name, () => {
	const result = suite.result();
	nodeAssert.equal(result.failed, 0, result.failures.join('\n'));
});
