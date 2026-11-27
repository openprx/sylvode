/**
 * Field errors by code (contract C2 §9): reads `details.errors` of a refused record write
 * (`validation_failed`) or schema save (`schema_invalid`) and turns a code into localised text.
 * The server's `message` is fixed English and never shown for these; the text comes from the
 * rule's own `message` in the schema, else from `lowcode.forms.errors.<code>`.
 */

import type {
	FormCarriedError,
	FormField,
	FormFieldErrorItem,
	FormSchemaIssue
} from '#lib/api/forms.js';

type Translate = (key: string, options?: { values?: Record<string, string | number> }) => string;

/** Codes with their own text under `lowcode.forms.errors`. */
const KNOWN_CODES: ReadonlySet<string> = new Set([
	'required',
	'read_only',
	'min',
	'max',
	'min_length',
	'max_length',
	'pattern',
	'enum',
	'email',
	'url',
	'cel_failed',
	'cel_error',
	'cel_limit',
	'transform_invalid',
	'default_invalid',
	'rule_invalid',
	'type_mismatch',
	'option_unsupported',
	'option_disabled',
	'option_archived',
	'installation_not_active',
	'installation_revoked',
	'plugin_deadline_exceeded',
	'plugin_fuel_exhausted',
	'plugin_memory_limit',
	'plugin_failed',
	'schema_version_unsupported',
	'schema_key_removed',
	'rule_unknown',
	'rule_not_allowed_for_type',
	'rule_param_invalid',
	'pattern_invalid',
	'pattern_unsupported',
	'cel_syntax',
	'cel_not_in_subset',
	'cel_unknown_variable',
	'cel_too_long',
	'cel_too_deep',
	'cel_too_complex',
	'cel_result_type',
	'plugin_rule_unavailable',
	'plugin_rule_stage',
	'plugin_config_invalid',
	'plugin_dep_invalid',
	'projection_affected',
	'limit_exceeded',
	'field_invalid',
	'rule_unreachable'
]);

/** The part of an error item a message needs. */
export interface ErrorCode {
	code: string;
	rule?: string | null;
	params?: Record<string, unknown> | null;
}

function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function errorItem(value: unknown): FormFieldErrorItem | null {
	if (!isObject(value) || typeof value.field !== 'string' || typeof value.code !== 'string') {
		return null;
	}
	return {
		field: value.field,
		field_id: typeof value.field_id === 'string' ? value.field_id : '',
		stage: typeof value.stage === 'string' ? value.stage : '',
		code: value.code,
		rule: typeof value.rule === 'string' ? value.rule : null,
		params: isObject(value.params) ? value.params : null,
		row: typeof value.row === 'number' ? value.row : null
	};
}

/** The field errors of a `validation_failed` refusal, or `null` for any other result. */
export function validationErrorsOf(result: {
	error_code?: string;
	details?: unknown;
}): FormFieldErrorItem[] | null {
	if (result.error_code !== 'validation_failed' || !isObject(result.details)) return null;
	const errors = result.details.errors;
	if (!Array.isArray(errors)) return [];
	return errors.map(errorItem).filter((item): item is FormFieldErrorItem => item !== null);
}

/** The issues of a `schema_invalid` refusal, or `null` for any other result. */
export function schemaIssuesOf(result: {
	error_code?: string;
	details?: unknown;
}): FormSchemaIssue[] | null {
	if (result.error_code !== 'schema_invalid' || !isObject(result.details)) return null;
	const errors = result.details.errors;
	if (!Array.isArray(errors)) return [];
	return errors
		.filter(isObject)
		.filter((item) => typeof item.path === 'string' && typeof item.code === 'string')
		.map((item) => ({ path: item.path as string, code: item.code as string }));
}

/** Errors grouped by field key, in report order. */
export function errorsByField<T extends { field: string }>(
	items: readonly T[]
): Record<string, T[]> {
	const grouped: Record<string, T[]> = {};
	for (const item of items) (grouped[item.field] ??= []).push(item);
	return grouped;
}

/** The `rules.validate` index of `validate[<n>]`, or `null`. */
export function validateIndex(rule: string | null | undefined): number | null {
	const match = /^validate\[(\d+)\]$/.exec(rule ?? '');
	return match ? Number(match[1]) : null;
}

function ruleMessage(
	field: FormField | undefined,
	rule: string | null | undefined,
	locale: string
) {
	const index = validateIndex(rule);
	if (index === null) return null;
	const message = field?.rules?.validate?.[index]?.message;
	if (!isObject(message)) return null;
	const text = message[locale] ?? message.en ?? message.zh;
	return typeof text === 'string' && text.trim() ? text : null;
}

function boundText(params: Record<string, unknown> | null | undefined): string {
	const bound = params?.bound;
	return bound === undefined || bound === null ? '' : String(bound);
}

/** The text of one error code for `field` in `locale` (`en` or `zh`). */
export function errorMessage(
	field: FormField | undefined,
	error: ErrorCode,
	translate: Translate,
	locale: string
): string {
	const own = ruleMessage(field, error.rule, locale);
	if (own) return own;
	const plugin = /^plugin\.(.+)\.([a-z0-9_]{1,64})$/.exec(error.code);
	if (plugin) {
		return translate('lowcode.forms.errors.plugin', {
			values: { package: plugin[1], code: plugin[2] }
		});
	}
	if (KNOWN_CODES.has(error.code)) {
		return translate(`lowcode.forms.errors.${error.code}`, {
			values: { bound: boundText(error.params) }
		});
	}
	return translate('lowcode.forms.errors.custom', { values: { code: error.code } });
}

/** One unobtrusive line for the rule failures a recalculation carried (empty list: `null`). */
export function carriedSummary(
	carried: readonly FormCarriedError[],
	fields: readonly FormField[],
	translate: Translate,
	locale: string
): string | null {
	if (carried.length === 0) return null;
	const items = carried.map((item) => {
		const field = fields.find((candidate) => candidate.key === item.field);
		return `${field?.label || item.field}: ${errorMessage(field, item, translate, locale)}`;
	});
	return translate('lowcode.forms.carried', {
		values: { count: carried.length, items: items.join('; ') }
	});
}
