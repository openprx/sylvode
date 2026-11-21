import type { Snippet } from 'svelte';

export interface Column<T> {
	id: string;
	header: string;
	width?: number;
	minWidth?: number;
	maxWidth?: number;
	align?: 'start' | 'end';
	sortable?: boolean;
	/** Sticks to the inline start while the table scrolls sideways. */
	frozen?: boolean;
	cell: Snippet<[T]>;
	/** Where the column goes in the two-line layout; the first column is `primary` by default. */
	twoLine?: 'primary' | 'secondary' | 'hidden';
}

export type SortDirection = 'asc' | 'desc';
export type SortState = { column: string; direction: SortDirection } | null;
export type TableState = 'ready' | 'loading' | 'empty' | 'noResults' | 'failed' | 'offline';

export const DEFAULT_WIDTH = 220;
export const MIN_WIDTH = 80;
export const MAX_WIDTH = 400;
/** Width of the selection column. */
export const CHECK_WIDTH = 40;
export const ACTIONS_WIDTH = 96;
/** At or below this container width rows become two lines (slots 340 and 480). */
export const TWO_LINE_UP_TO = 480;

/** Ascending, then descending, then none; a different column starts ascending. */
export function nextSort(sort: SortState, column: string): SortState {
	if (!sort || sort.column !== column) return { column, direction: 'asc' };
	return sort.direction === 'asc' ? { column, direction: 'desc' } : null;
}

/** Value of `aria-sort` for a header cell; undefined for columns that cannot be sorted. */
export function ariaSort(
	sort: SortState,
	column: string,
	sortable: boolean | undefined
): 'ascending' | 'descending' | 'none' | undefined {
	if (!sortable) return undefined;
	if (!sort || sort.column !== column) return 'none';
	return sort.direction === 'asc' ? 'ascending' : 'descending';
}

export type HeaderState = 'none' | 'some' | 'all';

/** State of the header checkbox: how many of this page's rows are selected. */
export function headerState(ids: readonly string[], selected: ReadonlySet<string>): HeaderState {
	if (ids.length === 0) return 'none';
	const count = ids.filter((id) => selected.has(id)).length;
	return count === 0 ? 'none' : count === ids.length ? 'all' : 'some';
}

/** Selects or clears every row of the page; other selected ids are kept. */
export function setPage(
	ids: readonly string[],
	selected: ReadonlySet<string>,
	on: boolean
): Set<string> {
	const next = new Set(selected);
	for (const id of ids) {
		if (on) next.add(id);
		else next.delete(id);
	}
	return next;
}

export function setRow(selected: ReadonlySet<string>, id: string, on: boolean): Set<string> {
	const next = new Set(selected);
	if (on) next.add(id);
	else next.delete(id);
	return next;
}

/** Sets every row between `anchor` and `target` (inclusive, either order) to `on`. */
export function setRange(
	ids: readonly string[],
	selected: ReadonlySet<string>,
	anchor: string | null,
	target: string,
	on: boolean
): Set<string> {
	const to = ids.indexOf(target);
	const from = anchor === null ? -1 : ids.indexOf(anchor);
	if (to < 0) return new Set(selected);
	if (from < 0) return setRow(selected, target, on);
	const [low, high] = from < to ? [from, to] : [to, from];
	const next = new Set(selected);
	for (let i = low; i <= high; i += 1) {
		if (on) next.add(ids[i]);
		else next.delete(ids[i]);
	}
	return next;
}

/** True when the container is narrow enough for two-line rows. */
export function isTwoLine(containerWidth: number): boolean {
	return containerWidth > 0 && containerWidth <= TWO_LINE_UP_TO;
}

/** Row to focus for ArrowUp/Down and Ctrl/Cmd+Home/End; no wrapping. */
export function nextRow(
	key: string,
	withModifier: boolean,
	index: number,
	count: number
): number | null {
	if (count === 0) return null;
	if (key === 'ArrowDown') return Math.min(count - 1, index + 1);
	if (key === 'ArrowUp') return Math.max(0, index - 1);
	if (withModifier && key === 'Home') return 0;
	if (withModifier && key === 'End') return count - 1;
	return null;
}

export function columnWidth<T>(column: Column<T>, widths?: Record<string, number>): number {
	const min = column.minWidth ?? MIN_WIDTH;
	const max = column.maxWidth ?? MAX_WIDTH;
	return Math.max(min, Math.min(max, widths?.[column.id] ?? column.width ?? DEFAULT_WIDTH));
}

/** Inline-start offset of each frozen column: after the selection column and earlier frozen ones. */
export function frozenOffsets<T>(
	columns: readonly Column<T>[],
	widths: Record<string, number> | undefined,
	leading: number
): Record<string, number> {
	const out: Record<string, number> = {};
	let offset = leading;
	for (const column of columns) {
		if (!column.frozen) continue;
		out[column.id] = offset;
		offset += columnWidth(column, widths);
	}
	return out;
}

export function tableWidth<T>(
	columns: readonly Column<T>[],
	widths: Record<string, number> | undefined,
	selectable: boolean,
	actions: boolean
): number {
	return (
		(selectable ? CHECK_WIDTH : 0) +
		columns.reduce((sum, column) => sum + columnWidth(column, widths), 0) +
		(actions ? ACTIONS_WIDTH : 0)
	);
}

/** Columns for the two-line layout: the first is primary unless set, the rest secondary. */
export function twoLineColumns<T>(columns: readonly Column<T>[]): {
	primary: Column<T>[];
	secondary: Column<T>[];
} {
	const primary: Column<T>[] = [];
	const secondary: Column<T>[] = [];
	columns.forEach((column, index) => {
		const role = column.twoLine ?? (index === 0 ? 'primary' : 'secondary');
		if (role === 'primary') primary.push(column);
		else if (role === 'secondary') secondary.push(column);
	});
	return { primary, secondary };
}
