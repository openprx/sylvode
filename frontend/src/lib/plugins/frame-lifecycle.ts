/**
 * Teardown of one plugin frame's message port.
 *
 * A frame that is being suspended (revocation, version change, session change) must get a real
 * chance to observe the `suspended` event, which a plugin document can only do while it is still
 * mounted. So suspension is two steps: `suspend()` first stops serving the frame at once
 * (`dispose`) and posts the event, then keeps the port and the (inert) frame for a short, fixed
 * grace period, and finally `close()` runs unconditionally: the port is closed and the frame is
 * removed whether or not the plugin reacted. The plugin cannot extend the grace period and has no
 * way to be served during it.
 */

/** How long a suspended frame stays mounted, inert, so that it can observe `suspended`. */
export const suspendGraceMs = 300;

export interface FramePort {
	postMessage(message: unknown): void;
	close(): void;
	/** Only ever cleared here; the owner installs its own handler. */
	onmessage: ((event: never) => void) | null;
}

export interface FrameTimers {
	set(callback: () => void, ms: number): unknown;
	clear(handle: unknown): void;
}

export interface FrameLifecycleOptions {
	port: FramePort;
	/** Stops serving the frame: every request from now on gets no result or a refusal. */
	dispose: () => void;
	/** Removes the frame from the document. Must be idempotent. */
	removeFrame: () => void;
	graceMs?: number;
	timers?: FrameTimers;
}

export interface FrameLifecycle {
	readonly state: 'live' | 'suspending' | 'closed';
	/** Stops serving, posts `suspendedEvent`, and closes after the grace period. Idempotent. */
	suspend(suspendedEvent: unknown): void;
	/** Closes the port and removes the frame now. Idempotent; also ends a pending grace period. */
	close(): void;
}

const defaultTimers: FrameTimers = {
	set: (callback, ms) => setTimeout(callback, ms),
	clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>)
};

export function createFrameLifecycle(options: FrameLifecycleOptions): FrameLifecycle {
	const timers = options.timers ?? defaultTimers;
	const graceMs = options.graceMs ?? suspendGraceMs;
	let state: FrameLifecycle['state'] = 'live';
	let timer: unknown;

	function stopServing() {
		options.port.onmessage = null;
		options.dispose();
	}

	function close() {
		if (state === 'closed') return;
		const wasLive = state === 'live';
		state = 'closed';
		timers.clear(timer);
		timer = undefined;
		if (wasLive) stopServing();
		try {
			options.port.close();
		} catch {
			/* A port that is already gone is closed. */
		}
		options.removeFrame();
	}

	return {
		get state() {
			return state;
		},
		suspend(suspendedEvent) {
			if (state !== 'live') return;
			state = 'suspending';
			stopServing();
			try {
				options.port.postMessage(suspendedEvent);
			} catch {
				/* The frame is closed regardless. */
			}
			timer = timers.set(close, graceMs);
		},
		close
	};
}
