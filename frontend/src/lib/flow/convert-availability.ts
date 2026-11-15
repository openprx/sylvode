import type { FlowObjectView } from '../api/flow';
export type ConversionAvailability =
	| { mode: 'unavailable' }
	| { mode: 'same_workspace'; workspaceId: string }
	| { mode: 'explicit_workspace' };
/** The source's actual binding/capability is authority, never the route or selected workspace. */
export function conversionAvailability(
	object: Pick<
		FlowObjectView,
		| 'object_type'
		| 'lifecycle_status'
		| 'workspace_id'
		| 'permitted_actions'
		| 'supported_forms_conversion'
		| 'forms_conversion_destination_mode'
	>
): ConversionAvailability {
	if (
		object.object_type !== 'page' ||
		object.lifecycle_status !== 'active' ||
		(object.permitted_actions !== undefined && !object.permitted_actions.includes('edit'))
	)
		return { mode: 'unavailable' };
	if (object.supported_forms_conversion === true) {
		if (object.forms_conversion_destination_mode === 'explicit_workspace')
			return { mode: 'explicit_workspace' };
		if (object.forms_conversion_destination_mode === 'same_workspace' && object.workspace_id)
			return { mode: 'same_workspace', workspaceId: object.workspace_id };
		return { mode: 'unavailable' };
	}
	// Older genuine legacy DTOs retain their existing workflow. Missing personal metadata fails closed.
	if (
		object.supported_forms_conversion === undefined &&
		object.forms_conversion_destination_mode === undefined &&
		object.workspace_id
	)
		return { mode: 'same_workspace', workspaceId: object.workspace_id };
	return { mode: 'unavailable' };
}
