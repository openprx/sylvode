import { describeResult } from '#lib/i18n/describe.js';
import {
	type FormField,
	type FormFieldDependency,
	type FormFieldErrorItem,
	type FormFieldType,
	type FormSchemaIssue,
	type FormFieldUsage,
	formsApi,
	type UniversalForm
} from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import type { DesignerSchemaChange, FieldDraft } from '#lib/forms/page-types.js';
import {
	blankFieldDraft,
	fieldDraftsFromSchema,
	fieldIdentity,
	parseFields,
	schemaFromDrafts
} from '#lib/forms/designer-schema.js';
import { fieldLabel } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { schemaIssuesOf } from '#lib/forms/field-errors.js';
import { descriptorDrift, parseSchema } from '#lib/forms/types/index.js';

let fieldTypeDriftChecked = false;

/** Dev builds: compares the descriptor table with `GET /api/v1/field-types` once per page load. */
async function warnOnFieldTypeDrift() {
	if (!import.meta.env.DEV || fieldTypeDriftChecked) return;
	fieldTypeDriftChecked = true;
	const response = await formsApi.getFieldTypes();
	if (response.code !== 0 || !response.data) return;
	const drift = descriptorDrift(response.data);
	if (drift.length > 0) console.warn('Field type descriptors differ from the server:', drift);
}

/** Schema issues by the key of the field their JSON Pointer names (`''` for the schema itself). */
export function issuesByField(
	issues: FormSchemaIssue[],
	schema: Record<string, unknown>
): Record<string, FormSchemaIssue[]> {
	const fields = Array.isArray(schema.fields) ? schema.fields : [];
	const grouped: Record<string, FormSchemaIssue[]> = {};
	for (const issue of issues) {
		const match = /^\/fields\/(\d+)(\/.*)?$/.exec(issue.path);
		const field = match ? (fields[Number(match[1])] as { key?: unknown } | undefined) : undefined;
		const key = typeof field?.key === 'string' ? field.key : '';
		(grouped[key] ??= []).push({ ...issue, path: match ? (match[2] ?? '') : issue.path });
	}
	return grouped;
}

export class DesignerController extends FormsController {
	/** The schema of the field drafts (`schemaFromDrafts`), mirrored into the JSON editor. */
	buildSchemaFromDrafts = (
		drafts: FieldDraft[] = this.s.fieldDrafts,
		syncFormDraft = true,
		schemaExtra: Record<string, unknown> = {}
	): Record<string, unknown> => {
		const schema = schemaFromDrafts(drafts, schemaExtra);

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
		this.s.designerSchemaIssues = {};
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
		void warnOnFieldTypeDrift();

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

	/**
	 * Runs `values` through the server pipeline against the unsaved designer schema
	 * (`POST /forms/{id}/validate` with `schema`, nothing stored): the server side of the designer
	 * preview.
	 */
	checkDraftOnServer = async (
		values: Record<string, unknown>
	): Promise<{ errors: FormFieldErrorItem[] } | { failure: string }> => {
		if (!this.s.selectedForm) return { failure: translate('forms.invalidSchema') };

		let schema: Record<string, unknown>;

		try {
			schema = this.buildSchemaFromDrafts(
				this.s.designerFieldDrafts,
				false,
				parseSchema(this.s.selectedForm.schema).extra
			);
		} catch (error) {
			return { failure: error instanceof Error ? error.message : translate('forms.invalidSchema') };
		}

		const response = await formsApi.validateRecord(this.s.selectedForm.id, {
			mode: 'create',
			values,
			schema
		});

		if (response.code === 0 && response.data) return { errors: response.data.errors };

		const issues = schemaIssuesOf(response);

		if (issues) this.s.designerSchemaIssues = issuesByField(issues, schema);

		return {
			failure: issues
				? translate('lowcode.forms.designer.schemaInvalid', { values: { count: issues.length } })
				: describeResult(response)
		};
	};

	saveDesignerSchema = async () => {
		if (!this.s.selectedForm) return;

		let schema: Record<string, unknown>;

		try {
			schema = this.buildSchemaFromDrafts(
				this.s.designerFieldDrafts,
				false,
				parseSchema(this.s.selectedForm.schema).extra
			);
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
			const issues = schemaIssuesOf(response);

			this.s.designerSchemaIssues = issues ? issuesByField(issues, schema) : {};
			toast.error(
				issues
					? translate('lowcode.forms.designer.schemaInvalid', { values: { count: issues.length } })
					: describeResult(response)
			);

			return;
		}

		this.s.designerSchemaIssues = {};

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
