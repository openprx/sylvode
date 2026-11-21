import { taskResource } from '../workbench/tabs';
import { widthClass as classify } from './width';
import type { SurfaceSpec } from './use-surface.svelte';
import type {
	SurfaceActions,
	SurfaceAmbient,
	SurfaceDensity,
	SurfaceHeaderVariant,
	SurfaceHost,
	SurfaceHostProvider,
	SurfaceOpenMode,
	SurfaceReport,
	SurfaceSlot,
	SurfaceTarget,
	SurfaceTheme,
	SurfaceTip,
	SurfaceTitle,
	SurfaceWidthClass
} from './context';

/** Id of the tip that `report()` sets; it is cleared when the route changes. */
export const SURFACE_REPORT_TIP_ID = 'surface-report';
const APP_NAME = 'Sylvode';

/** The task tab strip, as the host needs it. */
export interface TaskBridge {
	activeKey(): string | null;
	setTitle(key: string, title: string): void;
	markDirty(key: string, dirty: boolean): void;
}

export interface StandaloneHostDeps {
	navigate(
		url: string,
		options?: { replace?: boolean; keepFocus?: boolean; noScroll?: boolean }
	): void;
	currentUrl(): URL;
	/** The current destination's root: where close() goes when nothing else is known. */
	rootHref(): string;
	translate(key: string, values?: Record<string, string>): string;
	locale(): string;
	tasks?: TaskBridge;
	/** Task key of a URL; defaults to `taskResource(href)?.key`. */
	taskKey?(href: string): string | null;
}

/** Same-app path only: starts with one slash, no protocol-relative or backslash forms. */
function inAppPath(href: string): string | null {
	if (!href.startsWith('/') || href.startsWith('//') || href.includes('\\')) return null;
	return href;
}

/** What every owner-scoped host forwards to; implemented by the standalone frame and by scopes. */
interface HostCore {
	readonly frame: SurfaceHost['frame'];
	readonly slot: SurfaceSlot;
	readonly width: number;
	readonly widthClass: SurfaceWidthClass;
	readonly theme: SurfaceTheme;
	readonly density: SurfaceDensity;
	readonly globalShortcuts: boolean;
	isCurrent(owner: symbol): boolean;
	applyTitle(title: SurfaceTitle): void;
	applyActions(actions: SurfaceActions): void;
	applyHeader(variant: SurfaceHeaderVariant): void;
	applyDensity(density: SurfaceDensity): void;
	applyTip(tip: SurfaceTip | null): void;
	applyDirty(dirty: boolean): void;
	applySyncState(query: Record<string, string | null>, mode: 'replace' | 'push'): void;
	applyOpen(target: SurfaceTarget, how: SurfaceOpenMode): void;
	applyClose(fallbackHref?: string): void;
	applyReport(result: SurfaceReport): void;
}

/** A host bound to one owner: calls from a superseded or detached owner are ignored. */
class OwnedHost implements SurfaceHost {
	constructor(
		private readonly core: HostCore,
		private readonly owner: symbol
	) {}
	get frame() {
		return this.core.frame;
	}
	get slot() {
		return this.core.slot;
	}
	get width() {
		return this.core.width;
	}
	get widthClass() {
		return this.core.widthClass;
	}
	get theme() {
		return this.core.theme;
	}
	get density() {
		return this.core.density;
	}
	get globalShortcuts() {
		return this.core.globalShortcuts;
	}
	setTitle(title: SurfaceTitle) {
		if (this.core.isCurrent(this.owner)) this.core.applyTitle(title);
	}
	setActions(actions: SurfaceActions) {
		if (this.core.isCurrent(this.owner)) this.core.applyActions(actions);
	}
	setHeader(variant: SurfaceHeaderVariant) {
		if (this.core.isCurrent(this.owner)) this.core.applyHeader(variant);
	}
	setDensity(density: SurfaceDensity) {
		if (this.core.isCurrent(this.owner)) this.core.applyDensity(density);
	}
	setTip(tip: SurfaceTip | null) {
		if (this.core.isCurrent(this.owner)) this.core.applyTip(tip);
	}
	setDirty(dirty: boolean) {
		if (this.core.isCurrent(this.owner)) this.core.applyDirty(dirty);
	}
	syncState(query: Record<string, string | null>, mode: 'replace' | 'push' = 'replace') {
		if (this.core.isCurrent(this.owner)) this.core.applySyncState(query, mode);
	}
	open(target: SurfaceTarget, how: SurfaceOpenMode) {
		if (this.core.isCurrent(this.owner)) this.core.applyOpen(target, how);
	}
	close(fallbackHref?: string) {
		if (this.core.isCurrent(this.owner)) this.core.applyClose(fallbackHref);
	}
	report(result: SurfaceReport) {
		if (this.core.isCurrent(this.owner)) this.core.applyReport(result);
	}
}

export function formatReport(
	translate: StandaloneHostDeps['translate'],
	locale: string,
	result: SurfaceReport
): string {
	let list: (items: string[]) => string;
	try {
		const format = new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' });
		list = (items) => format.format(items);
	} catch {
		list = (items) => items.join(', ');
	}
	if (result.written.length === 0)
		return translate('surface.reportNone', { object: result.object });
	const values = { object: result.object, written: list(result.written) };
	if (result.unchanged.length === 0) return translate('surface.report', values);
	return translate('surface.reportUnchanged', { ...values, unchanged: list(result.unchanged) });
}

/**
 * The standalone shell's frame. The shell reads `title`, `actions`, `header`, `tip` and `loading`
 * to draw the content header and the tip slot; surfaces reach it only through `useSurface`.
 */
export class StandaloneSurfaceHost implements SurfaceHostProvider, HostCore {
	readonly frame = 'standalone' as const;
	readonly slot = 'main' as const;
	readonly globalShortcuts = true;

	width = $state(1024);
	theme = $state<SurfaceTheme>('light');
	density = $state<SurfaceDensity>('comfortable');
	title = $state.raw<SurfaceTitle | null>(null);
	actions = $state.raw<SurfaceActions>({ secondary: [] });
	header = $state<SurfaceHeaderVariant>('standard');
	tip = $state.raw<SurfaceTip | null>(null);
	dirty = $state(false);
	private owner = $state.raw<symbol | null>(null);
	private dirtyKey: string | null = null;
	private current: string | null = null;
	private previous: string | null = null;

	constructor(private readonly deps: StandaloneHostDeps) {}

	get widthClass(): SurfaceWidthClass {
		return classify(this.width);
	}
	/** A surface route has attached. */
	get hasSurface(): boolean {
		return this.owner !== null;
	}
	/** The surface is attached but has not sent a loaded title yet: the header shows its skeleton. */
	get loading(): boolean {
		return this.owner !== null && (this.title === null || this.title.loading === true);
	}
	/** `"{title} · Sylvode"` once the surface has a loaded title, else null (the layout falls back to its table). */
	get documentTitle(): string | null {
		const title = this.title;
		if (!title || title.loading || !title.title) return null;
		return `${title.title} · ${APP_NAME}`;
	}

	/** Called by the shell's ResizeObserver on `[data-shell="surface"]`. */
	setWidth(px: number): void {
		this.width = px;
	}
	setTheme(theme: SurfaceTheme): void {
		this.theme = theme;
	}

	/** Called by the shell's `afterNavigate` with the new URL; keeps the previous in-app URL for close(). */
	noteNavigation(url: URL): void {
		const next = url.pathname + url.search;
		if (this.current !== null && this.current !== next) {
			const pathChanged = this.current.split('?')[0] !== url.pathname;
			this.previous = this.current;
			if (pathChanged && this.tip?.id === SURFACE_REPORT_TIP_ID) this.tip = null;
		}
		this.current = next;
	}

	/** Re-applies the title and dirty flag to the task tab; call after the tab registry has visited the route. */
	syncTasks(): void {
		const key = this.activeTaskKey();
		if (!key || this.owner === null) return;
		if (this.title && !this.title.loading && this.title.title)
			this.deps.tasks?.setTitle(key, this.title.title);
		if (this.dirty || this.dirtyKey === key) this.deps.tasks?.markDirty(key, this.dirty);
		this.dirtyKey = this.dirty ? key : null;
	}

	attach(owner: symbol): SurfaceHost {
		this.owner = owner;
		this.title = null;
		this.actions = { secondary: [] };
		this.header = 'standard';
		this.density = 'comfortable';
		this.tip = null;
		this.setDirtyState(false);
		return new OwnedHost(this, owner);
	}

	detach(owner: symbol): void {
		if (this.owner !== owner) return;
		this.owner = null;
		this.title = null;
		this.actions = { secondary: [] };
		this.tip = null;
		this.setDirtyState(false);
	}

	ambient(): SurfaceAmbient {
		const self = () => this;
		return {
			frame: 'standalone',
			get theme() {
				return self().theme;
			},
			get density() {
				return self().density;
			},
			globalShortcuts: true,
			open: (target, how) => self().applyOpen(target, how),
			close: (fallbackHref) => self().applyClose(fallbackHref)
		};
	}

	isCurrent(owner: symbol): boolean {
		return this.owner === owner;
	}
	applyTitle(title: SurfaceTitle): void {
		this.title = title;
		const key = this.activeTaskKey();
		if (key && !title.loading && title.title) this.deps.tasks?.setTitle(key, title.title);
	}
	applyActions(actions: SurfaceActions): void {
		this.actions = actions;
	}
	applyHeader(variant: SurfaceHeaderVariant): void {
		this.header = variant;
	}
	applyDensity(density: SurfaceDensity): void {
		this.density = density;
	}
	applyTip(tip: SurfaceTip | null): void {
		this.tip = tip;
	}
	applyDirty(dirty: boolean): void {
		this.setDirtyState(dirty);
	}
	applySyncState(query: Record<string, string | null>, mode: 'replace' | 'push'): void {
		// A throwaway URL used to build the next address, not reactive state.
		// eslint-disable-next-line svelte/prefer-svelte-reactivity
		const url = new URL(this.deps.currentUrl());
		for (const [key, value] of Object.entries(query)) {
			if (value === null) url.searchParams.delete(key);
			else url.searchParams.set(key, value);
		}
		const next = url.pathname + url.search + url.hash;
		const now = this.deps.currentUrl();
		if (next === now.pathname + now.search + now.hash) return;
		this.deps.navigate(next, { replace: mode !== 'push', keepFocus: true, noScroll: true });
	}
	applyOpen(target: SurfaceTarget, how: SurfaceOpenMode): void {
		const href = inAppPath(how === 'panel' ? (target.panelHref ?? target.href) : target.href);
		if (href) this.deps.navigate(href);
	}
	applyClose(fallbackHref?: string): void {
		const href =
			(this.previous && inAppPath(this.previous)) ||
			(fallbackHref && inAppPath(fallbackHref)) ||
			this.deps.rootHref();
		this.deps.navigate(href);
	}
	applyReport(result: SurfaceReport): void {
		this.tip = {
			id: SURFACE_REPORT_TIP_ID,
			tone: 'success',
			message: formatReport(this.deps.translate, this.deps.locale(), result)
		};
	}

	private activeTaskKey(): string | null {
		const tasks = this.deps.tasks;
		if (!tasks) return null;
		const href = this.deps.currentUrl().href;
		const key = this.deps.taskKey ? this.deps.taskKey(href) : (taskResource(href)?.key ?? null);
		return key && tasks.activeKey() === key ? key : null;
	}

	private setDirtyState(dirty: boolean): void {
		this.dirty = dirty;
		const tasks = this.deps.tasks;
		if (!tasks) return;
		const key = this.activeTaskKey();
		if (dirty && key) {
			tasks.markDirty(key, true);
			this.dirtyKey = key;
		} else if (!dirty && this.dirtyKey) {
			tasks.markDirty(this.dirtyKey, false);
			this.dirtyKey = null;
		}
	}
}

export interface SurfaceScopeOptions {
	readonly slot: Exclude<SurfaceSlot, 'main'>;
	/** The enclosing frame, for theme, frame kind and the default open and close. */
	parent: SurfaceHostProvider | null;
	/** Receives the whole spec the scoped surface has declared so far. */
	onspec(spec: SurfaceSpec): void;
	readonly onopen?: (target: SurfaceTarget, how: SurfaceOpenMode) => void;
	readonly onclose?: () => void;
	translate: StandaloneHostDeps['translate'];
	locale(): string;
}

/** The child frame of a `SurfaceScope`: reports to its owner instead of the content header. */
export class SurfaceScopeProvider implements SurfaceHostProvider, HostCore {
	width = $state(0);
	private owner: symbol | null = null;
	private spec: SurfaceSpec = { title: '' };
	private readonly parent: SurfaceAmbient | null;

	constructor(private readonly options: SurfaceScopeOptions) {
		this.parent = options.parent?.ambient() ?? null;
	}

	get slot(): SurfaceSlot {
		return this.options.slot;
	}
	get frame() {
		return this.parent?.frame ?? 'standalone';
	}
	get theme(): SurfaceTheme {
		return this.parent?.theme ?? 'light';
	}
	get density(): SurfaceDensity {
		return this.spec.density ?? this.parent?.density ?? 'comfortable';
	}
	get globalShortcuts(): boolean {
		return this.parent?.globalShortcuts ?? true;
	}
	get widthClass(): SurfaceWidthClass {
		return classify(this.width);
	}
	setWidth(px: number): void {
		this.width = px;
	}

	attach(owner: symbol): SurfaceHost {
		this.owner = owner;
		this.spec = { title: '' };
		return new OwnedHost(this, owner);
	}
	detach(owner: symbol): void {
		if (this.owner !== owner) return;
		this.owner = null;
		this.spec = { title: '' };
		this.options.onspec(this.spec);
	}
	ambient(): SurfaceAmbient {
		const self = () => this;
		return {
			get frame() {
				return self().frame;
			},
			get theme() {
				return self().theme;
			},
			get density() {
				return self().density;
			},
			get globalShortcuts() {
				return self().globalShortcuts;
			},
			open: (target, how) => self().applyOpen(target, how),
			close: (fallbackHref) => self().applyClose(fallbackHref)
		};
	}

	isCurrent(owner: symbol): boolean {
		return this.owner === owner;
	}
	private emit(patch: Partial<SurfaceSpec>): void {
		this.spec = { ...this.spec, ...patch };
		this.options.onspec(this.spec);
	}
	applyTitle(title: SurfaceTitle): void {
		this.emit({ ...title });
	}
	applyActions(actions: SurfaceActions): void {
		this.emit({ primary: actions.primary, secondary: actions.secondary });
	}
	applyHeader(variant: SurfaceHeaderVariant): void {
		this.emit({ header: variant });
	}
	applyDensity(density: SurfaceDensity): void {
		this.emit({ density });
	}
	applyTip(tip: SurfaceTip | null): void {
		this.emit({ tip });
	}
	applyDirty(dirty: boolean): void {
		this.emit({ dirty });
	}
	applySyncState(query: Record<string, string | null>, mode: 'replace' | 'push'): void {
		// A scope has no URL of its own: the owner of the list surface decides what its query means.
		void query;
		void mode;
	}
	applyOpen(target: SurfaceTarget, how: SurfaceOpenMode): void {
		if (this.options.onopen) this.options.onopen(target, how);
		else this.parent?.open(target, how);
	}
	applyClose(fallbackHref?: string): void {
		if (this.options.onclose) this.options.onclose();
		else this.parent?.close(fallbackHref);
	}
	applyReport(result: SurfaceReport): void {
		// The owner renders the report line in the chrome it owns, from the tip it receives.
		this.emit({
			tip: {
				id: SURFACE_REPORT_TIP_ID,
				tone: 'success',
				message: formatReport(this.options.translate, this.options.locale(), result)
			}
		});
	}
}
