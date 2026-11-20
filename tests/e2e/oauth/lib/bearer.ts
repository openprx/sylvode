// Reading RFC 6750 challenges and the delegated error bodies.

import type { HttpResult } from './http.ts';
import { asRecord, check, str } from './util.ts';

export function challengeOf(res: HttpResult): string {
	return res.headers.get('www-authenticate') ?? '';
}

/** The quoted value of a challenge parameter, or undefined. */
export function challengeParam(challenge: string, name: string): string | undefined {
	const match = new RegExp(`(?:^|[\\s,])${name}="((?:[^"\\\\]|\\\\.)*)"`).exec(challenge);
	return match?.[1]?.replace(/\\(.)/g, '$1');
}

/** `error_code` of the delegated error body (also present as `error`). */
export function bodyErrorCode(res: HttpResult): string | undefined {
	const body = asRecord(res.json);
	return str(body?.error_code) ?? str(body?.error);
}

export function expectBearerFailure(
	res: HttpResult,
	label: string,
	expected: { status: number; error: string }
): void {
	check(
		res.status === expected.status,
		`${label}: expected HTTP ${expected.status}, got ${res.status}`
	);
	check(
		bodyErrorCode(res) === expected.error,
		`${label}: expected error ${expected.error}, got ${bodyErrorCode(res) ?? 'none'}`
	);
	check(
		challengeParam(challengeOf(res), 'error') === expected.error,
		`${label}: the WWW-Authenticate challenge does not carry error="${expected.error}"`
	);
}
