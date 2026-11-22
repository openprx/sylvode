/**
 * Client-side validation of the connect form and the advanced fields. Each rule mirrors a server
 * rule (`apps/api/src/services/bot_connections.rs`) and answers a message key, so the person sees
 * the problem before the request; the server stays the authority and its reason is shown under
 * the field when it still refuses.
 */
import { LEVELS, type Level } from './wire';

const PREFIX = 'execution.connection.error.';
export const ERRORS = {
	nameRequired: `${PREFIX}nameRequired`,
	nameLength: `${PREFIX}nameLength`,
	botChoose: `${PREFIX}botChoose`,
	urlInvalid: `${PREFIX}urlInvalid`,
	dirAbsolute: `${PREFIX}dirAbsolute`,
	dirSegments: `${PREFIX}dirSegments`,
	level: `${PREFIX}level`,
	reasonLength: `${PREFIX}reasonLength`,
	veto: `${PREFIX}veto`,
	domainDuplicate: `${PREFIX}domainDuplicate`,
	domainChoose: `${PREFIX}domainChoose`
} as const;

export const MAX_BOT_NAME = 100;
export const MAX_RUNNER_URL_BYTES = 2048;
export const MAX_DIRECTORY_BYTES = 1024;

export function validateBotName(raw: string): string | null {
	const name = raw.trim();
	if (name === '') return ERRORS.nameRequired;
	if ([...name].length > MAX_BOT_NAME) return ERRORS.nameLength;
	return null;
}

export function validateBotChoice(id: string | null): string | null {
	return id === null || id === '' ? ERRORS.botChoose : null;
}

/** The address as it is sent: trimmed, with one trailing slash dropped (the server does the same). */
export function normalizeRunnerUrl(raw: string): string {
	const trimmed = raw.trim();
	return trimmed.endsWith('/') ? trimmed.slice(0, -1) : trimmed;
}

export function validateRunnerUrl(raw: string): string | null {
	const url = normalizeRunnerUrl(raw);
	if (!/^https?:\/\//.test(url) || new TextEncoder().encode(url).length > MAX_RUNNER_URL_BYTES)
		return ERRORS.urlInvalid;
	if (/[\s?#]/.test(url) || url.endsWith('/')) return ERRORS.urlInvalid;
	const authority = url.replace(/^https?:\/\//, '').split('/')[0] ?? '';
	if (authority === '' || authority.includes('@')) return ERRORS.urlInvalid;
	try {
		return new URL(url).hostname === '' ? ERRORS.urlInvalid : null;
	} catch {
		return ERRORS.urlInvalid;
	}
}

/** The directory as it is sent: trimmed, with one trailing slash dropped. */
export function normalizeDirectory(raw: string): string {
	const trimmed = raw.trim();
	return trimmed.length > 1 && trimmed.endsWith('/') ? trimmed.slice(0, -1) : trimmed;
}

export function validateWorkingDirectory(raw: string): string | null {
	const path = normalizeDirectory(raw);
	const bytes = new TextEncoder().encode(path).length;
	if (!path.startsWith('/') || bytes < 2 || bytes > MAX_DIRECTORY_BYTES) return ERRORS.dirAbsolute;
	// eslint-disable-next-line no-control-regex -- the server refuses control characters
	if (/[\u0000-\u001f\u007f]/.test(path)) return ERRORS.dirAbsolute;
	if (path.includes('//') || path.endsWith('/')) return ERRORS.dirSegments;
	if (path.split('/').some((segment) => segment === '.' || segment === '..'))
		return ERRORS.dirSegments;
	return null;
}

export interface AdvancedDraft {
	level: Level | null;
	veto: boolean;
	reasonLength: string;
	domains: Array<{ domain: string | null; level: Level | null }>;
}

export interface AdvancedErrors {
	level: string | null;
	veto: string | null;
	reasonLength: string | null;
	domains: Array<string | null>;
}

/** Veto is allowed from level Vetoer up. */
export function vetoAllowed(level: Level | null): boolean {
	return level === 'vetoer' || level === 'autonomous';
}

/** `''` and non-integers are refused; the bounds are the server's (0 to 2000). */
export function parseReasonLength(raw: string): number | null {
	if (!/^\d{1,4}$/.test(raw.trim())) return null;
	const value = Number(raw.trim());
	return value >= 0 && value <= 2000 ? value : null;
}

export function validateAdvanced(draft: AdvancedDraft): AdvancedErrors {
	const seen = new Set<string>();
	const domains = draft.domains.map((row) => {
		if (row.domain === null || row.domain === '' || row.level === null) return ERRORS.domainChoose;
		if (seen.has(row.domain)) return ERRORS.domainDuplicate;
		seen.add(row.domain);
		return null;
	});
	return {
		level: draft.level !== null && LEVELS.includes(draft.level) ? null : ERRORS.level,
		veto: draft.veto && !vetoAllowed(draft.level) ? ERRORS.veto : null,
		reasonLength: parseReasonLength(draft.reasonLength) === null ? ERRORS.reasonLength : null,
		domains
	};
}

export function advancedValid(errors: AdvancedErrors): boolean {
	return (
		errors.level === null &&
		errors.veto === null &&
		errors.reasonLength === null &&
		errors.domains.every((error) => error === null)
	);
}
