import type { Destination } from './route-map';

/** The slice of Web Storage the shell uses; lets tests pass a memory store. */
export interface StorageLike {
	getItem(key: string): string | null;
	setItem(key: string, value: string): void;
	removeItem(key: string): void;
}

export const SHELL_STATE_PREFIX = 'sv-shell:v1:';
export const SHORTCUTS_PREFIX = 'sv-rail-shortcuts:v1:';
export const SETTINGS_RETURN_KEY = 'sv-settings-return:v1';

export const MAX_RECENT = 8;
export const MAX_EXPANDED_PER_WORKSPACE = 50;
const MAX_LAST_ENTRIES = 200;
const MAX_TITLE = 200;

export function shellStateKey(userId: string): string {
	return SHELL_STATE_PREFIX + userId;
}
export function shortcutsKey(userId: string): string {
	return SHORTCUTS_PREFIX + userId;
}

/** `localStorage` or `sessionStorage`, or null when the browser refuses access. */
export function browserStorage(kind: 'local' | 'session'): StorageLike | null {
	try {
		if (typeof window === 'undefined') return null;
		return kind === 'local' ? window.localStorage : window.sessionStorage;
	} catch {
		return null;
	}
}

/** Parsed JSON, or null when the key is missing, unreadable or not JSON. */
export function readJson(storage: StorageLike | null, key: string): unknown {
	if (!storage) return null;
	try {
		const raw = storage.getItem(key);
		return raw === null ? null : JSON.parse(raw);
	} catch {
		return null;
	}
}

/** True when the value was stored. A full or blocked store never throws. */
export function writeJson(storage: StorageLike | null, key: string, value: unknown): boolean {
	if (!storage) return false;
	try {
		storage.setItem(key, JSON.stringify(value));
		return true;
	} catch {
		return false;
	}
}

const SECRET_QUERY_KEYS = new Set([
	'token',
	'access_token',
	'refresh_token',
	'password',
	'secret',
	'authorization'
]);

function hasControlChar(value: string): boolean {
	for (let i = 0; i < value.length; i++) if (value.charCodeAt(i) < 32) return true;
	return false;
}

/**
 * A same-app path with its query (no fragment, no secrets in the query), or null. Rejects
 * absolute URLs, protocol-relative and backslash forms: only values that stay on this origin.
 */
export function safeInAppPath(value: unknown): string | null {
	if (typeof value !== 'string' || value.length > 2048) return null;
	if (
		!value.startsWith('/') ||
		value.startsWith('//') ||
		value.includes('\\') ||
		hasControlChar(value)
	)
		return null;
	let url: URL;
	try {
		url = new URL(value, 'http://local.invalid');
	} catch {
		return null;
	}
	if (url.origin !== 'http://local.invalid') return null;
	for (const key of [...url.searchParams.keys()])
		if (SECRET_QUERY_KEYS.has(key.toLowerCase())) url.searchParams.delete(key);
	return url.pathname + url.search;
}

export interface RecentEntry {
	href: string;
	title: string;
}

export type LastKey = Destination | 'settings';

export interface ShellFile {
	version: 1;
	/** 'personal' or a workspace id. */
	scope: string;
	collapsed: Partial<Record<Destination, boolean>>;
	expanded: Record<string, string[]>;
	last: Partial<Record<LastKey, Record<string, string>>>;
	recent: RecentEntry[];
}

export const DESTINATION_IDS: readonly Destination[] = ['home', 'projects', 'flow', 'governance'];
const LAST_KEYS: readonly LastKey[] = [...DESTINATION_IDS, 'settings'];

export function emptyShellFile(): ShellFile {
	return { version: 1, scope: 'personal', collapsed: {}, expanded: {}, last: {}, recent: [] };
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Keeps what is valid and drops the rest: invalid stored data never blocks the shell. */
export function parseShellFile(raw: unknown): ShellFile {
	const file = emptyShellFile();
	if (!isRecord(raw) || raw.version !== 1) return file;

	if (typeof raw.scope === 'string' && raw.scope.length > 0 && raw.scope.length <= 100)
		file.scope = raw.scope;

	if (isRecord(raw.collapsed))
		for (const id of DESTINATION_IDS) {
			const value = raw.collapsed[id];
			if (typeof value === 'boolean') file.collapsed[id] = value;
		}

	if (isRecord(raw.expanded))
		for (const [workspaceId, ids] of Object.entries(raw.expanded).slice(0, MAX_LAST_ENTRIES)) {
			if (!Array.isArray(ids)) continue;
			const unique = [
				...new Set(ids.filter((id): id is string => typeof id === 'string' && id.length > 0))
			];
			if (unique.length > 0) file.expanded[workspaceId] = unique.slice(-MAX_EXPANDED_PER_WORKSPACE);
		}

	if (isRecord(raw.last))
		for (const key of LAST_KEYS) {
			const byScope = raw.last[key];
			if (!isRecord(byScope)) continue;
			const clean: Record<string, string> = {};
			for (const [scope, path] of Object.entries(byScope).slice(0, MAX_LAST_ENTRIES)) {
				const safe = safeInAppPath(path);
				if (safe) clean[scope] = safe;
			}
			if (Object.keys(clean).length > 0) file.last[key] = clean;
		}

	if (Array.isArray(raw.recent)) {
		const seen = new Set<string>();
		for (const entry of raw.recent) {
			if (!isRecord(entry)) continue;
			const href = safeInAppPath(entry.href);
			if (!href || seen.has(href) || typeof entry.title !== 'string') continue;
			seen.add(href);
			file.recent.push({ href, title: entry.title.slice(0, MAX_TITLE) });
			if (file.recent.length >= MAX_RECENT) break;
		}
	}
	return file;
}
