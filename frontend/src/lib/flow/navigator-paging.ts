// Cursor paging for the Flow navigator. The list API refuses a limit above 100, so a workspace
// larger than one page is loaded by following `next_cursor`. This module only decides how pages
// combine; the component performs the requests.

export const NAVIGATOR_PAGE_LIMIT = 100;

export interface ObjectPage<T> {
	readonly items: readonly T[];
	readonly nextCursor: string | null;
}

/** Appends a page, skipping ids already present. Order of the first occurrence is kept. */
export function mergeObjectPage<T extends { readonly id: string }>(
	current: readonly T[],
	page: readonly T[]
): T[] {
	const seen = new Set(current.map((item) => item.id));
	const next = current.slice();
	for (const item of page) {
		if (seen.has(item.id)) continue;
		seen.add(item.id);
		next.push(item);
	}
	return next;
}

/** True when the scroller is within `threshold` pixels of the bottom. */
export function shouldLoadMore(
	scrollTop: number,
	clientHeight: number,
	scrollHeight: number,
	threshold = 48
): boolean {
	if (scrollHeight <= clientHeight) return false;
	return scrollTop + clientHeight >= scrollHeight - threshold;
}
