import { writable, get } from 'svelte/store';
export const WORKBENCH_CONTEXT = 'sv-task-workbench';
export const MAX_TASK_TABS = 8;
export interface TaskTab {
	key: string;
	url: string;
	title: string;
	dirty: boolean;
	changed: boolean;
	scroll: number;
}
export interface TaskState {
	userId: string | null;
	tabs: TaskTab[];
	active: string | null;
	limit: boolean;
	restoredTrimmed?: boolean;
}
export interface TaskViewHooks {
	capture(): unknown;
	dirty(): boolean;
	refresh(): Promise<void>;
	dispose?(): void;
}
export interface TaskResource {
	key: string;
	title: string;
	url: string;
}
export function taskResource(href: string): TaskResource | null {
	const url = new URL(href, 'http://local.invalid');
	for (const key of [...url.searchParams.keys()])
		if (
			['token', 'access_token', 'refresh_token', 'password', 'secret', 'authorization'].includes(
				key.toLowerCase()
			)
		)
			url.searchParams.delete(key);
	const path = url.pathname;
	const shared = path.match(/^\/shared-blocks\/([^/]+)$/);
	if (shared)
		return {
			key: `shared-block:${shared[1]}`,
			title: 'workbench.sharedBlock',
			url: path + url.search + url.hash
		};
	const flow = path.match(/^\/(?:workspace\/[^/]+\/)?flow\/([^/]+)$/);
	if (flow && !['package', 'conversions'].includes(flow[1]))
		return { key: `flow:${flow[1]}`, title: 'Flow', url: path + url.search + url.hash };
	if (path === '/flow') return { key: 'flow-home:personal', title: 'Flow', url: path + url.search };
	const forms = path.match(
		/^\/workspace\/([^/]+)\/projects\/([^/]+)\/forms(?:\/records\/([^/]+))?$/
	);
	if (forms) {
		const formId = url.searchParams.get('form') ?? url.searchParams.get('form_id');
		return {
			key: formId
				? `forms:${forms[2]}:${formId}`
				: forms[3]
					? `forms-entry:${forms[2]}`
					: `forms-entry:${forms[2]}`,
			title: 'workbench.forms',
			url: path + url.search + url.hash
		};
	}
	const project = path.match(/^\/workspace\/([^/]+)\/projects(?:\/([^/]+))?$/);
	if (project)
		return {
			key: project[2] ? `project:${project[2]}` : `projects:${project[1]}`,
			title: project[2] ? 'workbench.project' : 'workbench.projects',
			url: path + url.search
		};
	return null;
}
export class TaskWorkbench {
	private store = writable<TaskState>({ userId: null, tabs: [], active: null, limit: false });
	readonly subscribe = this.store.subscribe;
	private snapshots = new Map<string, unknown>();
	private hooks = new Map<string, TaskViewHooks>();
	private releases = new Map<string, () => void>();
	private auxiliary = new Map<string, Map<string, { dirty(): boolean; dispose(): void }>>();
	private intrinsicDirty = new Map<string, boolean>();
	private refreshing = false;
	private freshRestore = false;
	onViewReady: ((key: string) => void) | null = null;
	get current(): TaskState {
		return get(this.store);
	}
	activateUser(userId: string | null): void {
		if (this.current.userId === userId) return;
		for (const slots of this.auxiliary.values())
			for (const value of slots.values()) value.dispose();
		this.auxiliary.clear();
		this.intrinsicDirty.clear();
		for (const release of this.releases.values()) release();
		for (const hook of this.hooks.values()) hook.dispose?.();
		this.snapshots.clear();
		this.hooks.clear();
		this.releases.clear();
		let tabs: TaskTab[] = [];
		let active: string | null = null;
		if (userId && typeof localStorage !== 'undefined')
			try {
				const saved = JSON.parse(localStorage.getItem(this.storageKey(userId)) ?? 'null');
				if (saved?.userId === userId && Array.isArray(saved.tabs)) {
					tabs = saved.tabs.slice(0, MAX_TASK_TABS).flatMap((raw: unknown) => {
						if (!raw || typeof raw !== 'object' || !('url' in raw) || typeof raw.url !== 'string')
							return [];
						const resource = taskResource(raw.url);
						if (!resource) return [];
						const entry = raw as Partial<TaskTab>;
						return [
							{
								...resource,
								title: typeof entry.title === 'string' ? entry.title : resource.title,
								dirty: false,
								changed: false,
								scroll: typeof entry.scroll === 'number' ? entry.scroll : 0
							}
						];
					});
					active =
						typeof saved.active === 'string' && tabs.some((tab) => tab.key === saved.active)
							? saved.active
							: null;
				}
			} catch {
				/* Invalid position metadata never prevents sign-in. */
			}
		this.freshRestore = true;
		this.store.set({ userId, tabs, active, limit: false });
	}
	private storageKey(userId: string): string {
		return `sv-task-tabs:v1:${userId}`;
	}
	private persist(): void {
		const state = this.current;
		if (!state.userId || typeof localStorage === 'undefined') return;
		try {
			localStorage.setItem(
				this.storageKey(state.userId),
				JSON.stringify({
					userId: state.userId,
					active: state.active,
					tabs: state.tabs.map(({ key, url, title, scroll }) => ({ key, url, title, scroll }))
				})
			);
		} catch {
			/* Quota affects restoration, not current edits. */
		}
	}
	restoreRoute(href: string): boolean {
		if (!this.freshRestore) return this.visit(href);
		this.freshRestore = false;
		const item = taskResource(href);
		if (!item) return this.visit(href);
		const state = this.current;
		const existing = state.tabs.find((tab) => tab.key === item.key);
		const others = state.tabs.filter((tab) => tab.key !== item.key);
		const trimmed = others.length >= MAX_TASK_TABS;
		this.store.set({
			...state,
			active: item.key,
			limit: false,
			restoredTrimmed: trimmed,
			tabs: existing
				? state.tabs.map((tab) => (tab.key === item.key ? { ...tab, url: item.url } : tab))
				: [
						...(trimmed ? others.slice(1) : others),
						{ ...item, dirty: false, changed: false, scroll: 0 }
					]
		});
		this.persist();
		return true;
	}

	canOpen(href: string): boolean {
		const item = taskResource(href);
		return (
			!item ||
			this.current.tabs.some((tab) => tab.key === item.key) ||
			this.current.tabs.length < MAX_TASK_TABS
		);
	}
	visit(href: string): boolean {
		if (!this.current.userId) return true;
		const item = taskResource(href);
		if (!item) {
			this.store.update((state) => ({ ...state, active: null }));
			return true;
		}
		if (!this.canOpen(href)) {
			this.store.update((state) => ({ ...state, limit: true }));
			return false;
		}
		this.store.update((state) => ({
			...state,
			active: item.key,
			limit: false,
			tabs: state.tabs.some((tab) => tab.key === item.key)
				? state.tabs.map((tab) => (tab.key === item.key ? { ...tab, url: item.url } : tab))
				: [...state.tabs, { ...item, dirty: false, changed: false, scroll: 0 }]
		}));
		this.persist();
		return true;
	}
	register(key: string, hooks: TaskViewHooks): () => void {
		this.hooks.set(key, hooks);
		if (typeof requestAnimationFrame !== 'undefined')
			requestAnimationFrame(() => this.onViewReady?.(key));
		return () => {
			if (this.hooks.get(key) === hooks) {
				this.capture(key);
				this.hooks.delete(key);
			}
		};
	}
	capture(key = this.current.active): void {
		if (!key) return;
		const hook = this.hooks.get(key);
		if (hook) this.snapshots.set(key, hook.capture());
		if (hook) this.markDirty(key, hook.dirty());
	}
	snapshot<T>(key: string): T | undefined {
		return this.snapshots.get(key) as T | undefined;
	}
	saveSnapshot(key: string, value: unknown): void {
		if (this.current.tabs.some((tab) => key === tab.key || key.startsWith(tab.key + '::')))
			this.snapshots.set(key, value);
	}
	setTitle(key: string, title: string): void {
		this.store.update((state) => ({
			...state,
			tabs: state.tabs.map((tab) => (tab.key === key ? { ...tab, title } : tab))
		}));
		this.persist();
	}
	/** Reorder only tab metadata. Resource/session ownership and every tab object stay intact. */
	moveTab(key: string, destination: number): boolean {
		const state = this.current;
		const source = state.tabs.findIndex((tab) => tab.key === key);
		if (
			source < 0 ||
			!Number.isInteger(destination) ||
			destination < 0 ||
			destination >= state.tabs.length ||
			source === destination
		)
			return false;
		const tabs = [...state.tabs];
		const [tab] = tabs.splice(source, 1);
		tabs.splice(destination, 0, tab);
		this.store.set({ ...state, tabs });
		this.persist();
		return true;
	}
	moveTabRelative(key: string, target: string, side: 'before' | 'after'): boolean {
		if (key === target || !this.current.tabs.some((tab) => tab.key === key)) return false;
		const remaining = this.current.tabs.filter((tab) => tab.key !== key);
		const index = remaining.findIndex((tab) => tab.key === target);
		return index < 0 ? false : this.moveTab(key, index + (side === 'after' ? 1 : 0));
	}

	markDirty(key: string, dirty: boolean): void {
		this.intrinsicDirty.set(key, dirty);
		dirty = dirty || this.auxiliaryDirty(key);
		this.store.update((state) => ({
			...state,
			tabs: state.tabs.map((tab) => (tab.key === key ? { ...tab, dirty } : tab))
		}));
	}
	markChanged(key: string, changed = true): void {
		this.store.update((state) => ({
			...state,
			tabs: state.tabs.map((tab) => (tab.key === key ? { ...tab, changed } : tab))
		}));
	}
	showLimit(): void {
		this.store.update((state) => ({ ...state, limit: true }));
	}
	setScroll(key: string, scroll: number): void {
		this.store.update((state) => ({
			...state,
			tabs: state.tabs.map((tab) => (tab.key === key ? { ...tab, scroll } : tab))
		}));
		this.persist();
	}
	async refreshActive(): Promise<void> {
		if (this.refreshing) return;
		const key = this.current.active;
		if (!key) return;
		const hook = this.hooks.get(key);
		if (!hook) return;
		this.refreshing = true;
		try {
			await hook.refresh();
			if (this.current.active === key)
				this.store.update((state) => ({
					...state,
					tabs: state.tabs.map((tab) =>
						tab.key === key
							? { ...tab, changed: hook.dirty() ? tab.changed : false, dirty: hook.dirty() }
							: tab
					)
				}));
		} finally {
			this.refreshing = false;
		}
	}
	hasDirty(): boolean {
		return (
			this.current.tabs.some((tab) => tab.dirty) ||
			[...this.hooks.values()].some((hook) => hook.dirty()) ||
			[...this.auxiliary.keys()].some((key) => this.auxiliaryDirty(key))
		);
	}
	close(
		key: string,
		discardDraft = false
	):
		| { kind: 'closed'; url: string; wasActive: boolean }
		| { kind: 'confirm' }
		| { kind: 'missing' } {
		const state = this.current;
		const index = state.tabs.findIndex((tab) => tab.key === key);
		if (index < 0) return { kind: 'missing' };
		if (
			!discardDraft &&
			(state.tabs[index]?.dirty || this.hooks.get(key)?.dirty() || this.auxiliaryDirty(key))
		)
			return { kind: 'confirm' };
		for (const value of this.auxiliary.get(key)?.values() ?? []) value.dispose();
		this.auxiliary.delete(key);
		this.intrinsicDirty.delete(key);
		this.releases.get(key)?.();
		this.releases.delete(key);
		this.hooks.get(key)?.dispose?.();
		this.hooks.delete(key);
		for (const cached of this.snapshots.keys())
			if (cached === key || cached.startsWith(key + '::')) this.snapshots.delete(cached);
		const tabs = state.tabs.filter((tab) => tab.key !== key);
		const next =
			state.active === key ? (tabs[Math.min(index, tabs.length - 1)]?.key ?? null) : state.active;
		const active = state.active === key ? null : state.active;
		this.store.set({ ...state, tabs, active, limit: false });
		this.persist();
		return {
			kind: 'closed',
			url: tabs.find((tab) => tab.key === next)?.url ?? '/workspace',
			wasActive: state.active === key
		};
	}
	/** Non-document task resources, such as source publication drafts. No extra Page/session pin. */
	retainAuxiliary(
		key: string,
		slot: string,
		value: { dirty(): boolean; dispose(): void }
	): boolean {
		if (!this.current.userId || !this.current.tabs.some((tab) => tab.key === key)) return false;
		let slots = this.auxiliary.get(key);
		if (slots?.has(slot)) return false;
		if (!slots) {
			slots = new Map();
			this.auxiliary.set(key, slots);
		}
		if (!this.intrinsicDirty.has(key))
			this.intrinsicDirty.set(
				key,
				this.current.tabs.find((tab) => tab.key === key)?.dirty ?? false
			);
		slots.set(slot, value);
		this.refreshAuxiliaryDirty(key);
		return true;
	}
	private auxiliaryDirty(key: string): boolean {
		return [...(this.auxiliary.get(key)?.values() ?? [])].some((value) => value.dirty());
	}
	refreshAuxiliaryDirty(key: string): void {
		const dirty =
			(this.hooks.get(key)?.dirty() ?? this.intrinsicDirty.get(key) ?? false) ||
			this.auxiliaryDirty(key);
		if (this.current.tabs.find((tab) => tab.key === key)?.dirty === dirty) return;
		this.store.update((state) => ({
			...state,
			tabs: state.tabs.map((tab) => (tab.key === key ? { ...tab, dirty } : tab))
		}));
	}

	retain(key: string, release: () => void): boolean {
		if (this.releases.has(key)) return false;
		this.releases.set(key, release);
		return true;
	}
	dispose(): void {
		this.activateUser(null);
	}
}
