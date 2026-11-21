// Redaction of everything secret before it is printed, recorded or written.
//
// Three layers, so that a format change on the server cannot leak a value:
//  1. exact values the run has learned (every token, JWT, password, verifier and code is
//     registered the moment it appears) are replaced wherever they occur;
//  2. anything shaped like a Sylvode token or a JWT is replaced;
//  3. values under sensitive keys of JSON and form bodies and of URLs are replaced.
//
// The placeholders deliberately do not contain a token prefix followed by an underscore, because
// the artifact self-check searches the output for exactly those strings.

export const TOKEN_PREFIXES = ['sva_', 'svr_', 'svc_', 'sve_', 'svi_', 'svs_', 'opr_'] as const;

const TOKEN_SHAPE = /\b(?:sva|svr|svc|sve|svi|svs|opr)_[A-Za-z0-9_-]{4,}/g;
const JWT_SHAPE = /\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*/g;
const BEARER_SHAPE = /\b(Bearer)\s+[A-Za-z0-9._~+/=-]{8,}/gi;

const SENSITIVE_KEYS = new Set([
	'access_token',
	'refresh_token',
	'id_token',
	'code',
	'code_verifier',
	'password',
	'new_password',
	'current_password',
	'token',
	'consent_token',
	'client_secret',
	'secret',
	'signing_secret',
	'authorization',
	'x-sylvode-consent',
	'x-sylvode-forward-auth'
]);

const MARK = '<redacted>';

export class Redactor {
	private readonly known = new Map<string, string>();

	/** Teaches the redactor a value; from now on it is replaced wherever it appears. */
	register(value: string | undefined | null, label: string): void {
		if (typeof value !== 'string' || value.length < 4) return;
		this.known.set(value, label);
	}

	text(input: string): string {
		let out = input;
		// Longest first, so a value that contains another is replaced as a whole.
		const values = [...this.known.keys()].sort((a, b) => b.length - a.length);
		for (const value of values) {
			if (out.includes(value))
				out = out.split(value).join(`<redacted:${this.known.get(value) ?? 'value'}>`);
		}
		return out
			.replace(JWT_SHAPE, '<redacted:jwt>')
			.replace(TOKEN_SHAPE, (match) => `<redacted:${match.slice(0, 3)}>`)
			.replace(BEARER_SHAPE, '$1 <redacted>');
	}

	json(value: unknown, depth = 0): unknown {
		if (depth > 12) return MARK;
		if (typeof value === 'string') return this.text(value);
		if (Array.isArray(value)) return value.map((item) => this.json(item, depth + 1));
		if (typeof value === 'object' && value !== null) {
			const out: Record<string, unknown> = {};
			for (const [key, item] of Object.entries(value)) {
				out[key] =
					SENSITIVE_KEYS.has(key.toLowerCase()) && typeof item === 'string'
						? MARK
						: this.json(item, depth + 1);
			}
			return out;
		}
		return value;
	}

	form(raw: string): string {
		const params = new URLSearchParams(raw);
		const out = new URLSearchParams();
		for (const [key, value] of params) {
			out.append(key, SENSITIVE_KEYS.has(key.toLowerCase()) ? MARK : this.text(value));
		}
		return out.toString();
	}

	/** A response or request body as text, redacted according to its content type. */
	body(raw: string, contentType: string | null | undefined): string {
		const type = (contentType ?? '').toLowerCase();
		if (type.includes('application/x-www-form-urlencoded')) return this.form(raw);
		const trimmed = raw.trim();
		if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
			try {
				return JSON.stringify(this.json(JSON.parse(trimmed) as unknown));
			} catch {
				// not JSON after all: fall through to plain text
			}
		}
		return this.text(raw);
	}

	/** A URL with sensitive query parameters replaced. Fragments are dropped. */
	url(raw: string): string {
		let parsed: URL;
		try {
			parsed = new URL(raw);
		} catch {
			return this.text(raw);
		}
		for (const key of [...parsed.searchParams.keys()]) {
			if (SENSITIVE_KEYS.has(key.toLowerCase())) parsed.searchParams.set(key, MARK);
		}
		parsed.hash = '';
		return this.text(parsed.toString());
	}
}

/** The process-wide redactor the suite registers every secret it learns with. */
export const redactor = new Redactor();

export function truncate(text: string, max: number): string {
	return text.length <= max
		? text
		: `${text.slice(0, max)}...[${text.length - max} more characters]`;
}
