import type { FormField, FormFieldType } from '#lib/api/forms.js';
import { fieldConditionToText, parseFieldConditionText } from '#lib/forms/field-conditions.js';
import {
	fieldArchivedOptions,
	fieldDisabledOptions,
	fieldOptions,
	sanitizeOptionColor
} from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import type { FieldDraft } from '#lib/forms/page-types.js';
import { isRecord } from '#lib/forms/record-format.js';
import { fieldDefaultCel, fieldDefaultValue, fieldReadOnly } from '#lib/forms/field-rules.js';
import {
	checkDefaultCel,
	ruleDraftsFromRules,
	ruleObjectFromDraft
} from '#lib/forms/rule-drafts.js';
import { FORM_SCHEMA_VERSION, parseSchema } from '#lib/forms/types/index.js';

/** The fields of a stored schema (`parseSchema`), for callers that need only the fields. */
export function parseFields(schema: Record<string, unknown>): FormField[] {
	return parseSchema(schema).fields;
}

/**
 * The field a draft describes, as far as the rule hints need it (type, options, amount and the
 * rules), for the designer preview.
 */
export function fieldFromDraft(draft: FieldDraft): FormField {
	return {
		field_id: draft.field_id,
		key: draft.key.trim(),
		label: draft.label.trim() || draft.key.trim(),
		type: draft.type,
		required: draft.required,
		options: parseOptionsText(draft.optionsText),
		option_disabled: parseOptionsText(draft.optionDisabledText),
		option_archived: parseOptionsText(draft.optionArchivedText),
		amount: { currency: draft.currency.trim().toUpperCase() || 'CNY' },
		rules: {
			normalize: draft.normalize,
			validate: draft.rules.map((rule) => ruleObjectFromDraft(rule, draft.type))
		}
	};
}

function draftDefaultText(value: unknown): string {
	if (value === undefined || value === null) return '';
	return typeof value === 'string' ? value : JSON.stringify(value);
}

export function blankFieldDraft(): FieldDraft {
	return {
		field_id: undefined,
		key: '',
		label: '',
		type: 'text',
		required: false,
		optionsText: '',
		optionColorsText: '',
		optionDisabledText: '',
		optionArchivedText: '',
		optionDescriptionsText: '',
		optionGroupsText: '',
		currency: 'CNY',
		scale: '2',
		placeholder: '',
		helpText: '',
		defaultMode: 'value',
		defaultValue: '',
		defaultCel: '',
		rules: [],
		normalize: [],
		rulesExtra: {},
		extra: {},
		listVisible: true,
		detailVisible: true,
		conditionalHiddenWhenText: '',
		conditionalReadOnlyWhenText: '',
		readOnly: false,
		formulaOp: 'add',
		formulaArgsText: '',
		formulaScale: '2',
		formulaRelationKey: '',
		formulaChildField: '',
		relationTargetType: 'form_record',
		relationFormKey: '',
		relationKey: 'related',
		relationType: 'relation',
		relationDisplayField: '',
		attachmentAcceptText: '',
		attachmentMaxSizeMb: '',
		attachmentMultiple: false,
		attachmentPreview: true,
		attachmentThumbnailFormat: 'png',
		attachmentPreviewFormat: 'png',
		attachmentVariantsText: '',
		attachmentStoragePolicy: 'local',
		attachmentRetentionDays: '',
		attachmentSignedUrlTtlMinutes: '10'
	};
}

export function fieldDraftsFromSchema(schema: Record<string, unknown>): FieldDraft[] {
	const parsedFields = parseFields(schema);

	return parsedFields.length > 0
		? parsedFields.map((field) => ({
				field_id: field.field_id,
				key: field.key,
				label: field.label ?? field.key,
				type: (field.type ?? 'text') as FormFieldType,
				required: Boolean(field.required),
				optionsText: fieldOptions(field).join('\n'),
				optionColorsText: optionColorsToText(field.option_colors),
				optionDisabledText: fieldDisabledOptions(field).join('\n'),
				optionArchivedText: fieldArchivedOptions(field).join('\n'),
				optionDescriptionsText: optionMetadataToText(field.option_descriptions),
				optionGroupsText: optionMetadataToText(field.option_groups),
				currency: field.amount?.currency ?? 'CNY',
				scale: String(field.amount?.scale ?? 2),
				placeholder: field.placeholder ?? '',
				helpText: field.help_text ?? '',
				defaultMode: fieldDefaultCel(field) === undefined ? 'value' : 'cel',
				defaultValue: draftDefaultText(fieldDefaultValue(field)),
				defaultCel: fieldDefaultCel(field) ?? '',
				rules: ruleDraftsFromRules(field.rules?.validate, (field.type ?? 'text') as FormFieldType),
				normalize: Array.isArray(field.rules?.normalize) ? [...field.rules.normalize] : [],
				rulesExtra: Object.fromEntries(
					Object.entries(field.rules ?? {}).filter(
						([key]) => key !== 'validate' && key !== 'normalize'
					)
				),
				extra: { ...field.extra },
				listVisible: field.visibility?.list ?? true,
				detailVisible: field.visibility?.detail ?? true,
				conditionalHiddenWhenText: fieldConditionToText(field.conditional?.hidden_when),
				conditionalReadOnlyWhenText: fieldConditionToText(field.conditional?.read_only_when),
				readOnly: fieldReadOnly(field),
				formulaOp: field.formula?.op ?? 'add',
				formulaArgsText: formulaArgsToText(field.formula?.args),
				formulaScale:
					field.formula?.scale === undefined || field.formula?.scale === null
						? '2'
						: String(field.formula.scale),
				formulaRelationKey: field.formula?.relation_key ?? '',
				formulaChildField: field.formula?.field ?? '',
				relationTargetType: field.relation?.target_type ?? 'form_record',
				relationFormKey: field.relation?.form_key ?? '',
				relationKey: field.relation?.relation_key ?? 'related',
				relationType: field.relation?.relation_type ?? 'relation',
				relationDisplayField: field.relation?.display_field ?? '',
				attachmentAcceptText: (field.attachment?.accept ?? []).join('\n'),
				attachmentMaxSizeMb:
					field.attachment?.max_size_mb === undefined || field.attachment?.max_size_mb === null
						? ''
						: String(field.attachment.max_size_mb),
				attachmentMultiple: field.attachment?.multiple ?? false,
				attachmentPreview:
					field.attachment?.preview ?? field.attachment?.thumbnail ?? field.type === 'image',
				attachmentThumbnailFormat: field.attachment?.thumbnail_format ?? 'png',
				attachmentPreviewFormat: field.attachment?.preview_format ?? 'png',
				attachmentVariantsText: attachmentVariantsToText(field.attachment?.variants),
				attachmentStoragePolicy: field.attachment?.storage_policy ?? 'local',
				attachmentRetentionDays:
					field.attachment?.retention_days === undefined ||
					field.attachment?.retention_days === null
						? ''
						: String(field.attachment.retention_days),
				attachmentSignedUrlTtlMinutes:
					field.attachment?.signed_url_ttl_minutes === undefined ||
					field.attachment?.signed_url_ttl_minutes === null
						? '10'
						: String(field.attachment.signed_url_ttl_minutes)
			}))
		: [blankFieldDraft()];
}

export function parseOptionsText(value: string): string[] {
	return value
		.split(/[\n,]/)
		.map((item) => item.trim())
		.filter(Boolean);
}

export function parseOptionColorsText(value: string): Record<string, string> {
	const colors: Record<string, string> = {};

	for (const line of value.split('\n')) {
		const trimmed = line.trim();

		if (!trimmed) continue;

		const [rawOption, ...rawColorParts] = trimmed.split(/[:=]/);
		const option = rawOption?.trim();
		const color = sanitizeOptionColor(rawColorParts.join(':').trim());

		if (option && color) colors[option] = color;
	}

	return colors;
}

export function parseOptionMetadataText(value: string): Record<string, string> {
	const metadata: Record<string, string> = {};

	for (const line of value.split('\n')) {
		const trimmed = line.trim();

		if (!trimmed) continue;

		const separatorIndex = trimmed.search(/[:=]/);

		if (separatorIndex <= 0) continue;

		const option = trimmed.slice(0, separatorIndex).trim();
		const text = trimmed.slice(separatorIndex + 1).trim();

		if (option && text) metadata[option] = text;
	}

	return metadata;
}

export function optionColorsToText(value: unknown): string {
	if (!isRecord(value)) return '';

	return Object.entries(value)
		.filter(
			(entry): entry is [string, string] =>
				typeof entry[1] === 'string' && Boolean(sanitizeOptionColor(entry[1]))
		)
		.map(([option, color]) => `${option}: ${sanitizeOptionColor(color)}`)
		.join('\n');
}

export function optionMetadataToText(value: unknown): string {
	if (!isRecord(value)) return '';

	return Object.entries(value)
		.filter(
			(entry): entry is [string, string] => typeof entry[1] === 'string' && Boolean(entry[1].trim())
		)
		.map(([option, text]) => `${option}: ${text.trim()}`)
		.join('\n');
}

export function sanitizeAttachmentDerivativeFormat(value: string | undefined | null): string {
	const normalized = (value ?? '').trim().toLowerCase();

	if (normalized === 'jpg' || normalized === 'image/jpg' || normalized === 'image/jpeg')
		return 'jpeg';
	if (normalized === 'jpeg' || normalized === 'png' || normalized === 'webp') return normalized;

	return 'png';
}

export function attachmentVariantsToText(value: unknown): string {
	if (!Array.isArray(value)) return '';

	return value
		.map((variant) => {
			if (!isRecord(variant) || typeof variant.kind !== 'string') return '';

			const kind = variant.kind.trim();

			if (!kind) return '';

			const maxDimension =
				typeof variant.max_dimension === 'number' && Number.isFinite(variant.max_dimension)
					? String(variant.max_dimension)
					: '';

			const format =
				typeof variant.format === 'string'
					? sanitizeAttachmentDerivativeFormat(variant.format)
					: 'png';

			return [kind, maxDimension, format].filter(Boolean).join(':');
		})
		.filter(Boolean)
		.join('\n');
}

export function parseAttachmentVariantsText(
	value: string
): Array<{ kind: string; max_dimension?: number; format?: string }> {
	return value
		.split('\n')
		.map((line) => line.trim())
		.filter(Boolean)
		.map((line) => {
			const [rawKind, rawMaxDimension = '', rawFormat = ''] = line
				.split(':')
				.map((part) => part.trim());
			const variant: { kind: string; max_dimension?: number; format?: string } = {
				kind: rawKind.toLowerCase().replace(/[\s.]+/g, '-')
			};
			const maxDimension = Number.parseInt(rawMaxDimension, 10);

			if (Number.isFinite(maxDimension) && maxDimension > 0) variant.max_dimension = maxDimension;

			const format = sanitizeAttachmentDerivativeFormat(rawFormat);

			if (format !== 'png') variant.format = format;

			return variant;
		})
		.filter((variant) => /^[a-z0-9_-]{1,32}$/.test(variant.kind));
}

export function formulaArgsToText(args: unknown[] | undefined): string {
	if (!Array.isArray(args)) return '';

	return args
		.map((arg) => {
			if (typeof arg === 'string') return arg;
			if (isRecord(arg) && typeof arg.field === 'string') return arg.field;
			if (isRecord(arg) && typeof arg.value === 'string') return arg.value;

			return JSON.stringify(arg);
		})
		.join('\n');
}

/**
 * Splits one line on commas that are outside JSON objects, arrays and strings. A line whose
 * brackets or quotes never close is split on every comma instead.
 */
function splitFormulaArgsLine(line: string): string[] {
	const parts: string[] = [];
	let current = '';
	let depth = 0;
	let inString = false;
	let escaped = false;

	for (const char of line) {
		if (inString) {
			current += char;
			if (escaped) escaped = false;
			else if (char === '\\') escaped = true;
			else if (char === '"') inString = false;
			continue;
		}

		if (char === ',' && depth === 0) {
			parts.push(current);
			current = '';
			continue;
		}

		if (char === '"' && depth > 0) inString = true;
		else if (char === '{' || char === '[') depth += 1;
		else if ((char === '}' || char === ']') && depth > 0) depth -= 1;

		current += char;
	}

	if (depth > 0 || inString) return line.split(',');

	parts.push(current);

	return parts;
}

export function parseFormulaArgsText(value: string): unknown[] {
	return value
		.split('\n')
		.flatMap(splitFormulaArgsLine)
		.map((item) => item.trim())
		.filter(Boolean)
		.map((item) => {
			if (item.startsWith('{')) {
				try {
					return JSON.parse(item) as unknown;
				} catch {
					return item;
				}
			}

			return item;
		});
}

export function parseValidationBoundary(value: string): string | number {
	const numberValue = Number(value);

	return Number.isFinite(numberValue) && value.trim() !== '' ? numberValue : value;
}

export function parseFieldDefaultValue(value: string, type: FormFieldType): unknown {
	const trimmed = value.trim();

	if (trimmed === '') return undefined;
	if (type === 'boolean') return trimmed === 'true' || trimmed === 'yes' || trimmed === '1';

	if (type === 'integer') {
		const integerValue = Number.parseInt(trimmed, 10);

		return Number.isFinite(integerValue) ? integerValue : trimmed;
	}

	if (type === 'number') {
		const numberValue = Number(trimmed);

		return Number.isFinite(numberValue) ? numberValue : trimmed;
	}

	if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
		try {
			return JSON.parse(trimmed) as unknown;
		} catch {
			return trimmed;
		}
	}

	return trimmed;
}

export function parseSelectDefaultValue(
	value: string,
	type: FormFieldType,
	options: string[],
	disabledOptions: string[],
	archivedOptions: string[]
): unknown {
	const trimmed = value.trim();

	if (!trimmed) return undefined;

	const values = type === 'multi_select' ? parseDefaultMultiSelectValue(trimmed) : [trimmed];

	for (const item of values) {
		if (!options.includes(item)) {
			throw new Error(translate('forms.defaultOptionInvalid', { values: { option: item } }));
		}

		if (disabledOptions.includes(item)) {
			throw new Error(translate('forms.defaultOptionDisabled', { values: { option: item } }));
		}

		if (archivedOptions.includes(item)) {
			throw new Error(translate('forms.defaultOptionArchived', { values: { option: item } }));
		}
	}

	return type === 'multi_select' ? values : values[0];
}

export function parseDefaultMultiSelectValue(value: string): string[] {
	if (value.startsWith('[')) {
		try {
			const parsed = JSON.parse(value) as unknown;

			if (Array.isArray(parsed)) {
				return parsed.filter(
					(item): item is string => typeof item === 'string' && item.trim().length > 0
				);
			}
		} catch {
			// Fall through to delimiter parsing so the designer can recover from partial JSON.
		}
	}

	return value
		.split(/[,\n]/)
		.map((item) => item.trim())
		.filter(Boolean);
}

export function fieldIdentity(field: { field_id?: string; key: string }): string {
	return field.field_id || field.key;
}

/**
 * The schema of the field drafts. Members the designer does not edit are written back as stored:
 * `schemaExtra` at schema level, each draft's `extra` and `rulesExtra` per field. Throws a
 * localised message when a draft cannot be saved.
 */
export function schemaFromDrafts(
	drafts: FieldDraft[],
	schemaExtra: Record<string, unknown> = {}
): Record<string, unknown> {
	const fields = drafts
		.map((field) => ({
			field_id: field.field_id,
			key: field.key.trim(),
			label: field.label.trim(),
			type: field.type,
			required: field.required,
			options: parseOptionsText(field.optionsText),
			optionColors: parseOptionColorsText(field.optionColorsText),
			optionDisabled: parseOptionsText(field.optionDisabledText),
			optionArchived: parseOptionsText(field.optionArchivedText),
			optionDescriptions: parseOptionMetadataText(field.optionDescriptionsText),
			optionGroups: parseOptionMetadataText(field.optionGroupsText),
			currency: field.currency.trim().toUpperCase() || 'CNY',
			scale: Number.parseInt(field.scale, 10),
			placeholder: field.placeholder.trim(),
			helpText: field.helpText.trim(),
			defaultMode: field.defaultMode,
			defaultValue: field.defaultValue.trim(),
			defaultCel: field.defaultCel.trim(),
			rules: field.rules,
			normalize: field.normalize,
			rulesExtra: field.rulesExtra,
			extra: field.extra,
			listVisible: field.listVisible,
			detailVisible: field.detailVisible,
			conditionalHiddenWhen: parseFieldConditionText(field.conditionalHiddenWhenText),
			conditionalReadOnlyWhen: parseFieldConditionText(field.conditionalReadOnlyWhenText),
			readOnly: field.readOnly,
			formulaOp: field.formulaOp.trim(),
			formulaArgs: parseFormulaArgsText(field.formulaArgsText),
			formulaScale: Number.parseInt(field.formulaScale, 10),
			formulaRelationKey: field.formulaRelationKey.trim(),
			formulaChildField: field.formulaChildField.trim(),
			relationTargetType: field.relationTargetType.trim() || 'form_record',
			relationFormKey: field.relationFormKey.trim(),
			relationKey: field.relationKey.trim(),
			relationType: field.relationType.trim(),
			relationDisplayField: field.relationDisplayField.trim(),
			attachmentAccept: parseOptionsText(field.attachmentAcceptText),
			attachmentMaxSizeMb: Number.parseInt(field.attachmentMaxSizeMb, 10),
			attachmentMultiple: field.attachmentMultiple,
			attachmentPreview: field.attachmentPreview,
			attachmentThumbnailFormat: sanitizeAttachmentDerivativeFormat(
				field.attachmentThumbnailFormat
			),
			attachmentPreviewFormat: sanitizeAttachmentDerivativeFormat(field.attachmentPreviewFormat),
			attachmentVariants: parseAttachmentVariantsText(field.attachmentVariantsText),
			attachmentStoragePolicy: field.attachmentStoragePolicy.trim() || 'local',
			attachmentRetentionDays: Number.parseInt(field.attachmentRetentionDays, 10),
			attachmentSignedUrlTtlMinutes: Number.parseInt(field.attachmentSignedUrlTtlMinutes, 10)
		}))
		.filter((field) => field.key || field.label);

	if (fields.length === 0) {
		throw new Error(
			translate('forms.fieldRequired', { values: { field: translate('forms.fields') } })
		);
	}

	const schemaFields = fields.map((field) => {
		if (!field.key.match(/^[a-z][a-z0-9_]*$/)) {
			throw new Error(translate('forms.invalidFieldKey'));
		}

		const schemaField: Record<string, unknown> = {
			...field.extra,
			key: field.key,
			label: field.label || field.key,
			type: field.type,
			required: field.required
		};

		if (field.field_id) schemaField.field_id = field.field_id;

		if (['single_select', 'multi_select'].includes(field.type)) {
			if (field.options.length === 0) throw new Error(translate('forms.optionsRequired'));

			schemaField.options = field.options;

			const optionColors = Object.fromEntries(
				Object.entries(field.optionColors).filter(
					([option, color]) => field.options.includes(option) && Boolean(color)
				)
			);

			if (Object.keys(optionColors).length > 0) schemaField.option_colors = optionColors;

			const optionDisabled = field.optionDisabled.filter((option) =>
				field.options.includes(option)
			);

			if (optionDisabled.length > 0) schemaField.option_disabled = optionDisabled;

			const optionArchived = field.optionArchived.filter((option) =>
				field.options.includes(option)
			);

			if (optionArchived.length > 0) schemaField.option_archived = optionArchived;

			const optionDescriptions = Object.fromEntries(
				Object.entries(field.optionDescriptions).filter(
					([option, description]) => field.options.includes(option) && Boolean(description)
				)
			);

			if (Object.keys(optionDescriptions).length > 0)
				schemaField.option_descriptions = optionDescriptions;

			const optionGroups = Object.fromEntries(
				Object.entries(field.optionGroups).filter(
					([option, group]) => field.options.includes(option) && Boolean(group)
				)
			);

			if (Object.keys(optionGroups).length > 0) schemaField.option_groups = optionGroups;

			if (field.defaultMode === 'value' && field.defaultValue) {
				schemaField.default = {
					value: parseSelectDefaultValue(
						field.defaultValue,
						field.type,
						field.options,
						optionDisabled,
						optionArchived
					)
				};
			}
		}

		if (field.type === 'amount') {
			schemaField.amount = {
				currency: field.currency,
				scale: Number.isFinite(field.scale) ? Math.min(Math.max(field.scale, 0), 8) : 2
			};
		}

		if (field.placeholder) schemaField.placeholder = field.placeholder;
		if (field.helpText) schemaField.help_text = field.helpText;

		if (
			field.defaultMode === 'value' &&
			field.defaultValue &&
			!['single_select', 'multi_select'].includes(field.type)
		) {
			schemaField.default = { value: parseFieldDefaultValue(field.defaultValue, field.type) };
		}

		if (field.defaultMode === 'cel' && field.defaultCel) {
			const code = checkDefaultCel(field.defaultCel);

			if (code) {
				throw new Error(
					translate('lowcode.forms.designer.defaultCelInvalid', {
						values: { field: field.label || field.key, code }
					})
				);
			}

			schemaField.default = { cel: field.defaultCel };
		}

		const rules: Record<string, unknown> = { ...field.rulesExtra };

		if (field.normalize.length > 0) rules.normalize = field.normalize;
		if (field.rules.length > 0) {
			rules.validate = field.rules.map((rule) => ruleObjectFromDraft(rule, field.type));
		}
		if (Object.keys(rules).length > 0) schemaField.rules = rules;

		if (!field.listVisible || !field.detailVisible) {
			schemaField.visibility = { list: field.listVisible, detail: field.detailVisible };
		}

		const conditional: Record<string, unknown> = {};

		if (field.conditionalHiddenWhen) conditional.hidden_when = field.conditionalHiddenWhen;
		if (field.conditionalReadOnlyWhen) conditional.read_only_when = field.conditionalReadOnlyWhen;
		if (Object.keys(conditional).length > 0) schemaField.conditional = conditional;
		if (field.readOnly) schemaField.read_only = true;

		const isFormulaCapableField = ['formula', 'amount', 'number', 'integer'].includes(field.type);
		const isChildFormula = field.formulaOp.startsWith('child_');
		const hasChildFormulaConfig = isChildFormula && field.formulaRelationKey;
		const hasRegularFormulaConfig = !isChildFormula && field.formulaArgs.length > 0;

		if (
			isFormulaCapableField &&
			field.formulaOp &&
			(hasChildFormulaConfig || hasRegularFormulaConfig)
		) {
			const formula: Record<string, unknown> = { op: field.formulaOp };

			if (isChildFormula) {
				formula.relation_key = field.formulaRelationKey;

				if (field.formulaChildField) formula.field = field.formulaChildField;
			} else {
				formula.args = field.formulaArgs;
			}

			if (field.type === 'formula' && Number.isFinite(field.formulaScale)) {
				formula.scale = Math.min(Math.max(field.formulaScale, 0), 8);
			}

			schemaField.formula = formula;
		}

		if (field.type === 'relation' || field.type === 'child_table') {
			const relation: Record<string, unknown> = {
				target_type: field.type === 'child_table' ? 'form_record' : field.relationTargetType
			};

			if (field.relationFormKey) relation.form_key = field.relationFormKey;
			if (field.relationKey) relation.relation_key = field.relationKey;

			relation.relation_type =
				field.type === 'child_table' ? 'parent_child' : field.relationType || 'relation';

			if (field.relationDisplayField) relation.display_field = field.relationDisplayField;

			schemaField.relation = relation;
		}

		if (['attachment', 'image'].includes(field.type)) {
			const attachment: Record<string, unknown> = {
				multiple: field.attachmentMultiple,
				preview: field.attachmentPreview
			};

			if (field.attachmentThumbnailFormat !== 'png') {
				attachment.thumbnail_format = field.attachmentThumbnailFormat;
			}

			if (field.attachmentPreviewFormat !== 'png') {
				attachment.preview_format = field.attachmentPreviewFormat;
			}

			if (field.attachmentVariants.length > 0) {
				attachment.variants = field.attachmentVariants;
			}

			if (field.attachmentAccept.length > 0) attachment.accept = field.attachmentAccept;

			if (Number.isFinite(field.attachmentMaxSizeMb) && field.attachmentMaxSizeMb > 0) {
				attachment.max_size_mb = field.attachmentMaxSizeMb;
			}

			attachment.storage_policy = field.attachmentStoragePolicy;

			if (Number.isFinite(field.attachmentRetentionDays) && field.attachmentRetentionDays > 0) {
				attachment.retention_days = field.attachmentRetentionDays;
			}

			if (
				Number.isFinite(field.attachmentSignedUrlTtlMinutes) &&
				field.attachmentSignedUrlTtlMinutes > 0
			) {
				attachment.signed_url_ttl_minutes = field.attachmentSignedUrlTtlMinutes;
			}

			if (field.type === 'image') attachment.thumbnail = field.attachmentPreview;

			schemaField.attachment = attachment;
		}

		return schemaField;
	});

	return { ...schemaExtra, version: FORM_SCHEMA_VERSION, fields: schemaFields };
}
