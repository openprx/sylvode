/** Sizes and breakpoints of the secondary column (contract 2.4 and 2.5). */

/** Window width from which the column is 264 wide; below it, 232. */
export const COLUMN_BREAKPOINT = 1280;
export const COLUMN_WIDTH_WIDE = 264;
export const COLUMN_WIDTH_NARROW = 232;
/** At this width or less the rail becomes a top bar and one column shows at a time. */
export const TOPBAR_MAX_WIDTH = 800;

export function columnWidth(windowWidth: number): number {
	return windowWidth >= COLUMN_BREAKPOINT ? COLUMN_WIDTH_WIDE : COLUMN_WIDTH_NARROW;
}

/** True when the window is in the one-column-at-a-time layout. */
export function isNarrowWindow(windowWidth: number): boolean {
	return windowWidth <= TOPBAR_MAX_WIDTH;
}

/** The hint printed in the search field: the modifier key of the platform. */
export function searchHint(platform: string): string {
	return /mac|iphone|ipad/i.test(platform) ? '⌘K' : 'Ctrl K';
}
