import type { LayoutNode, LayoutSpec, LayoutValidationError } from '../types';

/**
 * The operation log of the structural layout editor.
 *
 * Every edit is one `LayoutOp` `{op, path, before, after}` whose inverse is computed from the op
 * itself, so a linear history gives undo and redo, and the same ops are the unit an AI patch can
 * carry. Ops are plain JSON: `path` is an RFC 6901 pointer into the `sv.layout.v1` spec and
 * `null` means "absent".
 *
 * - `set`: the value at `path` goes from `before` to `after` (`null` = the member does not exist).
 * - `insert`: `after` is a node that did not exist at `path` (an element of a `children` array).
 * - `remove`: `before` is the node that existed at `path`.
 * - `move`: `path` is a `children` array, the element at index `before` goes to index `after`.
 */
export type LayoutOp =
	| { op: 'set'; path: string; before: unknown; after: unknown }
	| { op: 'insert'; path: string; before: null; after: LayoutNode }
	| { op: 'remove'; path: string; before: LayoutNode; after: null }
	| { op: 'move'; path: string; before: number; after: number };

export interface OpHistory {
	/** Applied ops in order; `ops[0..cursor)` are in effect, the rest can be redone. */
	ops: readonly LayoutOp[];
	cursor: number;
}

export const EMPTY_HISTORY: OpHistory = { ops: [], cursor: 0 };

/** Longest history kept; the oldest op is dropped first. */
export const MAX_HISTORY = 200;

export const GRID_MAX_COLUMNS = 4;

export function inverse(op: LayoutOp): LayoutOp {
	switch (op.op) {
		case 'set':
			return { op: 'set', path: op.path, before: op.after, after: op.before };
		case 'insert':
			return { op: 'remove', path: op.path, before: op.after, after: null };
		case 'remove':
			return { op: 'insert', path: op.path, before: null, after: op.before };
		case 'move':
			return { op: 'move', path: op.path, before: op.after, after: op.before };
	}
}

function decode(segment: string): string {
	return segment.replace(/~1/g, '/').replace(/~0/g, '~');
}

export function pointerSegments(path: string): string[] | null {
	if (path === '') return [];
	if (!path.startsWith('/')) return null;
	return path.slice(1).split('/').map(decode);
}

type Container = Record<string, unknown> | unknown[];

function isContainer(value: unknown): value is Container {
	return typeof value === 'object' && value !== null;
}

function clone<T>(value: T): T {
	return structuredClone(value);
}

/** The value at a pointer, or `undefined`. */
export function valueAt(root: unknown, path: string): unknown {
	const segments = pointerSegments(path);
	if (!segments) return undefined;
	let current: unknown = root;
	for (const segment of segments) {
		if (!isContainer(current)) return undefined;
		current = (current as Record<string, unknown>)[segment];
	}
	return current;
}

/** The node at `path`, when the pointer names a node of the tree. */
export function nodeAt(spec: LayoutSpec, path: string): LayoutNode | null {
	const value = valueAt(spec, path);
	return value && typeof value === 'object' && typeof (value as LayoutNode).type === 'string'
		? (value as LayoutNode)
		: null;
}

function parentOf(path: string): { parent: string; key: string } | null {
	const cut = path.lastIndexOf('/');
	if (cut < 0) return null;
	return { parent: path.slice(0, cut), key: decode(path.slice(cut + 1)) };
}

/** Applies one op to a copy of the spec; `null` when the op does not fit the spec. */
export function applyOp(spec: LayoutSpec, op: LayoutOp): LayoutSpec | null {
	const next = clone(spec);
	if (op.op === 'move') {
		const list = valueAt(next, op.path);
		if (!Array.isArray(list)) return null;
		const { before: from, after: to } = op;
		if (!Number.isInteger(from) || !Number.isInteger(to)) return null;
		if (from < 0 || to < 0 || from >= list.length || to >= list.length) return null;
		const [item] = list.splice(from, 1);
		list.splice(to, 0, item);
		return next;
	}
	const place = parentOf(op.path);
	if (!place) return null;
	if (op.op === 'set') {
		let holder = valueAt(next, place.parent);
		if (holder === undefined && place.parent.endsWith('/props')) {
			const owner = parentOf(place.parent);
			const node = owner ? valueAt(next, owner.parent) : undefined;
			if (!isContainer(node) || Array.isArray(node)) return null;
			(node as Record<string, unknown>).props = {};
			holder = (node as Record<string, unknown>).props;
		}
		if (!isContainer(holder) || Array.isArray(holder)) return null;
		if (op.after === null) delete (holder as Record<string, unknown>)[place.key];
		else (holder as Record<string, unknown>)[place.key] = clone(op.after);
		return next;
	}
	const list = valueAt(next, place.parent);
	if (!Array.isArray(list)) return null;
	const index = Number(place.key);
	if (!Number.isInteger(index) || index < 0) return null;
	if (op.op === 'insert') {
		if (index > list.length) return null;
		list.splice(index, 0, clone(op.after));
		return next;
	}
	if (index >= list.length) return null;
	list.splice(index, 1);
	return next;
}

/** Records a new op after the cursor; everything that could have been redone is dropped. */
export function record(history: OpHistory, op: LayoutOp): OpHistory {
	const kept = history.ops.slice(0, history.cursor);
	kept.push(op);
	const ops = kept.length > MAX_HISTORY ? kept.slice(kept.length - MAX_HISTORY) : kept;
	return { ops, cursor: ops.length };
}

export const canUndo = (history: OpHistory) => history.cursor > 0;
export const canRedo = (history: OpHistory) => history.cursor < history.ops.length;

export interface Stepped {
	spec: LayoutSpec;
	history: OpHistory;
	op: LayoutOp;
}

export function undo(spec: LayoutSpec, history: OpHistory): Stepped | null {
	if (!canUndo(history)) return null;
	const op = history.ops[history.cursor - 1];
	const next = applyOp(spec, inverse(op));
	return next
		? { spec: next, history: { ops: history.ops, cursor: history.cursor - 1 }, op }
		: null;
}

export function redo(spec: LayoutSpec, history: OpHistory): Stepped | null {
	if (!canRedo(history)) return null;
	const op = history.ops[history.cursor];
	const next = applyOp(spec, op);
	return next
		? { spec: next, history: { ops: history.ops, cursor: history.cursor + 1 }, op }
		: null;
}

/** Applies a new op to the spec and records it; `null` when it does not fit. */
export function perform(
	spec: LayoutSpec,
	history: OpHistory,
	op: LayoutOp
): { spec: LayoutSpec; history: OpHistory } | null {
	const next = applyOp(spec, op);
	return next ? { spec: next, history: record(history, op) } : null;
}

// Op builders: they read `before` from the spec so the caller cannot get it wrong.

export function childrenPath(nodePath: string): string {
	return `${nodePath}/children`;
}

export function moveOp(
	nodePath: string,
	spec: LayoutSpec,
	from: number,
	to: number
): LayoutOp | null {
	const count = nodeAt(spec, nodePath)?.children?.length ?? 0;
	if (from === to || from < 0 || to < 0 || from >= count || to >= count) return null;
	return { op: 'move', path: childrenPath(nodePath), before: from, after: to };
}

export function insertOp(
	nodePath: string,
	spec: LayoutSpec,
	index: number,
	node: LayoutNode
): LayoutOp | null {
	const count = nodeAt(spec, nodePath)?.children?.length ?? 0;
	if (index < 0 || index > count) return null;
	return { op: 'insert', path: `${childrenPath(nodePath)}/${index}`, before: null, after: node };
}

export function removeOp(spec: LayoutSpec, path: string): LayoutOp | null {
	if (!/\/children\/\d+$/.test(path)) return null;
	const node = nodeAt(spec, path);
	return node ? { op: 'remove', path, before: clone(node), after: null } : null;
}

/** Columns of a grid as the renderers lay it out: `columns` 1..=4, 2 when absent. */
export function columnsOf(node: LayoutNode): number {
	const value = node.props?.columns;
	return typeof value === 'number' && value >= 1 && value <= GRID_MAX_COLUMNS ? value : 2;
}

/** Columns a grid child spans (`col_span`, 1 when absent or invalid). */
export function colSpanOf(node: LayoutNode): number {
	const value = node.col_span;
	return typeof value === 'number' && Number.isInteger(value) && value >= 1 ? value : 1;
}

/**
 * Sets `col_span` of a grid child to 1..=the grid's columns. A span of 1 removes the member.
 * The op is a plain `set` whose inverse restores the previous value.
 */
export function setColSpanOp(spec: LayoutSpec, childPath: string, span: number): LayoutOp | null {
	if (!/\/children\/\d+$/.test(childPath)) return null;
	const node = nodeAt(spec, childPath);
	const grid = nodeAt(spec, childPath.slice(0, childPath.lastIndexOf('/children/')));
	if (!node || !grid || grid.type !== 'grid') return null;
	const next = Math.min(columnsOf(grid), Math.max(1, Math.round(span)));
	const stored = node.col_span;
	if (stored === next || (stored === undefined && next === 1)) return null;
	return {
		op: 'set',
		path: `${childPath}/col_span`,
		before: stored === undefined ? null : stored,
		after: next === 1 ? null : next
	};
}

/** Ops that bring every child span of a grid within a new column count (run before shrinking). */
export function clampSpanOps(spec: LayoutSpec, gridPath: string, columns: number): LayoutOp[] {
	const grid = nodeAt(spec, gridPath);
	if (!grid || grid.type !== 'grid') return [];
	const ops: LayoutOp[] = [];
	(grid.children ?? []).forEach((child, index) => {
		if (colSpanOf(child) > columns) {
			ops.push({
				op: 'set',
				path: `${gridPath}/children/${index}/col_span`,
				before: child.col_span ?? null,
				after: columns === 1 ? null : columns
			});
		}
	});
	return ops;
}

/**
 * The span a handle drag lands on: the start span plus the pointer travel in whole columns,
 * where one column is `columnWidth` pixels wide.
 */
export function spanFromDrag(
	startSpan: number,
	dx: number,
	columnWidth: number,
	columns: number
): number {
	if (!(columnWidth > 0)) return startSpan;
	return Math.min(columns, Math.max(1, startSpan + Math.round(dx / columnWidth)));
}

export function setColumnsOp(spec: LayoutSpec, gridPath: string, columns: number): LayoutOp | null {
	const node = nodeAt(spec, gridPath);
	if (!node || node.type !== 'grid') return null;
	const next = Math.min(GRID_MAX_COLUMNS, Math.max(1, Math.round(columns)));
	const stored = node.props?.columns;
	if (stored === next) return null;
	return {
		op: 'set',
		path: `${gridPath}/props/columns`,
		before: stored === undefined ? null : stored,
		after: next
	};
}

// Geometry.

/** Snaps a pointer offset along the column ruler to a column count in 1..=4. */
export function snapColumns(offset: number, width: number): number {
	if (!(width > 0)) return 1;
	const raw = Math.ceil((offset / width) * GRID_MAX_COLUMNS);
	return Math.min(GRID_MAX_COLUMNS, Math.max(1, raw));
}

export interface Box {
	left: number;
	top: number;
	width: number;
	height: number;
}

/** The index of the sibling box nearest to a pointer position (centre distance). */
export function dropIndex(boxes: readonly Box[], x: number, y: number): number {
	let best = 0;
	let bestDistance = Infinity;
	boxes.forEach((box, index) => {
		const dx = x - (box.left + box.width / 2);
		const dy = y - (box.top + box.height / 2);
		const distance = dx * dx + dy * dy;
		if (distance < bestDistance) {
			bestDistance = distance;
			best = index;
		}
	});
	return best;
}

/** Index a keyboard arrow moves a child to inside a parent: grid rows step by the column count. */
export function arrowTarget(
	key: string,
	index: number,
	count: number,
	columns: number | null
): number | null {
	let delta = 0;
	if (key === 'ArrowLeft' || key === 'ArrowUp') delta = -1;
	else if (key === 'ArrowRight' || key === 'ArrowDown') delta = 1;
	else return null;
	if (columns && columns > 1 && (key === 'ArrowUp' || key === 'ArrowDown')) delta *= columns;
	const target = index + delta;
	if (target < 0 || target >= count) return null;
	return target;
}

// Validation errors on nodes.

const NODE_PREFIX = /^(\/root(?:\/children\/\d+)*)/;

/** The node pointer a server error path belongs to (`/root/children/1/props/columns` -> `/root/children/1`). */
export function nodeOfPath(path: string): string | null {
	return NODE_PREFIX.exec(path)?.[1] ?? null;
}

export function errorsByNode(
	errors: readonly LayoutValidationError[]
): Map<string, LayoutValidationError[]> {
	const map = new Map<string, LayoutValidationError[]>();
	for (const error of errors) {
		const key = nodeOfPath(error.path) ?? '';
		const list = map.get(key);
		if (list) list.push(error);
		else map.set(key, [error]);
	}
	return map;
}

// Palette.

const TEXT = (value: string) => ({ en: value });

/** A new node of `type` that carries the props the server requires; refs are filled by the user. */
export function defaultNode(type: string): LayoutNode {
	switch (type) {
		case 'stack':
			return { type, props: { direction: 'vertical' }, children: [] };
		case 'grid':
			return { type, props: { columns: 2 }, children: [] };
		case 'card':
			return { type, props: { title: TEXT('Card') }, children: [] };
		case 'text':
			return { type, props: { value: TEXT('Text') } };
		case 'markdown':
			return { type, props: { value: TEXT('Markdown') } };
		case 'stat':
			return { type, props: { label: TEXT('Label'), value: '0' } };
		case 'badge':
			return { type, props: { value: TEXT('Badge') } };
		case 'table':
			return { type, props: { rows: '$data.rows', columns: [{ field: 'title' }] } };
		case 'field-display':
			return { type, props: { record: '$data.record', field: 'title' } };
		case 'form':
			return { type, props: { data: 'form', submit: 'submit' } };
		case 'button':
			return { type, props: { label: TEXT('Button') } };
		default:
			return { type };
	}
}

const CONTAINERS = new Set(['stack', 'grid', 'card']);

/** Node types the palette adds. Tabs fix their child count, so they are edited as JSON. */
export const PALETTE_TYPES = [
	'stack',
	'grid',
	'card',
	'text',
	'markdown',
	'stat',
	'badge',
	'table',
	'field-display',
	'form',
	'button'
] as const;

export function takesChildren(node: LayoutNode): boolean {
	return CONTAINERS.has(node.type);
}

/**
 * What the palette offers under the node at `path`. Only `stack`, `grid` and `card` take freely
 * added children; the direct children of a root `stack` are limited to `stack_children` when the
 * slot lists them (`slot_node_not_allowed`).
 */
export function paletteFor(
	spec: LayoutSpec,
	path: string,
	slot: { stack_children?: string[] }
): string[] {
	const node = nodeAt(spec, path);
	if (!node || !takesChildren(node)) return [];
	const restricted =
		path === '/root' && node.type === 'stack' && (slot.stack_children?.length ?? 0) > 0;
	return PALETTE_TYPES.filter((type) => !restricted || slot.stack_children?.includes(type));
}

/** What the editor offers every node; reads are reactive, writes append to the op log. */
export interface StructureCtrl {
	errorsAt(path: string): LayoutValidationError[];
	palette(path: string): string[];
	dragging(): { parent: string; from: number; to: number } | null;
	beginDrag(parent: string, from: number, list: HTMLElement, event: PointerEvent): void;
	move(parent: string, from: number, to: number): void;
	remove(path: string): void;
	add(parent: string, type: string): void;
	setColumns(path: string, columns: number): void;
	setColSpan(path: string, span: number): void;
}
