/**
 * How inbound sources, events, rules and rule tests are shown: status derivations, the words of
 * server codes and the warnings derived from other answers. Pure: no requests, no stores.
 *
 * A code the server may add before the UI is never hidden: the mappers return `{ key: null,
 * code }` and the component prints the code itself in monospace.
 */
import { SEVERITIES, type InboundEvent, type Rule, type Severity, type Source } from './wire.js';

export type Tone = 'success' | 'warning' | 'danger' | 'neutral';

/** A word that is either a message key or, for a code the UI does not know, the code itself. */
export type Word = { key: string; code?: undefined } | { key: null; code: string };

function word(prefix: string, known: readonly string[], code: string): Word {
	return known.includes(code) ? { key: `${prefix}${code}` } : { key: null, code };
}

// --- source status --------------------------------------------------------------------------

export type SourceStatus = 'suspended' | 'paused' | 'waiting' | 'receiving' | 'idle';

export const RECEIVING_WINDOW_MS = 24 * 3600 * 1000;

/** Suspended wins over the others; then paused; then by the last use. */
export function sourceStatus(source: Source, now: Date): SourceStatus {
	if (source.suspended) return 'suspended';
	if (!source.enabled) return 'paused';
	if (source.last_used_at === null) return 'waiting';
	const used = Date.parse(source.last_used_at);
	if (Number.isNaN(used)) return 'waiting';
	return now.getTime() - used <= RECEIVING_WINDOW_MS ? 'receiving' : 'idle';
}

export function sourceChip(status: SourceStatus): { tone: Tone; key: string } {
	const key = `inbound.sources.status.${status}`;
	switch (status) {
		case 'receiving':
			return { tone: 'success', key };
		case 'suspended':
			return { tone: 'danger', key };
		default:
			return { tone: 'neutral', key };
	}
}

// --- severity -------------------------------------------------------------------------------

/** The five severities in rank order, rank 0 to 4. */
export function severityOfRank(rank: number): Severity | null {
	return Number.isInteger(rank) ? (SEVERITIES[rank] ?? null) : null;
}

export function severityTone(severity: Severity | null): Tone {
	if (severity === 'critical') return 'danger';
	if (severity === 'high') return 'warning';
	return 'neutral';
}

export function severityKey(severity: Severity | null): string {
	return `inbound.severity.${severity ?? 'unknown'}`;
}

// --- event outcomes -------------------------------------------------------------------------

export const OUTCOMES = [
	'no_rule',
	'alert_opened',
	'alert_attached',
	'task_queued',
	'suppressed_alert_cap',
	'bot_unavailable'
] as const;

export function outcomeWord(code: string): Word {
	return word('inbound.outcome.', OUTCOMES, code);
}

export function outcomeTone(code: string): Tone {
	return code === 'suppressed_alert_cap' || code === 'bot_unavailable' ? 'warning' : 'neutral';
}

/** The event's own work item link target, `null` when it led to none. */
export function eventWorkItemHref(
	workspaceId: string,
	projectId: string,
	event: InboundEvent
): string | null {
	return event.work_item
		? `/workspace/${encodeURIComponent(workspaceId)}/projects/${encodeURIComponent(projectId)}/issues/${encodeURIComponent(event.work_item.id)}`
		: null;
}

// --- rule test words ------------------------------------------------------------------------

export const TEST_ALERTS = [
	'no_rule',
	'would_open',
	'would_attach',
	'would_refuse_alert_cap'
] as const;
export const TEST_TASKS = [
	'none',
	'bot_unavailable',
	'would_start',
	'would_queue',
	'already_active',
	'already_waiting',
	'rearm_delay'
] as const;
export const TEST_CAUSES = ['task_cap', 'active_cap', 'workspace_cap', 'queue_ahead'] as const;
export const TEST_REASONS = [
	'matched',
	'disabled',
	'other_source',
	'kind_not_listed',
	'below_minimum_severity'
] as const;

export const testAlertWord = (code: string): Word => word('inbound.test.alert.', TEST_ALERTS, code);
export const testTaskWord = (code: string): Word => word('inbound.test.task.', TEST_TASKS, code);
export const testCauseWord = (code: string): Word => word('inbound.test.cause.', TEST_CAUSES, code);
export const testReasonWord = (code: string): Word =>
	word('inbound.test.reason.', TEST_REASONS, code);

// --- rule list ------------------------------------------------------------------------------

/**
 * The "bot is not connected" warning of a rule row, from the project's bot connections (the
 * backend's own test: a connection exists, is enabled and names the rule's bot). `connectedBotIds`
 * holds the bots of the project's enabled connections; empty when the project has none.
 */
export function botNotConnected(rule: Rule, connectedBotIds: ReadonlySet<string>): boolean {
	if (rule.action !== 'work_item_and_task' || rule.bot_user_id === null) return false;
	return !connectedBotIds.has(rule.bot_user_id);
}

/** No current human recipient among the workspace's people. */
export function hasNoRecipients(rule: Rule, humanIds: ReadonlySet<string>): boolean {
	return !rule.notify_user_ids.some((id) => humanIds.has(id));
}
