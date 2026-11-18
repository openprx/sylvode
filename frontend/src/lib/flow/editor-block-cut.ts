/** Loaded only with the editor engine. Cut moves placements; physical body containers never move. */
import { LoroList, LoroMap, type LoroDoc, type LoroTreeNode, type UndoManager } from 'loro-crdt';
import type { Node } from 'prosemirror-model';
import { atomicEditorWrite, EditorPreflightError } from './editor-batch';
import { structureEntries, hasPageStructure } from './vendor/loro-prosemirror/page-structure';
import { createLoroMap, type LoroDocType } from './vendor/loro-prosemirror/lib';
import { issueBlockCutTicket, blockCutTicket, consumeBlockCutTicket } from './body/clipboard';
interface CutRecord {
	blocks: string;
	state: 'prepared' | 'cut';
	history?: string;
	entries: { cid: string; depth: number; content: string }[];
}
function fail(): never {
	throw new EditorPreflightError(
		'The cut blocks changed or are unavailable. Undo the cut or copy them again.'
	);
}
function placementHistory(doc: LoroDocType, ids: Set<string>): string {
	return JSON.stringify(
		(doc as LoroDoc)
			.getTree('flow_editor_structure')
			.getNodes({ withDeleted: true })
			.filter((node) => ids.has(node.data.get('block_cid') as string))
			.map((node) => node.id)
			.sort()
	);
}

export function prepareBlockCut(doc: LoroDocType, ids: string[], blocks: unknown[]): string {
	if (!ids.length || ids.length > 256 || new Set(ids).size !== ids.length) fail();
	const physical = doc.getMap('doc').get('children');
	if (!(physical instanceof LoroList)) fail();
	const all: { block: LoroMap; depth: number }[] = hasPageStructure(doc)
		? structureEntries(doc)
		: (physical.toArray() as unknown as LoroMap[]).map((block) => {
				if (!(block instanceof LoroMap)) fail();
				const attrs = block.get('attributes');
				const depth = attrs instanceof LoroMap ? Number(attrs.get('indent') ?? 0) : 0;
				if (!Number.isInteger(depth) || depth < 0 || depth > 7) fail();
				return { block, depth };
			});
	const selected = new Set(ids);
	const entries = all.filter((entry) => selected.has(entry.block.id));
	if (
		entries.length !== ids.length ||
		entries.some((entry, index) => entry.block.id !== ids[index])
	)
		fail();
	const start = all.indexOf(entries[0]);
	if (all.slice(start, start + ids.length).some((entry) => !selected.has(entry.block.id))) fail();
	// A parent cut always owns its full subtree, never strands hidden children.
	for (const entry of entries) {
		const index = all.indexOf(entry);
		for (let i = index + 1; i < all.length && all[i].depth > entry.depth; i++)
			if (!selected.has(all[i].block.id)) fail();
	}
	const relative = new Map<string, number>();
	const record: CutRecord = {
		blocks: JSON.stringify(blocks),
		state: 'prepared',
		entries: entries.map((entry) => {
			const previous = all.slice(0, all.indexOf(entry)).reverse();
			const parent = previous.find((item) => item.depth < entry.depth);
			const depth =
				parent && relative.has(parent.block.id) ? relative.get(parent.block.id)! + 1 : 0;
			relative.set(entry.block.id, depth);
			return { cid: entry.block.id, depth, content: JSON.stringify(entry.block.toJSON()) };
		})
	};
	return issueBlockCutTicket(doc, record);
}
function recordFor(doc: LoroDocType, ticket: unknown, blocks: unknown[]): CutRecord {
	const record = blockCutTicket<CutRecord>(doc, ticket);
	if (!record || record.blocks !== JSON.stringify(blocks) || !hasPageStructure(doc)) fail();
	return record;
}
export function applyBlockCut(
	doc: LoroDocType,
	ticket: string,
	blocks: unknown[],
	manager: UndoManager,
	fallback: Node
): void {
	const record = recordFor(doc, ticket, blocks),
		all = structureEntries(doc);
	if (record.state !== 'prepared') fail();
	const selected = new Set(record.entries.map((entry) => entry.cid));
	for (const entry of record.entries) {
		const active = all.find((item) => item.block.id === entry.cid);
		if (!active || JSON.stringify(active.block.toJSON()) !== entry.content) fail();
	}
	// The asynchronous old-Page initialization cannot silently capture newly added descendants.
	const currentRelative = new Map<string, number>();
	for (const entry of all) {
		if (!selected.has(entry.block.id)) continue;
		const parentCid = entry.tree.parent()?.data.get('block_cid');
		const relative =
			typeof parentCid === 'string' && currentRelative.has(parentCid)
				? currentRelative.get(parentCid)! + 1
				: 0;
		currentRelative.set(entry.block.id, relative);
		if (record.entries.find((item) => item.cid === entry.block.id)?.depth !== relative) fail();
		const index = all.indexOf(entry);
		for (let i = index + 1; i < all.length && all[i].depth > entry.depth; i++)
			if (!selected.has(all[i].block.id)) fail();
	}
	const tree = (doc as LoroDoc).getTree('flow_editor_structure');
	const roots = all.filter(
		(entry) =>
			selected.has(entry.block.id) &&
			!selected.has(entry.tree.parent()?.data.get('block_cid') as string)
	);
	const needsFallback = all.length === record.entries.length;
	const physical = doc.getMap('doc').get('children');
	if (!(physical instanceof LoroList)) fail();
	atomicEditorWrite(
		doc,
		() => {
			if (needsFallback) {
				const carrier = createLoroMap(physical, null, fallback, new Map());
				doc.commit({ origin: 'sys:storage' });
				const placement = tree.createNode();
				placement.data.set('block_cid', carrier.id);
			}
			for (const entry of roots) tree.delete(entry.tree.id);
			doc.commit({ origin: 'flow:structure' });
		},
		{ undoManager: manager, groupIntent: true, protectStorage: needsFallback }
	);
	record.state = 'cut';
	record.history = placementHistory(doc, selected);
}
export function pasteBlockCut(
	doc: LoroDocType,
	ticket: string,
	blocks: unknown[],
	targetId: string,
	manager: UndoManager
): string[] {
	const record = recordFor(doc, ticket, blocks),
		all = structureEntries(doc);
	if (
		record.state !== 'cut' ||
		record.history !== placementHistory(doc, new Set(record.entries.map((entry) => entry.cid)))
	)
		fail();
	const active = new Set<string>(all.map((entry) => entry.block.id));
	const target = all.find((entry) => entry.block.id === targetId);
	if (!target || record.entries.some((entry) => active.has(entry.cid))) fail();
	if (Math.max(...record.entries.map((entry) => entry.depth)) + target.depth > 7) fail();
	const physical = doc.getMap('doc').get('children');
	if (!(physical instanceof LoroList)) fail();
	const carriers = new Map<string, LoroMap>(
		(physical.toArray() as unknown as LoroMap[])
			.filter((block) => block instanceof LoroMap)
			.map((block) => [block.id, block])
	);
	for (const entry of record.entries) {
		const block = carriers.get(entry.cid);
		if (!block || !('toJSON' in block) || JSON.stringify(block.toJSON()) !== entry.content) fail();
	}
	const tree = (doc as LoroDoc).getTree('flow_editor_structure');
	const parent = target.tree.parent();
	let rootIndex = (target.tree.index() ?? 0) + 1;
	atomicEditorWrite(
		doc,
		() => {
			const stack: LoroTreeNode[] = [];
			for (const entry of record.entries) {
				const ancestor = entry.depth ? stack[entry.depth - 1] : parent;
				const node = tree.createNode(
					ancestor?.id,
					entry.depth ? (ancestor?.children()?.length ?? 0) : rootIndex++
				);
				node.data.set('block_cid', entry.cid);
				stack[entry.depth] = node;
				stack.length = entry.depth + 1;
			}
			doc.commit({ origin: 'flow:structure' });
		},
		{ undoManager: manager, groupIntent: true }
	);
	consumeBlockCutTicket(ticket);
	return record.entries.map((entry) => entry.cid);
}
