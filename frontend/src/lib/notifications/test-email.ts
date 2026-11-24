// The admin "send a test e-mail" control: what a result of the endpoint means for the screen.

import type { ApiResult } from '#lib/api/client.js';

/** `queued` is not "delivered": the worker sends the message after the answer. */
export type TestEmailOutcome =
	| { kind: 'queued' }
	| { kind: 'not_set_up' }
	| { kind: 'forbidden' }
	| { kind: 'failed'; result: ApiResult<unknown> };

export function testEmailOutcome(result: ApiResult<{ queued: boolean }>): TestEmailOutcome {
	if (result.code === 0 && result.data?.queued === true) return { kind: 'queued' };
	if (result.code === 409 && result.error_code === 'feature_disabled')
		return { kind: 'not_set_up' };
	if (result.code === 403) return { kind: 'forbidden' };
	return { kind: 'failed', result };
}
