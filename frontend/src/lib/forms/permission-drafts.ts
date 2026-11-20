import type { FormPermissionAction } from '#lib/api/forms.js';
import { formPermissionActions } from '#lib/forms/page-constants.js';

export function defaultPermissionActions(): Record<FormPermissionAction, boolean> {
	return Object.fromEntries(formPermissionActions.map((action) => [action, true])) as Record<
		FormPermissionAction,
		boolean
	>;
}
