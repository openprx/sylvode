import { getContext, setContext } from 'svelte';
import type { ContextSegment } from '../ui/context-line/context-line';
import type { SurfaceAction } from '../ui/page-header/page-header';

export type { SurfaceAction, ContextSegment };

/** Which frame mounted the surface. */
export type SurfaceFrame = 'standalone' | 'hosted';
/** The three host slots. */
export type SurfaceSlot = 'main' | 'panel' | 'inline';
/** Width class of the surface container: xs < 480 <= sm < 600 <= md < 900 <= full. */
export type SurfaceWidthClass = 'xs' | 'sm' | 'md' | 'full';
/** 88 standard (pages), 56 compact (data surfaces), none (the surface draws no header). */
export type SurfaceHeaderVariant = 'standard' | 'compact' | 'none';
export type SurfaceDensity = 'comfortable' | 'compact';
export type SurfaceTheme = 'light' | 'dark';

export interface SurfaceTitle {
	/** Object name as the user sees it ("Northwind purchase order", "Issues"). No counts. */
	title: string;
	/** Identifier shown in the Latin face before the title ("LEAD-128"). Never in `context`. */
	identifier?: string;
	/** Ancestors and current label. Scope (Personal or workspace) is never a segment. */
	context?: { segments: ContextSegment[]; current: string };
	/** Hosted origin label. When absent the host builds "Sylvode · {identifier ?? title}". */
	origin?: string;
	/** True until the object has loaded: the header shows its skeleton. */
	loading?: boolean;
}

export interface SurfaceActions {
	/** At most one. */
	primary?: SurfaceAction;
	/** The header shows up to three; the rest, and all of them at 600 or less, go to the overflow. */
	secondary: SurfaceAction[];
}

/** The surface's tip bar slot: the state of this surface. */
export interface SurfaceTip {
	/** Stable id; setting a tip with the same id replaces it in place. */
	id: string;
	tone: 'neutral' | 'success' | 'warning' | 'danger';
	message: string;
	/** One text action, for example "Retry". */
	action?: { label: string; run: () => void | Promise<void> };
	/** Shows a close icon button. Default true for success, false otherwise. */
	dismissible?: boolean;
}

export type SurfaceTargetKind = 'record' | 'issue' | 'task' | 'page' | 'form' | 'project';

/** Something a surface asks its frame to open. */
export interface SurfaceTarget {
	kind: SurfaceTargetKind;
	id: string;
	workspaceId?: string;
	projectId?: string;
	formId?: string;
	/** Standalone URL that shows the target as main content. Always set. */
	href: string;
	/** Standalone URL that shows the target in the side panel of its list. */
	panelHref?: string;
	/** Name for history entries and announcements. */
	label?: string;
}

/** 'main': open as main content. 'panel': open in the side panel slot. 'replace': replace this surface in its slot. */
export type SurfaceOpenMode = 'main' | 'panel' | 'replace';

/** What a write changed and what it did not. */
export interface SurfaceReport {
	/** Object the write went to ("HOME-12", "Northwind purchase order"). */
	object: string;
	/** Labels of what was written ("1 comment", "status"). */
	written: string[];
	/** Labels of what was deliberately left unchanged ("assignee"). */
	unchanged: string[];
}

/** The object a surface uses. Obtained only through `useSurface`. */
export interface SurfaceHost {
	readonly frame: SurfaceFrame;
	readonly slot: SurfaceSlot;
	/** Container width in CSS px. Reactive. */
	readonly width: number;
	readonly widthClass: SurfaceWidthClass;
	/** Resolved theme of the frame (never 'system'). Reactive. */
	readonly theme: SurfaceTheme;
	/** Standalone: what the surface declared. Hosted: what the host set; the host wins. */
	readonly density: SurfaceDensity;
	/** False when hosted: the surface must not register global keys. */
	readonly globalShortcuts: boolean;

	setTitle(title: SurfaceTitle): void;
	setActions(actions: SurfaceActions): void;
	setHeader(variant: SurfaceHeaderVariant): void;
	/** Standalone: the density the surface declares. Hosted: ignored (the host sets it). */
	setDensity(density: SurfaceDensity): void;
	/** null clears. One tip at a time per surface; a new one replaces the old. */
	setTip(tip: SurfaceTip | null): void;
	/** Unsaved input exists. */
	setDirty(dirty: boolean): void;
	/** Surface-owned URL state (`?form=`, `?record=`, `?view=`). null removes a key. Default mode 'replace'. */
	syncState(query: Record<string, string | null>, mode?: 'replace' | 'push'): void;
	open(target: SurfaceTarget, how: SurfaceOpenMode): void;
	/** Leave this surface. `fallbackHref` is where a standalone frame goes with no previous in-app URL. */
	close(fallbackHref?: string): void;
	/** Show the write-back result line. */
	report(result: SurfaceReport): void;
}

/** What a frame inherits to a nested scope without taking over the content header. */
export interface SurfaceAmbient {
	readonly frame: SurfaceFrame;
	readonly theme: SurfaceTheme;
	readonly density: SurfaceDensity;
	readonly globalShortcuts: boolean;
	open(target: SurfaceTarget, how: SurfaceOpenMode): void;
	close(fallbackHref?: string): void;
}

/** What a frame provides through context. Surfaces never call it directly. */
export interface SurfaceHostProvider {
	/** Returns a host scoped to `owner`. Setters of a detached or superseded owner are ignored. */
	attach(owner: symbol): SurfaceHost;
	/** Clears title, actions, tip and dirty set by `owner`, unless a newer owner has attached since. */
	detach(owner: symbol): void;
	/** Read-only view of the frame, for nested scopes. Never claims ownership. */
	ambient(): SurfaceAmbient;
}

export const SURFACE_HOST_PROVIDER: unique symbol = Symbol('sv-surface-host-provider');

/** The provider of the enclosing frame; null outside any frame (unit tests, pages not yet in the shell). */
export function getSurfaceHostProvider(): SurfaceHostProvider | null {
	try {
		return getContext<SurfaceHostProvider | undefined>(SURFACE_HOST_PROVIDER) ?? null;
	} catch {
		return null;
	}
}

/** Called by a frame during component initialisation, before its children render. */
export function provideSurfaceHostProvider(provider: SurfaceHostProvider): void {
	setContext(SURFACE_HOST_PROVIDER, provider);
}
