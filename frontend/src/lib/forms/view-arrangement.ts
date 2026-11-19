/**
 * The working arrangement of a view: the filters, sort, columns and layout a user has changed
 * without saving. It applies to the list immediately and is kept per user, form and view on this
 * device, so "sort by due date" is still there tomorrow without a save step.
 *
 * A saved view's config and a config rebuilt from the editor draft can differ in representation
 * (key order, defaults the draft fills in), so comparisons go through {@link normalizedViewConfig}
 * and the page compares the draft against the config the draft produced right after the saved
 * view was applied (its baseline), not against the raw stored JSON.
 */

import type { FormView } from '#lib/api/forms.js';
import { browserStore, type KeyValueStore } from './view-precedence.js';

export interface StoredArrangement {
	/** `updated_at` of the saved view when the arrangement was made; `null` for the built-in table. */
	base_updated_at: string | null;
	view_type: FormView['view_type'];
	config: Record<string, unknown>;
	at: string;
}

/** Config keys that describe who sees a view, not how it looks. */
const AUDIENCE_KEYS = new Set(['visibility', 'is_default']);
const FILTER_KEYS = ['filter_expression', 'filter_groups', 'filters', 'filter'] as const;
export const ARRANGEMENT_CAP = 200;
export const DEFAULT_PER_PAGE = 100;
/** The server maximum for `per_page` (`list_form_records` clamps to 200). */
export const CLIENT_FILTER_PER_PAGE = 200;

function stableStringify(value: unknown): string {
	if (Array.isArray(value)) return `[${value.map(stableStringify).join(',')}]`;
	if (value && typeof value === 'object') {
		const entries = Object.entries(value as Record<string, unknown>)
			.filter(([, item]) => item !== undefined)
			.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
		return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${stableStringify(item)}`).join(',')}}`;
	}
	return JSON.stringify(value ?? null);
}

/** Stable, key-order-insensitive form of a view config without its audience keys. */
export function normalizedViewConfig(config: Record<string, unknown> | null | undefined): string {
	const copy: Record<string, unknown> = {};
	for (const [key, value] of Object.entries(config ?? {})) {
		if (!AUDIENCE_KEYS.has(key)) copy[key] = value;
	}
	return stableStringify(copy);
}

export function arrangementDiffers(
	base: Pick<FormView, 'view_type' | 'config'> | null,
	draftType: string,
	draftConfig: Record<string, unknown>
): boolean {
	const baseType = base?.view_type ?? 'grid';
	if (baseType !== draftType) return true;
	return normalizedViewConfig(base?.config ?? {}) !== normalizedViewConfig(draftConfig);
}

function filterPart(config: Record<string, unknown> | null | undefined): string {
	const picked: Record<string, unknown> = {};
	for (const key of FILTER_KEYS) {
		if (config && config[key] !== undefined) picked[key] = config[key];
	}
	return stableStringify(picked);
}

function hasFilters(config: Record<string, unknown>): boolean {
	return FILTER_KEYS.some((key) => {
		const value = config[key];
		if (Array.isArray(value)) return value.length > 0;
		return value !== undefined && value !== null && value !== '';
	});
}

export function sortParam(config: Record<string, unknown>): string | undefined {
	const sort = config.sort;
	if (!sort || typeof sort !== 'object') return undefined;
	const field = (sort as Record<string, unknown>).field;
	if (typeof field !== 'string' || !field.trim()) return undefined;
	const direction = (sort as Record<string, unknown>).direction === 'desc' ? 'desc' : 'asc';
	return `${field.trim()}:${direction}`;
}

export interface ServerQuery {
	view_id?: string;
	sort?: string;
	per_page: number;
	/** True when the view's filters run only in the browser over the fetched page. */
	clientFilter: boolean;
}

/**
 * How to ask the server for the records of the working view.
 *
 * - Unchanged saved view: `view_id` only, exactly as before.
 * - Saved view whose filters are unchanged: `view_id` plus the working sort (exact server result).
 * - Changed filters, or the built-in table: no `view_id`; the working filters run in the browser
 *   over up to {@link CLIENT_FILTER_PER_PAGE} records.
 *
 * `sort` is only sent for keys the server can sort (`isServerSortable`); other sorts run in the
 * browser, which the list already does for every view.
 */
export function serverQueryFor(
	base: Pick<FormView, 'id' | 'config'> | null,
	draftConfig: Record<string, unknown>,
	options: { modified: boolean; isServerSortable: (fieldKey: string) => boolean }
): ServerQuery {
	const draftSort = sortParam(draftConfig);
	const sort =
		draftSort && options.isServerSortable(draftSort.slice(0, draftSort.lastIndexOf(':')))
			? draftSort
			: undefined;
	if (base && !options.modified) {
		return { view_id: base.id, per_page: DEFAULT_PER_PAGE, clientFilter: false };
	}
	if (base && filterPart(base.config) === filterPart(draftConfig)) {
		return { view_id: base.id, sort, per_page: DEFAULT_PER_PAGE, clientFilter: false };
	}
	const filtered = hasFilters(draftConfig);
	return {
		sort,
		per_page: filtered ? CLIENT_FILTER_PER_PAGE : DEFAULT_PER_PAGE,
		clientFilter: filtered
	};
}

// Storage.

const ARRANGEMENT_PREFIX = 'sv-forms-arrangement:v1:';
const ARRANGEMENT_INDEX_PREFIX = 'sv-forms-arrangement-index:v1:';

function arrangementKey(userId: string, formId: string, viewId: string): string {
	return `${ARRANGEMENT_PREFIX}${userId}:${formId}:${viewId}`;
}

function indexKey(userId: string): string {
	return `${ARRANGEMENT_INDEX_PREFIX}${userId}`;
}

function readIndex(store: KeyValueStore, userId: string): string[] {
	try {
		const parsed: unknown = JSON.parse(store.getItem(indexKey(userId)) ?? '[]');
		return Array.isArray(parsed)
			? parsed.filter((item): item is string => typeof item === 'string')
			: [];
	} catch {
		return [];
	}
}

function writeIndex(store: KeyValueStore, userId: string, keys: string[]): void {
	store.setItem(indexKey(userId), JSON.stringify(keys));
}

function isStoredArrangement(value: unknown): value is StoredArrangement {
	if (!value || typeof value !== 'object') return false;
	const entry = value as Record<string, unknown>;
	return (
		(entry.base_updated_at === null || typeof entry.base_updated_at === 'string') &&
		typeof entry.view_type === 'string' &&
		!!entry.config &&
		typeof entry.config === 'object' &&
		!Array.isArray(entry.config) &&
		typeof entry.at === 'string'
	);
}

export function readArrangement(
	userId: string,
	formId: string,
	viewId: string,
	store: KeyValueStore | null = browserStore()
): StoredArrangement | null {
	if (!store || !userId || !formId || !viewId) return null;
	try {
		const parsed: unknown = JSON.parse(
			store.getItem(arrangementKey(userId, formId, viewId)) ?? 'null'
		);
		return isStoredArrangement(parsed) ? parsed : null;
	} catch {
		return null;
	}
}

/** Writes an arrangement and keeps at most {@link ARRANGEMENT_CAP} per user, dropping the least
 * recently written. */
export function writeArrangement(
	userId: string,
	formId: string,
	viewId: string,
	value: StoredArrangement,
	store: KeyValueStore | null = browserStore()
): void {
	if (!store || !userId || !formId || !viewId) return;
	const key = arrangementKey(userId, formId, viewId);
	try {
		store.setItem(key, JSON.stringify(value));
		const index = readIndex(store, userId).filter((item) => item !== key);
		index.push(key);
		while (index.length > ARRANGEMENT_CAP) {
			const evicted = index.shift();
			if (evicted) store.removeItem(evicted);
		}
		writeIndex(store, userId, index);
	} catch {
		/* A full or blocked store only means the arrangement lasts until reload. */
	}
}

export function clearArrangement(
	userId: string,
	formId: string,
	viewId: string,
	store: KeyValueStore | null = browserStore()
): void {
	if (!store || !userId || !formId || !viewId) return;
	const key = arrangementKey(userId, formId, viewId);
	try {
		store.removeItem(key);
		writeIndex(
			store,
			userId,
			readIndex(store, userId).filter((item) => item !== key)
		);
	} catch {
		/* Nothing stored. */
	}
}

/**
 * A name for "Save view…" that says what changed: the base name, then the sort field when the
 * working view is sorted ("Open deliveries · by Due date").
 */
export function suggestViewName(
	base: { name: string },
	draftConfig: Record<string, unknown>,
	fieldLabel: (key: string) => string,
	byLabel: (field: string) => string = (field) => `by ${field}`
): string {
	const name = base.name.trim();
	const sort = sortParam(draftConfig);
	if (!sort) return name;
	const field = sort.slice(0, sort.lastIndexOf(':'));
	const suggestion = `${name} · ${byLabel(fieldLabel(field))}`;
	return suggestion.length > 80 ? suggestion.slice(0, 80).trimEnd() : suggestion;
}

/** A `form_views.key` for a new view: `^[a-z][a-z0-9_]*$`, unique per form. Names in any script
 * are fine; only ASCII letters and digits survive into the key. */
export function generateViewKey(name: string, now = Date.now(), random = Math.random()): string {
	const slug = name
		.toLowerCase()
		.normalize('NFKD')
		.replace(/[^a-z0-9]+/g, '_')
		.replace(/^_+|_+$/g, '')
		.replace(/^[0-9_]+/, '')
		.slice(0, 40);
	const suffix = `${now.toString(36)}${Math.floor(random * 36 ** 4)
		.toString(36)
		.padStart(4, '0')}`;
	return `${slug || 'view'}_${suffix}`;
}

// Record panel width, per user (not per form): people size the panel for their screen.

export function readPanelWidth(
	userId: string,
	store: KeyValueStore | null = browserStore()
): number | null {
	if (!store || !userId) return null;
	try {
		const parsed: unknown = JSON.parse(
			store.getItem(`sv-forms-panel-width:v1:${userId}`) ?? 'null'
		);
		const px = parsed && typeof parsed === 'object' ? (parsed as { px?: unknown }).px : null;
		return typeof px === 'number' && Number.isFinite(px) && px > 0 ? px : null;
	} catch {
		return null;
	}
}

export function writePanelWidth(
	userId: string,
	px: number,
	store: KeyValueStore | null = browserStore()
): void {
	if (!store || !userId || !Number.isFinite(px)) return;
	try {
		store.setItem(`sv-forms-panel-width:v1:${userId}`, JSON.stringify({ px: Math.round(px) }));
	} catch {
		/* The width lasts until reload. */
	}
}
