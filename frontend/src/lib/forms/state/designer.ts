import { describeResult } from '#lib/i18n/describe.js';
import {
	type FormField,
	type FormFieldDependency,
	type FormFieldType,
	type FormFieldUsage,
	formsApi,
	type UniversalForm
} from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import { blankFieldDraft } from '#lib/forms/designer-schema.js';
import type { DesignerSchemaChange, FieldDraft } from '#lib/forms/page-types.js';
import {
	fieldDraftsFromSchema,
	fieldIdentity,
	parseAttachmentVariantsText,
	parseFieldDefaultValue,
	parseFields,
	parseFormulaArgsText,
	parseOptionColorsText,
	parseOptionMetadataText,
	parseOptionsText,
	parseSelectDefaultValue,
	sanitizeAttachmentDerivativeFormat
} from '#lib/forms/designer-schema.js';
import { parseFieldConditionText } from '#lib/forms/field-conditions.js';
import { fieldLabel } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { FormsController } from '#lib/forms/state/controller.js';
import {
	FORM_SCHEMA_VERSION,
	anchoredPattern,
	maxLengthAllowed,
	patternAllowed,
	ruleBound
} from '#lib/forms/schema-v2.js';

export class DesignerController extends FormsController {
	buildSchemaFromDrafts = (
		drafts: FieldDraft[] = this.s.fieldDrafts,
		syncFormDraft = true
	): Record<string, unknown> => {
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
				defaultValue: field.defaultValue.trim(),
				validationMin: field.validationMin.trim(),
				validationMax: field.validationMax.trim(),
				validationPattern: field.validationPattern.trim(),
				validationMaxLength: field.validationMaxLength.trim(),
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

				if (field.defaultValue) {
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

			if (field.defaultValue && !['single_select', 'multi_select'].includes(field.type)) {
				schemaField.default = { value: parseFieldDefaultValue(field.defaultValue, field.type) };
			}

			const validate: Record<string, unknown>[] = [];
			const minBound = ruleBound(field.type, field.validationMin);
			const maxBound = ruleBound(field.type, field.validationMax);

			if (minBound !== undefined) validate.push({ rule: 'min', value: minBound });
			if (maxBound !== undefined) validate.push({ rule: 'max', value: maxBound });

			if (field.validationPattern && patternAllowed(field.type)) {
				validate.push({ rule: 'pattern', value: anchoredPattern(field.validationPattern) });
			}

			if (field.validationMaxLength && maxLengthAllowed(field.type)) {
				const maxLength = Number.parseInt(field.validationMaxLength, 10);

				if (Number.isFinite(maxLength) && maxLength > 0) {
					validate.push({ rule: 'max_length', value: maxLength });
				}
			}

			if (validate.length > 0) schemaField.rules = { validate };

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

		const schema = { version: FORM_SCHEMA_VERSION, fields: schemaFields };

		if (syncFormDraft) this.s.formDraft.schemaText = JSON.stringify(schema, null, 2);

		return schema;
	};

	syncDesignerDrafts = (form: UniversalForm | null = this.s.selectedForm) => {
		if (!form) {
			this.s.designerFieldDrafts = [];
			this.s.designerFieldUsage = [];
			this.s.designerFieldDependencies = [];
			this.s.designerSyncedFormId = '';
			this.s.selectedDesignerFieldIndex = 0;

			return;
		}

		if (this.s.designerSyncedFormId === form.id) return;

		this.s.designerFieldDrafts = fieldDraftsFromSchema(form.schema);
		this.s.designerSyncedFormId = form.id;
		this.s.selectedDesignerFieldIndex = 0;
	};

	loadDesignerSafety = async (form: UniversalForm | null = this.s.selectedForm) => {
		if (!form) {
			this.s.designerFieldUsage = [];
			this.s.designerFieldDependencies = [];

			return;
		}

		this.s.designerUsageLoading = true;

		const [usageResponse, dependencyResponse] = await Promise.all([
			formsApi.getFieldUsage(form.id),
			formsApi.getFieldDependencies(form.id)
		]);

		this.s.designerUsageLoading = false;

		if (usageResponse.code !== 0) {
			this.s.designerFieldUsage = [];
			toast.error(describeResult(usageResponse));
		} else {
			this.s.designerFieldUsage = usageResponse.data?.fields ?? [];
		}

		if (dependencyResponse.code !== 0) {
			this.s.designerFieldDependencies = [];
			toast.error(describeResult(dependencyResponse));
		} else {
			this.s.designerFieldDependencies = dependencyResponse.data?.dependencies ?? [];
		}
	};

	blockingDependenciesForField = (field: FieldDraft): FormFieldDependency[] => {
		return this.s.designerFieldDependencies.filter(
			(dependency) =>
				dependency.blocking &&
				((field.field_id && dependency.field_id === field.field_id) ||
					dependency.field_key === field.key)
		);
	};

	usageForSchemaField = (field: FormField | FieldDraft): FormFieldUsage | null => {
		return (
			this.s.designerFieldUsage.find(
				(usage) =>
					(field.field_id && usage.field_id === field.field_id) || usage.field_key === field.key
			) ?? null
		);
	};

	dependenciesForSchemaField = (field: FormField | FieldDraft): FormFieldDependency[] => {
		return this.s.designerFieldDependencies.filter(
			(dependency) =>
				(field.field_id && dependency.field_id === field.field_id) ||
				dependency.field_key === field.key
		);
	};

	usageCountsForSchemaField = (field: FormField | FieldDraft) => {
		const usage = this.usageForSchemaField(field);
		const dependencies = this.dependenciesForSchemaField(field);

		return {
			valueCount: usage?.value_count ?? 0,
			indexedCount: usage?.indexed_count ?? 0,
			dependencyCount: dependencies.length,
			blockingDependencyCount: dependencies.filter((dependency) => dependency.blocking).length
		};
	};

	collectDesignerSchemaChanges = (drafts: FieldDraft[]): DesignerSchemaChange[] => {
		if (!this.s.selectedForm) return [];

		const originalFields = parseFields(this.s.selectedForm.schema);
		const draftByIdentity = new Map(
			drafts.filter((field) => field.key.trim()).map((field) => [fieldIdentity(field), field])
		);
		const changes: DesignerSchemaChange[] = [];

		for (const original of originalFields) {
			const identity = fieldIdentity(original);
			const draft = draftByIdentity.get(identity);
			const counts = this.usageCountsForSchemaField(original);

			if (!draft) {
				changes.push({
					kind: 'delete',
					field_id: original.field_id,
					fieldKey: original.key,
					fieldLabel: fieldLabel(original),
					from: original.key,
					...counts
				});

				continue;
			}

			if (draft.key !== original.key) {
				changes.push({
					kind: 'rename',
					field_id: original.field_id,
					fieldKey: original.key,
					fieldLabel: fieldLabel(original),
					from: original.key,
					to: draft.key,
					...counts
				});
			}

			const nextType = draft.type ?? 'text';
			const originalType = original.type ?? 'text';

			if (nextType !== originalType) {
				changes.push({
					kind: 'type_change',
					field_id: original.field_id,
					fieldKey: original.key,
					fieldLabel: fieldLabel(original),
					from: translate(`forms.fieldTypes.${originalType}`),
					to: translate(`forms.fieldTypes.${nextType}`),
					...counts
				});
			}
		}

		return changes;
	};

	closeDesignerConfirm = () => {
		this.s.designerConfirmOpen = false;
		this.s.pendingDesignerSchema = null;
		this.s.pendingDesignerChanges = [];
	};

	addDesignerField = (type: FormFieldType = 'text') => {
		const field = {
			...blankFieldDraft(),
			type,
			key: this.nextDesignerFieldKey(type),
			label: translate(`forms.fieldTypes.${type}`)
		};

		this.s.designerFieldDrafts = [...this.s.designerFieldDrafts, field];
		this.s.selectedDesignerFieldIndex = this.s.designerFieldDrafts.length - 1;
	};

	nextDesignerFieldKey = (type: FormFieldType): string => {
		const base = type.replaceAll('-', '_');
		const used = new Set(this.s.designerFieldDrafts.map((field) => field.key));
		let index = this.s.designerFieldDrafts.length + 1;
		let key = `${base}_${index}`;

		while (used.has(key)) {
			index += 1;
			key = `${base}_${index}`;
		}

		return key;
	};

	updateDesignerField = (index: number, patch: Partial<FieldDraft>) => {
		this.s.designerFieldDrafts = this.s.designerFieldDrafts.map((field, itemIndex) =>
			itemIndex === index ? { ...field, ...patch } : field
		);
	};

	duplicateDesignerField = (index: number) => {
		const field = this.s.designerFieldDrafts[index];

		if (!field) return;

		const duplicate = {
			...field,
			field_id: undefined,
			key: this.nextDesignerFieldKey(field.type),
			label: `${field.label || field.key} ${translate('forms.copySuffix')}`
		};

		this.s.designerFieldDrafts = [
			...this.s.designerFieldDrafts.slice(0, index + 1),
			duplicate,
			...this.s.designerFieldDrafts.slice(index + 1)
		];

		this.s.selectedDesignerFieldIndex = index + 1;
	};

	removeDesignerField = (index: number) => {
		if (this.s.designerFieldDrafts.length <= 1) return;

		const field = this.s.designerFieldDrafts[index];

		if (field && this.blockingDependenciesForField(field).length > 0) {
			toast.error(translate('forms.deleteBlockedByDependencies'));

			return;
		}

		this.s.designerFieldDrafts = this.s.designerFieldDrafts.filter(
			(_, itemIndex) => itemIndex !== index
		);
		this.s.selectedDesignerFieldIndex = Math.max(
			0,
			Math.min(index, this.s.designerFieldDrafts.length - 1)
		);
	};

	moveDesignerField = (index: number, direction: -1 | 1) => {
		const nextIndex = index + direction;

		if (nextIndex < 0 || nextIndex >= this.s.designerFieldDrafts.length) return;

		const next = [...this.s.designerFieldDrafts];

		[next[index], next[nextIndex]] = [next[nextIndex], next[index]];
		this.s.designerFieldDrafts = next;
		this.s.selectedDesignerFieldIndex = nextIndex;
	};

	saveDesignerSchema = async () => {
		if (!this.s.selectedForm) return;

		let schema: Record<string, unknown>;

		try {
			schema = this.buildSchemaFromDrafts(this.s.designerFieldDrafts, false);
		} catch (error) {
			toast.error(error instanceof Error ? error.message : translate('forms.invalidSchema'));

			return;
		}

		const changes = this.collectDesignerSchemaChanges(this.s.designerFieldDrafts);

		if (changes.length > 0) {
			this.s.pendingDesignerSchema = schema;
			this.s.pendingDesignerChanges = changes;
			this.s.designerConfirmOpen = true;

			return;
		}

		await this.persistDesignerSchema(schema);
	};

	confirmDesignerSchemaSave = async () => {
		if (!this.s.pendingDesignerSchema) return;

		const schema = this.s.pendingDesignerSchema;

		this.s.designerConfirmOpen = false;
		this.s.pendingDesignerSchema = null;
		this.s.pendingDesignerChanges = [];
		await this.persistDesignerSchema(schema);
	};

	persistDesignerSchema = async (schema: Record<string, unknown>) => {
		if (!this.s.selectedForm) return;

		this.s.designerSaving = true;

		const response = await formsApi.update(this.s.selectedForm.id, {
			schema,
			expected_schema_version:
				this.s.formsSessionSchemaVersion ?? this.s.selectedForm.schema_version
		});

		this.s.designerSaving = false;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));

			return;
		}

		this.s.forms = this.s.forms.map((form) =>
			form.id === response.data?.id ? response.data : form
		);
		this.s.designerSyncedFormId = '';
		this.syncDesignerDrafts(response.data);
		this.s.detailHeaderSyncedFingerprint = '';
		this.fp.detailLayoutEditor.syncDetailHeaderDraft(response.data);
		this.s.detailHighlightSyncedFingerprint = '';
		this.fp.detailLayoutEditor.syncDetailHighlightDraft(response.data);
		this.s.detailPageCompositionSyncedFingerprint = '';
		this.fp.detailLayoutEditor.syncDetailPageCompositionDraft(response.data);
		this.s.detailLayoutSectionSyncedFingerprint = '';
		this.fp.detailLayoutEditor.syncDetailLayoutSectionDrafts(response.data);
		await this.loadDesignerSafety(response.data);
		toast.success(translate('forms.designSaved'));
		await this.fp.loaders.loadRecords();
	};
}
