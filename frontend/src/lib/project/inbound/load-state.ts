import type { CallResult } from '#lib/api/inbound.js';

/** What the two sections show while and after their first load. */
export type LoadState = 'loading' | 'error' | 'forbidden' | 'ready';

/** A 403 is the API's own refusal for a member who typed the URL; anything else is a failure. */
export function loadStateOf(result: CallResult<unknown>): LoadState {
	if (result.ok) return 'ready';
	return result.kind === 'error' && result.code === 403 ? 'forbidden' : 'error';
}
