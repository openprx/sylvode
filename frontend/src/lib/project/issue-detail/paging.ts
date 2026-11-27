/** Paging of the lists on the work item page: comments, activity (both newest first). */

export const COMMENTS_PAGE_SIZE = 20;
export const ACTIVITY_PAGE_SIZE = 30;

interface Dated {
	id: string;
	created_at: string;
}

function newestFirst<T extends Dated>(a: T, b: T): number {
	return new Date(b.created_at).getTime() - new Date(a.created_at).getTime();
}

/** `more` appended to `loaded`: an entry that is already there (the list moved on meanwhile) is kept once. */
export function appendPage<T extends Dated>(loaded: readonly T[], more: readonly T[]): T[] {
	const known = new Set(loaded.map((item) => item.id));
	return [...loaded, ...more.filter((item) => !known.has(item.id))];
}

/**
 * The newest page read again, with the older entries that were loaded before it kept below.
 * Entries that are gone from the newest page but were on it before (deleted) are not kept.
 */
export function refreshNewest<T extends Dated>(
	loaded: readonly T[],
	newestPage: readonly T[],
	deleted: ReadonlySet<string> = new Set()
): T[] {
	const oldestFresh = newestPage.reduce(
		(oldest, item) => Math.min(oldest, new Date(item.created_at).getTime()),
		Number.POSITIVE_INFINITY
	);
	const older = loaded.filter(
		(item) =>
			!deleted.has(item.id) &&
			new Date(item.created_at).getTime() < oldestFresh &&
			!newestPage.some((fresh) => fresh.id === item.id)
	);
	return [...newestPage, ...older].sort(newestFirst);
}

/** Entries the server holds that the page has not loaded yet. */
export function remainingCount(total: number, loaded: number): number {
	return Math.max(0, total - loaded);
}
