/** Server queries wait this long after the last keystroke. */
export const SEARCH_DEBOUNCE_MS = 250;
/** Queries shorter than this are never sent to the server. */
export const MIN_SERVER_QUERY = 2;

export interface TimerApi {
	set(run: () => void, ms: number): unknown;
	clear(handle: unknown): void;
}

const realTimers: TimerApi = {
	set: (run, ms) => setTimeout(run, ms),
	clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>)
};

export interface ServerSearchOptions<T> {
	/** Runs one query; `null` means it failed and the group stays empty. */
	fetch: (query: string, signal: AbortSignal) => Promise<T | null>;
	/** Called with the result of the latest query only, or with null when there is none. */
	onresult: (query: string, result: T | null) => void;
	onloading?: (loading: boolean) => void;
	delay?: number;
	timers?: TimerApi;
}

export interface ServerSearch {
	/** Schedule `query` (trimmed); a shorter one cancels everything and clears the result. */
	request(query: string): void;
	/** Drop any pending or running query without calling `onresult`. */
	cancel(): void;
}

/**
 * Debounced, abortable server query. A new keystroke aborts the running request; a response that
 * arrives after a newer request was made is discarded.
 */
export function createServerSearch<T>(options: ServerSearchOptions<T>): ServerSearch {
	const timers = options.timers ?? realTimers;
	const delay = options.delay ?? SEARCH_DEBOUNCE_MS;
	let timer: unknown = null;
	let controller: AbortController | null = null;
	let generation = 0;

	function stop(): void {
		generation += 1;
		if (timer !== null) timers.clear(timer);
		timer = null;
		controller?.abort();
		controller = null;
	}

	return {
		request(raw) {
			const query = raw.trim();
			stop();
			if ([...query].length < MIN_SERVER_QUERY) {
				options.onloading?.(false);
				options.onresult(query, null);
				return;
			}
			const mine = generation;
			options.onloading?.(true);
			timer = timers.set(() => {
				timer = null;
				const abort = new AbortController();
				controller = abort;
				options
					.fetch(query, abort.signal)
					.catch(() => null)
					.then((result) => {
						if (mine !== generation) return;
						controller = null;
						options.onloading?.(false);
						options.onresult(query, result);
					});
			}, delay);
		},
		cancel() {
			stop();
			options.onloading?.(false);
		}
	};
}
