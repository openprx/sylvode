import { describeResult } from '#lib/i18n/describe.js';
import {
	type FormField,
	type FormPermissionAction,
	type FormPermissions,
	formsApi
} from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import type { FormRecordScope } from '#lib/forms/page-types.js';
import { defaultPermissionActions } from '#lib/forms/permission-drafts.js';
import { translate } from '#lib/forms/i18n.js';
import { rolePermissionActions, roleRecordScope } from '#lib/forms/permission-drafts.js';
import { FormsController } from '#lib/forms/state/controller.js';

export class PermissionsController extends FormsController {
	loadFormPermissions = async () => {
		if (!this.s.selectedFormId) {
			this.s.formPermissions = null;
			this.s.memberPermissionDraft = defaultPermissionActions();
			this.s.memberRecordScopeDraft = 'all';
			this.s.memberFieldReadDraft = {};
			this.s.memberFieldWriteDraft = {};

			return;
		}

		this.s.permissionsLoading = true;

		const response = await formsApi.getPermissions(this.s.selectedFormId);

		this.s.permissionsLoading = false;

		if (response.code !== 0 || !response.data) {
			this.s.formPermissions = null;
			this.s.memberPermissionDraft = defaultPermissionActions();
			this.s.memberRecordScopeDraft = 'all';
			this.s.memberFieldReadDraft = this.defaultFieldReadDraft();
			this.s.memberFieldWriteDraft = this.defaultFieldWriteDraft();
			toast.error(describeResult(response));

			return;
		}

		this.s.formPermissions = response.data;
		this.s.memberPermissionDraft = rolePermissionActions(response.data, 'member');
		this.s.memberRecordScopeDraft = roleRecordScope(response.data, 'member');
		this.s.memberFieldReadDraft = this.roleFieldReadPermissions(response.data, 'member');
		this.s.memberFieldWriteDraft = this.roleFieldWritePermissions(response.data, 'member');
	};

	saveMemberPermissions = async () => {
		if (!this.s.selectedForm) return;

		this.s.permissionsSaving = true;

		const response = await formsApi.updatePermissions(this.s.selectedForm.id, [
			{
				subject_type: 'role',
				subject_id: 'member',
				policy: {
					actions: this.s.memberPermissionDraft,
					record_scope: this.s.memberRecordScopeDraft,
					fields: this.memberFieldPolicy()
				}
			}
		]);

		this.s.permissionsSaving = false;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));

			return;
		}

		this.s.formPermissions = response.data;
		this.s.memberPermissionDraft = rolePermissionActions(response.data, 'member');
		this.s.memberRecordScopeDraft = roleRecordScope(response.data, 'member');
		this.s.memberFieldReadDraft = this.roleFieldReadPermissions(response.data, 'member');
		this.s.memberFieldWriteDraft = this.roleFieldWritePermissions(response.data, 'member');
		toast.success(translate('forms.permissionsSaved'));
	};

	defaultFieldWriteDraft = (): Record<string, boolean> => {
		return Object.fromEntries(this.s.fields.map((field) => [field.key, true]));
	};

	defaultFieldReadDraft = (): Record<string, boolean> => {
		return Object.fromEntries(this.s.fields.map((field) => [field.key, true]));
	};

	roleFieldReadPermissions = (
		permissions: FormPermissions,
		role: 'owner' | 'admin' | 'member'
	): Record<string, boolean> => {
		const row = permissions.policies.find(
			(policy) => policy.subject_type === 'role' && policy.subject_id === role
		);
		const fieldPolicies = row?.policy?.fields ?? {};

		return Object.fromEntries(
			this.s.fields.map((field) => [field.key, fieldPolicies[field.key]?.read ?? true])
		);
	};

	roleFieldWritePermissions = (
		permissions: FormPermissions,
		role: 'owner' | 'admin' | 'member'
	): Record<string, boolean> => {
		const row = permissions.policies.find(
			(policy) => policy.subject_type === 'role' && policy.subject_id === role
		);
		const fieldPolicies = row?.policy?.fields ?? {};

		return Object.fromEntries(
			this.s.fields.map((field) => [field.key, fieldPolicies[field.key]?.write ?? true])
		);
	};

	memberFieldPolicy = (): Record<string, { read?: boolean; write?: boolean }> => {
		const policy: Record<string, { read?: boolean; write?: boolean }> = {};

		for (const field of this.s.fields) {
			const fieldPolicy: { read?: boolean; write?: boolean } = {};

			if (this.s.memberFieldReadDraft[field.key] === false) fieldPolicy.read = false;
			if (this.s.memberFieldWriteDraft[field.key] === false) fieldPolicy.write = false;
			if (Object.keys(fieldPolicy).length > 0) policy[field.key] = fieldPolicy;
		}

		return policy;
	};

	setMemberPermission = (action: FormPermissionAction, enabled: boolean) => {
		this.s.memberPermissionDraft = { ...this.s.memberPermissionDraft, [action]: enabled };
	};

	setMemberRecordScope = (scope: FormRecordScope) => {
		this.s.memberRecordScopeDraft = scope;
	};

	setMemberFieldRead = (fieldKey: string, enabled: boolean) => {
		this.s.memberFieldReadDraft = { ...this.s.memberFieldReadDraft, [fieldKey]: enabled };

		if (!enabled) {
			this.s.memberFieldWriteDraft = { ...this.s.memberFieldWriteDraft, [fieldKey]: false };
		}
	};

	setMemberFieldWrite = (fieldKey: string, enabled: boolean) => {
		this.s.memberFieldWriteDraft = { ...this.s.memberFieldWriteDraft, [fieldKey]: enabled };

		if (enabled) {
			this.s.memberFieldReadDraft = { ...this.s.memberFieldReadDraft, [fieldKey]: true };
		}
	};

	memberFieldHidden = (field: FormField): boolean => {
		return this.s.memberFieldReadDraft[field.key] === false;
	};

	memberFieldLocked = (field: FormField): boolean => {
		return this.s.memberFieldWriteDraft[field.key] === false;
	};
}
