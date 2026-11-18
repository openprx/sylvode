/** Authorized normal-Page preview only. Never used by a shared-recipient provider. */
import type { LoroDocType } from 'loro-prosemirror';
import { LoroList, LoroMap, LoroText } from 'loro-crdt';
import { hasPageStructure, structureEntries } from '../vendor/loro-prosemirror/page-structure';
export interface TransferPreviewBlock {
	cid: string;
	depth: number;
	kind: string;
	text: string;
}
export function transferPreviewBlocks(doc: LoroDocType): TransferPreviewBlock[] {
	const root = doc.getMap('doc'),
		physical = root.get('children');
	if (!(physical instanceof LoroList)) throw new Error('unsupported_transfer_page');
	const entries: { block: LoroMap; depth: number }[] = hasPageStructure(doc)
		? structureEntries(doc)
		: (physical.toArray() as unknown as LoroMap[]).map((block) => {
				const attrs = block.get('attributes');
				return { block, depth: attrs instanceof LoroMap ? Number(attrs.get('indent') ?? 0) : 0 };
			});
	return entries.map(({ block, depth }) => {
		const kind = String(block.get('nodeName')),
			children = block.get('children'),
			attrs = block.get('attributes');
		let text = '';
		if (kind === 'attachment' && attrs instanceof LoroMap) text = String(attrs.get('name') ?? '');
		else if (kind === 'image' && attrs instanceof LoroMap) text = String(attrs.get('alt') ?? '');
		else if (kind !== 'childPage' && children instanceof LoroList)
			text = children
				.toArray()
				.filter((value) => value instanceof LoroText)
				.map((value) => (value as LoroText).toString())
				.join('');
		return { cid: block.id, depth, kind, text: Array.from(text).slice(0, 160).join('') };
	});
}
export function targetSiblingCount(
	blocks: readonly TransferPreviewBlock[],
	parentCid: string | null
): number {
	if (parentCid === null) return blocks.filter((block) => block.depth === 0).length;
	const index = blocks.findIndex((block) => block.cid === parentCid);
	if (index < 0) throw new Error('target_parent_unavailable');
	let count = 0;
	for (let i = index + 1; i < blocks.length && blocks[i].depth > blocks[index].depth; i++)
		if (blocks[i].depth === blocks[index].depth + 1) count++;
	return count;
}
