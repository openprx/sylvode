import { describeResult } from '#lib/i18n/describe.js';
import {
	type FormField,
	type FormFieldErrorItem,
	type FormRecord,
	type FormRelationTarget,
	formsApi
} from '#lib/api/forms.js';
import type { FormWorkflowMode } from '#lib/components/forms/FormWorkflowTabs.svelte';
import {
	clearedFieldPayload,
	isMissingRequiredValue,
	recordValueFromDraftValue,
	titleForSave
} from '#lib/forms/record-values.js';
import { toast } from '#lib/stores/toast.js';
import { confirmAction } from '#lib/ui/dialog/confirm.js';
import { fieldHiddenByCondition } from '#lib/forms/field-conditions.js';
import { fieldLabel, fieldType, optionArchived, optionDisabled } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { defaultDraftValueForField, draftValueForField } from '#lib/forms/record-draft-values.js';
import { isRecord, selectValue } from '#lib/forms/record-format.js';
import { errorsByField, validationErrorsOf } from '#lib/forms/field-errors.js';
import { recordHints, type RuleHint } from '#lib/forms/record-checks.js';
import { FormsController } from '#lib/forms/state/controller.js';

export class RecordsController extends FormsController {
	handleSaveRecord = async () => {
		if (!this.s.selectedForm) return;

		let values: Record<string, unknown>;

		try {
			values = this.buildRecordPayload(this.s.fields);
		} catch (error) {
			toast.error(error instanceof Error ? error.message : translate('forms.invalidRecordValues'));

			return;
		}

		this.s.recordSubmitting = true;

		const wasEditing = Boolean(this.s.editingRecordId);

		const response = this.s.editingRecordId
			? await this.fp.session.updateRecordWithVersion(this.s.editingRecordId, {
					title: titleForSave(this.s.recordTitle, this.editedRecordTitle()),
					values,
					source: { type: 'web' }
				})
			: await formsApi.createRecord(this.s.selectedForm.id, {
					title: this.s.recordTitle.trim() || undefined,
					values,
					source: { type: 'web' }
				});

		this.s.recordSubmitting = false;

		if (response.code !== 0 || !response.data) {
			this.showRefusal(response, (errors) => (this.s.recordFieldErrors = errors));

			return;
		}

		this.s.recordFieldErrors = {};

		if (!wasEditing) {
			await this.createParentChildLinksFromRelationFields(response.data, values);
		}

		this.s.records = this.s.editingRecordId
			? this.s.records.map((record) => (record.id === response.data?.id ? response.data : record))
			: [response.data, ...this.s.records];

		this.s.filterPreviewRecords = this.s.editingRecordId
			? this.s.filterPreviewRecords.map((record) =>
					record.id === response.data?.id ? response.data : record
				)
			: [response.data, ...this.s.filterPreviewRecords];

		this.s.selectedRecord = response.data;
		this.s.editingRecordId = null;
		this.s.recordEditorOpen = false;
		if (this.s.panelRecordId)
			this.fp.recordPanel.announce(translate('forms.recordPanel.announceSaved'));
		this.s.recordAttachments = [];
		this.resetRecordDraft();
		toast.success(wasEditing ? translate('forms.recordUpdated') : translate('forms.recordSaved'));

		await Promise.all([
			this.fp.recordPanel.reloadSelectedRecord(),
			this.fp.loaders.loadAggregates(),
			this.fp.recordPanel.loadSelectedRecordLinks(),
			this.fp.recordPanel.loadSelectedRecordChildren(),
			this.fp.recordPanel.loadSelectedRecordComments(),
			this.fp.loaders.loadPrintJobs()
		]);
	};

	/**
	 * Shows a refused save: a `validation_failed` refusal goes to the fields (`setErrors`) with a
	 * localised summary; any other failure is the usual message.
	 */
	showRefusal = (
		response: Parameters<typeof describeResult>[0] & { error_code?: string; details?: unknown },
		setErrors: (errors: Record<string, FormFieldErrorItem[]>) => void
	) => {
		const errors = validationErrorsOf(response);

		if (!errors) {
			setErrors({});
			toast.error(describeResult(response));

			return;
		}

		setErrors(errorsByField(errors));
		toast.error(translate('lowcode.forms.validationFailed', { values: { count: errors.length } }));
	};

	/**
	 * Rule hints for a draft: the values a save would send (`buildValuesFromDraft` without the
	 * required check), run through the client mirror of the server rules.
	 */
	draftHints = (
		formFields: FormField[],
		draft: Record<string, unknown>,
		existingValues?: Record<string, unknown>
	): Record<string, RuleHint[]> => {
		const values: Record<string, unknown> = {};

		for (const field of formFields) {
			if (fieldHiddenByCondition(field, draft)) continue;

			const value = recordValueFromDraftValue(field, draft[field.key], {
				relationTarget: (relationField, recordId) =>
					this.s.relationTargets[relationField.key]?.find((item) => item.record_id === recordId),
				member: (userId) => this.s.workspaceMembers.find((item) => item.user_id === userId),
				existingValue: existingValues?.[field.key]
			});

			if (value !== undefined) values[field.key] = value;
		}

		return recordHints(formFields, values, this.s.checkContext(), existingValues);
	};

	createParentChildLinksFromRelationFields = async (
		record: FormRecord,
		values: Record<string, unknown>
	) => {
		const relationFields = this.s.fields.filter(
			(field) =>
				(field.type === 'relation' || field.type === 'child_table') &&
				field.relation?.relation_type === 'parent_child'
		);

		for (const field of relationFields) {
			const value = values[field.key];

			if (!isRecord(value) || typeof value.record_id !== 'string' || !value.record_id) continue;

			const response = await formsApi.createRecordLink(record.id, {
				target_type: 'form_record',
				target_id: value.record_id,
				relation_key: field.relation?.relation_key || field.key,
				relation_type: 'parent_child',
				metadata: { source: 'web', relation_field: field.key }
			});

			if (response.code !== 0) {
				toast.error(describeResult(response));
			}
		}
	};

	handleDeleteRecord = async (record: FormRecord) => {
		const label = record.title || record.id;

		if (
			!(await confirmAction({
				tone: 'destructive',
				title: translate('forms.dialogs.deleteRecord.title', { values: { title: label } }),
				body: translate('forms.dialogs.deleteRecord.body'),
				confirmLabel: translate('forms.dialogs.deleteRecord.confirm', { values: { title: label } })
			}))
		) {
			return;
		}

		this.s.deletingRecordId = record.id;

		const response = await formsApi.deleteRecord(record.id);

		this.s.deletingRecordId = null;

		if (response.code !== 0) {
			toast.error(describeResult(response));

			return;
		}

		this.s.lastDeletedRecord = { id: record.id, title: label };
		if (this.s.panelRecordId === record.id) this.fp.recordPanel.closeRecordPanel();
		this.s.records = this.s.records.filter((item) => item.id !== record.id);
		this.s.filterPreviewRecords = this.s.filterPreviewRecords.filter(
			(item) => item.id !== record.id
		);
		this.s.printJobs = this.s.printJobs.filter((item) => item.id !== record.id);

		if (this.s.selectedRecord?.id === record.id) {
			this.s.selectedRecord = null;
			this.s.selectedRecordLinks = [];
			this.s.selectedRecordChildren = [];
			this.s.selectedRecordComments = [];
			this.s.recordCommentDraft = '';
		}

		if (this.s.editingRecordId === record.id) {
			this.cancelEditRecord();
		}

		this.s.focusedRecordIds = this.s.focusedRecordIds.filter((item) => item !== record.id);
		toast.success(translate('forms.recordDeleted'));
		await this.fp.loaders.loadAggregates();
	};

	/**
	 * Deletes are soft on the server, so surface the only entry point that can
	 * undo one instead of leaving the record unreachable.
	 */
	handleRestoreLastDeletedRecord = async () => {
		const target = this.s.lastDeletedRecord;

		if (!target) return;

		this.s.restoringRecordId = target.id;

		const response = await formsApi.restoreRecord(target.id);

		this.s.restoringRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));

			return;
		}

		this.s.lastDeletedRecord = null;
		toast.success(translate('forms.recordRestored'));
		await this.fp.loaders.loadRecords();
	};

	dismissLastDeletedRecord = () => {
		this.s.lastDeletedRecord = null;
	};

	clearFocusedRecord = () => {
		this.s.focusedRecordIds = [];
	};

	replaceLocalRecord = (record: FormRecord) => {
		this.s.records = this.s.records.map((item) => (item.id === record.id ? record : item));
		this.s.filterPreviewRecords = this.s.filterPreviewRecords.map((item) =>
			item.id === record.id ? record : item
		);

		if (this.s.selectedRecord?.id === record.id) {
			this.s.selectedRecord = record;
		}
	};

	startEditRecord = (record: FormRecord) => {
		this.s.formsSessionRecordVersion = {
			recordId: record.id,
			updatedAt: record.updated_at,
			schemaVersion: this.s.selectedForm?.schema_version ?? 0
		};
		this.s.selectedRecord = record;
		this.s.editingRecordId = record.id;
		this.s.activeMode = 'data';
		this.s.recordTitle = record.title;
		this.s.recordDraft = Object.fromEntries(
			this.s.fields.map((field) => [field.key, draftValueForField(field, record)])
		);
		this.s.recordFieldErrors = {};
		void this.fp.attachments.loadRecordAttachments(record.id);
		this.fp.session.cleanFormsGroup('record');
	};

	openRecordCreateDrawer = () => {
		this.s.editingRecordId = null;
		this.s.selectedRecord = null;
		this.s.recordAttachments = [];
		this.resetRecordDraft();
		this.s.activeMode = 'data';
		this.s.recordEditorOpen = true;
	};

	openRecordEditDrawer = (record: FormRecord) => {
		this.startEditRecord(record);
		this.s.recordEditorOpen = true;
	};

	closeRecordEditorDrawer = () => {
		this.s.recordEditorOpen = false;
		this.cancelEditRecord();
	};

	/** Task-local editor: navigation chrome stays reachable; do not trap Tab globally. */
	handleGlobalKeydown = (event: KeyboardEvent) => {
		if (!this.s.recordEditorOpen || !this.s.recordEditorPanel?.contains(document.activeElement))
			return;
		if (event.key === 'Escape') {
			event.preventDefault();
			this.closeRecordEditorDrawer();
		}
	};

	cancelEditRecord = () => {
		this.s.editingRecordId = null;
		this.s.recordTitle = '';
		this.resetRecordDraft();

		if (this.s.selectedRecord) {
			void this.fp.attachments.loadRecordAttachments(this.s.selectedRecord.id);
		} else {
			this.s.recordAttachments = [];
		}
	};

	handleFormChange = (event: Event) => {
		void this.fp.session.openForm(selectValue(event));
	};

	handleModeChange = (mode: FormWorkflowMode) => {
		this.s.activeMode = mode;

		if (mode !== 'data') {
			this.s.recordEditorOpen = false;
			this.s.editingRecordId = null;
		}

		if (mode === 'design') {
			this.fp.designer.syncDesignerDrafts();
			this.fp.detailLayoutEditor.syncDetailHeaderDraft();
			this.fp.detailLayoutEditor.syncDetailHighlightDraft();
			this.fp.detailLayoutEditor.syncDetailPageCompositionDraft();
			this.fp.detailLayoutEditor.syncDetailLayoutSectionDrafts();
			void this.fp.designer.loadDesignerSafety();
		}
	};

	resetRecordDraft = () => {
		const next: Record<string, unknown> = {};

		for (const field of this.s.fields) {
			next[field.key] = defaultDraftValueForField(field);
		}

		this.s.recordDraft = next;
		this.s.recordTitle = '';
		this.s.recordFieldErrors = {};
		this.s.formsSessionRecordVersion = null;
		this.fp.session.cleanFormsGroup('record');
	};

	buildRecordPayload = (formFields: FormField[]): Record<string, unknown> => {
		return this.buildValuesFromDraft(formFields, this.s.recordDraft, this.editedRecordValues());
	};

	editedRecordTitle = (): string | undefined => {
		if (!this.s.editingRecordId) return undefined;

		const current =
			this.s.records.find((record) => record.id === this.s.editingRecordId) ??
			this.s.selectedRecord;

		return current?.id === this.s.editingRecordId ? current.title : undefined;
	};

	editedRecordValues = (): Record<string, unknown> | undefined => {
		if (!this.s.editingRecordId) return undefined;

		const current =
			this.s.records.find((record) => record.id === this.s.editingRecordId) ??
			this.s.selectedRecord;

		return current?.id === this.s.editingRecordId ? current.values : undefined;
	};

	/**
	 * Builds the `values` payload. On update an absent key means "unchanged"
	 * and `null` clears, so a field the user emptied is sent as `null` when the
	 * record holds a value for it (`existingValues` is set only when editing).
	 */
	buildValuesFromDraft = (
		formFields: FormField[],
		draft: Record<string, unknown>,
		existingValues?: Record<string, unknown>
	): Record<string, unknown> => {
		const values: Record<string, unknown> = {};

		for (const field of formFields) {
			if (fieldHiddenByCondition(field, draft)) continue;

			const value = recordValueFromDraftValue(field, draft[field.key], {
				relationTarget: (relationField, recordId) =>
					this.s.relationTargets[relationField.key]?.find((item) => item.record_id === recordId),
				member: (userId) => this.s.workspaceMembers.find((item) => item.user_id === userId),
				existingValue: existingValues?.[field.key]
			});

			if (isMissingRequiredValue(field, value)) {
				throw new Error(
					fieldType(field) === 'child_table'
						? translate('forms.childTableRequiredAfterSave', {
								values: { field: fieldLabel(field) }
							})
						: translate('forms.fieldRequired', { values: { field: fieldLabel(field) } })
				);
			}

			if (value === undefined) {
				const cleared = clearedFieldPayload(
					existingValues !== undefined,
					existingValues?.[field.key]
				);

				if (cleared === null) values[field.key] = null;

				continue;
			}

			values[field.key] = value;
		}

		return values;
	};

	relationTargetOptions = (field: FormField): FormRelationTarget[] => {
		return this.s.relationTargets[field.key] ?? [];
	};

	relationDraftValue = (field: FormField): string => {
		const value = this.s.recordDraft[field.key];

		if (typeof value === 'string') return value;
		if (isRecord(value) && typeof value.record_id === 'string') return value.record_id;

		return '';
	};

	setDraftValue = (fieldKey: string, value: unknown) => {
		this.s.recordDraft = { ...this.s.recordDraft, [fieldKey]: value };
	};

	draftArray = (fieldKey: string): string[] => {
		const value = this.s.recordDraft[fieldKey];

		return Array.isArray(value)
			? value.filter((item): item is string => typeof item === 'string')
			: [];
	};

	isDraftOptionSelected = (fieldKey: string, option: string): boolean => {
		return this.draftArray(fieldKey).includes(option);
	};

	toggleDraftOption = (fieldKey: string, option: string, checked: boolean) => {
		const current = this.draftArray(fieldKey);

		this.setDraftValue(
			fieldKey,
			checked
				? Array.from(new Set([...current, option]))
				: current.filter((item) => item !== option)
		);
	};

	draftString = (fieldKey: string): string => {
		const value = this.s.recordDraft[fieldKey];

		return typeof value === 'string' ? value : '';
	};

	optionDisabledForSingleDraft = (field: FormField, option: string): boolean => {
		return (
			(optionDisabled(field, option) || optionArchived(field, option)) &&
			this.draftString(field.key) !== option
		);
	};

	optionDisabledForMultiDraft = (field: FormField, option: string): boolean => {
		return (
			(optionDisabled(field, option) || optionArchived(field, option)) &&
			!this.isDraftOptionSelected(field.key, option)
		);
	};
}
