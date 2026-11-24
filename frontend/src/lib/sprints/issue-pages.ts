// The Cycles page never loads a whole project: progress comes from the sprint list (`progress`),
// and the items of a sprint are read 50 at a time when the sprint is expanded ("Load more").

import type { Issue } from '#lib/api/issues.js';
import type { Sprint } from '#lib/api/sprints.js';

export const SPRINT_ISSUES_PER_PAGE = 50;

export interface SprintProgressView {
	done: number;
	total: number;
	percent: number;
}

/** Progress as the server counted it for the sprint (`progress` of the sprint list). */
export function sprintProgressOf(sprint: Pick<Sprint, 'progress'>): SprintProgressView {
	const total = Math.max(0, sprint.progress?.total ?? 0);
	const done = Math.min(total, Math.max(0, sprint.progress?.done ?? 0));
	return { done, total, percent: total > 0 ? Math.round((done / total) * 100) : 0 };
}

/** The items of one expanded sprint. */
export interface SprintIssueList {
	items: Issue[];
	/** Pages read so far. */
	page: number;
	totalPages: number;
	loading: boolean;
	/** The last read failed; the button reads "Load more" again. */
	failed: boolean;
}

export function emptyIssueList(): SprintIssueList {
	return { items: [], page: 0, totalPages: 1, loading: false, failed: false };
}

export function hasMoreIssues(list: SprintIssueList): boolean {
	return list.page < list.totalPages;
}

/** The query of the next read, or `null` when everything is read or a read is under way. */
export function nextIssuePage(
	sprintId: string,
	list: SprintIssueList
): { sprint_id: string; page: number; per_page: number } | null {
	if (list.loading || !hasMoreIssues(list)) return null;
	return { sprint_id: sprintId, page: list.page + 1, per_page: SPRINT_ISSUES_PER_PAGE };
}

/** The list after a page arrived; an item that is already shown is not shown twice. */
export function withIssuePage(
	list: SprintIssueList,
	data: { items: Issue[]; page: number; total_pages: number }
): SprintIssueList {
	const known = new Set(list.items.map((issue) => issue.id));
	return {
		items: [...list.items, ...data.items.filter((issue) => !known.has(issue.id))],
		page: data.page,
		totalPages: Math.max(1, data.total_pages),
		loading: false,
		failed: false
	};
}
