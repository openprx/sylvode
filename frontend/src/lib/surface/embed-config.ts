/** Version of the hosted-frame message format. A message with another number is ignored. */
export const EMBED_PROTOCOL = 1;
/** Value of `source` in every envelope. */
export const EMBED_SOURCE = 'sylvode-surface';
/** How long a hosted frame waits for `sv:init` before it follows the system theme in slot `main`. */
export const EMBED_INIT_TIMEOUT_MS = 1000;

/**
 * Origins whose messages a hosted frame accepts, and the only target origin it posts to. In this
 * phase that is the frame's own origin; framing from another origin and its authentication belong
 * to a later track, so adding an origin here is not enough on its own.
 */
export function embedOrigins(
	selfOrigin: string = typeof location === 'undefined' ? '' : location.origin
): readonly string[] {
	return selfOrigin && selfOrigin !== 'null' ? [selfOrigin] : [];
}

/** True when `origin` is one of the allowed origins. The opaque origin `"null"` never is. */
export function isEmbedOrigin(origin: string, allowed: readonly string[]): boolean {
	return origin !== 'null' && allowed.includes(origin);
}

/** Frame ids come from a query string or a message: letters, digits, `_` and `-` only. */
export const FRAME_ID_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;
