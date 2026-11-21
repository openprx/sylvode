import { taskResource } from '../workbench/tabs';
import { PERSONAL_SCOPE, type ScopeKey } from './destinations';
import type { Destination, RouteContext } from './route-map';
import {
	MAX_EXPANDED_PER_WORKSPACE,
	MAX_RECENT,
	SETTINGS_RETURN_KEY,
	browserStorage,
	emptyShellFile,
	parseShellFile,
	readJson,
	safeInAppPath,
	shellStateKey,
	writeJson,
	type LastKey,
	type RecentEntry,
	type ShellFile,
	type StorageLike
} from './storage';

export type NarrowView = 'list' | 'content';

export interface ShellStateOptions {
	/** Signed-in user; with null nothing is read or written. */
	userId: string | null;
	/** Default: `localStorage`. */
	local?: StorageLike | null;
	/** Default: `sessionStorage`. */
	session?: StorageLike | null;
}

/**
 * The scope a URL puts the shell in. A workspace URL wins; personal Flow (`/flow/...`) is the
 * personal scope; everything else keeps the current scope.
 */
export function scopeOfRoute(url: URL, ctx: RouteContext, current: ScopeKey): ScopeKey {
	if (ctx.workspaceId) return ctx.workspaceId;
	if (url.pathname === '/flow' || url.pathname.startsWith('/flow/')) return PERSONAL_SCOPE;
	return current;
}

/**
 * Shell state that survives a reload (scope, column collapse, expanded projects, last locations,
 * recent items) or a browser tab (the settings return URL), plus the narrow-window view. One
 * instance per signed-in user; create a new one when the user changes.
 */
export class ShellState {
	readonly userId: string | null;
	private readonly local: StorageLike | null;
	private readonly session: StorageLike | null;

	scope = $state<ScopeKey>(PERSONAL_SCOPE);
	private collapsed = $state.raw<ShellFile['collapsed']>({});
	private expanded = $state.raw<ShellFile['expanded']>({});
	private last = $state.raw<ShellFile['last']>({});
	/** Most recent first. */
	recent = $state.raw<RecentEntry[]>([]);
	/** At 800 or less only one column shows: the list or the content. Memory only. */
	narrowView = $state<NarrowView>('list');
	/** Where "Close settings" goes; null when unknown. */
	settingsReturn = $state.raw<string | null>(null);

	private lastNonSettings: string | null = null;
	private inSettings = false;

	constructor(options: ShellStateOptions) {
		this.userId = options.userId;
		this.local = options.userId ? (options.local ?? browserStorage('local')) : null;
		this.session = options.session ?? browserStorage('session');
		if (!options.userId) return;
		const file = parseShellFile(readJson(this.local, shellStateKey(options.userId)));
		this.scope = file.scope;
		this.collapsed = file.collapsed;
		this.expanded = file.expanded;
		this.last = file.last;
		this.recent = file.recent;
		this.settingsReturn = safeInAppPath(readJson(this.session, SETTINGS_RETURN_KEY));
	}

	// --- scope ---

	setScope(scope: ScopeKey): void {
		if (!scope || scope === this.scope) return;
		this.scope = scope;
		this.persist();
	}

	// --- column collapse ---

	/**
	 * Stored choice, else the default: expanded, except Flow on a Flow canvas route, which starts
	 * collapsed while the Flow navigator still lives inside the Flow shell.
	 */
	isCollapsed(destination: Destination, options: { flowCanvas?: boolean } = {}): boolean {
		return this.collapsed[destination] ?? (destination === 'flow' && options.flowCanvas === true);
	}
	setCollapsed(destination: Destination, collapsed: boolean): void {
		this.collapsed = { ...this.collapsed, [destination]: collapsed };
		this.persist();
	}
	toggleCollapsed(destination: Destination, options: { flowCanvas?: boolean } = {}): boolean {
		const next = !this.isCollapsed(destination, options);
		this.setCollapsed(destination, next);
		return next;
	}

	// --- expanded projects ---

	isProjectExpanded(workspaceId: string, projectId: string): boolean {
		return this.expanded[workspaceId]?.includes(projectId) ?? false;
	}
	expandedProjects(workspaceId: string): readonly string[] {
		return this.expanded[workspaceId] ?? [];
	}
	setProjectExpanded(workspaceId: string, projectId: string, open: boolean): void {
		const current = this.expanded[workspaceId] ?? [];
		if (current.includes(projectId) === open) return;
		const next = open
			? [...current, projectId].slice(-MAX_EXPANDED_PER_WORKSPACE)
			: current.filter((id) => id !== projectId);
		const { [workspaceId]: _removed, ...rest } = this.expanded;
		void _removed;
		this.expanded = next.length > 0 ? { ...rest, [workspaceId]: next } : rest;
		this.persist();
	}

	// --- last location and recent ---

	/** The remembered in-app path for a destination (or 'settings') in a scope. */
	lastFor(key: LastKey, scope: ScopeKey): string | undefined {
		return this.last[key]?.[scope];
	}

	/** Recent task resources: only URLs the task workbench recognises are recorded. */
	recordVisit(href: string, title: string): void {
		const resource = taskResource(href);
		const path = resource ? safeInAppPath(resource.url) : null;
		if (!resource || !path) return;
		const entry = { href: path, title: title || resource.title };
		this.recent = [entry, ...this.recent.filter((r) => r.href !== path)].slice(0, MAX_RECENT);
		this.persist();
	}
	clearRecent(): void {
		if (this.recent.length === 0) return;
		this.recent = [];
		this.persist();
	}

	/**
	 * Call once per client navigation. Adopts the scope the URL implies, remembers the location
	 * for its destination, and keeps the URL to return to when settings mode closes.
	 */
	observe(url: URL, ctx: RouteContext): void {
		const scope = scopeOfRoute(url, ctx, this.scope);
		const path = safeInAppPath(url.pathname + url.search);
		let changed = false;
		if (scope !== this.scope) {
			this.scope = scope;
			changed = true;
		}
		const key: LastKey | null = ctx.settings ? 'settings' : ctx.destination;
		if (key && path) {
			const byScope = this.last[key] ?? {};
			if (byScope[scope] !== path) {
				this.last = { ...this.last, [key]: { ...byScope, [scope]: path } };
				changed = true;
			}
		}
		if (!ctx.settings) {
			if (path) this.lastNonSettings = path;
			this.inSettings = false;
		} else if (!this.inSettings) {
			this.inSettings = true;
			if (this.lastNonSettings && this.lastNonSettings !== this.settingsReturn) {
				this.settingsReturn = this.lastNonSettings;
				writeJson(this.session, SETTINGS_RETURN_KEY, this.settingsReturn);
			}
		}
		if (changed) this.persist();
	}

	// --- narrow window ---

	showList(): void {
		this.narrowView = 'list';
	}
	showContent(): void {
		this.narrowView = 'content';
	}

	// --- persistence ---

	toFile(): ShellFile {
		return {
			...emptyShellFile(),
			scope: this.scope,
			collapsed: this.collapsed,
			expanded: this.expanded,
			last: this.last,
			recent: this.recent
		};
	}

	private persist(): void {
		if (!this.userId) return;
		writeJson(this.local, shellStateKey(this.userId), this.toFile());
	}
}
