import type { FlowAppearance, FlowNavigatorNode } from '#lib/api/flow.js';
import { clientError } from './errors';

export interface NavigatorPlacement {
	readonly mountId: string;
	readonly objectId: string;
	readonly scopeId: string;
	readonly title: string;
	readonly appearance?: FlowAppearance;
	readonly status: 'active' | 'source_archived';
	readonly canRemove: boolean;
}

/** Placements never become canonical objects or entries in the navigator CRDT order map. */
export function navigatorPlacements(nodes: readonly FlowNavigatorNode[]): NavigatorPlacement[] {
	const items = new Map<string, NavigatorPlacement>();
	for (const node of nodes) {
		if (node.ownership !== 'external') continue;
		if (
			typeof node.mount_id !== 'string' ||
			!node.mount_id ||
			typeof node.scope_id !== 'string' ||
			!node.scope_id ||
			typeof node.object_id !== 'string' ||
			!node.object_id ||
			(node.mount_status !== undefined &&
				node.mount_status !== 'active' &&
				node.mount_status !== 'source_archived')
		) {
			throw clientError('unsupported_protocol', { reason: 'invalid_placement_metadata' });
		}
		const previous = items.get(node.mount_id);
		if (previous && (previous.objectId !== node.object_id || previous.scopeId !== node.scope_id)) {
			throw clientError('unsupported_protocol', { reason: 'conflicting_placement_identity' });
		}
		items.set(node.mount_id, {
			mountId: node.mount_id,
			objectId: node.object_id,
			scopeId: node.scope_id,
			title: node.title,
			appearance: node.appearance,
			status: node.mount_status ?? 'active',
			canRemove: node.can_remove === true
		});
	}
	return [...items.values()];
}
