/** Source-only explicit review. Never imported by recipient projection/provider modules. */
import type { LoroDocType } from 'loro-prosemirror';
import { LoroList, LoroMap, LoroText, VersionVector } from 'loro-crdt';
import { hasPageStructure, structureEntries } from '../vendor/loro-prosemirror/page-structure';
export interface BlockShareReview {
	root_block_cid: string;
	selection: 'block' | 'current_subtree';
	expected_frontier: string;
	members: { block_cid: string; depth: number; kind: string; text: string; truncated: boolean }[];
}
export class ShareReviewError extends Error {
	constructor(readonly reason: 'source_changed' | 'unsupported' | 'limit') {
		super(reason);
	}
}
export function sourceVersion(doc: LoroDocType): string {
	const version = doc.version();
	try {
		return btoa(Array.from(version.encode(), (byte) => String.fromCharCode(byte)).join(''));
	} finally {
		version.free();
	}
}
/** Frontier tokens stay opaque for CAS; decoded VV equality proves local state, not encoded map order. */
export function nativeVersionMatches(local: VersionVector, frontier: string): boolean {
	let remote: VersionVector | undefined;
	try {
		remote = VersionVector.decode(Uint8Array.from(atob(frontier), (char) => char.charCodeAt(0)));
		return local.compare(remote) === 0;
	} catch {
		return false;
	} finally {
		remote?.free();
	}
}
export function sourceFrontierMatches(doc: LoroDocType, frontier: string): boolean {
	const local = doc.version();
	try {
		return nativeVersionMatches(local, frontier);
	} finally {
		local.free();
	}
}

export function captureBlockShareReview(
	doc: LoroDocType,
	root: string,
	selection: 'block' | 'current_subtree',
	frontier: string
): BlockShareReview {
	if (!sourceFrontierMatches(doc, frontier)) throw new ShareReviewError('source_changed');
	if (!hasPageStructure(doc)) throw new ShareReviewError('unsupported');
	const all = structureEntries(doc),
		index = all.findIndex((entry) => entry.block.id === root);
	if (index < 0) throw new ShareReviewError('source_changed');
	let end = index + 1;
	if (selection === 'current_subtree')
		while (end < all.length && all[end].depth > all[index].depth) end++;
	if (end - index > 256) throw new ShareReviewError('limit');
	const members = all.slice(index, end).map((entry) => {
		const kind = entry.block.get('nodeName'),
			children = entry.block.get('children'),
			attrs = entry.block.get('attributes');
		if (typeof kind !== 'string' || !(children instanceof LoroList) || !(attrs instanceof LoroMap))
			throw new ShareReviewError('unsupported');
		let text = '';
		if (kind === 'image')
			text = [attrs.get('alt'), attrs.get('caption')]
				.filter((value) => typeof value === 'string')
				.join('\n');
		else if (kind === 'attachment')
			text = [attrs.get('name'), attrs.get('caption')]
				.filter((value) => typeof value === 'string')
				.join('\n');
		else if (kind !== 'childPage')
			for (const child of children.toArray()) {
				if (!(child instanceof LoroText)) throw new ShareReviewError('unsupported');
				text += child.toString();
			}
		return {
			block_cid: entry.block.id,
			depth: entry.depth - all[index].depth,
			kind,
			text: text.slice(0, 2000),
			truncated: text.length > 2000
		};
	});
	return { root_block_cid: root, selection, expected_frontier: frontier, members };
}
