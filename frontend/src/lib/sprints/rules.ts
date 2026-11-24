// Which sprints the pages offer, and to whom. The server enforces the same rules (a status change
// needs an owner or admin; only a planned sprint of the project can take rolled-over issues; an
// issue can be put in any sprint, but only a planned or active one is a sensible target); these
// helpers keep the screens from offering what the server would refuse.

import type { Sprint, SprintRollover } from '#lib/api/sprints.js';
import { plural } from '#lib/ui/foundations/plural.js';
import { isWorkspaceAdminRole, type WorkspaceRole } from '#lib/workspace/home.js';

/** Starting, completing, cancelling and deleting a sprint are for workspace owners and admins. */
export function canManageSprints(role: WorkspaceRole | null | undefined): boolean {
	return isWorkspaceAdminRole(role);
}

/** The planned sprints that can take the unfinished issues of the sprint being ended. */
export function rolloverTargets(sprints: readonly Sprint[], endingSprintId: string): Sprint[] {
	return sprints.filter((sprint) => sprint.status === 'planned' && sprint.id !== endingSprintId);
}

/**
 * Completed and cancelled sprints, the most recently ended first; with `only`, just the completed
 * or just the cancelled ones.
 */
export function pastSprints(
	sprints: readonly Sprint[],
	only?: 'completed' | 'cancelled'
): Sprint[] {
	const ended = (sprint: Sprint): number =>
		sprint.end_date ? new Date(sprint.end_date).getTime() : Number.NEGATIVE_INFINITY;
	return sprints
		.filter((sprint) =>
			only ? sprint.status === only : sprint.status === 'completed' || sprint.status === 'cancelled'
		)
		.sort((a, b) => ended(b) - ended(a));
}

export interface SprintOption {
	id: string;
	name: string;
	/** False for a sprint that is only listed because the issue is still in it. */
	selectable: boolean;
}

/**
 * The sprints an issue can be moved into: the planned and active ones. A completed or cancelled
 * sprint appears only when it is the issue's current sprint, so the control can show it, and it
 * cannot be chosen.
 */
export function sprintOptionsForIssue(
	sprints: readonly Sprint[],
	currentSprintId: string | null | undefined
): SprintOption[] {
	return sprints
		.filter(
			(sprint) =>
				sprint.status === 'planned' || sprint.status === 'active' || sprint.id === currentSprintId
		)
		.map((sprint) => ({
			id: sprint.id,
			name: sprint.name,
			selectable: sprint.status === 'planned' || sprint.status === 'active'
		}));
}

export interface SprintChoice {
	/** The sprint id; `''` is "No sprint". */
	value: string;
	label: string;
	disabled: boolean;
}

/**
 * The options of a Sprint select, for the issue page and the create dialog alike: "No sprint",
 * then the active and planned sprints (and the issue's current sprint when it has ended, shown
 * with its status and disabled).
 */
export function sprintChoices(
	sprints: readonly Sprint[],
	currentSprintId: string | null | undefined,
	text: { none: string; status: (status: Sprint['status']) => string }
): SprintChoice[] {
	return [
		{ value: '', label: text.none, disabled: false },
		...sprintOptionsForIssue(sprints, currentSprintId).map((option) => {
			const sprint = sprints.find((item) => item.id === option.id);
			const suffix = !option.selectable && sprint ? ` (${text.status(sprint.status)})` : '';
			return { value: option.id, label: `${option.name}${suffix}`, disabled: !option.selectable };
		})
	];
}

/** A translation function as `svelte-i18n`'s `$t` has it. */
export type Translate = (key: string, options?: { values?: Record<string, unknown> }) => string;

/**
 * The text after completing or cancelling a sprint: what the server moved and where. The target
 * comes from the server's answer; `targets` are the sprints that could take the items, read before
 * the request (the list changes once the sprint has ended).
 */
export function rolloverMessage(
	translate: Translate,
	status: 'completed' | 'cancelled',
	rollover: SprintRollover | null | undefined,
	targets: readonly Pick<Sprint, 'id' | 'name'>[],
	lang?: string | null
): string {
	const done = translate(status === 'completed' ? 'cycles.completed' : 'cycles.cancelled');
	const moved = rollover?.moved_count ?? 0;
	if (moved <= 0) return `${done}. ${translate('cycles.rolloverNone')}`;
	const targetId = rollover?.target_sprint_id ?? null;
	const target = targetId
		? (targets.find((sprint) => sprint.id === targetId)?.name ?? null)
		: translate('cycles.rolloverBacklog');
	const base = target === null ? 'cycles.rolloverMovedElsewhere' : 'cycles.rolloverMoved';
	return `${done}. ${plural(translate, base, moved, lang, { target })}`;
}

/** What a refused sprint call answers; the fields of `ApiResult` that matter here. */
export interface SprintFailure {
	message: string;
	error_code?: string;
	details?: unknown;
}

function detailText(details: unknown, key: string): string | null {
	if (!details || typeof details !== 'object') return null;
	const value = (details as Record<string, unknown>)[key];
	return typeof value === 'string' && value ? value : null;
}

/**
 * The text for a refused sprint call. The sprint rules the server names with an `error_code` get
 * a translated sentence (the active sprint is named without its id); anything else is the generic
 * failure text followed by the server's own message.
 */
export function sprintFailureMessage(translate: Translate, failure: SprintFailure): string {
	const { details } = failure;
	switch (failure.error_code) {
		case 'sprint_active_exists': {
			const name = detailText(details, 'active_sprint_name');
			return name
				? translate('cycles.error.activeExists', { values: { name } })
				: translate('cycles.error.activeExistsUnnamed');
		}
		case 'sprint_invalid_transition': {
			const from = detailText(details, 'from');
			const to = detailText(details, 'to');
			if (from && to) {
				return translate('cycles.error.invalidTransition', {
					values: {
						from: translate(`cycles.status.${from}`),
						to: translate(`cycles.status.${to}`)
					}
				});
			}
			return translate('cycles.error.invalidTransitionUnnamed');
		}
		case 'sprint_end_before_start':
			return translate('cycles.endDateAfterStart');
		case 'sprint_name_taken':
			return translate('cycles.error.nameTaken');
		default:
			return translate('cycles.error.failed', { values: { message: failure.message } });
	}
}
