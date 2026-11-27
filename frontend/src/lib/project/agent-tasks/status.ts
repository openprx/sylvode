/**
 * View-model of the Agent tasks panel: pure functions from the `ai-tasks` response to what the
 * components print. Text that an agent wrote (plan, progress notes, failure reasons, reply
 * warnings) is passed through untouched as plain strings; the components render it as text only.
 */
import type { AiTask, AiTaskEvent } from '../../api/ai-tasks';

export type Tone = 'neutral' | 'success' | 'warning' | 'danger';

export interface StatusView {
	/** Message key under `agentTasks.status.*`. */
	labelKey: string;
	tone: Tone;
	values: Record<string, string | number>;
}

/** Longest text shown before the expand control (characters). */
export const CLAMP_CHARS = 240;

const ACTIVE = new Set(['processing', 'acknowledged']);

export function isActive(task: AiTask): boolean {
	return ACTIVE.has(task.status);
}

function str(payload: Record<string, unknown>, key: string): string | null {
	const value = payload[key];
	return typeof value === 'string' ? value : null;
}

function num(payload: Record<string, unknown>, key: string): number | null {
	const value = payload[key];
	return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/** Events of the attempt in progress: everything after the last `picked_up` (all when none). */
export function currentAttemptEvents(task: AiTask): AiTaskEvent[] {
	let start = 0;
	task.events.forEach((event, index) => {
		if (event.event_type === 'picked_up') start = index + 1;
	});
	return task.events.slice(start);
}

/** The queue position the runner last reported in this attempt, when it reported one. */
export function queuePosition(task: AiTask): number | null {
	const events = currentAttemptEvents(task);
	for (let index = events.length - 1; index >= 0; index -= 1) {
		const event = events[index];
		if (event.event_type !== 'runner.queued' && event.event_type !== 'runner.alive') continue;
		const position = num(event.payload, 'queue_position');
		if (position !== null) return position;
	}
	return null;
}

export function statusView(task: AiTask): StatusView {
	const view = (labelKey: string, tone: Tone = 'neutral', values: StatusView['values'] = {}) => ({
		labelKey,
		tone,
		values
	});
	const events = currentAttemptEvents(task);
	if (isActive(task) && task.runner_phase === 'blocked') {
		return view('agentTasks.status.blocked', 'warning');
	}
	switch (task.status) {
		case 'pending':
			return view('agentTasks.status.queued');
		case 'processing': {
			if (!events.some((event) => event.event_type === 'delivered')) {
				return view('agentTasks.status.queued');
			}
			if (task.runner_phase === 'queued') {
				const position = queuePosition(task);
				return position === null
					? view('agentTasks.status.waitingQueue')
					: view('agentTasks.status.waitingQueuePosition', 'neutral', { position });
			}
			return view('agentTasks.status.delivered');
		}
		case 'acknowledged':
			return events.some((event) => event.event_type === 'progress')
				? view('agentTasks.status.working')
				: view('agentTasks.status.acknowledged');
		case 'completed':
			return view('agentTasks.status.completed', 'success');
		case 'failed':
			return view('agentTasks.status.failed', 'danger');
		case 'cancelled':
			return view('agentTasks.status.cancelled');
		case 'retried':
			return view('agentTasks.status.retried');
		default:
			return view('agentTasks.status.unknown', 'neutral', { status: task.status });
	}
}

/** "Attempt n of m" values, or null before the first attempt started. */
export function attemptValues(task: AiTask): { attempt: number; max: number } | null {
	if (task.attempts < 1) return null;
	return { attempt: task.attempts, max: Math.max(task.max_attempts, task.attempts) };
}

/** The bot's name from the members list, else the given neutral text (never its id). */
export function botName(
	names: Readonly<Record<string, string>>,
	participantId: string,
	neutral: string
): string {
	return names[participantId] || neutral;
}

export interface Clamped {
	text: string;
	truncated: boolean;
}

/** Cuts `text` after `max` characters (code points, never inside a surrogate pair). */
export function clamp(text: string, max: number = CLAMP_CHARS): Clamped {
	const chars = Array.from(text);
	if (chars.length <= max) return { text, truncated: false };
	return { text: chars.slice(0, max).join(''), truncated: true };
}

export interface Progress {
	note: string | null;
	percent: number | null;
	/** When the agent last reported: the task's `last_progress_at`, else the event time. */
	at: string | null;
}

/** The agent's latest progress report of the current attempt. */
export function latestProgress(task: AiTask): Progress | null {
	const reports = currentAttemptEvents(task).filter((event) => event.event_type === 'progress');
	const last = reports[reports.length - 1];
	if (!last) return null;
	const note = str(last.payload, 'note');
	return {
		note: note !== null && note.trim() !== '' ? note : null,
		percent: num(last.payload, 'percent'),
		at: task.last_progress_at ?? last.created_at
	};
}

export type ReplyLine =
	| { kind: 'comment' }
	| { kind: 'state'; from: string; to: string }
	| { kind: 'label' }
	| { kind: 'vote'; vote: string }
	| { kind: 'warning'; text: string };

/** What the server wrote in reply, from a `reply_routed` payload (`ReplyReport`). */
export function replyLines(payload: Record<string, unknown>): ReplyLine[] {
	const lines: ReplyLine[] = [];
	if (typeof payload.comment_id === 'string') lines.push({ kind: 'comment' });
	const changed = payload.state_changed;
	if (typeof changed === 'object' && changed !== null && !Array.isArray(changed)) {
		const record = changed as Record<string, unknown>;
		const from = str(record, 'from');
		const to = str(record, 'to');
		if (from !== null && to !== null) lines.push({ kind: 'state', from, to });
	}
	if (typeof payload.label_added === 'string') lines.push({ kind: 'label' });
	const vote = str(payload, 'vote');
	if (vote !== null) lines.push({ kind: 'vote', vote });
	if (Array.isArray(payload.warnings)) {
		for (const warning of payload.warnings) {
			if (typeof warning === 'string') lines.push({ kind: 'warning', text: warning });
		}
	}
	return lines;
}

/** The reply of the task's last `reply_routed` event; null when none was written. */
export function taskReply(task: AiTask): ReplyLine[] | null {
	for (let index = task.events.length - 1; index >= 0; index -= 1) {
		const event = task.events[index];
		if (event.event_type === 'reply_routed') return replyLines(event.payload);
	}
	return null;
}

export type ChainKind =
	| 'delivered'
	| 'queued'
	| 'pushed'
	| 'acknowledged'
	| 'progress'
	| 'alive'
	| 'reminded'
	| 'blocked'
	| 'completed'
	| 'failed'
	| 'retried'
	| 'reply'
	| 'other';

export interface ChainEntry {
	kind: ChainKind;
	/** Raw event type; shown as plain text for a kind the panel does not know. */
	eventType: string;
	/** Time of the (last) event, ISO string. */
	at: string;
	/** `alive`: how many heartbeats were folded into this entry; 1 otherwise. */
	count: number;
	/** Agent- or runner-written text (plan, note, reason): untrusted. */
	detail: string | null;
	percent: number | null;
	position: number | null;
	reply: ReplyLine[];
}

const KIND_OF: Record<string, ChainKind> = {
	delivered: 'delivered',
	'runner.queued': 'queued',
	'runner.pushed': 'pushed',
	acknowledged: 'acknowledged',
	progress: 'progress',
	'runner.alive': 'alive',
	'runner.reminded': 'reminded',
	'runner.blocked': 'blocked',
	completed: 'completed',
	failed: 'failed',
	retried: 'retried',
	reply_routed: 'reply'
};

/** Bookkeeping events that carry nothing for a reader. */
const HIDDEN = new Set(['created', 'picked_up']);

function nonEmpty(value: string | null): string | null {
	return value !== null && value.trim() !== '' ? value : null;
}

function detailOf(kind: ChainKind, payload: Record<string, unknown>): string | null {
	switch (kind) {
		case 'acknowledged':
			return nonEmpty(str(payload, 'plan'));
		case 'progress':
		case 'queued':
		case 'pushed':
		case 'alive':
		case 'reminded':
		case 'blocked':
			return nonEmpty(str(payload, 'note'));
		case 'failed':
		case 'retried':
			return nonEmpty(str(payload, 'error_message'));
		default:
			return null;
	}
}

/**
 * The event chain in time order. Runner heartbeats (`runner.alive`) collapse into one entry with
 * a count, placed at the last heartbeat; bookkeeping events are left out.
 */
export function eventChain(events: readonly AiTaskEvent[]): ChainEntry[] {
	const entries: ChainEntry[] = [];
	let alive: ChainEntry | null = null;
	for (const event of events) {
		if (HIDDEN.has(event.event_type)) continue;
		const kind = KIND_OF[event.event_type] ?? 'other';
		if (kind === 'alive') {
			if (alive) {
				alive.count += 1;
				alive.at = event.created_at;
				alive.detail = detailOf('alive', event.payload) ?? alive.detail;
				continue;
			}
			alive = {
				kind,
				eventType: event.event_type,
				at: event.created_at,
				count: 1,
				detail: detailOf(kind, event.payload),
				percent: null,
				position: null,
				reply: []
			};
			entries.push(alive);
			continue;
		}
		entries.push({
			kind,
			eventType: event.event_type,
			at: event.created_at,
			count: 1,
			detail: detailOf(kind, event.payload),
			percent: kind === 'progress' ? num(event.payload, 'percent') : null,
			position: kind === 'queued' ? num(event.payload, 'queue_position') : null,
			reply: kind === 'reply' ? replyLines(event.payload) : []
		});
	}
	return entries;
}
