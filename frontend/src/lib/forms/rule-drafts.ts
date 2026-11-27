/**
 * The designer's editable form of `rules.validate` entries and `default` (contract C2 §2, §3.2):
 * one draft per rule with its parameters as typed text, converted back to the rule object on save
 * and checked live with the client rule engine (`lib/forms/rules`), with the server's codes.
 */

import type { FormFieldRuleObject, FormFieldType } from '#lib/api/forms.js';
import { compile, parseRule, type RuleDefError, type RuleKind } from '#lib/forms/rules/index.js';
import { FIELD_TYPES } from '#lib/forms/types/index.js';

/** Per-field limit of `rules.validate` entries. */
export const MAX_VALIDATE_RULES = 16;

export interface RuleDraft {
	kind: RuleKind;
	/** Bound (`min`, `max`), length (`min_length`, `max_length`), pattern source or CEL source. */
	value: string;
	/** `pattern`. */
	caseInsensitive: boolean;
	/** `enum`: one value per line. */
	valuesText: string;
	/** `url`. */
	schemes: string[];
	/** Optional code override (`^[a-z][a-z0-9_]{0,39}$`). */
	code: string;
	/** Failure text by language; the designer edits `en` and `zh` and keeps the others. */
	message: Record<string, string>;
	/** `cel` and `plugin` only. */
	runOnEmpty: boolean;
	/** `plugin`: `plugin`, `deps` and `config` as stored (not edited by the designer). */
	plugin: Record<string, unknown> | null;
}

export type DefaultMode = 'value' | 'cel';

function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The rule kinds a type allows that the designer can add (plugin rules come from packages). */
export function addableRuleKinds(type: FormFieldType): RuleKind[] {
	return (FIELD_TYPES[type]?.allowedRules ?? []).filter((kind) => kind !== 'plugin');
}

export function blankRuleDraft(kind: RuleKind): RuleDraft {
	return {
		kind,
		value: '',
		caseInsensitive: false,
		valuesText: '',
		schemes: ['https'],
		code: '',
		message: {},
		runOnEmpty: false,
		plugin: null
	};
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

function stringRecord(value: unknown): Record<string, string> {
	if (!isObject(value)) return {};
	return Object.fromEntries(
		Object.entries(value).filter((entry): entry is [string, string] => typeof entry[1] === 'string')
	);
}

/** The draft of a stored rule object; `null` for an entry that is not an object. */
export function ruleDraftFromObject(entry: unknown, type: FormFieldType): RuleDraft | null {
	if (!isObject(entry)) return null;
	const rule = entry as FormFieldRuleObject;
	const common = {
		code: typeof rule.code === 'string' ? rule.code : '',
		message: stringRecord(rule.message),
		runOnEmpty: rule.run_on_empty === true
	};
	if (typeof rule.cel === 'string') {
		return { ...blankRuleDraft('cel'), ...common, value: rule.cel };
	}
	if (isObject(rule.plugin)) {
		const plugin: Record<string, unknown> = { plugin: rule.plugin };
		if ('deps' in rule) plugin.deps = rule.deps;
		if ('config' in rule) plugin.config = rule.config;
		return { ...blankRuleDraft('plugin'), ...common, plugin };
	}
	const kind = rule.rule as RuleKind;
	const draft = { ...blankRuleDraft(kind), ...common };
	switch (kind) {
		case 'min':
		case 'max':
			return { ...draft, value: ruleBoundText(type, rule.value) };
		case 'min_length':
		case 'max_length':
			return { ...draft, value: rule.value === undefined ? '' : String(rule.value) };
		case 'pattern':
			return {
				...draft,
				value: typeof rule.value === 'string' ? rule.value : '',
				caseInsensitive: rule.case_insensitive === true
			};
		case 'enum':
			return {
				...draft,
				valuesText: Array.isArray(rule.values) ? rule.values.join('\n') : ''
			};
		case 'url':
			return { ...draft, schemes: Array.isArray(rule.schemes) ? [...rule.schemes] : ['https'] };
		default:
			return draft;
	}
}

/** The drafts of a stored `rules.validate` list (non-object entries are dropped). */
export function ruleDraftsFromRules(validate: unknown, type: FormFieldType): RuleDraft[] {
	if (!Array.isArray(validate)) return [];
	return validate
		.map((entry) => ruleDraftFromObject(entry, type))
		.filter((draft): draft is RuleDraft => draft !== null);
}

function lengthValue(text: string): unknown {
	const trimmed = text.trim();
	return /^\d+$/.test(trimmed) ? Number.parseInt(trimmed, 10) : trimmed || undefined;
}

/** The rule object of a draft, as the server reads it (also when a parameter is invalid). */
export function ruleObjectFromDraft(
	draft: RuleDraft,
	type: FormFieldType
): Record<string, unknown> {
	let rule: Record<string, unknown>;
	switch (draft.kind) {
		case 'cel':
			rule = { cel: draft.value.trim() };
			break;
		case 'plugin':
			rule = { ...(draft.plugin ?? {}) };
			break;
		case 'min':
		case 'max': {
			const bound = ruleBound(type, draft.value);
			rule = { rule: draft.kind };
			if (bound !== undefined) rule.value = bound;
			else if (draft.value.trim()) rule.value = draft.value.trim();
			break;
		}
		case 'min_length':
		case 'max_length':
			rule = { rule: draft.kind, value: lengthValue(draft.value) };
			break;
		case 'pattern':
			rule = { rule: 'pattern', value: draft.value };
			if (draft.caseInsensitive) rule.case_insensitive = true;
			break;
		case 'enum':
			rule = {
				rule: 'enum',
				values: draft.valuesText
					.split('\n')
					.map((item) => item.trim())
					.filter(Boolean)
			};
			break;
		case 'url':
			rule = { rule: 'url' };
			if (draft.schemes.length !== 1 || draft.schemes[0] !== 'https') rule.schemes = draft.schemes;
			break;
		default:
			rule = { rule: draft.kind };
	}
	if (draft.code.trim()) rule.code = draft.code.trim();
	const message = Object.fromEntries(
		Object.entries(draft.message)
			.map(([language, text]) => [language, text.trim()] as const)
			.filter(([, text]) => text)
	);
	if (Object.keys(message).length > 0) rule.message = message;
	if (draft.runOnEmpty && (draft.kind === 'cel' || draft.kind === 'plugin')) {
		rule.run_on_empty = true;
	}
	return rule;
}

const BOUND_KINDS: Partial<Record<FormFieldType, RegExp>> = {
	integer: /^-?\d+$/,
	rating: /^-?\d+$/,
	progress: /^-?\d+$/,
	number: /^-?\d+(\.\d+)?$/,
	amount: /^-?\d+(\.\d+)?$/,
	date: /^\d{4}-\d{2}-\d{2}$/,
	datetime: /^\d{4}-\d{2}-\d{2}T/
};

/**
 * The live check of a draft: the rule object through the client rule parser with the type's
 * allowed rules, plus the server's bound-fits-type check. `null` when the rule is valid.
 */
export function checkRuleDraft(draft: RuleDraft, type: FormFieldType): RuleDefError | null {
	const allowed = FIELD_TYPES[type]?.allowedRules ?? [];
	const parsed = parseRule(ruleObjectFromDraft(draft, type), allowed);
	if (!parsed.ok) return parsed.error;
	if (draft.kind === 'min' || draft.kind === 'max') {
		const pattern = BOUND_KINDS[type];
		if (!pattern || !pattern.test(String(ruleBound(type, draft.value) ?? ''))) {
			return { path: '/value', code: 'rule_param_invalid' };
		}
	}
	return null;
}

/** The live check of a CEL default expression; `null` when it compiles. */
export function checkDefaultCel(source: string): string | null {
	const result = compile(source.trim(), 'default');
	return result.ok ? null : result.code;
}
