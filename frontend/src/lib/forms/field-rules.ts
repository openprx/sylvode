/**
 * Readers of the schema v2 field members `read_only`, `default` and `rules` for the record editor
 * and the displays. The server is the authority (`apps/api/src/forms/rules`).
 */

import type { FormField } from '#lib/api/forms.js';

/** Whether the field may be changed by no caller (only defaults and server computations). */
export function fieldReadOnly(field: FormField): boolean {
	return field.read_only === true;
}

/** The field's fixed `default.value`, or `undefined` (also for a CEL default). */
export function fieldDefaultValue(field: FormField): unknown {
	const fallback = field.default;
	return fallback && 'value' in fallback ? fallback.value : undefined;
}

/** The field's CEL default expression, or `undefined`. */
export function fieldDefaultCel(field: FormField): string | undefined {
	const fallback = field.default;
	return fallback && 'cel' in fallback ? fallback.cel : undefined;
}

/** The `value` of the first built-in `rules.validate` entry named `rule`. */
export function fieldRuleValue(
	field: FormField,
	rule: 'min' | 'max' | 'min_length' | 'max_length'
): unknown {
	const validate = field.rules?.validate;
	if (!Array.isArray(validate)) return undefined;
	return validate.find((entry) => entry && entry.rule === rule)?.value;
}

export interface FieldInputHints {
	min?: string;
	max?: string;
	maxlength?: number;
}

/**
 * Browser input attributes from the field's built-in rules: `min` / `max` bounds and
 * `max_length`. Patterns are not mapped: an HTML `pattern` matches the whole value while a v2
 * `pattern` is unanchored, so the rule hint (`lib/forms/record-checks.ts`) reports it instead.
 */
export function fieldInputHints(field: FormField): FieldInputHints {
	const hints: FieldInputHints = {};
	const min = fieldRuleValue(field, 'min');
	const max = fieldRuleValue(field, 'max');
	const maxLength = fieldRuleValue(field, 'max_length');
	if (typeof min === 'number' || typeof min === 'string') hints.min = inputBound(field, min);
	if (typeof max === 'number' || typeof max === 'string') hints.max = inputBound(field, max);
	if (typeof maxLength === 'number' && Number.isSafeInteger(maxLength) && maxLength >= 0) {
		hints.maxlength = maxLength;
	}
	return hints;
}

/** A bound in the form a browser input takes (`datetime-local` has no zone). */
function inputBound(field: FormField, bound: number | string): string {
	const text = String(bound);
	if (field.type !== 'datetime') return text;
	const local = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2})?)(?:\.\d+)?Z$/.exec(text);
	return local ? local[1] : text;
}
