/**
 * Text checks shared with `crates/sv-rules/src/text.rs`: the same hand-written grammar on both
 * sides, so the `email` and `url` verdicts agree without a regex engine.
 */

const MAX_EMAIL_LEN = 254;
const MAX_LOCAL_LEN = 64;
const MAX_HOST_LEN = 253;
const MAX_LABEL_LEN = 63;
const MAX_URL_LEN = 2048;

const LOCAL_CHAR = /^[A-Za-z0-9!#$%&'*+/=?^_`{|}~.-]$/;
const LABEL = /^[A-Za-z0-9-]+$/;
const CONTROL_OR_SPACE = /[\p{Cc}\p{White_Space}]/u;

/** Unicode scalar value count (the length of `min_length`, `max_length` and CEL `size`). */
export function scalarLength(s: string): number {
	return Array.from(s).length;
}

function isHostName(host: string): boolean {
	if (host === '' || host.length > MAX_HOST_LEN) return false;
	return host
		.split('.')
		.every(
			(label) =>
				label !== '' &&
				label.length <= MAX_LABEL_LEN &&
				LABEL.test(label) &&
				!label.startsWith('-') &&
				!label.endsWith('-')
		);
}

/** The `email` rule; see `is_email` in `text.rs`. */
export function isEmail(s: string): boolean {
	// eslint-disable-next-line no-control-regex
	if (s.length > MAX_EMAIL_LEN || !/^[\x00-\x7f]*$/.test(s)) return false;
	const parts = s.split('@');
	if (parts.length !== 2) return false;
	const [local, domain] = parts;
	if (
		local === '' ||
		local.length > MAX_LOCAL_LEN ||
		![...local].every((c) => LOCAL_CHAR.test(c)) ||
		local.startsWith('.') ||
		local.endsWith('.') ||
		local.includes('..')
	) {
		return false;
	}
	if (!isHostName(domain)) return false;
	const labels = domain.split('.');
	return labels.length >= 2 && !/^[0-9]+$/.test(labels[labels.length - 1]);
}

function isPort(p: string): boolean {
	if (!/^[0-9]{1,5}$/.test(p)) return false;
	const n = Number(p);
	return n >= 1 && n <= 65_535;
}

function isAuthority(authority: string): boolean {
	if (authority === '' || authority.includes('@')) return false;
	if (authority.startsWith('[')) {
		const close = authority.indexOf(']');
		if (close < 0) return false;
		const inner = authority.slice(1, close);
		const after = authority.slice(close + 1);
		if (inner === '' || !inner.includes(':') || !/^[0-9A-Fa-f:.]+$/.test(inner)) return false;
		return after === '' || (after.startsWith(':') && isPort(after.slice(1)));
	}
	const colon = authority.indexOf(':');
	if (colon < 0) return isHostName(authority);
	return isHostName(authority.slice(0, colon)) && isPort(authority.slice(colon + 1));
}

/** The `url` rule; see `is_url` in `text.rs`. */
export function isUrl(s: string, schemes: readonly string[]): boolean {
	if (scalarLength(s) > MAX_URL_LEN || CONTROL_OR_SPACE.test(s)) return false;
	const sep = s.indexOf('://');
	if (sep < 0) return false;
	const scheme = s.slice(0, sep).replace(/[A-Z]/g, (c) => c.toLowerCase());
	if (!schemes.includes(scheme)) return false;
	const rest = s.slice(sep + 3);
	const end = rest.search(/[/?#]/);
	return isAuthority(end < 0 ? rest : rest.slice(0, end));
}

/** Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant's algorithm). */
export function daysFromCivil(y: number, m: number, d: number): number {
	const yy = m <= 2 ? y - 1 : y;
	const era = Math.floor(yy / 400);
	const yoe = yy - era * 400;
	const doy = Math.floor((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5) + d - 1;
	const doe = yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy;
	return era * 146_097 + doe - 719_468;
}

function daysInMonth(y: number, m: number): number {
	if (m === 2) return (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0 ? 29 : 28;
	return [4, 6, 9, 11].includes(m) ? 30 : 31;
}

/** A strict `YYYY-MM-DD` (years 0001..9999) as days since the epoch; `null` when invalid. */
export function parseIsoDate(s: string): number | null {
	const m = /^([0-9]{4})-([0-9]{2})-([0-9]{2})$/.exec(s);
	if (!m) return null;
	const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
	if (y === 0 || mo < 1 || mo > 12 || d < 1 || d > daysInMonth(y, mo)) return null;
	return daysFromCivil(y, mo, d);
}

/** An RFC 3339 date-time as nanoseconds since the epoch; `null` when invalid. */
export function parseRfc3339(s: string): bigint | null {
	const m =
		/^([0-9]{4})-([0-9]{2})-([0-9]{2})[Tt]([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.([0-9]{1,9}))?(?:([Zz])|([+-])([0-9]{2}):([0-9]{2}))$/.exec(
			s
		);
	if (!m) return null;
	const days = parseIsoDate(`${m[1]}-${m[2]}-${m[3]}`);
	const [h, mi, sec] = [Number(m[4]), Number(m[5]), Number(m[6])];
	if (days === null || h > 23 || mi > 59 || sec > 59) return null;
	let offset = 0;
	if (!m[8]) {
		const [oh, om] = [Number(m[10]), Number(m[11])];
		if (oh > 23 || om > 59) return null;
		offset = (oh * 60 + om) * 60 * (m[9] === '-' ? -1 : 1);
	}
	const seconds = BigInt(days) * 86_400n + BigInt(h * 3600 + mi * 60 + sec - offset);
	const nanos = BigInt((m[7] ?? '').padEnd(9, '0') || '0');
	return seconds * 1_000_000_000n + nanos;
}
