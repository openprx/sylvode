import { apiClient, type ApiResult } from '../api/client';
import {
	pluginExtensionsApi,
	type PluginContributionItem,
	type PluginPreference,
	type PluginSlotV3
} from '../api/plugin-packages';
import { projectsApi } from '../api/projects';
import { bridgeLimits } from './bridge';
import type { NavigateTarget } from './host-methods';

/** How many entries each slot shows inline; the rest go into the slot's "More" menu. */
export const slotLimits: Record<PluginSlotV3, number> = {
	'form.record.detail': 6,
	'form.record.actions': 4,
	'form.list.toolbar': 4,
	'project.view': 12
};

export const defaultPosition = 50;
export const frameHeights = {
	view: { min: 320, max: 4000 },
	panel: { min: 160, max: 800 }
} as const;

type Item = PluginContributionItem;
type Api<T> = Promise<ApiResult<T>>;

/* ------------------------------------------------------------------------------------------
 * Ordering and preference merging
 * ---------------------------------------------------------------------------------------- */

export const itemKey = (item: Item) => `${item.key}:${item.contribution.id}`;

/** The personal position wins; without one the project layout position applies. */
export const effectivePosition = (item: Item): number =>
	item.preference.position ?? item.project_position;

function compareText(first: string, second: string): number {
	if (first === second) return 0;
	return first < second ? -1 : 1;
}

/** Total order for one slot: (effective position, plugin key, contribution id). */
export function compareItems(left: Item, right: Item): number {
	return (
		effectivePosition(left) - effectivePosition(right) ||
		compareText(left.key, right.key) ||
		compareText(left.contribution.id, right.contribution.id)
	);
}

export function sortItems(items: readonly Item[]): Item[] {
	return [...items].sort(compareItems);
}

/** Applies a personal preference to an item and recomputes its effective position. */
export function withPreference(item: Item, preference: PluginPreference): Item {
	const next = { ...item, preference: { ...preference } };
	return { ...next, effective_position: effectivePosition(next) };
}

/**
 * Splits already ordered entries into the inline ones and the "More" menu: collapsed entries
 * and entries beyond the slot limit go to `more`, keeping their order.
 */
export function partitionInline<T>(
	entries: readonly T[],
	limit: number,
	collapsed: (entry: T) => boolean
): { inline: T[]; more: T[] } {
	const inline: T[] = [];
	const more: T[] = [];
	for (const entry of entries) {
		if (!collapsed(entry) && inline.length < limit) inline.push(entry);
		else more.push(entry);
	}
	return { inline, more };
}

export interface SlotLayout {
	inline: Item[];
	more: Item[];
	hidden: Item[];
}

/** Orders one slot's items and applies the personal hidden/collapsed preferences. */
export function layoutSlot(items: readonly Item[], slot: PluginSlotV3): SlotLayout {
	const ordered = sortItems(items.filter((item) => item.contribution.slot === slot));
	const hidden = ordered.filter((item) => item.preference.hidden);
	const visible = ordered.filter((item) => !item.preference.hidden);
	return {
		...partitionInline(visible, slotLimits[slot], (item) => item.preference.collapsed),
		hidden
	};
}

/* ------------------------------------------------------------------------------------------
 * Equivalence
 * ---------------------------------------------------------------------------------------- */

export function sameGrants(first: readonly string[], second: readonly string[]): boolean {
	return first.length === second.length && first.every((grant) => second.includes(grant));
}

/** Everything that defines what a frame runs and may do. A change closes an open frame. */
export function sameFrameIdentity(first: Item, second: Item): boolean {
	return (
		first.package_id === second.package_id &&
		first.key === second.key &&
		first.generation === second.generation &&
		first.version === second.version &&
		first.package_sha256 === second.package_sha256 &&
		first.web_sha256 === second.web_sha256 &&
		first.contribution.id === second.contribution.id &&
		first.contribution.view === second.contribution.view &&
		sameGrants(first.grants, second.grants)
	);
}

/** Frame identity plus everything the host displays or orders by. */
export function sameItem(first: Item, second: Item): boolean {
	const a = first.contribution;
	const b = second.contribution;
	return (
		sameFrameIdentity(first, second) &&
		first.name === second.name &&
		first.profile === second.profile &&
		a.kind === b.kind &&
		a.slot === b.slot &&
		a.label === b.label &&
		a.route === b.route &&
		a.form_key === b.form_key &&
		first.project_position === second.project_position &&
		first.effective_position === second.effective_position &&
		first.preference.hidden === second.preference.hidden &&
		first.preference.collapsed === second.preference.collapsed &&
		first.preference.position === second.preference.position
	);
}

export function sameItems(first: readonly Item[], second: readonly Item[]): boolean {
	return first.length === second.length && first.every((item, i) => sameItem(item, second[i]));
}

/* ------------------------------------------------------------------------------------------
 * Routing helpers
 * ---------------------------------------------------------------------------------------- */

export const projectHref = (workspaceId: string, projectId: string) =>
	`/workspace/${workspaceId}/projects/${projectId}`;

export const recordHref = (workspaceId: string, projectId: string, recordId: string) =>
	`${projectHref(workspaceId, projectId)}/forms/records/${encodeURIComponent(recordId)}`;

export function appHref(
	workspaceId: string,
	projectId: string,
	key: string,
	route: string,
	options: { state?: string | null; from?: string | null } = {}
): string {
	const query = new URLSearchParams();
	if (options.state) query.set('state', options.state);
	if (options.from) query.set('from', options.from);
	const qs = query.toString();
	return `${projectHref(workspaceId, projectId)}/apps/${encodeURIComponent(key)}/${encodeURIComponent(route)}${qs ? `?${qs}` : ''}`;
}

const parseBase = 'http://sylvode.invalid';

/**
 * Validates the `?from=` return path. Only same-project in-app paths under
 * `/workspace/{w}/projects/{p}/` survive; everything else returns the project page.
 */
export function safeFrom(
	raw: string | null | undefined,
	workspaceId: string,
	projectId: string
): string {
	const fallback = projectHref(workspaceId, projectId);
	// eslint-disable-next-line no-control-regex
	if (!raw || raw.length > 2048 || /[\\\u0000-\u001f\u007f]/.test(raw)) return fallback;
	if (!raw.startsWith('/') || raw.startsWith('//')) return fallback;
	let url: URL;
	let decoded: string;
	try {
		url = new URL(raw, parseBase);
		decoded = decodeURIComponent(url.pathname);
	} catch {
		return fallback;
	}
	if (url.origin !== parseBase || !url.pathname.startsWith(`${fallback}/`)) return fallback;
	if (decoded.includes('\\') || decoded.split('/').some((part) => part === '.' || part === '..'))
		return fallback;
	return `${url.pathname}${url.search}${url.hash}`;
}

/** `?state=` is passed to the plugin only when it fits the protocol limit. */
export function parseState(raw: string | null): { state: string | null; tooLong: boolean } {
	if (raw === null || raw === '') return { state: null, tooLong: false };
	if (raw.length > bridgeLimits.stateLength) return { state: null, tooLong: true };
	return { state: raw, tooLong: false };
}

export function findByRoute(items: readonly Item[], key: string, route: string): Item | undefined {
	return items.find(
		(item) =>
			item.key === key &&
			item.contribution.slot === 'project.view' &&
			item.contribution.route === route
	);
}

/** Routes of a package's enabled `project.view` contributions, from the current discovery. */
export function enabledRoutes(items: readonly Item[], key: string): string[] {
	return items.flatMap((item) =>
		item.key === key && item.contribution.slot === 'project.view' && item.contribution.route
			? [item.contribution.route]
			: []
	);
}

/** Maps a validated `host.navigate` target to an in-app URL. */
export function navigationHref(
	target: NavigateTarget,
	where: { workspaceId: string; projectId: string; key: string; from?: string | null }
): string {
	const { workspaceId, projectId, key } = where;
	if (target.target === 'view')
		return appHref(workspaceId, projectId, key, target.route, {
			state: target.state,
			from: where.from
		});
	if (target.target === 'record') return recordHref(workspaceId, projectId, target.record_id);
	return projectHref(workspaceId, projectId);
}

/* ------------------------------------------------------------------------------------------
 * Discovery store
 * ---------------------------------------------------------------------------------------- */

export type DiscoveryStatus = 'loading' | 'ready' | 'off' | 'denied' | 'error';

export interface DiscoveryState {
	status: DiscoveryStatus;
	items: Item[];
}

export interface DiscoveryStore {
	subscribe(run: (value: DiscoveryState) => void): () => void;
	/** Starts polling for the caller. Call the returned function when done. */
	retain(): () => void;
	/** Marks that a frame is open, so polling speeds up. Call the returned function to clear. */
	holdBusy(): () => void;
	refresh(): Promise<void>;
	/** The latest state, for callbacks that cannot subscribe. */
	snapshot(): DiscoveryState;
}

export interface DiscoveryOptions {
	idleMs?: number;
	busyMs?: number;
	offMs?: number;
	onIdle?: () => void;
}

/** The backend answers 404 or `extensions_disabled` when the extension flag is off. */
export function isExtensionsOff(response: Pick<ApiResult<unknown>, 'code' | 'details'>): boolean {
	const reason =
		response.details !== null && typeof response.details === 'object'
			? (response.details as { reason?: unknown }).reason
			: undefined;
	return response.code === 404 || reason === 'extensions_disabled';
}

export function discoveryState(response: ApiResult<{ items: Item[] }>, previous: Item[]) {
	if (response.code === 0 && Array.isArray(response.data?.items))
		return { status: 'ready', items: sortItems(response.data.items) } satisfies DiscoveryState;
	if (isExtensionsOff(response)) return { status: 'off', items: [] } satisfies DiscoveryState;
	if (response.code === 401 || response.code === 403)
		return { status: 'denied', items: [] } satisfies DiscoveryState;
	return { status: 'error', items: previous } satisfies DiscoveryState;
}

export function createDiscoveryStore(
	fetcher: () => Api<{ items: Item[] }>,
	options: DiscoveryOptions = {}
): DiscoveryStore {
	const idleMs = options.idleMs ?? 10_000;
	const busyMs = options.busyMs ?? 2_000;
	const offMs = options.offMs ?? 60_000;
	const subscribers = new Set<(value: DiscoveryState) => void>();
	let state: DiscoveryState = { status: 'loading', items: [] };
	let holders = 0;
	let busy = 0;
	let timer: ReturnType<typeof setTimeout> | undefined;
	let running: Promise<void> | null = null;
	let again = false;
	let epoch = apiClient.getAuthSessionEpoch();

	function publish(next: DiscoveryState) {
		if (next.status === state.status && sameItems(next.items, state.items)) return;
		state = next;
		for (const run of subscribers) run(state);
	}

	function schedule() {
		clearTimeout(timer);
		if (holders === 0) return;
		const delay = state.status === 'off' ? offMs : busy > 0 ? busyMs : idleMs;
		timer = setTimeout(() => void refresh(), delay);
	}

	async function load() {
		if (!apiClient.isAuthSessionEpoch(epoch)) {
			epoch = apiClient.getAuthSessionEpoch();
			publish({ status: 'loading', items: [] });
		}
		const mine = epoch;
		let response: ApiResult<{ items: Item[] }>;
		try {
			response = await fetcher();
		} catch {
			response = { code: 500, message: 'discovery failed', data: null };
		}
		if (!apiClient.isAuthSessionEpoch(mine)) return;
		publish(discoveryState(response, state.items));
	}

	function refresh(): Promise<void> {
		if (running) {
			again = true;
			return running;
		}
		running = (async () => {
			try {
				do {
					again = false;
					await load();
				} while (again && holders > 0);
			} finally {
				running = null;
				schedule();
			}
		})();
		return running;
	}

	const onFocus = () => void refresh();

	return {
		subscribe(run) {
			subscribers.add(run);
			run(state);
			return () => subscribers.delete(run);
		},
		retain() {
			holders++;
			if (holders === 1) {
				if (typeof window !== 'undefined') window.addEventListener('focus', onFocus);
				void refresh();
			}
			let released = false;
			return () => {
				if (released) return;
				released = true;
				holders--;
				if (holders === 0) {
					if (typeof window !== 'undefined') window.removeEventListener('focus', onFocus);
					clearTimeout(timer);
					options.onIdle?.();
				}
			};
		},
		holdBusy() {
			busy++;
			schedule();
			let released = false;
			return () => {
				if (released) return;
				released = true;
				busy--;
				schedule();
			};
		},
		refresh,
		snapshot: () => state
	};
}

interface Registered {
	projectId: string;
	store: DiscoveryStore;
}
const registry = new Map<string, Registered>();

function shared(
	id: string,
	projectId: string,
	fetcher: () => Api<{ items: Item[] }>
): DiscoveryStore {
	const existing = registry.get(id);
	if (existing) return existing.store;
	const store = createDiscoveryStore(fetcher, { onIdle: () => registry.delete(id) });
	registry.set(id, { projectId, store });
	return store;
}

/** One shared `project.view` discovery per project. */
export const projectDiscovery = (projectId: string) =>
	shared(`project:${projectId}`, projectId, () =>
		pluginExtensionsApi.projectContributions(projectId)
	);

export const formDiscovery = (projectId: string, formId: string) =>
	shared(`form:${projectId}:${formId}`, projectId, () =>
		pluginExtensionsApi.formContributions(projectId, formId)
	);

export const recordDiscovery = (projectId: string, recordId: string) =>
	shared(`record:${recordId}`, projectId, () => pluginExtensionsApi.recordContributions(recordId));

/** Refreshes every live discovery of a project, e.g. after a personal preference changed. */
export function refreshProjectDiscoveries(projectId: string): Promise<void[]> {
	return Promise.all(
		[...registry.values()]
			.filter((entry) => entry.projectId === projectId)
			.map((entry) => entry.store.refresh())
	);
}

/* ------------------------------------------------------------------------------------------
 * Open frames
 * ---------------------------------------------------------------------------------------- */

export type FrameVerdict = 'ok' | 'gone' | 'changed' | 'error';

/** Whether a frame opened for `opened` may keep running given the latest discovery. */
export function frameVerdict(state: DiscoveryState, opened: Item): FrameVerdict {
	if (state.status === 'loading') return 'ok';
	if (state.status === 'off' || state.status === 'denied') return 'gone';
	if (state.status === 'error') return 'error';
	const found = state.items.find(
		(item) => item.key === opened.key && item.contribution.id === opened.contribution.id
	);
	if (!found) return 'gone';
	return sameFrameIdentity(found, opened) ? 'ok' : 'changed';
}

export const verdictReasons: Record<Exclude<FrameVerdict, 'ok'>, string> = {
	gone: 'plugins.accessChanged',
	changed: 'plugins.versionChanged',
	error: 'plugins.checkFailed'
};

export type EntryOutcome =
	| { ok: true; item: Item; html: string }
	| { ok: false; reason: 'unavailable' | 'failed' };

/** Fetches a contribution's entry and checks it answers for the contribution that was asked. */
export async function loadEntry(
	projectId: string,
	item: Item,
	context: { recordId?: string; formId?: string },
	api: Pick<typeof pluginExtensionsApi, 'entry'> = pluginExtensionsApi
): Promise<EntryOutcome> {
	let response: Awaited<ReturnType<typeof pluginExtensionsApi.entry>>;
	try {
		response = await api.entry(projectId, item.key, item.contribution.id, context);
	} catch {
		return { ok: false, reason: 'failed' };
	}
	if (response.code !== 0 || !response.data) {
		const details = response.details as { reason?: unknown } | null | undefined;
		const gone =
			[401, 403, 404].includes(response.code) ||
			details?.reason === 'contribution_unavailable' ||
			details?.reason === 'extensions_disabled';
		return { ok: false, reason: gone ? 'unavailable' : 'failed' };
	}
	const entry = response.data;
	if (
		typeof entry.html !== 'string' ||
		entry.item?.key !== item.key ||
		entry.item.contribution?.id !== item.contribution.id ||
		entry.item.contribution.slot !== item.contribution.slot ||
		!Array.isArray(entry.item.grants)
	)
		return { ok: false, reason: 'failed' };
	return { ok: true, item: entry.item, html: entry.html };
}

/** Key that changes whenever a frame must be recreated (version, digest or grants changed). */
export const frameKey = (item: Item) =>
	JSON.stringify([
		item.package_id,
		item.generation,
		item.web_sha256,
		item.contribution.id,
		[...item.grants].sort()
	]);

/* ------------------------------------------------------------------------------------------
 * Personal preferences
 * ---------------------------------------------------------------------------------------- */

export type PreferenceApi = Pick<
	typeof pluginExtensionsApi,
	'setPreference' | 'resetPreference' | 'resetAllPreferences'
>;

/** i18n key describing why a preference call failed. */
export function preferenceError(
	response: Pick<ApiResult<unknown>, 'details'>
): 'plugins.prefs.limitReached' | 'plugins.prefs.unavailable' | 'plugins.prefs.saveFailed' {
	const reason =
		response.details !== null && typeof response.details === 'object'
			? (response.details as { reason?: unknown }).reason
			: undefined;
	if (reason === 'preference_limit') return 'plugins.prefs.limitReached';
	if (reason === 'contribution_unavailable') return 'plugins.prefs.unavailable';
	return 'plugins.prefs.saveFailed';
}

export async function savePreference(
	projectId: string,
	item: Item,
	preference: PluginPreference,
	api: PreferenceApi = pluginExtensionsApi
) {
	const response = await api.setPreference(projectId, item.key, item.contribution.id, preference);
	if (response.code === 0) void refreshProjectDiscoveries(projectId);
	return response;
}

export async function clearPreference(
	projectId: string,
	item: Item,
	api: PreferenceApi = pluginExtensionsApi
) {
	const response = await api.resetPreference(projectId, item.key, item.contribution.id);
	if (response.code === 0) void refreshProjectDiscoveries(projectId);
	return response;
}

export async function clearAllPreferences(
	projectId: string,
	api: PreferenceApi = pluginExtensionsApi
) {
	const response = await api.resetAllPreferences(projectId);
	if (response.code === 0) void refreshProjectDiscoveries(projectId);
	return response;
}

/** The position a "move up" would set, or null when the item is already first. */
export function moveUpPosition(ordered: readonly Item[], item: Item): number | null {
	const index = ordered.findIndex((entry) => itemKey(entry) === itemKey(item));
	if (index <= 0) return null;
	const target = effectivePosition(ordered[index - 1]) - 1;
	return target < 0 ? null : target;
}

/** The position a "move down" would set, or null when the item is already last. */
export function moveDownPosition(ordered: readonly Item[], item: Item): number | null {
	const index = ordered.findIndex((entry) => itemKey(entry) === itemKey(item));
	if (index < 0 || index >= ordered.length - 1) return null;
	const target = effectivePosition(ordered[index + 1]) + 1;
	return target > 99 ? null : target;
}

/* ------------------------------------------------------------------------------------------
 * Environment
 * ---------------------------------------------------------------------------------------- */

export function currentTheme(): 'light' | 'dark' {
	return typeof document !== 'undefined' && document.documentElement.classList.contains('dark')
		? 'dark'
		: 'light';
}

const projectNames = new Map<string, string>();

/** Project display name for the plugin context. Empty when it cannot be loaded. */
export async function projectName(projectId: string): Promise<string> {
	const cached = projectNames.get(projectId);
	if (cached !== undefined) return cached;
	try {
		const response = await projectsApi.get(projectId);
		const name = response.code === 0 && response.data ? response.data.name : '';
		if (name) projectNames.set(projectId, name);
		return name;
	} catch {
		return '';
	}
}
