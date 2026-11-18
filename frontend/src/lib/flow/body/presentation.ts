import type { FlowPublicBlock } from '#lib/api/flow.js';
export type PublicBodyAtom =
	| { kind: 'image' | 'attachment'; assetId: string; alt: string; caption: string; name: string }
	| { kind: 'childPage' };
export function publicBodyAtom(block: FlowPublicBlock): PublicBodyAtom | null {
	// The public child contract deliberately redacts target identity, title and text.
	if (block.kind === 'childPage') return { kind: 'childPage' };
	if (
		(block.kind === 'image' || block.kind === 'attachment') &&
		/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(block.asset_id ?? '')
	)
		return {
			kind: block.kind,
			assetId: block.asset_id!,
			alt: typeof block.alt === 'string' ? block.alt : '',
			caption: typeof block.caption === 'string' ? block.caption : '',
			name: typeof block.name === 'string' ? block.name : ''
		};
	return null;
}
