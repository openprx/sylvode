/**
 * The one place the app makes random identifiers.
 *
 * `crypto.randomUUID` only exists in secure contexts (https or localhost). A page served over
 * plain http on a LAN address has `crypto.getRandomValues` but not `randomUUID`, so the helper
 * builds the same RFC 4122 version 4 UUID from `getRandomValues` there. It never falls back to
 * `Math.random`: an id that goes to the server (client id, session id, update id, idempotency
 * key) must come from a cryptographic source or not be made at all.
 */

type UuidCrypto = Partial<Pick<Crypto, 'randomUUID' | 'getRandomValues'>>;

const HEX: string[] = Array.from({ length: 256 }, (_, byte) => byte.toString(16).padStart(2, '0'));

/** Formats 16 bytes as a lowercase UUID string, setting the version (4) and variant (10) bits. */
function formatV4(bytes: Uint8Array): string {
	bytes[6] = (bytes[6] & 0x0f) | 0x40;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	let out = '';
	for (let i = 0; i < 16; i += 1) {
		if (i === 4 || i === 6 || i === 8 || i === 10) out += '-';
		out += HEX[bytes[i]];
	}
	return out;
}

/** A random RFC 4122 version 4 UUID. Throws when the browser has no secure random source. */
export function randomUuid(source: UuidCrypto | undefined = globalThis.crypto): string {
	if (source && typeof source.randomUUID === 'function') return source.randomUUID();
	if (source && typeof source.getRandomValues === 'function') {
		return formatV4(source.getRandomValues(new Uint8Array(16)));
	}
	throw new Error('No secure random source is available: crypto.getRandomValues is missing.');
}
