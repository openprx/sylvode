/**
 * Row model of the secondary column, selection from the current route, and the keyboard rules of
 * a list whose rows keep the native link role (contract 2.4.2).
 */
import type { Shortcut } from '../shortcuts.svelte';
import type { TreeKind, TreeNode } from '../../project/nav';

export type GlyphName =
	| 'house'
	| 'bell'
	| 'list-checks'
	| 'clock'
	| 'folder'
	| 'folder-kanban'
	| 'layout-list'
	| 'kanban'
	| 'repeat'
	| 'table-2'
	| 'file-text'
	| 'app-window'
	| 'settings'
	| 'workflow'
	| 'bot'
	| 'sparkles'
	| 'landmark'
	| 'flag'
	| 'users'
	| 'briefcase'
	| 'link'
	| 'external-link'
	| 'star'
	| 'layout-template'
	| 'globe'
	| 'puzzle';

export interface NavRow {
	/** Unique in the whole column. */
	id: string;
	label: string;
	href: string;
	glyph: GlyphName;
	/** Indent level, 0 for top rows. */
	level: number;
	parentId: string | null;
	/** Has children (a chevron and `aria-expanded`). */
	expandable?: boolean;
	expanded?: boolean;
	/** Trailing count (unread notifications). */
	count?: number;
	/** False for rows that duplicate a parent's link ("All forms"): route selection skips them. */
	selectable?: boolean;
	/** Data attributes rendered on the link. */
	attrs?: Record<`data-${string}`, string>;
	/** Present when "P" may add the row to the rail. */
	pin?: Shortcut;
	/** An external link: opens in a new tab and shows the external glyph. */
	external?: boolean;
	/** A section caption, not a link (`href` is empty). */
	caption?: boolean;
}

// --- selection from the route ---

function splitHref(href: string): { path: string; query: URLSearchParams } {
	const url = new URL(href, 'http://x');
	return { path: trimSlash(url.pathname), query: url.searchParams };
}
function trimSlash(path: string): string {
	return path.length > 1 && path.endsWith('/') ? path.slice(0, -1) : path;
}

/**
 * The selected row for a URL: among rows whose path equals the pathname or is a parent of it (and
 * whose query parameters all match), the one with the longest path, then one with a query, then
 * the deepest. Null when no row matches.
 */
export function selectedRowId(
	rows: readonly NavRow[],
	url: { pathname: string; search: string }
): string | null {
	const pathname = trimSlash(url.pathname);
	const current = new URLSearchParams(url.search);
	let best: { id: string; score: [number, number, number] } | null = null;
	for (const row of rows) {
		if (row.selectable === false) continue;
		const { path, query } = splitHref(row.href);
		const pathMatches = pathname === path || pathname.startsWith(path === '/' ? '/' : `${path}/`);
		if (!pathMatches) continue;
		let queryMatches = true;
		for (const [key, value] of query) if (current.get(key) !== value) queryMatches = false;
		if (!queryMatches) continue;
		const score: [number, number, number] = [path.length, query.size > 0 ? 1 : 0, row.level];
		if (
			!best ||
			score[0] > best.score[0] ||
			(score[0] === best.score[0] &&
				(score[1] > best.score[1] || (score[1] === best.score[1] && score[2] >= best.score[2])))
		)
			best = { id: row.id, score };
	}
	return best?.id ?? null;
}

// --- tree rows from project tree data ---

const TREE_GLYPHS: Record<TreeKind, GlyphName> = {
	project: 'folder',
	overview: 'layout-list',
	issues: 'list-checks',
	board: 'kanban',
	cycles: 'repeat',
	forms: 'table-2',
	form: 'file-text',
	'all-forms': 'table-2',
	apps: 'app-window',
	app: 'puzzle',
	link: 'link',
	caption: 'link',
	settings: 'settings'
};

/**
 * Flattens a project tree into visible rows. `expanded` holds the ids of nodes that show their
 * children; the root is always listed. A node is expandable when it has children or loads them
 * on first expansion.
 */
export function flattenTree(
	root: TreeNode,
	expanded: ReadonlySet<string>,
	options: { workspaceId: string }
): NavRow[] {
	const rows: NavRow[] = [];
	const projectId = root.id;
	const walk = (node: TreeNode, level: number, parentId: string | null) => {
		const expandable = node.lazy === true || (node.children?.length ?? 0) > 0;
		const open = expandable && expanded.has(node.id);
		const attrs: Record<`data-${string}`, string> = {
			'data-tree-kind': node.kind,
			'data-project-id': projectId
		};
		if (node.kind === 'form') attrs['data-form-id'] = node.id;
		const row: NavRow = {
			id: node.id,
			label: node.label,
			href: node.href,
			glyph: TREE_GLYPHS[node.kind],
			level,
			parentId,
			expandable,
			expanded: open,
			selectable: node.kind !== 'all-forms' && node.kind !== 'caption' && !node.external,
			attrs
		};
		if (node.glyph) row.glyph = node.glyph as GlyphName;
		if (node.external) row.external = true;
		if (node.kind === 'caption') row.caption = true;
		if (node.kind === 'project')
			row.pin = {
				id: `project:${node.id}`,
				kind: 'project',
				label: node.label,
				href: node.href,
				workspaceId: options.workspaceId,
				projectId: node.id,
				objectId: node.id
			};
		else if (node.kind === 'form')
			row.pin = {
				id: `form:${node.id}`,
				kind: 'form',
				label: node.label,
				href: node.href,
				workspaceId: options.workspaceId,
				projectId,
				formId: node.id,
				objectId: node.id
			};
		rows.push(row);
		if (open) for (const child of node.children ?? []) walk(child, level + 1, node.id);
	};
	walk(root, 0, null);
	return rows;
}

// --- id sets (plain module: the sets are replaced, never mutated) ---

export function emptyIdSet(): ReadonlySet<string> {
	return new Set();
}
export function unionIds(...sets: ReadonlyArray<Iterable<string>>): ReadonlySet<string> {
	return new Set(sets.flatMap((s) => [...s]));
}
/** A copy of `ids` with `id` added (`open`) or removed. */
export function toggledIds(
	ids: ReadonlySet<string>,
	id: string,
	open: boolean
): ReadonlySet<string> {
	const next = new Set(ids);
	if (open) next.add(id);
	else next.delete(id);
	return next;
}

// --- keyboard ---

/** What the keyboard handler needs to know about the focused row. */
export interface RowRef {
	id: string;
	parentId: string | null;
	expandable: boolean;
	expanded: boolean;
	firstChildId: string | null;
}

export type TreeKeyAction =
	| { type: 'expand'; id: string }
	| { type: 'collapse'; id: string }
	| { type: 'focus'; id: string }
	| { type: 'pin'; id: string };

/**
 * Keys beyond the vertical movement the roving group already handles: right expands or moves to
 * the first child, left collapses or moves to the parent, P pins the row to the rail. Null for
 * any other key.
 */
export function treeKeyAction(key: string, row: RowRef): TreeKeyAction | null {
	if (key === 'ArrowRight') {
		if (!row.expandable) return null;
		if (!row.expanded) return { type: 'expand', id: row.id };
		return row.firstChildId ? { type: 'focus', id: row.firstChildId } : null;
	}
	if (key === 'ArrowLeft') {
		if (row.expandable && row.expanded) return { type: 'collapse', id: row.id };
		return row.parentId ? { type: 'focus', id: row.parentId } : null;
	}
	if (key === 'p' || key === 'P') return { type: 'pin', id: row.id };
	return null;
}
