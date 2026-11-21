import { onDestroy } from 'svelte';
import { afterNavigate } from '$app/navigation';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';
import type { SurfaceHost, SurfaceTip } from './context';

/** Id of the tip this helper owns: a new error replaces the previous one in place. */
export const INLINE_ERROR_TIP_ID = 'inline-error';

/** The part of `ApiResult<T>` this helper reads. */
export interface InlineResult {
	code: number;
	message: string;
}

export interface InlineErrorOptions {
	/** Adds a Retry action to the tip. */
	retry?: () => void | Promise<void>;
	/** Shown when the failure carries no message. Default: the generic message. */
	fallback?: string;
}

export interface InlineError {
	/** Shows a failed request in the tip slot, replacing the previous error. */
	fail(message: string | null | undefined, options?: InlineErrorOptions): void;
	/**
	 * Success (`code === 0`) clears the error and returns true; a failure shows it and returns
	 * false, so a page writes `if (!errors.check(result, { retry })) return;`.
	 */
	check(result: InlineResult, options?: InlineErrorOptions): boolean;
	/** Removes the error from the tip slot. Does nothing when none is showing. */
	clear(): void;
}

export interface InlineErrorEnv {
	translate: (key: string) => string;
	/** Registers a callback for the next and every later navigation. */
	onNavigate: (callback: () => void) => void;
	/** Registers a callback for when the owning component is destroyed. */
	onDispose: (callback: () => void) => void;
}

/** Framework-free core of `useInlineError`; tests drive it directly. */
export function createInlineError(
	host: Pick<SurfaceHost, 'setTip'>,
	env: InlineErrorEnv
): InlineError {
	let showing = false;

	function clear(): void {
		if (!showing) return;
		showing = false;
		host.setTip(null);
	}

	function fail(message: string | null | undefined, options: InlineErrorOptions = {}): void {
		const text = message?.trim() || options.fallback || env.translate('surface.error.generic');
		const { retry } = options;
		const tip: SurfaceTip = {
			id: INLINE_ERROR_TIP_ID,
			tone: 'danger',
			message: text,
			dismissible: true,
			action: retry
				? { label: env.translate('surface.error.retry'), run: () => retry() }
				: undefined
		};
		showing = true;
		host.setTip(tip);
	}

	env.onNavigate(clear);
	env.onDispose(clear);

	return {
		fail,
		check(result, options) {
			if (result.code === 0) {
				clear();
				return true;
			}
			fail(result.message, options);
			return false;
		},
		clear
	};
}

/**
 * For a surface: call during component initialisation with the host from `useSurface`. The
 * error goes to the surface's tip slot, clears on navigation and when the surface is destroyed.
 */
export function useInlineError(host: Pick<SurfaceHost, 'setTip'>): InlineError {
	return createInlineError(host, {
		translate: (key) => get(t)(key),
		onNavigate: (callback) => afterNavigate(() => callback()),
		onDispose: (callback) => onDestroy(callback)
	});
}
