/**
 * Form schema v2 field members (`sv.form.schema.v2`): `read_only`, `default` and `rules`
 * replace the v1 `policy.read_only`, `default_value` and `validation`. The server is the
 * authority (`apps/api/src/forms/rules`); these helpers read and write the v2 members for the
 * designer and the record editor until the descriptor table and rule editor land.
 */

import type { FormField, FormFieldType } from '#lib/api/forms.js';

export const FORM_SCHEMA_VERSION = 'sv.form.schema.v2';

export interface FieldRuleObject {
	rule?: string;
	value?: unknown;
	[key: string]: unknown;
}

export interface FieldRulesV2 {
	normalize?: unknown[];
	validate?: FieldRuleObject[];
	transform?: unknown[];
}

/** A schema field with the v2 members. */
export type FormFieldV2 = FormField & {
	read_only?: boolean;
	default?: { value?: unknown; cel?: string };
	rules?: FieldRulesV2;
};

/** Whether the field may be changed by no caller (only defaults and server computations). */
export function fieldReadOnly(field: FormField): boolean {
	return (field as FormFieldV2).read_only === true;
}

/** The field's `default.value`, or `undefined`. */
export function fieldDefaultValue(field: FormField): unknown {
	return (field as FormFieldV2).default?.value;
}

/** The `value` of the first built-in `rules.validate` entry named `rule`. */
export function fieldRuleValue(
	field: FormField,
	rule: 'min' | 'max' | 'pattern' | 'max_length'
): unknown {
	const validate = (field as FormFieldV2).rules?.validate;
	if (!Array.isArray(validate)) return undefined;
	return validate.find((entry) => entry && entry.rule === rule)?.value;
}

const ANCHOR_PREFIX = '^(?:';
const ANCHOR_SUFFIX = ')$';

/** A pattern that must match the whole value (v2 `pattern` matching is unanchored). */
export function anchoredPattern(pattern: string): string {
	return `${ANCHOR_PREFIX}${pattern}${ANCHOR_SUFFIX}`;
}

/** The designer's whole-value pattern from a stored v2 pattern. */
export function unanchoredPattern(pattern: string): string {
	return pattern.startsWith(ANCHOR_PREFIX) && pattern.endsWith(ANCHOR_SUFFIX)
		? pattern.slice(ANCHOR_PREFIX.length, pattern.length - ANCHOR_SUFFIX.length)
		: pattern;
}

/**
 * A `min` / `max` bound in the form the server accepts for the type: a JSON integer for integer
 * types, a decimal string for number and amount, an ISO date, or an RFC 3339 date-time (a
 * browser `datetime-local` value is read as UTC). `undefined` when the text does not fit.
 */
export function ruleBound(type: FormFieldType, text: string): unknown {
	const trimmed = text.trim();
	if (!trimmed) return undefined;
	if (type === 'integer' || type === 'rating' || type === 'progress') {
		return /^-?\d+$/.test(trimmed) ? Number.parseInt(trimmed, 10) : undefined;
	}
	if (type === 'number' || type === 'amount') {
		return /^-?\d+(\.\d+)?$/.test(trimmed) ? trimmed : undefined;
	}
	if (type === 'date') {
		return /^\d{4}-\d{2}-\d{2}$/.test(trimmed) ? trimmed : undefined;
	}
	if (type === 'datetime') {
		if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(trimmed)) return `${trimmed}:00Z`;
		if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}$/.test(trimmed)) return `${trimmed}Z`;
		return trimmed;
	}
	return undefined;
}

/** The designer's bound text from a stored bound. */
export function ruleBoundText(type: FormFieldType, value: unknown): string {
	if (value === undefined || value === null) return '';
	const text = String(value);
	if (type === 'datetime') {
		const local = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2})(:00)?Z$/.exec(text);
		if (local) return local[1];
	}
	return text;
}

const TEXT_LIKE: readonly FormFieldType[] = [
	'text',
	'textarea',
	'phone',
	'email',
	'address',
	'scan'
];

/** Whether `pattern` may be declared on the type. */
export function patternAllowed(type: FormFieldType): boolean {
	return TEXT_LIKE.includes(type);
}

/** Whether `max_length` may be declared on the type. */
export function maxLengthAllowed(type: FormFieldType): boolean {
	return TEXT_LIKE.includes(type) || type === 'rich_text';
}
