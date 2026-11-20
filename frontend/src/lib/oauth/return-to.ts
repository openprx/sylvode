/**
 * The sign-in return target of the consent page (contract 14.4).
 *
 * The login page navigates to `return_to` after a successful sign-in only when the value is
 * exactly `/oauth/authorize?request=<uuid>`: one fixed same-origin path and one query parameter
 * holding a request id. Everything else is ignored, so the parameter can never be used as an open
 * redirect. The check is a single anchored pattern over the value as the browser decoded it once:
 * a scheme, a host, `//`, a backslash, a percent sign (a second layer of encoding), whitespace, a
 * control character or any extra parameter or fragment all fail it.
 */

export const CONSENT_PATH = '/oauth/authorize';

const UUID = '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}';
const RETURN_TO = new RegExp(`^/oauth/authorize\\?request=(${UUID})$`);
const REQUEST_ID = new RegExp(`^${UUID}$`);

/** The longest value looked at; a valid target is 36 characters of id plus 26 of path. */
const MAX_LENGTH = 128;

/** True for a canonical textual UUID. */
export function isRequestId(value: string): boolean {
	return REQUEST_ID.test(value);
}

/** The value to navigate to after sign-in, or null when it is not the consent route. */
export function safeReturnTo(value: string | null | undefined): string | null {
	if (typeof value !== 'string' || value.length === 0 || value.length > MAX_LENGTH) return null;
	return RETURN_TO.test(value) ? value : null;
}

/** The consent route that reloads a stored request by id; null for anything but a UUID. */
export function consentReturnTarget(requestId: string): string | null {
	return isRequestId(requestId) ? `${CONSENT_PATH}?request=${requestId}` : null;
}

/** The login URL that comes back to the stored request; null for an id that is not a UUID. */
export function loginUrlForRequest(requestId: string): string | null {
	const target = consentReturnTarget(requestId);
	return target === null ? null : `/auth/login?return_to=${encodeURIComponent(target)}`;
}
