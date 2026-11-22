/**
 * Agent tasks panel on the work item page: response narrowing (`lib/api/ai-tasks.ts`), the pure
 * view-model (`lib/project/agent-tasks/status.ts`), the feature locale files and the markup of
 * the rows for agent-written text.
 *
 * Run standalone: `bun tests/agent-tasks.test.ts`
 */
import { readFileSync } from 'node:fs';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import {
	narrowAiTask,
	narrowAiTaskList,
	type AiTask,
	type AiTaskEvent
} from '../src/lib/api/ai-tasks';
import {
	CLAMP_CHARS,
	attemptValues,
	botName,
	clamp,
	eventChain,
	latestProgress,
	orderTasks,
	queuePosition,
	replyLines,
	statusView,
	taskReply
} from '../src/lib/project/agent-tasks/status';

await installSvelteSsr();

const suite = new Suite('agent-tasks');

let seq = 0;
function event(event_type: string, payload: Record<string, unknown> = {}): AiTaskEvent {
	seq += 1;
	return {
		event_type,
		payload,
		created_at: new Date(Date.UTC(2026, 10, 20, 10, 0, seq)).toISOString()
	};
}

function task(overrides: Partial<AiTask> = {}): AiTask {
	return {
		id: 'a1b2c3d4-0000-4000-8000-000000000001',
		project_id: 'p',
		ai_participant_id: '11112222-3333-4444-5555-666677778888',
		task_type: 'issue_assigned',
		status: 'pending',
		attempts: 1,
		max_attempts: 3,
		error_message: null,
		next_retry_at: null,
		started_at: null,
		completed_at: null,
		created_at: '2026-11-20T10:00:00Z',
		updated_at: '2026-11-20T10:00:00Z',
		acknowledged_at: null,
		plan: null,
		last_progress_at: null,
		lease_expires_at: null,
		runner_phase: null,
		events: [],
		events_omitted: false,
		...overrides
	};
}

const rawTask = (extra: Record<string, unknown> = {}) => ({
	id: 't1',
	project_id: 'p1',
	ai_participant_id: 'b1',
	task_type: 'issue_assigned',
	reference_type: 'work_item',
	reference_id: 'w1',
	status: 'acknowledged',
	priority: 0,
	payload: {},
	result: null,
	error_message: null,
	attempts: 1,
	max_attempts: 3,
	next_retry_at: null,
	started_at: '2026-11-20T10:00:00Z',
	completed_at: null,
	created_at: '2026-11-20T09:59:00Z',
	updated_at: '2026-11-20T10:00:00Z',
	acknowledged_at: '2026-11-20T10:00:01Z',
	plan: 'Do it',
	last_progress_at: null,
	lease_expires_at: '2026-11-20T10:30:00Z',
	runner_phase: 'pushed',
	events: [
		{ event_type: 'delivered', payload: { attempts: 1 }, created_at: '2026-11-20T10:00:00Z' }
	],
	events_omitted: false,
	...extra
});

// Narrowing

suite.check('a complete response narrows to typed tasks', () => {
	const tasks = narrowAiTaskList([rawTask()]);
	assert(tasks !== null, 'narrowed');
	assertEqual(tasks.length, 1, 'one task');
	assertEqual(tasks[0].plan, 'Do it', 'plan');
	assertEqual(tasks[0].runner_phase, 'pushed', 'phase');
	assertEqual(tasks[0].events[0].event_type, 'delivered', 'event');
	assertEqual(tasks[0].last_progress_at, null, 'null stays null');
});

suite.check('the omitted-events flag is carried and must be a boolean', () => {
	const narrowed = narrowAiTaskList([rawTask({ events_omitted: true }), rawTask()]);
	assert(narrowed !== null, 'narrowed');
	assertEqual(narrowed[0].events_omitted, true, 'older events were left out');
	assertEqual(narrowed[1].events_omitted, false, 'nothing left out');
	assertEqual(narrowAiTaskList([rawTask({ events_omitted: 'yes' })]), null, 'flag type');
	const raw: Record<string, unknown> = rawTask();
	delete raw.events_omitted;
	assertEqual(narrowAiTaskList([raw]), null, 'the flag is part of the response');
});

suite.check('absent optional fields become null and missing events an empty list', () => {
	const raw: Record<string, unknown> = rawTask();
	delete raw.plan;
	delete raw.runner_phase;
	delete raw.events;
	const narrowed = narrowAiTask(raw);
	assert(narrowed !== null, 'narrowed');
	assertEqual(narrowed.plan, null, 'plan');
	assertEqual(narrowed.runner_phase, null, 'phase');
	assertDeepEqual(narrowed.events, [], 'events');
});

suite.check('malformed responses are rejected, not guessed', () => {
	assertEqual(narrowAiTaskList(null), null, 'null');
	assertEqual(narrowAiTaskList({ items: [] }), null, 'object');
	assertEqual(narrowAiTaskList([rawTask({ status: 5 })]), null, 'status type');
	assertEqual(narrowAiTaskList([rawTask({ attempts: '1' })]), null, 'attempts type');
	assertEqual(narrowAiTaskList([rawTask({ plan: 7 })]), null, 'plan type');
	assertEqual(narrowAiTaskList([rawTask({ events: {} })]), null, 'events type');
	assertEqual(narrowAiTaskList([rawTask({ events: [{ event_type: 1 }] })]), null, 'event type');
	assertEqual(narrowAiTaskList([rawTask(), 'x']), null, 'one bad entry');
	assertDeepEqual(narrowAiTaskList([]), [], 'empty list is valid');
});

suite.check('an event payload that is not an object becomes an empty record', () => {
	const narrowed = narrowAiTask(
		rawTask({
			events: [{ event_type: 'progress', payload: 'x', created_at: '2026-11-20T10:00:00Z' }]
		})
	);
	assert(narrowed !== null, 'narrowed');
	assertDeepEqual(narrowed.events[0].payload, {}, 'payload');
});

// Status mapping

function label(t: AiTask): string {
	const v = statusView(t);
	return `${v.labelKey}|${v.tone}`;
}

suite.check('status mapping table', () => {
	const delivered = event('delivered', { attempts: 1 });
	const table: Array<[string, AiTask, string]> = [
		['pending', task(), 'agentTasks.status.queued|neutral'],
		[
			'processing, not delivered',
			task({ status: 'processing' }),
			'agentTasks.status.queued|neutral'
		],
		[
			'processing, delivered, queued phase',
			task({ status: 'processing', runner_phase: 'queued', events: [delivered] }),
			'agentTasks.status.waitingQueue|neutral'
		],
		[
			'processing, delivered, pushed phase',
			task({ status: 'processing', runner_phase: 'pushed', events: [delivered] }),
			'agentTasks.status.delivered|neutral'
		],
		[
			'processing, delivered, no phase',
			task({ status: 'processing', events: [delivered] }),
			'agentTasks.status.delivered|neutral'
		],
		[
			'acknowledged, no progress',
			task({ status: 'acknowledged', events: [delivered, event('acknowledged')] }),
			'agentTasks.status.acknowledged|neutral'
		],
		[
			'acknowledged, progress',
			task({
				status: 'acknowledged',
				events: [delivered, event('acknowledged'), event('progress')]
			}),
			'agentTasks.status.working|neutral'
		],
		[
			'blocked while processing',
			task({ status: 'processing', runner_phase: 'blocked', events: [delivered] }),
			'agentTasks.status.blocked|warning'
		],
		[
			'blocked while acknowledged',
			task({ status: 'acknowledged', runner_phase: 'blocked' }),
			'agentTasks.status.blocked|warning'
		],
		['completed', task({ status: 'completed' }), 'agentTasks.status.completed|success'],
		[
			'completed keeps phase out of it',
			task({ status: 'completed', runner_phase: 'blocked' }),
			'agentTasks.status.completed|success'
		],
		['failed', task({ status: 'failed', error_message: 'x' }), 'agentTasks.status.failed|danger'],
		['cancelled', task({ status: 'cancelled' }), 'agentTasks.status.cancelled|neutral'],
		['retried', task({ status: 'retried' }), 'agentTasks.status.retried|neutral'],
		['unknown status', task({ status: 'weird' }), 'agentTasks.status.unknown|neutral']
	];
	for (const [name, input, expected] of table) assertEqual(label(input), expected, name);
	assertDeepEqual(
		statusView(task({ status: 'weird' })).values,
		{ status: 'weird' },
		'unknown shows the raw status'
	);
});

suite.check('queue position comes from the latest report of the current attempt', () => {
	const t = task({
		status: 'processing',
		runner_phase: 'queued',
		events: [
			event('delivered'),
			event('runner.queued', { queue_position: 3 }),
			event('runner.alive', { queue_position: 2 })
		]
	});
	assertEqual(queuePosition(t), 2, 'latest');
	assertDeepEqual(statusView(t).values, { position: 2 }, 'values');
	assertEqual(statusView(t).labelKey, 'agentTasks.status.waitingQueuePosition', 'label');
});

suite.check('a retried attempt ignores the events of the earlier attempt', () => {
	const t = task({
		status: 'processing',
		attempts: 2,
		events: [
			event('picked_up'),
			event('delivered'),
			event('acknowledged'),
			event('progress', { note: 'old' }),
			event('retried', { error_message: 'timeout' }),
			event('picked_up')
		]
	});
	assertEqual(statusView(t).labelKey, 'agentTasks.status.queued', 'not delivered in this attempt');
	assertEqual(latestProgress(t), null, 'no progress yet');
	assertDeepEqual(attemptValues(t), { attempt: 2, max: 3 }, 'attempt n of m');
});

suite.check('attempt values', () => {
	assertEqual(attemptValues(task({ attempts: 0 })), null, 'before the first attempt');
	assertDeepEqual(
		attemptValues(task({ attempts: 4, max_attempts: 3 })),
		{ attempt: 4, max: 4 },
		'never above max'
	);
});

// Ordering, names, truncation

suite.check('active tasks come first, then newest first, ties keep server order', () => {
	const a = task({ id: 'a', status: 'completed', created_at: '2026-11-20T12:00:00Z' });
	const b = task({ id: 'b', status: 'processing', created_at: '2026-11-20T08:00:00Z' });
	const c = task({ id: 'c', status: 'failed', created_at: '2026-11-20T13:00:00Z' });
	const d = task({ id: 'd', status: 'failed', created_at: '2026-11-20T13:00:00Z' });
	assertDeepEqual(
		orderTasks([a, b, c, d]).map((x) => x.id),
		['b', 'c', 'd', 'a'],
		'order'
	);
	assertDeepEqual(
		orderTasks([]).map((x) => x.id),
		[],
		'empty'
	);
});

suite.check('bot name falls back to the first 8 characters of the id', () => {
	assertEqual(botName({ abc12345678: 'Builder' }, 'abc12345678'), 'Builder', 'known');
	assertEqual(botName({}, 'abc12345678'), 'abc12345', 'unknown');
	assertEqual(botName({ x: '' }, 'x'), 'x', 'empty name');
});

suite.check('clamp cuts long text by code points and leaves short text alone', () => {
	assertDeepEqual(clamp('short'), { text: 'short', truncated: false }, 'short');
	const exact = 'a'.repeat(CLAMP_CHARS);
	assertEqual(clamp(exact).truncated, false, 'exactly at the limit');
	const over = clamp('a'.repeat(CLAMP_CHARS + 1));
	assertEqual(over.truncated, true, 'over');
	assertEqual(over.text.length, CLAMP_CHARS, 'cut length');
	const emoji = clamp('😀'.repeat(5), 3);
	assertEqual(emoji.text, '😀😀😀', 'no split surrogate pair');
});

// Reply, progress, chain

suite.check('reply lines follow the frozen report shape', () => {
	assertDeepEqual(
		replyLines({
			comment_id: 'c1',
			state_changed: { from: 'todo', to: 'done' },
			label_added: 'l1',
			vote: 'approve',
			warnings: ['state x not in workflow', 3]
		}),
		[
			{ kind: 'comment' },
			{ kind: 'state', from: 'todo', to: 'done' },
			{ kind: 'label' },
			{ kind: 'vote', vote: 'approve' },
			{ kind: 'warning', text: 'state x not in workflow' }
		],
		'all parts'
	);
	assertDeepEqual(
		replyLines({ comment_id: null, state_changed: null, warnings: [] }),
		[],
		'nothing written'
	);
	assertDeepEqual(
		replyLines({ state_changed: { from: 1, to: 'x' } }),
		[],
		'malformed state change'
	);
	const t = task({
		status: 'completed',
		events: [event('completed'), event('reply_routed', { comment_id: 'c' })]
	});
	assertDeepEqual(taskReply(t), [{ kind: 'comment' }], 'last reply of the task');
	assertEqual(taskReply(task()), null, 'no reply event');
});

suite.check('latest progress reads the newest report', () => {
	const t = task({
		status: 'acknowledged',
		last_progress_at: '2026-11-20T10:05:00Z',
		events: [
			event('progress', { note: 'one', percent: 10 }),
			event('progress', { note: ' ', percent: 50 })
		]
	});
	assertDeepEqual(
		latestProgress(t),
		{ note: null, percent: 50, at: '2026-11-20T10:05:00Z' },
		'newest'
	);
});

suite.check('event chain collapses heartbeats and hides bookkeeping events', () => {
	const chain = eventChain([
		event('created'),
		event('picked_up'),
		event('delivered'),
		event('runner.queued', { queue_position: 1, note: 'q' }),
		event('runner.alive'),
		event('runner.alive'),
		event('runner.alive'),
		event('runner.pushed'),
		event('acknowledged', { plan: 'the plan' }),
		event('progress', { note: 'half', percent: 50 }),
		event('runner.reminded'),
		event('runner.blocked', { note: 'dialog' }),
		event('completed', { result: {} }),
		event('reply_routed', { comment_id: 'c', warnings: ['w'] }),
		event('mystery')
	]);
	assertDeepEqual(
		chain.map((entry) => entry.kind),
		[
			'delivered',
			'queued',
			'alive',
			'pushed',
			'acknowledged',
			'progress',
			'reminded',
			'blocked',
			'completed',
			'reply',
			'other'
		],
		'kinds in time order'
	);
	const alive = chain.find((entry) => entry.kind === 'alive');
	assertEqual(alive?.count, 3, 'heartbeat count');
	assertEqual(chain[1].position, 1, 'queue position');
	assertEqual(chain[4].detail, 'the plan', 'plan');
	assertEqual(chain[5].percent, 50, 'percent');
	assertEqual(chain[7].detail, 'dialog', 'blocked note');
	assertEqual(chain[9].reply.length, 2, 'reply lines');
	assertEqual(chain[10].eventType, 'mystery', 'unknown type kept for display');
});

suite.check('failed and retried entries carry the reason', () => {
	const chain = eventChain([
		event('retried', { error_message: 'timeout' }),
		event('failed', { error_message: 'no way' })
	]);
	assertDeepEqual(
		chain.map((e) => [e.kind, e.detail]),
		[
			['retried', 'timeout'],
			['failed', 'no way']
		],
		'reasons'
	);
});

// Locale files

function leaves(node: unknown, prefix = ''): string[] {
	if (typeof node !== 'object' || node === null) return [prefix];
	return Object.entries(node).flatMap(([k, v]) => leaves(v, prefix ? `${prefix}.${k}` : k));
}
const read = (name: string) =>
	JSON.parse(
		readFileSync(new URL(`../src/lib/project/agent-tasks/${name}`, import.meta.url), 'utf8')
	);

suite.check('feature locale files have the same keys and every used key exists', () => {
	const en = new Set(leaves(read('locale.en.json')));
	const zh = new Set(leaves(read('locale.zh.json')));
	assertDeepEqual([...en].sort(), [...zh].sort(), 'en and zh keys');
	for (const key of [
		...[
			'queued',
			'waitingQueue',
			'waitingQueuePosition',
			'delivered',
			'acknowledged',
			'working',
			'blocked',
			'completed',
			'failed',
			'cancelled',
			'retried',
			'unknown'
		].map((k) => `agentTasks.status.${k}`),
		...[
			'delivered',
			'queued',
			'queuedPosition',
			'pushed',
			'acknowledged',
			'progress',
			'progressPercent',
			'alive',
			'aliveCount',
			'reminded',
			'blocked',
			'completed',
			'failed',
			'retried',
			'reply',
			'other'
		].map((k) => `agentTasks.event.${k}`),
		...['comment', 'state', 'label', 'vote', 'warning', 'none'].map(
			(k) => `agentTasks.replyLine.${k}`
		)
	]) {
		assert(en.has(key), `missing ${key}`);
	}
});

suite.check('feature keys are not defined in the shared locale files', () => {
	for (const file of ['en.json', 'zh.json']) {
		const shared = JSON.parse(
			readFileSync(new URL(`../src/lib/i18n/${file}`, import.meta.url), 'utf8')
		);
		assert(!('agentTasks' in shared), `${file} must not define agentTasks`);
	}
});

// Markup: agent text is plain text

const { default: TaskRow } = await import('../src/lib/project/agent-tasks/TaskRow.svelte');
const { default: ClampedText } = await import('../src/lib/project/agent-tasks/ClampedText.svelte');

await suite.checkAsync(
	'agent text is escaped, never linked, and long text is cut with an expand control',
	async () => {
		const hostile =
			'<img src=x onerror=alert(1)> https://evil.example/x [a](https://evil.example) **b**';
		const html = await renderToHtml(TaskRow, {
			names: {},
			task: task({
				status: 'failed',
				plan: hostile,
				error_message: `<script>alert(1)</script>${'z'.repeat(CLAMP_CHARS + 50)}`,
				events: [event('delivered'), event('progress', { note: hostile })]
			})
		});
		assert(!html.includes('<img'), 'no raw img');
		assert(!html.includes('<script'), 'no raw script');
		assert(html.includes('&lt;img src=x onerror=alert(1)'), 'escaped text present');
		assert(!/<a[\s>]/.test(html), 'no anchor element');
		assert(!html.includes('<strong>'), 'no markdown rendering');
		assert(html.includes('agentTasks.showMore'), 'expand control for the long reason');
		assert(!html.includes('z'.repeat(CLAMP_CHARS + 1)), 'long text is cut before expansion');
	}
);

await suite.checkAsync('short text has no expand control', async () => {
	const short = await renderToHtml(ClampedText, { text: 'hello' });
	assert(!short.includes('agentTasks.showMore'), 'no control');
});

finish(suite.result());
