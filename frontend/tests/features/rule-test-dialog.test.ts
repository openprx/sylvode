/**
 * The rule test dialog's states as rendered: matched, no match, a draft rule included, unknown
 * codes printed as the code, an error, hostile content as plain text, no stale result.
 *
 * Only the first render is observable (effects do not run in SSR) and the stand-in i18n prints
 * message keys, so the assertions look for keys, codes and escaped content.
 *
 * Run standalone: `bun test tests/features/rule-test-dialog.test.ts`
 */
import { test } from 'node:test';
import nodeAssert from 'node:assert/strict';
import { Suite, assert } from '../support/harness';
import { installSvelteSsr, renderToHtml } from '../support/svelte-ssr';
import { begin, settle } from '../../src/lib/project/inbound/test-view';
import { parseTestResult, type TestResult } from '../../src/lib/project/inbound/wire';

await installSvelteSsr();
const { default: Outcome } = await import('../../src/lib/project/inbound/RuleTestOutcome.svelte');
const { default: Dialog } = await import('../../src/lib/project/inbound/RuleTestDialog.svelte');

const suite = new Suite('rule-test-dialog');

const result = (over: Record<string, unknown> = {}): TestResult => {
	const parsed = parseTestResult({
		matched: true,
		rule: { id: 'r2', name: 'WAF high', position: 1, matched: true, reason: 'matched' },
		rules_evaluated: [
			{ id: 'r1', name: 'Other', position: 0, matched: false, reason: 'kind_not_listed' },
			{ id: 'r2', name: 'WAF high', position: 1, matched: true, reason: 'matched' },
			{ id: 'r3', name: 'Later', position: 2, matched: false, reason: 'disabled' }
		],
		alert: 'would_open',
		task: 'would_start',
		queue_cause: null,
		would_escalate: true,
		caps: {
			tasks_last_hour: 1,
			active_for_rule: 0,
			active_in_workspace: 0,
			max_tasks_per_hour: 6,
			max_active_tasks: 2,
			max_active_tasks_per_workspace: 3,
			alerts_opened_last_hour: 1,
			max_alerts_per_rule_per_hour: 10
		},
		notify_recipients: 2,
		title: '[HIGH] src · waf.x · host',
		...over
	});
	assert(parsed !== null, 'fixture parses');
	return parsed;
};
const html = (state: Parameters<typeof settle>[0] | null, mode: 'recent' | 'sample' = 'recent') =>
	renderToHtml(Outcome, {
		state: state === null ? begin() : settle(state, mode)
	});
const ok = (value: TestResult) => ({ ok: true, value }) as const;
const fail = (code: number, message: string) =>
	({ ok: false, kind: 'error', code, message, errorCode: null }) as const;

await suite.checkAsync(
	'matched: verdict, rows, limits, checked list and the forecast footnote',
	async () => {
		const out = await html(ok(result()));
		for (const key of [
			'inbound.test.verdict.match',
			'inbound.test.row.alert',
			'inbound.test.alert.would_open',
			'inbound.test.task.would_start',
			'inbound.test.row.escalation',
			'inbound.test.row.recipients.other',
			'inbound.test.limit.tasksHour',
			'inbound.test.limit.alertsHour',
			'inbound.test.row.rules',
			'inbound.test.reason.kind_not_listed',
			'inbound.test.reason.matched',
			'inbound.test.reason.notChecked',
			'inbound.test.forecast'
		])
			assert(out.includes(key), `contains ${key}`);
		assert(!out.includes('inbound.test.reason.disabled'), 'rule after the match is not decided');
		assert(out.includes('aria-live="polite"'), 'live region');
	}
);

await suite.checkAsync(
	'no match: the event would only be stored; no limits block without caps',
	async () => {
		const out = await html(
			ok(
				result({
					matched: false,
					rule: null,
					alert: 'no_rule',
					task: 'none',
					would_escalate: false,
					caps: null,
					rules_evaluated: []
				})
			)
		);
		assert(out.includes('inbound.test.verdict.noMatch'), 'no match line');
		assert(out.includes('inbound.test.alert.no_rule'), 'no alert');
		assert(!out.includes('inbound.test.row.limits'), 'no limits without caps');
		assert(!out.includes('inbound.test.row.escalation'), 'no escalation row');
	}
);

await suite.checkAsync('a draft rule is marked in the checked list', async () => {
	const out = await html(
		ok(
			result({
				rule: {
					id: 'r2',
					name: 'My draft',
					position: 1,
					matched: true,
					reason: 'matched',
					draft: true
				},
				rules_evaluated: [
					{ id: 'r2', name: 'My draft', position: 1, matched: true, reason: 'matched', draft: true }
				]
			})
		)
	);
	assert(out.includes('My draft'), 'name');
	assert(out.includes('inbound.test.reason.draft'), 'unsaved marker');
});

await suite.checkAsync('unknown codes are printed as the code, in monospace', async () => {
	const out = await html(
		ok(
			result({
				alert: 'would_teleport',
				task: 'would_levitate',
				rules_evaluated: [
					{ id: 'r1', name: 'A', position: 0, matched: false, reason: 'cosmic_rays' }
				],
				rule: null,
				matched: false
			})
		)
	);
	assert(new RegExp('<code[^>]*>would_teleport</code>').test(out), 'alert code');
	assert(new RegExp('<code[^>]*>would_levitate</code>').test(out), 'task code');
	assert(new RegExp('<code[^>]*>cosmic_rays</code>').test(out), 'reason code');
	assert(!out.includes('inbound.test.alert.would_teleport'), 'no made-up key');
});

await suite.checkAsync('hostile content is escaped text, never markup', async () => {
	const evil = '<img src=x onerror=alert(1)><script>alert(2)</script>';
	const out = await html(
		ok(
			result({
				title: evil,
				rule: { id: 'r2', name: evil, position: 1, matched: true, reason: 'matched' },
				rules_evaluated: [{ id: 'r2', name: evil, position: 1, matched: true, reason: 'matched' }],
				alert: evil
			})
		)
	);
	assert(!out.includes('<img'), 'no element from the title');
	assert(!out.includes('<script'), 'no script element');
	assert(out.includes('&lt;img src=x onerror=alert(1)>'), 'shown literally');
	const failed = await html(fail(500, evil));
	assert(!failed.includes('<img') && !failed.includes('<script'), 'error text is escaped too');
});

await suite.checkAsync('error: tip bar with the message, no result', async () => {
	const out = await html(fail(500, 'database unavailable'));
	assert(out.includes('rule-test-error'), 'error bar');
	assert(out.includes('database unavailable'), 'message');
	assert(out.includes('inbound.test.more.failed'), 'lead-in');
	assert(!out.includes('rule-test-result'), 'no result block');
	const gone = await html(fail(404, 'inbound event not found'));
	assert(gone.includes('inbound.test.eventGone'), 'event gone copy');
	const denied = await html(fail(403, 'forbidden'));
	assert(denied.includes('inbound.forbidden'), 'forbidden copy');
});

await suite.checkAsync(
	'a sample field error shows under the field, not in the outcome area',
	async () => {
		const out = await html(fail(400, 'envelope $.kind: invalid'), 'sample');
		assert(!out.includes('rule-test-error'), 'no tip bar');
		assert(!out.includes('rule-test-result'), 'no result');
	}
);

await suite.checkAsync(
	'stale result is cleared: running and failed runs render no result',
	async () => {
		const running = await renderToHtml(Outcome, { state: begin() });
		assert(running.includes('inbound.test.running'), 'running text');
		assert(!running.includes('rule-test-result'), 'no result while running');
		const failed = await html(fail(500, 'x'));
		assert(!failed.includes('inbound.test.verdict'), 'no verdict after a failure');
	}
);

await suite.checkAsync(
	'dialog: open, the first paint says nothing is created and offers the run',
	async () => {
		const out = await renderToHtml(Dialog, {
			open: true,
			onclose: () => {},
			projectId: 'p'
		});
		assert(out.includes('inbound.test.title'), 'title');
		assert(out.includes('inbound.test.more.nothingCreated'), 'nothing-created copy');
		assert(out.includes('inbound.test.run'), 'run button');
		assert(!out.includes('rule-test-result'), 'no result before a run');
		assert(!out.includes('rule-test-draft'), 'no draft notice without a draft');
		const drafted = await renderToHtml(Dialog, {
			open: true,
			onclose: () => {},
			projectId: 'p',
			draftRule: { name: 'd' }
		});
		assert(drafted.includes('inbound.test.more.draftIncluded'), 'draft notice');
		const closed = await renderToHtml(Dialog, {
			open: false,
			onclose: () => {},
			projectId: 'p'
		});
		assert(!closed.includes('inbound.test.title'), 'closed renders nothing');
	}
);

test(suite.name, () => {
	const result = suite.result();
	nodeAssert.equal(result.failed, 0, result.failures.join('\n'));
});
