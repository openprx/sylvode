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

/**
 * The destination the convert page starts with. A same-workspace conversion is fixed to the
 * source's workspace. An explicit-destination conversion of a workspace Page starts with that
 * workspace when the user may pick it (it is among the offered workspaces); a personal source, or a
 * source workspace that is not offered, starts with no destination chosen.
 */
export function defaultConversionDestination(
	availability: ConversionAvailability,
	sourceWorkspaceId: string | null | undefined,
	offered: readonly { id: string }[]
): string {
	if (availability.mode === 'same_workspace')
		return offered.some((workspace) => workspace.id === availability.workspaceId)
			? availability.workspaceId
			: '';
	if (availability.mode === 'explicit_workspace' && sourceWorkspaceId)
		return offered.some((workspace) => workspace.id === sourceWorkspaceId) ? sourceWorkspaceId : '';
	return '';
}
