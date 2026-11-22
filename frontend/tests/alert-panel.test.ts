/**
 * Alert panel on the work item page: response narrowing (`parseIssueAlert` in `lib/project/inbound/wire.ts`), the
 * pure view-model (`view.ts`), the feature locale files and the markup of the panel for every
 * state, including plain-text rendering of text written by an external system.
 *
 * Run standalone: `bun tests/alert-panel.test.ts`
 */
import { readFileSync } from 'node:fs';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import { parseIssueAlert, type IssueAlert } from '../src/lib/project/inbound/wire';
import {
	KIND_MAX,
	NAME_MAX,
	clampText,
	outcomeOf,
	severityWord,
	taskRowView
} from '../src/lib/project/alert-panel/view';

await installSvelteSsr();
const { default: AlertPanelView } =
	await import('../src/lib/project/alert-panel/AlertPanelView.svelte');

const suite = new Suite('alert-panel');

function wire(overrides: Record<string, unknown> = {}): Record<string, unknown> {
	return {
		issue_id: 'i1',
		alert_id: 'a1',
		source: { id: 's1', name: 'prx-waf edge-01' },
		kind: 'waf.attack_detected',
		severity: 'high',
		occurrences: 3,
		first_seen_at: '2026-11-21T10:00:00Z',
		last_seen_at: '2026-11-21T10:30:00Z',
		open: true,
		rule: { id: 'r1', name: 'WAF high to triage' },
		task: null,
		task_state: 'none',
		queued_at: null,
		queue_expires_at: null,
		...overrides
	};
}

function alert(overrides: Record<string, unknown> = {}): IssueAlert {
	const narrowed = parseIssueAlert(wire(overrides));
	if (narrowed === null) throw new Error('fixture must narrow');
	return narrowed;
}

const base = {
	failed: false,
	loading: false,
	workspaceId: 'w1',
	projectId: 'p1',
	isAdmin: false,
	onrefresh: () => {}
};

suite.check('narrowing accepts the documented shape and nulls', () => {
	const a = parseIssueAlert(wire());
	assert(a !== null, 'narrows');
	assertEqual(a?.occurrences, 3, 'occurrences');
	const bare = parseIssueAlert(
		wire({ source: null, rule: null, kind: null, severity: null, task: null })
	);
	assertEqual(bare?.source, null, 'deleted source');
	assertEqual(bare?.rule, null, 'deleted rule');
	const withTask = parseIssueAlert(wire({ task: { id: 't1', status: 'pending' } }));
	assertDeepEqual(withTask?.task, { id: 't1', status: 'pending' }, 'task');
});

suite.check('narrowing refuses wrong types', () => {
	assertEqual(parseIssueAlert(null), null, 'null');
	assertEqual(parseIssueAlert([]), null, 'array');
	assertEqual(parseIssueAlert(wire({ occurrences: '3' })), null, 'occurrences string');
	assertEqual(parseIssueAlert(wire({ open: 'yes' })), null, 'open string');
	assertEqual(parseIssueAlert(wire({ source: { id: 1, name: 'x' } })), null, 'source id');
	assertEqual(parseIssueAlert(wire({ task: { id: 't' } })), null, 'task status missing');
	assertEqual(parseIssueAlert(wire({ first_seen_at: 5 })), null, 'time number');
	assertEqual(parseIssueAlert(wire({ queue_expires_at: 5 })), null, 'expiry number');
});

suite.check('404 and 403 hide the panel, other failures show the retry state', () => {
	assertEqual(outcomeOf(0, true), 'shown', 'ok');
	assertEqual(outcomeOf(404, false), 'hidden', 'not an alert');
	assertEqual(outcomeOf(403, false), 'hidden', 'forbidden');
	assertEqual(outcomeOf(500, false), 'failed', 'server error');
	assertEqual(outcomeOf(0, false), 'failed', 'empty answer');
});

suite.check('task rows for none, created, waiting, suppressed and unknown', () => {
	assertDeepEqual(taskRowView(alert()), { kind: 'none' }, 'none');
	const created = taskRowView(
		alert({ task_state: 'created', task: { id: 't1', status: 'completed' } })
	);
	assertEqual(created.kind, 'created', 'created');
	if (created.kind === 'created') {
		assertEqual(created.labelKey, 'agentTasks.status.completed', 'agent tasks words');
		assertEqual(created.tone, 'success', 'tone');
	}
	const waiting = taskRowView(
		alert({ task_state: 'waiting', queue_expires_at: '2026-11-21T11:00:00Z' })
	);
	assertDeepEqual(waiting, { kind: 'waiting', until: '2026-11-21T11:00:00Z' }, 'waiting');
	assertDeepEqual(taskRowView(alert({ task_state: 'suppressed' })), { kind: 'suppressed' }, 'sup');
	assertDeepEqual(
		taskRowView(alert({ task_state: 'later_state' })),
		{ kind: 'unknown', code: 'later_state' },
		'unknown code'
	);
});

suite.check('severity words map to the shared severities', () => {
	for (const word of ['critical', 'high', 'medium', 'low', 'info']) {
		assertEqual(severityWord(word), word, word);
	}
	assertEqual(severityWord('bogus'), null, 'unknown');
	assertEqual(severityWord(null), null, 'null');
});

suite.check('clamping counts characters and marks the cut', () => {
	assertEqual(clampText('abc', 5), 'abc', 'short');
	assertEqual(clampText('abcdef', 3), 'abc…', 'cut');
	assertEqual(clampText('😀😀😀', 2), '😀😀…', 'code points');
	assertEqual(KIND_MAX, 64, 'kind limit');
	assertEqual(NAME_MAX, 100, 'name limit');
});

await suite.checkAsync('nothing renders for a non-alert item', async () => {
	const html = await renderToHtml(AlertPanelView, { ...base, alert: null });
	assert(!html.includes('alert-panel'), 'no panel');
});

await suite.checkAsync('retry state renders when loading failed', async () => {
	const html = await renderToHtml(AlertPanelView, { ...base, alert: null, failed: true });
	assert(html.includes('data-testid="alert-panel"'), 'frame');
	assert(html.includes('data-ui="state-block"') || html.includes('states.'), 'state block');
	assert(!html.includes('alert-panel-rows'), 'no rows');
});

await suite.checkAsync('shown state prints the facts without a task', async () => {
	const html = await renderToHtml(AlertPanelView, { ...base, alert: alert() });
	assert(html.includes('inbound.severity.high'), 'severity word');
	assert(html.includes('alertPanel.open'), 'open chip');
	assert(html.includes('prx-waf edge-01'), 'source');
	assert(html.includes('waf.attack_detected'), 'kind');
	assert(html.includes('WAF high to triage'), 'rule');
	assert(html.includes('alertPanel.taskNone'), 'no task');
	assert(!/<a[\s>]/.test(html), 'member sees no link');
});

await suite.checkAsync('closed alert and deleted source and rule', async () => {
	const html = await renderToHtml(AlertPanelView, {
		...base,
		alert: alert({ open: false, source: null, rule: null })
	});
	assert(html.includes('alertPanel.closed'), 'closed');
	assert(html.includes('alertPanel.deletedSource'), 'deleted source');
	assert(html.includes('alertPanel.deletedRule'), 'deleted rule');
});

await suite.checkAsync('admins get the rule link, members do not', async () => {
	const admin = await renderToHtml(AlertPanelView, { ...base, isAdmin: true, alert: alert() });
	assert(
		// The test build resolves a typed route to its pattern (see support/svelte-ssr.ts).
		admin.includes(
			'href="/(app)/workspace/[workspaceId]/projects/[projectId]/settings/alert-rules"'
		),
		'admin link to the rules section'
	);
	const member = await renderToHtml(AlertPanelView, { ...base, alert: alert() });
	assert(!member.includes('settings/alert-rules'), 'member plain text');
});

await suite.checkAsync('waiting and suppressed and created task rows', async () => {
	const waiting = await renderToHtml(AlertPanelView, {
		...base,
		alert: alert({ task_state: 'waiting', queue_expires_at: '2026-11-21T14:05:00Z' })
	});
	assert(waiting.includes('alertPanel.taskWaiting'), 'waiting chip');
	assert(waiting.includes('alertPanel.taskWaitingUntil'), 'expiry text');
	const suppressed = await renderToHtml(AlertPanelView, {
		...base,
		alert: alert({ task_state: 'suppressed' })
	});
	assert(suppressed.includes('alertPanel.taskSuppressed'), 'suppressed chip');
	const created = await renderToHtml(AlertPanelView, {
		...base,
		alert: alert({ task_state: 'created', task: { id: 't1', status: 'failed' } })
	});
	assert(created.includes('agentTasks.status.failed'), 'task status words');
	assert(created.includes('href="#agent-tasks-title"'), 'scroll link');
});

await suite.checkAsync('hostile external text is plain text', async () => {
	const hostile = '<script>alert(1)</script> [x](https://evil.example) https://evil.example/y';
	const html = await renderToHtml(AlertPanelView, {
		...base,
		isAdmin: true,
		alert: alert({
			source: { id: 's1', name: `<img src=x onerror=alert(1)>${hostile}` },
			kind: hostile,
			rule: { id: 'r1', name: hostile },
			task_state: hostile
		})
	});
	assert(!html.includes('<script'), 'no raw script');
	assert(!html.includes('<img'), 'no raw img');
	assert(html.includes('&lt;script>alert(1)'), 'escaped text present');
	assert(!html.includes('evil.example/y"'), 'no link target from text');
	const anchors = html.match(/<a[\s>][^>]*>/g) ?? [];
	for (const anchor of anchors) {
		assert(!anchor.includes('evil.example'), 'no anchor made from external text');
	}
});

await suite.checkAsync('long external text is cut', async () => {
	const html = await renderToHtml(AlertPanelView, {
		...base,
		alert: alert({ kind: 'k'.repeat(300), source: { id: 's', name: 'n'.repeat(500) } })
	});
	assert(!html.includes('k'.repeat(KIND_MAX + 1)), 'kind cut');
	assert(!html.includes('n'.repeat(NAME_MAX + 1)), 'name cut');
	assert(html.includes('…'), 'cut marked');
});

suite.check('locale files have identical key sets and the page mounts the panel', () => {
	const read = (path: string) => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
	const keys = (value: unknown, prefix = ''): string[] =>
		Object.entries(value as Record<string, unknown>).flatMap(([key, entry]) =>
			typeof entry === 'object' && entry !== null
				? keys(entry, `${prefix}${key}.`)
				: [`${prefix}${key}`]
		);
	const en = keys(read('../src/lib/project/alert-panel/locale.en.json')).sort();
	const zh = keys(read('../src/lib/project/alert-panel/locale.zh.json')).sort();
	assertDeepEqual(en, zh, 'same keys');
	assert(en.includes('alertPanel.taskWaitingUntil'), 'waiting copy key');
	const page = readFileSync(
		new URL(
			'../src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte',
			import.meta.url
		),
		'utf8'
	);
	assert(page.indexOf('<AlertPanel') < page.indexOf('<AgentTasksPanel'), 'before agent tasks');
});

finish(suite.result());
