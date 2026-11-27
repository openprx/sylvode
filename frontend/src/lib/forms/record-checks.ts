/**
 * Client hints of the write pipeline (contract C2 §6, stages 5-6): the field type check, the
 * built-in normalize ops, then every built-in and CEL rule of `rules.validate`, with the codes the
 * server reports. Plugin rules, transforms and formulas run on the server only. The hints never
 * block a save; the server stays the authority and its refusal is rendered the same way.
 */

import type { FormField, FormFieldRuleObject } from '#lib/api/forms.js';
import { checkBuiltin, parseRule, ruleParams } from '#lib/forms/rules/index.js';
import { fieldTypeDescriptor } from '#lib/forms/types/index.js';

export interface RuleHint {
	field: string;
	code: string;
	/** `validate[<n>]` for a rule failure. */
	rule: string | null;
	params: Record<string, unknown> | null;
}

/** The CEL `user` and `now` of a check (`now` in Unix seconds). */
export interface CheckContext {
	now: number;
	user: { id: string; role: string };
}

/** A value the server treats as empty: absent, `null`, `""` or `[]`. */
export function isEmptyValue(value: unknown): boolean {
	return (
		value === undefined ||
		value === null ||
		value === '' ||
		(Array.isArray(value) && value.length === 0)
	);
}

function collapseSpaces(text: string): string {
	return text.replace(/\s+/gu, ' ');
}

/** Applies one built-in normalize op (`{"op": ...}`) to a string; other entries keep the value. */
function applyNormalizeOp(entry: unknown, value: unknown): unknown {
	if (typeof value !== 'string' || typeof entry !== 'object' || entry === null) return value;
	switch ((entry as { op?: unknown }).op) {
		case 'trim':
			return value.trim();
		case 'collapse_spaces':
			return collapseSpaces(value);
		case 'lower':
			return value.toLowerCase();
		case 'upper':
			return value.toUpperCase();
		case 'nfc':
			return value.normalize('NFC');
		default:
			return value;
	}
}

type Normalized = { ok: true; value: unknown } | { ok: false; code: string };

/** Stage 5: type check, built-in normalize ops, type check again. */
export function normalizeFieldValue(field: FormField, raw: unknown, prior?: unknown): Normalized {
	const descriptor = fieldTypeDescriptor(field);
	const first = descriptor.normalize(field, raw, prior);
	if (!first.ok) return first;
	const ops = field.rules?.normalize ?? [];
	if (ops.length === 0) return first;
	const value = ops.reduce<unknown>(
		(current, entry) => applyNormalizeOp(entry, current),
		first.value
	);
	if (isEmptyValue(value)) return { ok: true, value };
	return descriptor.normalize(field, value, prior);
}

/** CEL `record`: the values of the fields with a CEL type, bound by their descriptors. */
export function celRecord(
	fields: readonly FormField[],
	values: Readonly<Record<string, unknown>>
): Record<string, unknown> {
	const record: Record<string, unknown> = {};
	for (const field of fields) {
		const descriptor = fieldTypeDescriptor(field);
		if (descriptor.celType === null || !(field.key in values)) continue;
		const value = values[field.key];
		if (isEmptyValue(value)) continue;
		const normalized = normalizeFieldValue(field, value);
		const bound = normalized.ok ? descriptor.celValue(field, normalized.value) : undefined;
		if (bound !== undefined) record[field.key] = bound;
	}
	return record;
}

function ruleHint(
	field: FormField,
	index: number,
	entry: FormFieldRuleObject,
	value: unknown,
	record: Record<string, unknown>,
	ctx: CheckContext
): RuleHint | null {
	const descriptor = fieldTypeDescriptor(field);
	const label = `validate[${index}]`;
	const parsed = parseRule(entry, descriptor.allowedRules);
	if (!parsed.ok) return { field: field.key, code: 'rule_invalid', rule: label, params: null };
	const spec = parsed.rule;
	const custom = spec.code;
	if (spec.spec.type === 'plugin') return null;
	if (spec.spec.type === 'cel') {
		const bound = value === undefined ? null : (descriptor.celValue(field, value) ?? null);
		const result = spec.spec.program.evalBool({
			value: bound,
			record,
			now: ctx.now,
			user: ctx.user
		});
		if (result.ok && result.value) return null;
		const code = result.ok ? (custom ?? 'cel_failed') : result.code;
		return { field: field.key, code, rule: label, params: null };
	}
	if (value === undefined) return null;
	const rule = spec.spec.rule;
	let code: string | null;
	if (rule.kind === 'enum' && Array.isArray(value)) {
		code = null;
		for (const item of value) {
			code =
				typeof item === 'string'
					? checkBuiltin(rule, { kind: 'str', value: item })
					: 'rule_invalid';
			if (code) break;
		}
	} else {
		const operand = descriptor.operand(field, value);
		code = operand === null ? rule.kind : checkBuiltin(rule, operand);
	}
	if (code === null) return null;
	return {
		field: field.key,
		code: custom && code !== 'rule_invalid' ? custom : code,
		rule: label,
		params: ruleParams(rule)
	};
}

/**
 * The hints of one field value. `record` is the CEL `record` of the whole draft; `prior` the
 * stored value (option carry-over). `required` is reported by the caller's own check.
 */
export function fieldHints(
	field: FormField,
	raw: unknown,
	record: Record<string, unknown>,
	ctx: CheckContext,
	prior?: unknown
): RuleHint[] {
	const validate = field.rules?.validate ?? [];
	if (isEmptyValue(raw)) {
		return validate
			.map((entry, index) =>
				entry?.run_on_empty === true ? ruleHint(field, index, entry, undefined, record, ctx) : null
			)
			.filter((hint): hint is RuleHint => hint !== null);
	}
	const normalized = normalizeFieldValue(field, raw, prior);
	if (!normalized.ok)
		return [{ field: field.key, code: normalized.code, rule: null, params: null }];
	if (isEmptyValue(normalized.value)) return [];
	return validate
		.map((entry, index) => ruleHint(field, index, entry, normalized.value, record, ctx))
		.filter((hint): hint is RuleHint => hint !== null);
}

/**
 * Hints for a record payload: every field whose key is in `values` (the keys a save would send),
 * with `existing` merged in for the CEL `record`.
 */
export function recordHints(
	fields: readonly FormField[],
	values: Readonly<Record<string, unknown>>,
	ctx: CheckContext,
	existing: Readonly<Record<string, unknown>> = {}
): Record<string, RuleHint[]> {
	const record = celRecord(fields, { ...existing, ...values });
	const hints: Record<string, RuleHint[]> = {};
	for (const field of fields) {
		if (!(field.key in values) || fieldTypeDescriptor(field).serverWritten) continue;
		const found = fieldHints(field, values[field.key], record, ctx, existing[field.key]);
		if (found.length > 0) hints[field.key] = found;
	}
	return hints;
}
