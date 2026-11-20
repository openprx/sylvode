// PKCE (RFC 7636, S256) and the per-request random values of an authorization request.

import { createHash, randomBytes } from 'node:crypto';

export function base64url(data: Uint8Array): string {
	return Buffer.from(data)
		.toString('base64')
		.replace(/\+/g, '-')
		.replace(/\//g, '_')
		.replace(/=+$/, '');
}

/** BASE64URL(SHA-256(verifier)), the S256 challenge. */
export function codeChallengeS256(verifier: string): string {
	return base64url(createHash('sha256').update(verifier, 'ascii').digest());
}

/** 32 random bytes, base64url-encoded: 43 characters from the unreserved set. */
export function newVerifier(): string {
	return base64url(randomBytes(32));
}

export function newState(): string {
	return base64url(randomBytes(32));
}

export function isValidVerifier(verifier: string): boolean {
	return /^[A-Za-z0-9\-._~]{43,128}$/.test(verifier);
}
