export interface TimerApi {
	setTimeout: (fn: () => void, ms: number) => unknown;
	clearTimeout: (handle: unknown) => void;
}

export interface TooltipControllerOptions {
	/** Hover delay before the tooltip shows (specification: 500 ms). */
	delay?: number;
	/** Grace after the pointer leaves, so it can move onto the tooltip (100 ms). */
	grace?: number;
	onchange: (visible: boolean) => void;
	timers?: TimerApi;
}

/**
 * Show and hide rules of the tooltip, free of the DOM so they can be tested with fake timers:
 * hover shows after `delay`; keyboard focus shows at once; pointer leave hides after `grace`
 * unless the pointer is on the tooltip; blur, Escape and scroll hide at once.
 */
export function createTooltipController({
	delay = 500,
	grace = 100,
	onchange,
	timers = globalThis as unknown as TimerApi
}: TooltipControllerOptions) {
	let visible = false;
	let handle: unknown = null;
	let overTooltip = false;

	function set(next: boolean) {
		if (visible === next) return;
		visible = next;
		onchange(next);
	}
	function cancel() {
		if (handle !== null) timers.clearTimeout(handle);
		handle = null;
	}
	function schedule(next: boolean, ms: number) {
		cancel();
		handle = timers.setTimeout(() => {
			handle = null;
			set(next);
		}, ms);
	}

	return {
		get visible() {
			return visible;
		},
		pointerEnter() {
			if (visible) cancel();
			else schedule(true, delay);
		},
		pointerLeave() {
			if (visible) {
				if (!overTooltip) schedule(false, grace);
			} else cancel();
		},
		tooltipEnter() {
			overTooltip = true;
			cancel();
		},
		tooltipLeave() {
			overTooltip = false;
			schedule(false, grace);
		},
		/** Keyboard focus only (`:focus-visible`); the caller filters out pointer focus. */
		focus() {
			cancel();
			set(true);
		},
		hide() {
			cancel();
			overTooltip = false;
			set(false);
		}
	};
}
