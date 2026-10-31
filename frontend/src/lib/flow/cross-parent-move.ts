// Cross-parent Flow moves. Same-parent reorder stays in `navigator-reorder`; a parent change is
// the `move_object` command (`target_object_id` is the new parent). The server rejects cycles.

export interface MoveNode {
	readonly id: string;
	readonly parent_id: string | null;
}

export interface MoveObjectCommand {
	readonly type: 'move_object';
	readonly payload: { readonly target_object_id: string };
}

export function moveObjectCommand(targetObjectId: string): MoveObjectCommand {
	return { type: 'move_object', payload: { target_object_id: targetObjectId } };
}

/** Ids strictly below `ancestorId`. */
export function descendantIds(nodes: readonly MoveNode[], ancestorId: string): Set<string> {
	const children = new Map<string, string[]>();
	for (const node of nodes) {
		if (!node.parent_id) continue;
		const list = children.get(node.parent_id) ?? [];
		list.push(node.id);
		children.set(node.parent_id, list);
	}
	const out = new Set<string>();
	const stack = [...(children.get(ancestorId) ?? [])];
	while (stack.length > 0) {
		const id = stack.pop();
		if (!id || out.has(id)) continue;
		out.add(id);
		for (const child of children.get(id) ?? []) stack.push(child);
	}
	return out;
}

/** Pages that can become the new parent: not self, not a descendant, not the current parent. */
export function moveInsideTargets(nodes: readonly MoveNode[], objectId: string): string[] {
	const blocked = descendantIds(nodes, objectId);
	blocked.add(objectId);
	const self = nodes.find((node) => node.id === objectId);
	if (self?.parent_id) blocked.add(self.parent_id);
	return nodes.filter((node) => !blocked.has(node.id)).map((node) => node.id);
}

/**
 * New parent for "move outside": the current parent's parent.
 * Returns null when the object is already directly under a parent that is not in `nodes`
 * (the navigator root) or the parent itself has no parent.
 */
export function moveOutsideTarget(nodes: readonly MoveNode[], objectId: string): string | null {
	const self = nodes.find((node) => node.id === objectId);
	if (!self?.parent_id) return null;
	const parent = nodes.find((node) => node.id === self.parent_id);
	if (!parent?.parent_id) return null;
	return parent.parent_id;
}
