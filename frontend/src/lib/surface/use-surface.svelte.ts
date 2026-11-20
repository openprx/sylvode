import { onDestroy, untrack } from 'svelte';
import {
	getSurfaceHostProvider,
	type SurfaceDensity,
	type SurfaceHeaderVariant,
	type SurfaceHost,
	type SurfaceAction,
	type SurfaceTip,
	type SurfaceTitle
} from './context';

export interface SurfaceSpec extends SurfaceTitle {
	/** Default 'standard'. */
	header?: SurfaceHeaderVariant;
	/** Default 'comfortable'. */
	density?: SurfaceDensity;
	primary?: SurfaceAction;
	secondary?: SurfaceAction[];
	/** Undefined leaves the current tip alone; null clears it. */
	tip?: SurfaceTip | null;
	/** Undefined leaves the dirty flag alone. */
	dirty?: boolean;
	/** Where close() goes in standalone when there is no previous in-app URL. */
	closeHref?: string;
}

export interface SurfaceHandle {
	/** For width, widthClass, frame and slot. */
	readonly host: SurfaceHost;
	open: SurfaceHost['open'];
	close: () => void;
	report: SurfaceHost['report'];
	syncState: SurfaceHost['syncState'];
}

/** A host that accepts every call and does nothing: used outside any frame. */
export function createInertHost(): SurfaceHost {
	return {
		frame: 'standalone',
		slot: 'main',
		width: 1024,
		widthClass: 'full',
		theme: 'light',
		density: 'comfortable',
		globalShortcuts: true,
		setTitle() {},
		setActions() {},
		setHeader() {},
		setDensity() {},
		setTip() {},
		setDirty() {},
		syncState() {},
		open() {},
		close() {},
		report() {}
	};
}

/** Pushes a whole spec to a host. Cheap; the host diffs. */
export function applySpec(host: SurfaceHost, spec: SurfaceSpec): void {
	const { title, identifier, context, origin, loading } = spec;
	host.setTitle({ title, identifier, context, origin, loading });
	host.setActions({ primary: spec.primary, secondary: spec.secondary ?? [] });
	host.setHeader(spec.header ?? 'standard');
	host.setDensity(spec.density ?? 'comfortable');
	if (spec.tip !== undefined) host.setTip(spec.tip);
	if (spec.dirty !== undefined) host.setDirty(spec.dirty);
}

/**
 * Call during component initialisation. Re-sends the spec whenever it changes and detaches on
 * destroy. Outside any frame it returns an inert handle, so a surface never crashes.
 */
export function useSurface(spec: () => SurfaceSpec): SurfaceHandle {
	const provider = getSurfaceHostProvider();
	if (!provider) {
		const host = createInertHost();
		return {
			host,
			open: host.open,
			close: host.close,
			report: host.report,
			syncState: host.syncState
		};
	}
	const owner = Symbol('surface');
	const host = provider.attach(owner);
	$effect(() => {
		const next = spec();
		untrack(() => applySpec(host, next));
	});
	onDestroy(() => provider.detach(owner));
	return {
		host,
		open: (target, how) => host.open(target, how),
		close: () => host.close(spec().closeHref),
		report: (result) => host.report(result),
		syncState: (query, mode) => host.syncState(query, mode)
	};
}
