/**
 * The form schema parser (`sv.form.schema.v2`, contract C2 §2): the one place that turns a stored
 * schema into typed fields. Members the client does not model are kept in `extra` (per schema and
 * per field), so a designer save writes them back unchanged.
 */

import type { FormField, FormFieldDefault, FormFieldRules, FormFieldType } from '#lib/api/forms.js';

export const FORM_SCHEMA_VERSION = 'sv.form.schema.v2';

export interface FormSchemaV2 {
	version: string;
	fields: FormField[];
	/** Schema-level members other than `version` and `fields`. */
	extra: Record<string, unknown>;
}

/** Field members the client models; every other member of a field object lands in `extra`. */
export const MODELED_FIELD_KEYS: ReadonlySet<string> = new Set([
	'field_id',
	'key',
	'label',
	'type',
	'required',
	'options',
	'option_colors',
	'option_disabled',
	'option_archived',
	'option_descriptions',
	'option_groups',
	'amount',
	'placeholder',
	'help_text',
	'read_only',
	'default',
	'rules',
	'visibility',
	'conditional',
	'formula',
	'relation',
	'attachment'
]);

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function stringList(value: unknown): string[] | undefined {
	return Array.isArray(value)
		? value.filter((item): item is string => typeof item === 'string')
		: undefined;
}

function stringMap(value: unknown): Record<string, string> | undefined {
	return isRecord(value)
		? Object.fromEntries(
				Object.entries(value).filter(
					(entry): entry is [string, string] => typeof entry[1] === 'string'
				)
			)
		: undefined;
}

function parseDefault(value: unknown): FormFieldDefault | undefined {
	if (!isRecord(value)) return undefined;
	if (typeof value.cel === 'string') return { cel: value.cel };
	if ('value' in value) return { value: value.value };
	return undefined;
}

/** The members of `raw` that are not in `modeled`, in their original order. */
export function unmodeledMembers(
	raw: Record<string, unknown>,
	modeled: ReadonlySet<string>
): Record<string, unknown> {
	return Object.fromEntries(Object.entries(raw).filter(([key]) => !modeled.has(key)));
}

function parseField(raw: Record<string, unknown>): FormField {
	const rawType = typeof raw.type === 'string' ? raw.type : 'text';
	const extra = unmodeledMembers(raw, MODELED_FIELD_KEYS);

	return {
		field_id: typeof raw.field_id === 'string' ? raw.field_id : undefined,
		key: typeof raw.key === 'string' ? raw.key : '',
		label: typeof raw.label === 'string' ? raw.label : undefined,
		type: (rawType === 'select' ? 'single_select' : rawType) as FormFieldType,
		required: Boolean(raw.required),
		options: stringList(raw.options) ?? [],
		option_colors: stringMap(raw.option_colors),
		option_disabled: stringList(raw.option_disabled),
		option_archived: stringList(raw.option_archived),
		option_descriptions: stringMap(raw.option_descriptions),
		option_groups: stringMap(raw.option_groups),
		amount: isRecord(raw.amount) ? (raw.amount as FormField['amount']) : undefined,
		placeholder: typeof raw.placeholder === 'string' ? raw.placeholder : undefined,
		help_text: typeof raw.help_text === 'string' ? raw.help_text : undefined,
		read_only: raw.read_only === true ? true : undefined,
		default: parseDefault(raw.default),
		rules: isRecord(raw.rules) ? (raw.rules as FormFieldRules) : undefined,
		visibility: isRecord(raw.visibility) ? (raw.visibility as FormField['visibility']) : undefined,
		conditional: isRecord(raw.conditional)
			? (raw.conditional as FormField['conditional'])
			: undefined,
		formula: isRecord(raw.formula) ? (raw.formula as FormField['formula']) : undefined,
		relation: isRecord(raw.relation) ? (raw.relation as FormField['relation']) : undefined,
		attachment: isRecord(raw.attachment) ? (raw.attachment as FormField['attachment']) : undefined,
		extra: Object.keys(extra).length > 0 ? extra : undefined
	};
}

/** Parses a stored schema; fields without a key are dropped, unknown members are kept. */
export function parseSchema(raw: unknown): FormSchemaV2 {
	const schema = isRecord(raw) ? raw : {};
	const fields = Array.isArray(schema.fields) ? schema.fields : [];

	return {
		version: typeof schema.version === 'string' ? schema.version : FORM_SCHEMA_VERSION,
		fields: fields
			.filter(isRecord)
			.map(parseField)
			.filter((field) => field.key),
		extra: unmodeledMembers(schema, new Set(['version', 'fields']))
	};
}
