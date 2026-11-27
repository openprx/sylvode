import { describeResult } from '#lib/i18n/describe.js';
import {
	type FormChildRecord,
	type FormField,
	formsApi,
	type UniversalForm
} from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import { confirmAction } from '#lib/ui/dialog/confirm.js';
import { parseFields } from '#lib/forms/designer-schema.js';
import { fieldType, optionArchived, optionDisabled } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { titleForSave } from '#lib/forms/record-values.js';
import {
	childRelationKeyForField,
	defaultDraftForFields,
	draftArrayFrom,
	draftStringFrom,
	draftValueForField
} from '#lib/forms/record-draft-values.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { attachmentManifestUrls } from '#lib/forms/zip.js';

export class RecordDetailController extends FormsController {
	closeRecordDetail = () => {
		this.s.selectedRecord = null;
		this.s.selectedRecordLinks = [];
		this.s.selectedRecordChildren = [];
		this.s.selectedRecordComments = [];
		this.s.recordCommentDraft = '';
		this.s.recordAttachments = [];
		this.s.activeMode = 'data';
		void this.fp.recordPanel.syncRecordDetailUrl(null);
	};

	handleCreateLink = async () => {
		if (!this.s.selectedRecord) return;

		this.s.linkSubmitting = true;

		const response = await formsApi.createRecordLink(this.s.selectedRecord.id, {
			target_type: this.s.linkDraft.target_type.trim() || 'form_record',
			target_id: this.s.linkDraft.target_id.trim(),
			relation_key: this.s.linkDraft.relation_key.trim() || 'related',
			relation_type: this.s.linkDraft.relation_type.trim() || 'reference',
			metadata: { source: 'web' }
		});

		this.s.linkSubmitting = false;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));

			return;
		}

		this.s.selectedRecordLinks = [response.data, ...this.s.selectedRecordLinks];
		this.s.linkDraft = { ...this.s.linkDraft, target_id: '' };
		toast.success(translate('forms.linkCreated'));
		await this.fp.recordPanel.loadSelectedRecordChildren();
	};

	handleCreateRecordComment = async () => {
		if (!this.s.selectedRecord) return;

		const body = this.s.recordCommentDraft.trim();

		if (!body) return;

		this.s.recordCommentSubmitting = true;

		const response = await formsApi.createRecordComment(this.s.selectedRecord.id, {
			body,
			metadata: {
				source: 'web',
				form_id: this.s.selectedForm?.id,
				form_key: this.s.selectedForm?.key
			}
		});

		this.s.recordCommentSubmitting = false;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));

			return;
		}

		this.s.selectedRecordComments = [
			response.data,
			...this.s.selectedRecordComments.filter((item) => item.id !== response.data?.id)
		];

		this.s.recordCommentDraft = '';
		await this.fp.loaders.loadFormEvents();
		toast.success(translate('forms.recordCommentAdded'));
	};

	childFormForRelationField = (field: FormField): UniversalForm | null => {
		const formKey = field.relation?.form_key;

		return formKey ? (this.s.forms.find((form) => form.key === formKey) ?? null) : null;
	};

	parentChildRelationFields = (): FormField[] => {
		return this.s.fields.filter(
			(field) =>
				(field.type === 'relation' || field.type === 'child_table') &&
				field.relation?.relation_type === 'parent_child' &&
				Boolean(this.childFormForRelationField(field))
		);
	};

	childFieldsForForm = (formId: string): FormField[] => {
		const childForm = this.s.forms.find((form) => form.id === formId);

		return childForm ? parseFields(childForm.schema) : [];
	};

	startAddChildRecord = (field: FormField) => {
		const childForm = this.childFormForRelationField(field);

		if (!childForm) return;

		this.s.childEditor = {
			relationFieldKey: field.key,
			childFormId: childForm.id,
			title: '',
			draft: defaultDraftForFields(parseFields(childForm.schema))
		};
	};

	startEditChildRecord = (child: FormChildRecord) => {
		const childForm = this.s.forms.find((form) => form.id === child.record.form_id);

		if (!childForm) return;

		const relationField = this.s.fields.find(
			(field) =>
				(field.type === 'relation' || field.type === 'child_table') &&
				field.relation?.relation_type === 'parent_child' &&
				field.relation?.relation_key === child.relation_key &&
				field.relation?.form_key === childForm.key
		);

		if (!relationField) return;

		const childFields = parseFields(childForm.schema);

		this.s.childEditor = {
			relationFieldKey: relationField.key,
			childFormId: childForm.id,
			recordId: child.record.id,
			title: child.record.title,
			draft: Object.fromEntries(
				childFields.map((field) => [field.key, draftValueForField(field, child.record)])
			)
		};
	};

	cancelChildEditor = () => {
		this.s.childEditor = null;
	};

	saveChildRecord = async () => {
		if (!this.s.selectedRecord || !this.s.childEditor) return;

		const relationField = this.s.fields.find(
			(field) => field.key === this.s.childEditor?.relationFieldKey
		);
		const childForm = this.s.forms.find((form) => form.id === this.s.childEditor?.childFormId);

		if (!relationField || !childForm) return;

		const editedChild = this.s.childEditor.recordId
			? this.s.selectedRecordChildren.find(
					(item) => item.record.id === this.s.childEditor?.recordId
				)
			: undefined;

		let values: Record<string, unknown>;

		try {
			values = this.fp.records.buildValuesFromDraft(
				parseFields(childForm.schema),
				this.s.childEditor.draft,
				editedChild?.record.values
			);
		} catch (error) {
			toast.error(error instanceof Error ? error.message : translate('forms.invalidRecordValues'));

			return;
		}

		this.s.childSubmitting = true;

		const response = this.s.childEditor.recordId
			? await this.fp.session.updateRecordWithVersion(this.s.childEditor.recordId, {
					title: titleForSave(this.s.childEditor.title, editedChild?.record.title),
					values,
					source: { type: 'web', parent_record_id: this.s.selectedRecord.id }
				})
			: await formsApi.createRecord(childForm.id, {
					title: this.s.childEditor.title.trim() || undefined,
					values,
					source: { type: 'web', parent_record_id: this.s.selectedRecord.id }
				});

		this.s.childSubmitting = false;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));

			return;
		}

		if (!this.s.childEditor.recordId) {
			const linkResponse = await formsApi.createRecordLink(this.s.selectedRecord.id, {
				target_type: 'form_record',
				target_id: response.data.id,
				relation_key: relationField.relation?.relation_key || relationField.key,
				relation_type: 'parent_child',
				metadata: {
					source: 'web',
					inline_child: true,
					relation_field: relationField.key
				}
			});

			if (linkResponse.code !== 0) {
				toast.error(describeResult(linkResponse));
			}
		}

		this.s.childEditor = null;

		await Promise.all([
			this.fp.recordPanel.reloadSelectedRecord(),
			this.fp.recordPanel.loadSelectedRecordChildren(),
			this.fp.recordPanel.loadSelectedRecordLinks(),
			this.fp.loaders.loadAggregates()
		]);

		toast.success(translate('forms.childRecordSaved'));
	};

	deleteChildRecord = async (child: FormChildRecord) => {
		const label = child.record.title || child.record.id;

		if (
			!(await confirmAction({
				tone: 'destructive',
				title: translate('forms.dialogs.deleteChildRecord.title', { values: { title: label } }),
				body: translate('forms.dialogs.deleteChildRecord.body'),
				confirmLabel: translate('forms.dialogs.deleteChildRecord.confirm', {
					values: { title: label }
				})
			}))
		) {
			return;
		}

		this.s.deletingChildRecordId = child.record.id;

		const response = await formsApi.deleteRecord(child.record.id);

		this.s.deletingChildRecordId = null;

		if (response.code !== 0) {
			toast.error(describeResult(response));

			return;
		}

		if (this.s.childEditor?.recordId === child.record.id) this.s.childEditor = null;

		await Promise.all([
			this.fp.recordPanel.reloadSelectedRecord(),
			this.fp.recordPanel.loadSelectedRecordChildren(),
			this.fp.recordPanel.loadSelectedRecordLinks(),
			this.fp.loaders.loadAggregates()
		]);

		toast.success(translate('forms.childRecordDeleted'));
	};

	selectedRecordAttachmentFields = (): FormField[] => {
		return this.s.fields.filter((field) => {
			const type = fieldType(field);

			return ['attachment', 'image'].includes(type) && field.visibility?.detail !== false;
		});
	};

	selectedRecordAttachmentUrls = (field: FormField): string[] => {
		if (!this.s.selectedRecord) return [];

		return attachmentManifestUrls(this.s.selectedRecord.values[field.key]);
	};

	selectedRecordAttachmentValueUrls = (field: FormField): string[] => {
		const metadataUrls = new Set(
			this.fp.attachments.fieldAttachments(field.key).map((attachment) => attachment.url)
		);

		return this.selectedRecordAttachmentUrls(field).filter((url) => url && !metadataUrls.has(url));
	};

	updateChildDraftValue = (fieldKey: string, value: unknown) => {
		if (!this.s.childEditor) return;

		this.s.childEditor = {
			...this.s.childEditor,
			draft: { ...this.s.childEditor.draft, [fieldKey]: value }
		};
	};

	updateChildTitle = (value: string) => {
		if (!this.s.childEditor) return;

		this.s.childEditor = { ...this.s.childEditor, title: value };
	};

	isChildDraftOptionSelected = (fieldKey: string, option: string): boolean => {
		return this.s.childEditor
			? draftArrayFrom(this.s.childEditor.draft, fieldKey).includes(option)
			: false;
	};

	toggleChildDraftOption = (fieldKey: string, option: string, checked: boolean) => {
		if (!this.s.childEditor) return;

		const current = draftArrayFrom(this.s.childEditor.draft, fieldKey);

		this.updateChildDraftValue(
			fieldKey,
			checked
				? Array.from(new Set([...current, option]))
				: current.filter((item) => item !== option)
		);
	};

	optionDisabledForChildSingleDraft = (field: FormField, option: string): boolean => {
		return (
			(optionDisabled(field, option) || optionArchived(field, option)) &&
			(!this.s.childEditor || draftStringFrom(this.s.childEditor.draft, field.key) !== option)
		);
	};

	optionDisabledForChildMultiDraft = (field: FormField, option: string): boolean => {
		return (
			(optionDisabled(field, option) || optionArchived(field, option)) &&
			!this.isChildDraftOptionSelected(field.key, option)
		);
	};

	childRecordFields = (child: FormChildRecord): FormField[] => {
		const childForm = this.s.forms.find((form) => form.id === child.record.form_id);

		return childForm
			? parseFields(childForm.schema).filter((field) => field.visibility?.list !== false)
			: [];
	};

	childRecordsForRelationField = (field: FormField): FormChildRecord[] => {
		const childForm = this.childFormForRelationField(field);
		const relationKey = childRelationKeyForField(field);

		return this.s.selectedRecordChildren.filter(
			(child) =>
				child.relation_key === relationKey && (!childForm || child.record.form_id === childForm.id)
		);
	};

	childGridFieldsForRelationField = (field: FormField): FormField[] => {
		const childForm = this.childFormForRelationField(field);

		return childForm
			? parseFields(childForm.schema).filter(
					(childField) => childField.visibility?.list !== false && childField.type !== 'child_table'
				)
			: [];
	};
}
