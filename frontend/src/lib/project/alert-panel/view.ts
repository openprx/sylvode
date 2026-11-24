/**
 * View-model of the Alert panel on the work item page: pure functions from the alert facts to
 * what the components print. Source, kind and rule names come from outside Sylvode and are
 * returned as plain strings (clamped); the component renders them as text only, never as markup
 * and never as links.
 */
import type { AiTask } from '../../api/ai-tasks';
import { statusView, type Tone } from '../agent-tasks/status';
import { SEVERITIES, type IssueAlert, type Severity } from '../inbound/wire';

/** Longest kind shown (the server accepts at most 64 characters; anything longer is cut). */
export const KIND_MAX = 64;
/** Longest source or rule name shown (the server limit is 100). */
export const NAME_MAX = 100;

export type PanelOutcome = 'hidden' | 'failed' | 'shown' | 'loading';

/**
 * What the panel does with one load answer: 404 is "not an alert" and 403 is "the page itself is
 * refused", both render nothing; any other failure shows the retry state.
 */
export function outcomeOf(code: number, hasData: boolean): PanelOutcome {
	if (code === 0 && hasData) return 'shown';
	if (code === 404 || code === 403) return 'hidden';
	return 'failed';
}

/** Cut `text` to `max` characters (code points), marking the cut with an ellipsis. */
export function clampText(text: string, max: number): string {
	const points = Array.from(text);
	return points.length <= max ? text : `${points.slice(0, max).join('')}…`;
}

/** The severity word of the alert for the shared `SeverityChip`; `null` when it is unknown. */
export function severityWord(severity: string | null): Severity | null {
	return SEVERITIES.find((word) => word === severity) ?? null;
}

export type TaskRowView =
	| { kind: 'none' }
	| { kind: 'created'; labelKey: string; tone: Tone; values: Record<string, string | number> }
	| { kind: 'waiting'; until: string | null }
	| { kind: 'suppressed' }
	| { kind: 'unknown'; code: string };

/** The Bot task row by `task_state`; the "created" words are the Agent tasks panel's own. */
export function taskRowView(alert: IssueAlert): TaskRowView {
	switch (alert.task_state) {
		case 'none':
			return { kind: 'none' };
		case 'created': {
			if (alert.task === null) return { kind: 'none' };
			// Only the status is known here, so the status view sees a task without events.
			const task = {
				status: alert.task.status,
				runner_phase: null,
				events: []
			} as unknown as AiTask;
			const view = statusView(task);
			return { kind: 'created', labelKey: view.labelKey, tone: view.tone, values: view.values };
		}
		case 'waiting':
			return { kind: 'waiting', until: alert.queue_expires_at };
		case 'suppressed':
			return { kind: 'suppressed' };
		default:
			return { kind: 'unknown', code: clampText(alert.task_state, KIND_MAX) };
	}
}
