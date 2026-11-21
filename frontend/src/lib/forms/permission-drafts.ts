import type { FormPermissionAction, FormPermissions } from '#lib/api/forms.js';
import type { FormRecordScope } from '#lib/forms/page-types.js';
import { translate } from '#lib/forms/i18n.js';
import { formPermissionActions } from '#lib/forms/page-constants.js';

export function defaultPermissionActions(): Record<FormPermissionAction, boolean> {
	return Object.fromEntries(formPermissionActions.map((action) => [action, true])) as Record<
		FormPermissionAction,
		boolean
	>;
}

export function rolePermissionActions(
	permissions: FormPermissions,
	role: 'owner' | 'admin' | 'member'
): Record<FormPermissionAction, boolean> {
	const row = permissions.policies.find(
		(policy) => policy.subject_type === 'role' && policy.subject_id === role
	);
	const actions = row?.policy?.actions ?? {};

	return Object.fromEntries(
		formPermissionActions.map((action) => [action, actions[action] ?? true])
	) as Record<FormPermissionAction, boolean>;
}

export function roleRecordScope(
	permissions: FormPermissions,
	role: 'owner' | 'admin' | 'member'
): FormRecordScope {
	const row = permissions.policies.find(
		(policy) => policy.subject_type === 'role' && policy.subject_id === role
	);

	return row?.policy?.record_scope === 'owned' ? 'owned' : 'all';
}

export function permissionActionLabel(action: FormPermissionAction): string {
	return translate(`forms.permissionActions.${action.replace('.', '_')}`);
}

export function permissionFieldList(labels: string[]): string {
	return labels.length > 0 ? labels.join(', ') : translate('forms.noFieldPolicyEffects');
}
