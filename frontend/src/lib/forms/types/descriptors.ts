/**
 * Field type descriptors: the client mirror of `apps/api/src/forms/types` (contract C2 §4.3).
 * One entry per type with its storage, CEL type, allowed rules and ops, the type check and
 * canonical form of a value, the operand built-in rules compare and the CEL binding. The server
 * stays the authority; the designer and the record editor use this table for their hints, and dev
 * builds compare it with `GET /api/v1/field-types` once (`descriptorDrift`).
 */

import type { FieldTypeDocument, FormField, FormFieldType } from '#lib/api/forms.js';
import type { Operand, RuleKind } from '#lib/forms/rules/index.js';
import { parseRfc3339 } from '#lib/forms/rules/text.js';

export type StorageKind =
	| 'text'
	| 'number'
	| 'bool'
	| 'date'
	| 'datetime'
	| 'json'
	| 'ref'
	| 'blob';
export type CelType = 'string' | 'int' | 'bool' | 'timestamp' | 'list_string';

export type NormalizeResult = { ok: true; value: unknown } | { ok: false; code: string };

export interface FieldTypeDescriptor {
	name: FormFieldType;
	storage: StorageKind;
	celType: CelType | null;
	allowedRules: readonly RuleKind[];
	normalizeOps: readonly string[];
	transformOps: readonly string[];
	/** Whether `mask` may be declared (text-like types). */
	maskable: boolean;
	/** Whether the server writes the value (formula output, autonumber, AI summary). */
	serverWritten: boolean;
	/** Type check and canonical form; `prior` is the stored value (option carry-over). */
	normalize(field: FormField, raw: unknown, prior?: unknown): NormalizeResult;
	/** Operand of `min` / `max` / length rules for a canonical value; `null` = not comparable. */
	operand(field: FormField, value: unknown): Operand | null;
	/** The CEL binding of a canonical value (contract C2 §5.3); `undefined` = not exposed. */
	celValue(field: FormField, value: unknown): unknown;
}

const TEXT_NORMALIZE_OPS = ['trim', 'collapse_spaces', 'lower', 'upper', 'nfc'] as const;
const TEXT_RULES: readonly RuleKind[] = [
	'min_length',
	'max_length',
	'pattern',
	'enum',
	'email',
	'url',
	'cel',
	'plugin'
];
const ORDERED_RULES: readonly RuleKind[] = ['min', 'max', 'cel', 'plugin'];

const mismatch: NormalizeResult = { ok: false, code: 'type_mismatch' };

function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function stringValue(raw: unknown): NormalizeResult {
	return typeof raw === 'string' ? { ok: true, value: raw } : mismatch;
}

function textOperand(value: unknown): Operand | null {
	return typeof value === 'string' ? { kind: 'str', value } : null;
}

function textCel(value: unknown): unknown {
	return typeof value === 'string' ? value : undefined;
}

const DECIMAL_TEXT = /^([+-]?)(\d*)(?:\.(\d+))?$/;

/** A decimal string in the canonical form `rust_decimal` prints (`-?[0-9]+(\.[0-9]+)?`). */
function canonicalDecimal(text: string): string | null {
	const match = DECIMAL_TEXT.exec(text.trim());
	if (!match || (match[2] === '' && match[3] === undefined)) return null;
	const int = match[2].replace(/^0+(?=\d)/, '') || '0';
	const body = match[3] === undefined ? int : `${int}.${match[3]}`;
	const zero = /^0(\.0+)?$/.test(body);
	return match[1] === '-' && !zero ? `-${body}` : body;
}

/** The decimal of a stored `{decimal}` value without trailing zeros (`Decimal::normalize`). */
function storedDecimal(value: unknown): string | null {
	if (!isObject(value) || typeof value.decimal !== 'string') return null;
	return canonicalDecimal(value.decimal);
}

function trimFraction(decimal: string): string {
	return decimal.includes('.') ? decimal.replace(/\.?0+$/, '') : decimal;
}

function integerInput(raw: unknown): bigint | null {
	const source = isObject(raw) && 'value' in raw ? raw.value : raw;
	if (typeof source === 'number') return Number.isSafeInteger(source) ? BigInt(source) : null;
	if (typeof source === 'string' && /^[+-]?\d+$/.test(source.trim())) {
		return BigInt(source.trim());
	}
	return null;
}

function storedInteger(value: unknown): bigint | null {
	if (!isObject(value) || typeof value.value !== 'number') return null;
	return Number.isSafeInteger(value.value) ? BigInt(value.value) : null;
}

const LOCAL_DATE_TIME = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2})(:\d{2})?$/;

/** An RFC 3339 date-time, or a zone-less one read as UTC (a `datetime-local` value). */
export function dateTimeText(text: string): string | null {
	const trimmed = text.trim();
	if (parseRfc3339(trimmed) !== null) return trimmed;
	const local = LOCAL_DATE_TIME.exec(trimmed);
	if (!local) return null;
	const utc = `${local[1]}${local[2] ?? ':00'}Z`;
	return parseRfc3339(utc) === null ? null : utc;
}

function checkOption(field: FormField, value: string, prior: unknown): string | null {
	const options = field.options ?? [];
	if (options.length > 0 && !options.includes(value)) return 'option_unsupported';
	const disabled = (field.option_disabled ?? []).includes(value);
	const archived = (field.option_archived ?? []).includes(value);
	if (!disabled && !archived) return null;
	const existed = prior === value || (Array.isArray(prior) && prior.some((item) => item === value));
	if (existed) return null;
	return archived ? 'option_archived' : 'option_disabled';
}

interface Shape {
	storage: StorageKind;
	celType: CelType | null;
	allowedRules: readonly RuleKind[];
	normalizeOps?: readonly string[];
	transformOps?: readonly string[];
	maskable?: boolean;
	serverWritten?: boolean;
	normalize?: FieldTypeDescriptor['normalize'];
	operand?: FieldTypeDescriptor['operand'];
	celValue?: FieldTypeDescriptor['celValue'];
}

function descriptor(name: FormFieldType, shape: Shape): FieldTypeDescriptor {
	return {
		name,
		storage: shape.storage,
		celType: shape.celType,
		allowedRules: shape.allowedRules,
		normalizeOps: shape.normalizeOps ?? [],
		transformOps: shape.transformOps ?? [],
		maskable: shape.maskable ?? false,
		serverWritten: shape.serverWritten ?? false,
		normalize: shape.normalize ?? ((_field, raw) => ({ ok: true, value: raw })),
		operand: shape.operand ?? (() => null),
		celValue: shape.celValue ?? (() => undefined)
	};
}

function textLike(name: FormFieldType, transformOps: readonly string[]): FieldTypeDescriptor {
	return descriptor(name, {
		storage: 'text',
		celType: 'string',
		allowedRules: TEXT_RULES,
		normalizeOps: TEXT_NORMALIZE_OPS,
		transformOps,
		maskable: true,
		normalize: (_field, raw) => stringValue(raw),
		operand: (_field, value) => textOperand(value),
		celValue: (_field, value) => textCel(value)
	});
}

function systemString(name: FormFieldType): FieldTypeDescriptor {
	return descriptor(name, {
		storage: 'text',
		celType: null,
		allowedRules: [],
		serverWritten: true,
		normalize: (_field, raw) => stringValue(raw)
	});
}

function integerType(name: FormFieldType, range: [number, number] | null): FieldTypeDescriptor {
	return descriptor(name, {
		storage: 'number',
		celType: 'int',
		allowedRules: ORDERED_RULES,
		normalize: (_field, raw) => {
			const value = integerInput(raw);
			if (value === null) return mismatch;
			if (range && (value < BigInt(range[0]) || value > BigInt(range[1]))) return mismatch;
			return { ok: true, value: { type: name, value: Number(value) } };
		},
		operand: (_field, value) => {
			const int = storedInteger(value);
			return int === null ? null : { kind: 'int', value: int };
		},
		celValue: (_field, value) => {
			const int = storedInteger(value);
			return int === null ? undefined : Number(int);
		}
	});
}

function decimalType(name: 'number' | 'amount'): FieldTypeDescriptor {
	return descriptor(name, {
		storage: 'number',
		celType: 'string',
		allowedRules: ORDERED_RULES,
		transformOps: ['round'],
		// Amount rounding to the field's scale stays on the server; the hint compares the sent value.
		normalize: (field, raw) => {
			if (typeof raw === 'number') return mismatch;
			const text =
				typeof raw === 'string'
					? raw
					: isObject(raw) && typeof raw.decimal === 'string'
						? raw.decimal
						: null;
			const decimal = text === null ? null : canonicalDecimal(text);
			if (decimal === null) return mismatch;
			if (name === 'number') return { ok: true, value: { type: 'decimal', decimal } };
			const currency =
				isObject(raw) && typeof raw.currency === 'string'
					? raw.currency.toUpperCase()
					: (field.amount?.currency ?? 'CNY').toUpperCase();
			if (currency !== (field.amount?.currency ?? 'CNY').toUpperCase()) return mismatch;
			return { ok: true, value: { type: 'amount', decimal, currency } };
		},
		operand: (_field, value) => {
			const decimal = storedDecimal(value);
			return decimal === null ? null : { kind: 'decimal', value: decimal };
		},
		celValue: (_field, value) => {
			const decimal = storedDecimal(value);
			return decimal === null ? undefined : trimFraction(decimal);
		}
	});
}

function opaque(
	name: FormFieldType,
	storage: StorageKind,
	serverWritten = false
): FieldTypeDescriptor {
	return descriptor(name, { storage, celType: null, allowedRules: [], serverWritten });
}

/** Every field type, keyed by name; a type added to `FormFieldType` without an entry fails to compile. */
export const FIELD_TYPES: Readonly<Record<FormFieldType, FieldTypeDescriptor>> = {
	text: textLike('text', ['slug', 'truncate']),
	textarea: textLike('textarea', ['truncate']),
	phone: textLike('phone', ['truncate']),
	address: textLike('address', ['truncate']),
	scan: textLike('scan', ['truncate']),
	email: descriptor('email', {
		storage: 'text',
		celType: 'string',
		allowedRules: TEXT_RULES,
		normalizeOps: TEXT_NORMALIZE_OPS,
		transformOps: ['truncate'],
		maskable: true,
		normalize: (_field, raw) => {
			if (typeof raw !== 'string') return mismatch;
			const value = raw.trim();
			if (!value.includes('@') || value.startsWith('@') || value.endsWith('@')) return mismatch;
			return { ok: true, value };
		},
		operand: (_field, value) => textOperand(value),
		celValue: (_field, value) => textCel(value)
	}),
	rich_text: descriptor('rich_text', {
		storage: 'text',
		celType: 'string',
		allowedRules: ['min_length', 'max_length', 'cel', 'plugin'],
		normalizeOps: ['nfc'],
		normalize: (_field, raw) => stringValue(raw),
		operand: (_field, value) => textOperand(value),
		celValue: (_field, value) => textCel(value)
	}),
	signature: descriptor('signature', {
		storage: 'blob',
		celType: null,
		allowedRules: [],
		normalize: (_field, raw) => stringValue(raw)
	}),
	autonumber: systemString('autonumber'),
	ai_summary: systemString('ai_summary'),
	number: decimalType('number'),
	amount: decimalType('amount'),
	integer: integerType('integer', null),
	rating: integerType('rating', [1, 5]),
	progress: integerType('progress', [0, 100]),
	date: descriptor('date', {
		storage: 'date',
		celType: 'string',
		allowedRules: ORDERED_RULES,
		normalize: (_field, raw) => stringValue(raw),
		operand: (_field, value) =>
			typeof value === 'string' ? { kind: 'date', value: value.trim() } : null,
		celValue: (_field, value) => textCel(value)
	}),
	datetime: descriptor('datetime', {
		storage: 'datetime',
		celType: 'timestamp',
		allowedRules: ORDERED_RULES,
		normalize: (_field, raw) => stringValue(raw),
		operand: (_field, value) => {
			const text = typeof value === 'string' ? dateTimeText(value) : null;
			return text === null ? null : { kind: 'datetime', value: text };
		},
		celValue: (_field, value) => {
			const text = typeof value === 'string' ? dateTimeText(value) : null;
			const nanos = text === null ? null : parseRfc3339(text);
			return nanos === null ? undefined : Number(nanos / 1_000_000_000n);
		}
	}),
	single_select: descriptor('single_select', {
		storage: 'text',
		celType: 'string',
		allowedRules: ['enum', 'cel', 'plugin'],
		normalize: (field, raw, prior) => {
			if (typeof raw !== 'string') return mismatch;
			const code = checkOption(field, raw, prior);
			return code ? { ok: false, code } : { ok: true, value: raw };
		},
		operand: (_field, value) => textOperand(value),
		celValue: (_field, value) => textCel(value)
	}),
	multi_select: descriptor('multi_select', {
		storage: 'json',
		celType: 'list_string',
		allowedRules: ['min_length', 'max_length', 'enum', 'cel', 'plugin'],
		normalize: (field, raw, prior) => {
			if (!Array.isArray(raw)) return mismatch;
			for (const item of raw) {
				if (typeof item !== 'string') return mismatch;
				const code = checkOption(field, item, prior);
				if (code) return { ok: false, code };
			}
			return { ok: true, value: [...raw] };
		},
		operand: (_field, value) =>
			Array.isArray(value) ? { kind: 'count', value: value.length } : null,
		celValue: (_field, value) =>
			Array.isArray(value) && value.every((item) => typeof item === 'string')
				? [...value]
				: undefined
	}),
	boolean: descriptor('boolean', {
		storage: 'bool',
		celType: 'bool',
		allowedRules: ['cel', 'plugin'],
		normalize: (_field, raw) => (typeof raw === 'boolean' ? { ok: true, value: raw } : mismatch),
		celValue: (_field, value) => (typeof value === 'boolean' ? value : undefined)
	}),
	member: descriptor('member', {
		storage: 'ref',
		celType: 'string',
		allowedRules: ['cel', 'plugin'],
		normalize: (_field, raw) => {
			const id = typeof raw === 'string' ? raw : isObject(raw) ? raw.user_id : null;
			if (typeof id !== 'string' || !id.trim()) return mismatch;
			return { ok: true, value: { ...(isObject(raw) ? raw : {}), user_id: id.trim() } };
		},
		celValue: (_field, value) =>
			isObject(value) && typeof value.user_id === 'string' ? value.user_id : undefined
	}),
	location: opaque('location', 'json'),
	attachment: opaque('attachment', 'blob'),
	image: opaque('image', 'blob'),
	relation: opaque('relation', 'ref'),
	child_table: opaque('child_table', 'ref'),
	formula: opaque('formula', 'json', true)
};

/** The descriptor of a field (`text` when the type is absent or unknown). */
export function fieldTypeDescriptor(field: Pick<FormField, 'type'>): FieldTypeDescriptor {
	return FIELD_TYPES[field.type ?? 'text'] ?? FIELD_TYPES.text;
}

function sameList(a: readonly string[], b: readonly string[]): boolean {
	return a.length === b.length && a.every((item, index) => item === b[index]);
}

/**
 * Differences between this table and `GET /api/v1/field-types` (names, allowed rules, CEL type),
 * one line each; empty when they agree.
 */
export function descriptorDrift(document: FieldTypeDocument): string[] {
	const drift: string[] = [];
	const remote = new Map(document.types.map((entry) => [entry.name, entry]));
	for (const local of Object.values(FIELD_TYPES)) {
		const entry = remote.get(local.name);
		if (!entry) {
			drift.push(`${local.name}: missing on the server`);
			continue;
		}
		if (!sameList(local.allowedRules, entry.allowed_rules)) {
			drift.push(`${local.name}: allowed rules differ`);
		}
		if (local.celType !== entry.cel_type) drift.push(`${local.name}: CEL type differs`);
	}
	for (const name of remote.keys()) {
		if (!(name in FIELD_TYPES)) drift.push(`${name}: missing in the client`);
	}
	return drift;
}
