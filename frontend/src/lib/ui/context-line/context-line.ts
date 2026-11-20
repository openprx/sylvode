export interface ContextSegment {
	id: string;
	label: string;
	/** Absent for plain text (restricted or unavailable ancestors). */
	href?: string;
	state?: 'normal' | 'restricted' | 'unavailable';
}

/** "A / B / C": the string a hosted frame passes to the host as its context. */
export function contextString(segments: readonly ContextSegment[], current: string): string {
	return [...segments.map((s) => s.label), current].join(' / ');
}

/** True when the text of `el` is cut by an ellipsis. */
export function isTruncated(el: HTMLElement): boolean {
	return el.scrollWidth > el.clientWidth;
}

export interface CollapseInput {
	/** Natural width of each ancestor segment, separator included. */
	segments: readonly number[];
	/** Natural width of the current segment, separator included. */
	current: number;
	/** Width of the "..." button, separator included. */
	ellipsis: number;
	/** Width the line may use. */
	available: number;
}

/**
 * Which ancestor segments move into the "..." menu (indexes, ascending, always one run that
 * starts at 1). Rules: never the first, never the parent of the current and never the current
 * one; if the line still does not fit with only first, "...", parent and current, the parent goes
 * too. What still does not fit is truncated by CSS, not here.
 */
export function collapseSegments({
	segments,
	current,
	ellipsis,
	available
}: CollapseInput): number[] {
	const n = segments.length;
	const fits = (hidden: number[]) => {
		let total = current + (hidden.length ? ellipsis : 0);
		for (let i = 0; i < n; i += 1) if (!hidden.includes(i)) total += segments[i];
		return total <= available;
	};
	const hidden: number[] = [];
	if (fits(hidden)) return hidden;
	for (let i = 1; i < n - 1; i += 1) {
		hidden.push(i);
		if (fits(hidden)) return hidden;
	}
	if (n >= 2) hidden.push(n - 1);
	return hidden;
}
