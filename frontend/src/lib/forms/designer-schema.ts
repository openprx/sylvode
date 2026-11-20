import type { FormField, FormFieldType } from '#lib/api/forms.js';
import type { ScenarioTemplate } from '#lib/api/projects.js';
import { fieldConditionToText } from '#lib/forms/field-conditions.js';
import {
	fieldArchivedOptions,
	fieldDisabledOptions,
	fieldOptions,
	sanitizeOptionColor
} from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import type { FieldDraft } from '#lib/forms/page-types.js';
import { isRecord } from '#lib/forms/record-format.js';

export function parseFields(schema: Record<string, unknown>): FormField[] {
	const rawFields = Array.isArray(schema.fields) ? schema.fields : [];

	return rawFields
		.filter(isRecord)
		.map((field) => {
			const rawType = typeof field.type === 'string' ? field.type : 'text';

			return {
				field_id: typeof field.field_id === 'string' ? field.field_id : undefined,
				key: typeof field.key === 'string' ? field.key : '',
				label: typeof field.label === 'string' ? field.label : undefined,
				type: (rawType === 'select' ? 'single_select' : rawType) as FormField['type'],
				required: Boolean(field.required),
				options: Array.isArray(field.options)
					? field.options.filter((item): item is string => typeof item === 'string')
					: [],

				option_colors: isRecord(field.option_colors)
					? Object.fromEntries(
							Object.entries(field.option_colors).filter(
								(entry): entry is [string, string] =>
									typeof entry[0] === 'string' && typeof entry[1] === 'string'
							)
						)
					: undefined,

				option_disabled: Array.isArray(field.option_disabled)
					? field.option_disabled.filter((item): item is string => typeof item === 'string')
					: undefined,

				option_archived: Array.isArray(field.option_archived)
					? field.option_archived.filter((item): item is string => typeof item === 'string')
					: undefined,

				option_descriptions: isRecord(field.option_descriptions)
					? Object.fromEntries(
							Object.entries(field.option_descriptions).filter(
								(entry): entry is [string, string] =>
									typeof entry[0] === 'string' && typeof entry[1] === 'string'
							)
						)
					: undefined,

				option_groups: isRecord(field.option_groups)
					? Object.fromEntries(
							Object.entries(field.option_groups).filter(
								(entry): entry is [string, string] =>
									typeof entry[0] === 'string' && typeof entry[1] === 'string'
							)
						)
					: undefined,
				amount: isRecord(field.amount) ? (field.amount as FormField['amount']) : undefined,
				placeholder: typeof field.placeholder === 'string' ? field.placeholder : undefined,
				help_text: typeof field.help_text === 'string' ? field.help_text : undefined,
				default_value: field.default_value,
				validation: isRecord(field.validation)
					? (field.validation as FormField['validation'])
					: undefined,

				visibility: isRecord(field.visibility)
					? (field.visibility as FormField['visibility'])
					: undefined,

				conditional: isRecord(field.conditional)
					? (field.conditional as FormField['conditional'])
					: undefined,
				policy: isRecord(field.policy) ? (field.policy as FormField['policy']) : undefined,
				formula: isRecord(field.formula) ? (field.formula as FormField['formula']) : undefined,
				relation: isRecord(field.relation) ? (field.relation as FormField['relation']) : undefined,
				attachment: isRecord(field.attachment)
					? (field.attachment as FormField['attachment'])
					: undefined
			};
		})
		.filter((field) => field.key);
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
		defaultValue: '',
		validationMin: '',
		validationMax: '',
		validationPattern: '',
		validationMaxLength: '',
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

export function normalizeTemplateSchema(schema: Record<string, unknown>): Record<string, unknown> {
	const fields = Array.isArray(schema.fields)
		? schema.fields.map((field) => {
				if (!isRecord(field)) return field;

				return {
					...field,
					type: field.type === 'select' ? 'single_select' : field.type
				};
			})
		: [];

	return {
		...schema,
		version: typeof schema.version === 'string' ? schema.version : 'openpr.form.schema.v1',
		fields
	};
}

export function templateFormKey(template: ScenarioTemplate): string {
	return template.key.replace(/_default$/, '');
}

export function templateTitleTemplate(schema: Record<string, unknown>): string {
	const field = parseFields(schema).find((item) => item.required) ?? parseFields(schema)[0];

	return field ? `{${field.key}}` : '{id}';
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
				defaultValue:
					field.default_value === undefined || field.default_value === null
						? ''
						: typeof field.default_value === 'string'
							? field.default_value
							: JSON.stringify(field.default_value),
				validationMin:
					field.validation?.min === undefined || field.validation?.min === null
						? ''
						: String(field.validation.min),
				validationMax:
					field.validation?.max === undefined || field.validation?.max === null
						? ''
						: String(field.validation.max),
				validationPattern: field.validation?.pattern ?? '',
				validationMaxLength:
					field.validation?.max_length === undefined || field.validation?.max_length === null
						? ''
						: String(field.validation.max_length),
				listVisible: field.visibility?.list ?? true,
				detailVisible: field.visibility?.detail ?? true,
				conditionalHiddenWhenText: fieldConditionToText(field.conditional?.hidden_when),
				conditionalReadOnlyWhenText: fieldConditionToText(field.conditional?.read_only_when),
				readOnly: field.policy?.read_only ?? false,
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

export function parseFormulaArgsText(value: string): unknown[] {
	return value
		.split(/[\n,]/)
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
