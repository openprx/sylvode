// Failures the client produces itself carry a locale key, never English text, so these modules run
// (and are tested) without an initialised locale. `describeResult` (`lib/i18n/describe.ts`) resolves
// the key when a toast or a state block needs words. This file has no imports on purpose.

export interface TransportFailureShape {
	code: number;
	message: string;
	messageKey?: string;
	data: null;
	error_code?: string;
	details?: unknown;
}

/** Locale keys of the failures the transport and Flow clients produce themselves. */
export type TransportKey =
	| 'transport.invalidResponse'
	| 'transport.sessionChanged'
	| 'transport.requestAborted'
	| 'transport.networkError'
	| 'transport.downloadFailed'
	| 'transport.sessionRefreshFailed'
	| 'transport.rateLimited'
	| 'transport.unsupportedDownloadUrl'
	| 'transport.assetResponseInvalid'
	| 'transport.privateAssetUnavailable'
	| 'transport.publicAssetUnavailable'
	| 'transport.sharedBlockUnavailable'
	| 'transport.sharedBlockInvalid'
	| 'transport.sharedBlockFailed';

/** A failure produced by the client itself: no English text, a locale key instead. */
export function transportFailure<T = never>(
	code: number,
	messageKey: TransportKey,
	extra: { error_code?: string; details?: unknown } = {}
): TransportFailureShape & { data: T | null } {
	return { code, message: '', messageKey, data: null, ...extra };
}
