/** SV patch: Tree order references immutable physical block containers. No container reparenting. */
import { LoroMap, LoroList, type LoroTreeNode, type LoroDoc, type ContainerID } from 'loro-crdt';
import type { LoroDocType, LoroNodeMapping, LoroNode, LoroChildrenListType } from './lib';
const contexts = new WeakMap<LoroNodeMapping, LoroDocType>();
export function attachStructureContext(mapping: LoroNodeMapping, doc: LoroDocType): void {
	contexts.set(mapping, doc);
}
export function hasPageStructure(doc: LoroDocType): boolean {
	return (
		(doc as LoroDoc).getMap('flow_editor_structure_meta').get('initialized') === true &&
		(doc as LoroDoc).getMap('flow_editor_structure_meta').get('version') === 1
	);
}
export function structureEntries(
	doc: LoroDocType
): { block: LoroNode; tree: LoroTreeNode; depth: number }[] {
	const tree = (doc as LoroDoc).getTree('flow_editor_structure');
	const result: { block: LoroNode; tree: LoroTreeNode; depth: number }[] = [];
	const seen = new Set<string>();
	const walk = (nodes: LoroTreeNode[], depth: number) => {
		if (nodes.length && depth > 7) throw new Error('Page hierarchy exceeds supported depth');
		for (const node of nodes) {
			const cid = node.data.get('block_cid');
			if (typeof cid !== 'string' || seen.has(cid))
				throw new Error('Invalid Page structure identity');
			seen.add(cid);
			const block = doc.getContainerById(cid as ContainerID);
			if (!(block instanceof LoroMap)) throw new Error('Page structure references unknown block');
			result.push({ block: block as LoroNode, tree: node, depth });
			walk(node.children() ?? [], depth + 1);
		}
	};
	walk(tree.roots(), 0);
	return result;
}
export function structuredRoot(obj: LoroNode, mapping: LoroNodeMapping): LoroDocType | null {
	const doc = contexts.get(mapping);
	return doc && obj.id === doc.getMap('doc').id && hasPageStructure(doc) ? doc : null;
}
export function logicalRootChildren(doc: LoroDocType): LoroNode[] {
	return structureEntries(doc).map((entry) => entry.block);
}
/** Only the root writer uses this mutable logical list. Tree changes are committed after diffing. */
export function structuredWriter(doc: LoroDocType,requiresModel5=false): {
	list: LoroChildrenListType;
	finish: (depths: number[]) => void;
	beforeUserWrites:()=>void;
} {
	const physical = doc.getMap('doc').get('children') as LoroList;
	const entries = logicalRootChildren(doc);
	const tree = (doc as LoroDoc).getTree('flow_editor_structure');
	const meta=(doc as LoroDoc).getMap('flow_editor_structure_meta');
	let needsFloor=requiresModel5&&meta.get('model')!=='rich_page_v5';
	const writeFloor=()=>{if(needsFloor){meta.set('model','rich_page_v5');needsFloor=false;}};
	const beforeUserWrites=()=>{if(needsFloor){writeFloor();doc.commit({origin:'sys:storage'});}};
	const list = {
		withStorageCreation: (create: () => LoroMap) => {
			doc.commit({ origin: 'loroSyncPlugin' });
			const carrier = create();
			writeFloor();
			doc.commit({ origin: 'sys:storage' });
			return carrier;
		},
		get: (index: number) => entries[index],
		get length() {
			return entries.length;
		},
		toArray: () => entries.slice(),
		delete: (index: number, count: number) => {
			entries.splice(index, count);
		},
		insertContainer: (index: number, container: LoroMap) => {
			const attached = physical.insertContainer(physical.length, container) as LoroNode;
			entries.splice(index, 0, attached);
			return attached;
		}
	} as unknown as LoroChildrenListType;
	const finish = (depths: number[]) => {
		// Undo recreates a placement with the same body CID and a new TreeID. Historical
		// tombstones must never shadow that unique active placement on the next edit.
		const existing = new Map(structureEntries(doc).map((entry) => [entry.block.id, entry.tree]));
		const stack: LoroTreeNode[] = [];
		const siblingCounts = new Map<string, number>();
		const keep = new Set<string>();
		for (const [entryIndex, block] of entries.entries()) {
			const depth = depths[entryIndex] ?? 0;
			if (depth < 0 || depth > 7 || depth > stack.length)
				throw new Error('Invalid Page subtree depth');
			const parent = depth ? stack[depth - 1] : undefined;
			const parentKey = parent?.id ?? '';
			const index = siblingCounts.get(parentKey) ?? 0;
			siblingCounts.set(parentKey, index + 1);
			let node = existing.get(block.id);
			if (!node || node.isDeleted()) {
				node = tree.createNode(parent?.id, index);
				node.data.set('block_cid', block.id);
			} else if (node.parent()?.id !== parent?.id || node.index() !== index)
				tree.move(node.id, parent?.id, index);
			stack[depth] = node;
			stack.length = depth + 1;
			keep.add(block.id);
		}
		for (const node of tree.getNodes()) {
			const cid = node.data.get('block_cid');
			if (typeof cid === 'string' && !keep.has(cid) && !node.isDeleted()) tree.delete(node.id);
		}
	};
	return { list, finish,beforeUserWrites };
}
