// Which sprints the pages offer, and to whom. The server enforces the same rules (a status change
// needs an owner or admin; only a planned sprint of the project can take rolled-over issues; an
// issue can be put in any sprint, but only a planned or active one is a sensible target); these
// helpers keep the screens from offering what the server would refuse.

import type { Sprint } from '#lib/api/sprints.js';
import { isWorkspaceAdminRole, type WorkspaceRole } from '#lib/workspace/home.js';

/** Starting, completing, cancelling and deleting a sprint are for workspace owners and admins. */
export function canManageSprints(role: WorkspaceRole | null | undefined): boolean {
	return isWorkspaceAdminRole(role);
}

/** The planned sprints that can take the unfinished issues of the sprint being ended. */
export function rolloverTargets(sprints: readonly Sprint[], endingSprintId: string): Sprint[] {
	return sprints.filter((sprint) => sprint.status === 'planned' && sprint.id !== endingSprintId);
}

/** Completed and cancelled sprints, the most recently ended first. */
export function pastSprints(sprints: readonly Sprint[]): Sprint[] {
	const ended = (sprint: Sprint): number =>
		sprint.end_date ? new Date(sprint.end_date).getTime() : Number.NEGATIVE_INFINITY;
	return sprints
		.filter((sprint) => sprint.status === 'completed' || sprint.status === 'cancelled')
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
