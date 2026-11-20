import { nextRovingIndex } from '../foundations/roving';

export const TAB_MIN = 120;
export const TAB_MAX = 200;
export const TAB_GAP = 2;
/** Strip padding on both sides (8 + 8). */
export const STRIP_PADDING = 16;
/** Room kept for the overflow button (with its "+n" label). */
export const OVERFLOW_RESERVE = 80;
/** Below this width the strip shows only the active tab and the overflow button. */
export const COLLAPSE_BELOW = 600;

export interface OverflowLayout {
	mode: 'tabs' | 'collapsed';
	/** Indexes of the tabs drawn in the strip, ascending. */
	visible: number[];
	/** Indexes moved into the overflow menu, ascending. */
	hidden: number[];
	/** Width of each drawn tab. */
	tabWidth: number;
}

/**
 * Which tabs fit. Tabs shrink from 200 to 120; beyond that the rightmost inactive tabs are
 * hidden (the active tab always stays); below 600 only the active tab is drawn.
 */
export function computeOverflow(
	count: number,
	activeIndex: number,
	containerWidth: number
): OverflowLayout {
	const all = Array.from({ length: count }, (_, i) => i);
	const active = activeIndex >= 0 && activeIndex < count ? activeIndex : 0;
	if (count === 0) return { mode: 'tabs', visible: [], hidden: [], tabWidth: TAB_MAX };
	if (containerWidth < COLLAPSE_BELOW) {
		return {
			mode: 'collapsed',
			visible: [active],
			hidden: all.filter((i) => i !== active),
			tabWidth: Math.max(
				TAB_MIN,
				Math.min(TAB_MAX, containerWidth - STRIP_PADDING - OVERFLOW_RESERVE)
			)
		};
	}
	const room = containerWidth - STRIP_PADDING - OVERFLOW_RESERVE;
	const widthFor = (n: number) => Math.floor((room - TAB_GAP * (n - 1)) / n);
	if (widthFor(count) >= TAB_MIN) {
		return {
			mode: 'tabs',
			visible: all,
			hidden: [],
			tabWidth: Math.min(TAB_MAX, widthFor(count))
		};
	}
	let fit = count;
	while (fit > 1 && widthFor(fit) < TAB_MIN) fit -= 1;
	const visible = new Set<number>([active]);
	for (const i of all) {
		if (visible.size >= fit) break;
		visible.add(i);
	}
	const shown = [...visible].sort((a, b) => a - b);
	return {
		mode: 'tabs',
		visible: shown,
		hidden: all.filter((i) => !visible.has(i)),
		tabWidth: Math.min(TAB_MAX, Math.max(TAB_MIN, widthFor(fit)))
	};
}

/** The new key order after moving `key` to `toIndex` (clamped); the same array when nothing moves. */
export function reorder(keys: readonly string[], key: string, toIndex: number): string[] {
	const from = keys.indexOf(key);
	if (from < 0) return [...keys];
	const to = Math.max(0, Math.min(keys.length - 1, toIndex));
	if (to === from) return [...keys];
	const next = [...keys];
	next.splice(from, 1);
	next.splice(to, 0, key);
	return next;
}

/** The index `source` ends at when dropped before or after `target`. */
export function dropIndex(
	keys: readonly string[],
	source: string,
	target: string,
	side: 'before' | 'after'
): number | null {
	const from = keys.indexOf(source);
	const over = keys.indexOf(target);
	if (from < 0 || over < 0 || source === target) return null;
	let to = side === 'before' ? over : over + 1;
	if (from < to) to -= 1;
	return to === from ? null : to;
}

/** Index of the tab that takes focus for ArrowLeft/Right, Home and End (wraps), else null. */
export function focusTarget(key: string, current: number, count: number): number | null {
	if (count === 0) return null;
	return nextRovingIndex(key, current, Array<boolean>(count).fill(true), {
		orientation: 'horizontal',
		loop: true
	});
}

export type TabKeyAction =
	| { kind: 'focus'; index: number }
	| { kind: 'close' }
	| { kind: 'move'; toIndex: number }
	| { kind: 'menu' };

export interface TabKeyEvent {
	key: string;
	altKey: boolean;
	shiftKey: boolean;
	ctrlKey: boolean;
	metaKey: boolean;
}

/** What a key on a focused tab does. Alt+Shift+Arrow moves the tab; Delete closes it. */
export function tabKeyAction(
	event: TabKeyEvent,
	index: number,
	count: number
): TabKeyAction | null {
	if (event.altKey && event.shiftKey && !event.ctrlKey && !event.metaKey) {
		if (event.key === 'ArrowLeft') return { kind: 'move', toIndex: Math.max(0, index - 1) };
		if (event.key === 'ArrowRight')
			return { kind: 'move', toIndex: Math.min(count - 1, index + 1) };
		return null;
	}
	if (event.altKey || event.ctrlKey || event.metaKey) return null;
	if (event.key === 'Delete') return { kind: 'close' };
	if (event.shiftKey && event.key === 'F10') return { kind: 'menu' };
	if (event.key === 'ContextMenu') return { kind: 'menu' };
	const target = focusTarget(event.key, index, count);
	return target === null ? null : { kind: 'focus', index: target };
}
