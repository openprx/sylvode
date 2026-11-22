/**
 * The rule test without the dialog: sample event checks, request shape, state of a run, result rows.
 *
 * Run standalone: `bun tests/rule-test-view.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	SAMPLE_MAX_BYTES,
	begin,
	buildRequest,
	defaultEvent,
	defaultMode,
	defaultSource,
	failureOf,
	limitRows,
	parseSample,
	ruleRows,
	sampleFieldError,
	sampleText,
	settle,
	verdictView
} from '../src/lib/project/inbound/test-view';
import { parseTestResult, type TestResult } from '../src/lib/project/inbound/wire';

const suite = new Suite('rule-test-view');

const answer = (over: Record<string, unknown> = {}): TestResult => {
	const parsed = parseTestResult({
		matched: true,
		rule: { id: 'r2', name: 'WAF high', position: 1, matched: true, reason: 'matched' },
		rules_evaluated: [
			{ id: 'r1', name: 'Other', position: 0, matched: false, reason: 'kind_not_listed' },
			{ id: 'r2', name: 'WAF high', position: 1, matched: true, reason: 'matched' },
			{ id: 'r3', name: 'Later', position: 2, matched: true, reason: 'matched' }
		],
		alert: 'would_open',
		task: 'would_queue',
		queue_cause: 'task_cap',
		would_escalate: false,
		caps: {
			tasks_last_hour: 6,
			active_for_rule: 1,
			active_in_workspace: 2,
			max_tasks_per_hour: 6,
			max_active_tasks: 2,
			max_active_tasks_per_workspace: 3,
			alerts_opened_last_hour: 4,
			max_alerts_per_rule_per_hour: 10
		},
		notify_recipients: 2,
		title: '[HIGH] src · waf.x · host',
		...over
	});
	assert(parsed !== null, 'fixture parses');
	return parsed;
};
const fail = (code: number, message: string) =>
	({ ok: false, kind: 'error', code, message, errorCode: null }) as const;

suite.check('sample text is a valid envelope with the editor kind or the default', () => {
	const now = new Date('2026-11-22T12:00:00Z');
	const filled = JSON.parse(sampleText('e-1', now, 'waf.login_failed'));
	assertEqual(filled.spec, 'sylvode.inbound_event.v1', 'spec');
	assertEqual(filled.event_id, 'e-1', 'fresh id');
	assertEqual(filled.occurred_at, '2026-11-22T12:00:00.000Z', 'now');
	assertEqual(filled.kind, 'waf.login_failed', 'editor kind');
	assertEqual(filled.severity, 'high', 'severity');
	assert(typeof filled.title === 'string' && filled.title !== '', 'title');
	assertEqual(JSON.parse(sampleText('e', now)).kind, 'waf.attack_detected', 'default kind');
	assertEqual(JSON.parse(sampleText('e', now, '  ')).kind, 'waf.attack_detected', 'blank kind');
	assert(parseSample(sampleText('e', now)).ok, 'the default text passes the checks');
});

suite.check('sample checks: empty, not JSON, not an object, too large', () => {
	assertDeepEqual(parseSample('  \n'), { ok: false, error: 'empty' }, 'empty');
	assertDeepEqual(parseSample('{"a":'), { ok: false, error: 'notJson' }, 'broken');
	assertDeepEqual(parseSample('[1]'), { ok: false, error: 'notObject' }, 'array');
	assertDeepEqual(parseSample('"x"'), { ok: false, error: 'notObject' }, 'string');
	assertDeepEqual(parseSample('null'), { ok: false, error: 'notObject' }, 'null');
	const atCap = `{"a":"${'x'.repeat(SAMPLE_MAX_BYTES - 8)}"}`;
	assertEqual(new TextEncoder().encode(atCap).length, SAMPLE_MAX_BYTES, 'fixture at the cap');
	assert(parseSample(atCap).ok, 'exactly the cap passes');
	assertDeepEqual(parseSample(`${atCap} `), { ok: false, error: 'tooLarge' }, 'one over');
	// The cap counts bytes, not characters.
	const wide = `{"a":"${'é'.repeat(SAMPLE_MAX_BYTES / 2)}"}`;
	assertDeepEqual(parseSample(wide), { ok: false, error: 'tooLarge' }, 'multi-byte');
	const ok = parseSample('{"kind":"x"}');
	assert(ok.ok && ok.envelope.kind === 'x', 'object accepted');
});

suite.check('request: draft_rule and replaces are top level; one input only', () => {
	const draft = { name: 'd' } as never;
	assertDeepEqual(
		buildRequest({ sourceId: 's', mode: 'recent', eventId: 'e', envelope: { a: 1 } }),
		{ source_id: 's', inbound_event_id: 'e' },
		'recent ignores the envelope'
	);
	assertDeepEqual(
		buildRequest({ sourceId: 's', mode: 'sample', eventId: 'e', envelope: { a: 1 } }),
		{ source_id: 's', envelope: { a: 1 } },
		'sample ignores the event'
	);
	assertDeepEqual(
		buildRequest({
			sourceId: 's',
			mode: 'recent',
			eventId: 'e',
			envelope: null,
			draftRule: draft,
			replaces: 'r1'
		}),
		{ source_id: 's', inbound_event_id: 'e', draft_rule: draft, replaces: 'r1' },
		'draft with replaces'
	);
	const last = buildRequest({
		sourceId: 's',
		mode: 'sample',
		eventId: null,
		envelope: {},
		draftRule: draft
	});
	assert(last !== null && !('replaces' in last), 'no replaces without a saved rule');
	assertEqual(
		buildRequest({ sourceId: 's', mode: 'recent', eventId: null, envelope: null }),
		null,
		'recent without an event'
	);
	assertEqual(
		buildRequest({ sourceId: 's', mode: 'sample', eventId: null, envelope: null }),
		null,
		'sample without an envelope'
	);
	const without = buildRequest({ sourceId: 's', mode: 'recent', eventId: 'e', envelope: null });
	assert(without !== null && !('draft_rule' in without), 'no draft key without a draft');
});

suite.check('defaults: source, event and mode', () => {
	const list = [{ id: 'a' }, { id: 'b' }];
	assertEqual(defaultSource(list, 'b'), 'b', 'preselected source');
	assertEqual(defaultSource(list, 'zz'), 'a', 'unknown preselected falls back to first');
	assertEqual(defaultSource([], 'a'), null, 'none');
	assertEqual(defaultEvent(list, 'b'), 'b', 'preselected event');
	assertEqual(defaultEvent(list, null), 'a', 'newest');
	assertEqual(defaultEvent([], 'a'), null, 'none');
	assertEqual(defaultMode(3), 'recent', 'has events');
	assertEqual(defaultMode(0), 'sample', 'no events');
});

suite.check('verdict: match carries ordinal, draft flag, words and cause', () => {
	const view = verdictView(answer());
	assert(view.kind === 'match', 'match');
	assertEqual(view.position, 2, 'ordinal among the rules checked, 1-based');
	assertEqual(view.name, 'WAF high', 'name');
	assertEqual(view.draft, false, 'not a draft');
	assertDeepEqual(view.alert, { key: 'inbound.test.alert.would_open' }, 'alert word');
	assertDeepEqual(view.task, { key: 'inbound.test.task.would_queue' }, 'task word');
	assertDeepEqual(view.cause, { key: 'inbound.test.cause.task_cap' }, 'cause');
	const none = verdictView(answer({ matched: false, rule: null, alert: 'no_rule', task: 'none' }));
	assertEqual(none.kind, 'noMatch', 'no match');
});

suite.check('verdict: a draft is marked; unknown codes come back as the code itself', () => {
	const drafted = answer({
		rule: { id: 'r2', name: 'WAF high', position: 1, matched: true, reason: 'matched', draft: true }
	});
	const view = verdictView(drafted);
	assert(view.kind === 'match' && view.draft, 'draft flagged');
	const odd = verdictView(answer({ alert: 'future_alert', task: 'future_task', queue_cause: 'x' }));
	assert(odd.kind === 'match', 'match');
	assertDeepEqual(odd.alert, { key: null, code: 'future_alert' }, 'unknown alert');
	assertDeepEqual(odd.task, { key: null, code: 'future_task' }, 'unknown task');
	assertDeepEqual(odd.cause, { key: null, code: 'x' }, 'unknown cause');
});

suite.check('rules checked: decided up to the match, the rest "not checked"', () => {
	const rows = ruleRows(answer());
	assertEqual(rows.length, 3, 'all rules listed');
	assertEqual(rows[0].state, 'reason', 'before the match: its reason');
	assertDeepEqual(
		rows[0].state === 'reason' ? rows[0].reason : null,
		{ key: 'inbound.test.reason.kind_not_listed' },
		'reason word'
	);
	assertEqual(rows[1].state, 'matched', 'the match');
	assertEqual(rows[2].state, 'notChecked', 'after the match, although the server says matched');
	assertDeepEqual(
		rows.map((r) => r.position),
		[1, 2, 3],
		'positions follow the list order'
	);
});

suite.check(
	'rules checked: without a match every rule shows a reason; unknown reason is the code',
	() => {
		const rows = ruleRows(
			answer({
				matched: false,
				rule: null,
				rules_evaluated: [
					{ id: 'a', name: 'A', position: 0, matched: false, reason: 'disabled' },
					{ id: 'b', name: 'B', position: 1, matched: false, reason: 'brand_new', draft: true }
				]
			})
		);
		assert(
			rows.every((r) => r.state === 'reason'),
			'all reasons'
		);
		const second = rows[1];
		assert(second.state === 'reason' && second.draft, 'draft row flagged');
		assertDeepEqual(
			second.state === 'reason' ? second.reason : null,
			{ key: null, code: 'brand_new' },
			'code'
		);
		assertEqual(ruleRows(answer({ rules_evaluated: [] })).length, 0, 'no rules');
	}
);

suite.check('limits: four rows with value and maximum', () => {
	const caps = answer().caps;
	assert(caps !== null, 'caps');
	assertDeepEqual(
		limitRows(caps),
		[
			{ key: 'tasksHour', value: 6, max: 6 },
			{ key: 'active', value: 1, max: 2 },
			{ key: 'workspace', value: 2, max: 3 },
			{ key: 'alertsHour', value: 4, max: 10 }
		],
		'rows'
	);
});

suite.check('run state: a failed call carries no earlier result (stale result cleared)', () => {
	let state = settle({ ok: true, value: answer() }, 'recent');
	assertEqual(state.phase, 'result', 'first run shows a result');
	state = begin();
	assertEqual(state.phase, 'running', 'running drops the result');
	assert(!('result' in state), 'no result while running');
	state = settle(fail(500, 'boom'), 'recent');
	assertEqual(state.phase, 'error', 'failure');
	assert(!('result' in state), 'no result after a failure');
	assertDeepEqual(
		state.phase === 'error' ? state.failure : null,
		{ place: 'bar', message: 'boom', forbidden: false },
		'bar'
	);
});

suite.check('failures: field, event gone, forbidden, other', () => {
	assertDeepEqual(
		failureOf(fail(400, 'envelope $.kind: must be 1-64 characters'), 'sample'),
		{ place: 'sample', message: 'envelope $.kind: must be 1-64 characters' },
		'bad envelope under the field'
	);
	assertEqual(
		failureOf(fail(400, 'name must be 1-100 characters'), 'sample').place,
		'bar',
		'a bad draft rule is not a sample error'
	);
	assertEqual(
		failureOf(fail(400, 'envelope $.kind: x'), 'recent').place,
		'bar',
		'no sample field in recent mode'
	);
	assertEqual(failureOf(fail(404, 'inbound event not found'), 'recent').place, 'gone', 'pruned');
	assertEqual(failureOf(fail(404, 'project not found'), 'recent').place, 'bar', 'other 404');
	const forbidden = failureOf(fail(403, 'forbidden'), 'recent');
	assert(forbidden.place === 'bar' && forbidden.forbidden, 'forbidden');
	assertEqual(
		sampleFieldError(settle(fail(400, 'envelope $.spec: must be x'), 'sample')),
		'envelope $.spec: must be x',
		'field error surfaced'
	);
	assertEqual(sampleFieldError({ phase: 'idle' }), null, 'none when idle');
});

finish(suite.result());
