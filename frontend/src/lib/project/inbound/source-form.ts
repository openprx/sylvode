/**
 * The create and edit form of an inbound source as pure logic: the draft, the client rules that
 * mirror the server (ALERT-STAGE-2-UI-RULES.md 2.2), the request bodies and the routing of a
 * server message to the field it names. No requests, no stores.
 */
import type { CreateSourceInput, UpdateSourceInput } from '#lib/api/inbound.js';
import type { Source } from './wire.js';

export const NAME_MAX = 100;
export const RATE_RANGE = { min: 1, max: 6000 } as const;
export const CAP_RANGE = { min: 1, max: 1_000_000 } as const;
export const ALLOWLIST_MAX = 64;
export const DEFAULT_RATE = 120;
export const DEFAULT_CAP = 20_000;

/** Text fields hold the typed text, so what the person typed is never rewritten. */
export interface SourceDraft {
	name: string;
	requireSignature: boolean;
	rate: string;
	cap: string;
	/** One address or block per line. */
	allowlist: string;
}

export type SourceField = 'name' | 'rate' | 'cap' | 'allowlist';

/** Message keys of the client errors, by field; a field without an error is absent. */
export type SourceErrors = Partial<Record<SourceField, string>>;

export function defaultDraft(): SourceDraft {
	return {
		name: '',
		requireSignature: true,
		rate: String(DEFAULT_RATE),
		cap: String(DEFAULT_CAP),
		allowlist: ''
	};
}

export function draftOf(source: Source): SourceDraft {
	return {
		name: source.name,
		requireSignature: source.require_signature,
		rate: String(source.rate_per_minute),
		cap: String(source.daily_cap),
		allowlist: source.ip_allowlist.join('\n')
	};
}

/** The non-empty, trimmed lines of the allow-list text. The server decides their validity. */
export function parseAllowlist(text: string): string[] {
	return text
		.split(/\r?\n/)
		.map((line) => line.trim())
		.filter((line) => line !== '');
}

/** A whole number written with digits only; `null` for anything else (including `1e3`, `-1`, ``). */
export function wholeNumber(text: string): number | null {
	const trimmed = text.trim();
	if (!/^\d{1,9}$/.test(trimmed)) return null;
	return Number(trimmed);
}

function inRange(text: string, range: { min: number; max: number }): boolean {
	const value = wholeNumber(text);
	return value !== null && value >= range.min && value <= range.max;
}

export function validateDraft(draft: SourceDraft): SourceErrors {
	const errors: SourceErrors = {};
	const name = draft.name.trim();
	if (name === '') errors.name = 'inbound.error.nameRequired';
	else if (Array.from(name).length > NAME_MAX) errors.name = 'inbound.error.nameLength';
	if (!inRange(draft.rate, RATE_RANGE)) errors.rate = 'inbound.error.rateRange';
	if (!inRange(draft.cap, CAP_RANGE)) errors.cap = 'inbound.error.capRange';
	if (parseAllowlist(draft.allowlist).length > ALLOWLIST_MAX)
		errors.allowlist = 'inbound.error.allowlistCount';
	return errors;
}

export function isValid(errors: SourceErrors): boolean {
	return Object.keys(errors).length === 0;
}

/** Create states every field, so no server default can differ from what the form showed. */
export function createBody(draft: SourceDraft): CreateSourceInput {
	return {
		name: draft.name.trim(),
		require_signature: draft.requireSignature,
		rate_per_minute: wholeNumber(draft.rate) ?? DEFAULT_RATE,
		daily_cap: wholeNumber(draft.cap) ?? DEFAULT_CAP,
		ip_allowlist: parseAllowlist(draft.allowlist)
	};
}

function sameList(a: readonly string[], b: readonly string[]): boolean {
	return a.length === b.length && a.every((value, at) => value === b[at]);
}

/** Only the fields that differ from the stored source; an emptied allow-list is sent as `[]`. */
export function changedFields(draft: SourceDraft, source: Source): UpdateSourceInput {
	const change: UpdateSourceInput = {};
	const name = draft.name.trim();
	if (name !== source.name) change.name = name;
	if (draft.requireSignature !== source.require_signature)
		change.require_signature = draft.requireSignature;
	const rate = wholeNumber(draft.rate);
	if (rate !== null && rate !== source.rate_per_minute) change.rate_per_minute = rate;
	const cap = wholeNumber(draft.cap);
	if (cap !== null && cap !== source.daily_cap) change.daily_cap = cap;
	const list = parseAllowlist(draft.allowlist);
	if (!sameList(list, source.ip_allowlist)) change.ip_allowlist = list;
	return change;
}

export function hasChanges(change: UpdateSourceInput): boolean {
	return Object.keys(change).length > 0;
}

/** Turning "Require signed requests" off on a source that requires it asks first. */
export function turnsSignatureOff(change: UpdateSourceInput): boolean {
	return change.require_signature === false;
}

const SERVER_FIELDS: Array<[string, SourceField]> = [
	['name', 'name'],
	['rate_per_minute', 'rate'],
	['daily_cap', 'cap'],
	['ip_allowlist', 'allowlist']
];

/**
 * The field a server message names. The backend starts its messages with the field name
 * (`name must be ...`, `ip_allowlist entry ...`); anything else is not tied to a field.
 */
export function serverField(message: string): SourceField | null {
	for (const [prefix, field] of SERVER_FIELDS) {
		if (message.startsWith(`${prefix} `) || message.startsWith(`${prefix}:`)) return field;
	}
	return null;
}

/** How a failed write is shown: under the field it names, as a stale-state notice, or on top. */
export type Failure =
	| { type: 'field'; field: SourceField; message: string }
	| { type: 'stale'; message: string }
	| { type: 'other'; message: string };

/**
 * Routes a failed source write. A field-named message goes under its field; a 409 means the
 * source changed (or is gone) since the list was read, so the section offers a reload; anything
 * else goes in a tip bar at the top of the form. What the person typed is always kept.
 */
export function routeFailure(failure: { code: number; message: string }): Failure {
	const field = serverField(failure.message);
	if (field !== null) return { type: 'field', field, message: failure.message };
	if (failure.code === 409) return { type: 'stale', message: failure.message };
	return { type: 'other', message: failure.message };
}
