import type { FlowObjectView } from '#lib/api/flow.js';
import type { BlockTransferKind, PrepareBlockTransfer } from './contracts';
export type TransferPlanFailure =
	| 'same_page'
	| 'source_unavailable'
	| 'target_unavailable'
	| 'source_permission'
	| 'target_permission'
	| 'unsaved_content'
	| 'open_and_sync'
	| 'invalid_placement'
	| 'explicit_subtree_required'
	| 'missing_block'
	| 'invalid_hierarchy';
export class TransferPlanError extends Error {
	constructor(readonly reason: TransferPlanFailure) {
		super(reason);
	}
}
export interface TransferPageReadiness {
	object: Pick<
		FlowObjectView,
		'id' | 'title' | 'object_type' | 'lifecycle_status' | 'permitted_actions' | 'editor_state'
	>;
	/** Original server token, kept byte-for-byte for server CAS. */
	frontier: string;
	nativeVersionMatches: boolean;
	pending: number;
	persistedPending: number;
	readinessKnown: boolean;
	localReplica: 'open' | 'not_open';
	bodyJobs: number;
	enqueuePending: boolean;
	paused: boolean;
}
export interface TransferBlock {
	cid: string;
	depth: number;
}
const cid = /^cid:\d+@\d+:Map$/;
function ready(
	page: TransferPageReadiness,
	side: 'source' | 'target',
	kind: BlockTransferKind
): void {
	if (
		page.object.object_type !== 'page' ||
		page.object.lifecycle_status !== 'active' ||
		page.object.editor_state !== 'ready'
	)
		throw new TransferPlanError(side === 'source' ? 'source_unavailable' : 'target_unavailable');
	const required = kind === 'move' ? 'manage' : side === 'source' ? 'view' : 'edit';
	if (!page.object.permitted_actions?.includes(required))
		throw new TransferPlanError(side === 'source' ? 'source_permission' : 'target_permission');
	if (!page.readinessKnown || page.persistedPending !== 0)
		throw new TransferPlanError('open_and_sync');
	if (
		page.paused ||
		page.pending !== 0 ||
		page.bodyJobs !== 0 ||
		page.enqueuePending ||
		(page.localReplica === 'open' && !page.nativeVersionMatches) ||
		!page.frontier
	)
		throw new TransferPlanError('unsaved_content');
}
export function selectedTransferBlocks(
	blocks: readonly TransferBlock[],
	root: string,
	kind: BlockTransferKind,
	selection: 'block' | 'current_subtree'
): TransferBlock[] {
	const ids = new Set<string>();
	let previous = -1;
	for (const block of blocks) {
		if (
			!cid.test(block.cid) ||
			ids.has(block.cid) ||
			!Number.isInteger(block.depth) ||
			block.depth < 0 ||
			block.depth > 7 ||
			block.depth > previous + 1
		)
			throw new TransferPlanError('invalid_hierarchy');
		ids.add(block.cid);
		previous = block.depth;
	}
	const index = blocks.findIndex((block) => block.cid === root);
	if (index < 0) throw new TransferPlanError('missing_block');
	let end = index + 1;
	while (end < blocks.length && blocks[end].depth > blocks[index].depth) end++;
	if (kind === 'move' && selection === 'block' && end > index + 1)
		throw new TransferPlanError('explicit_subtree_required');
	const selected = blocks.slice(index, selection === 'block' ? index + 1 : end);
	return selected.map((block) => ({ cid: block.cid, depth: block.depth - blocks[index].depth }));
}
export function planBlockTransfer(input: {
	kind: BlockTransferKind;
	source: TransferPageReadiness;
	target: TransferPageReadiness;
	rootCid: string;
	selection: 'block' | 'current_subtree';
	sourceBlocks: readonly TransferBlock[];
	targetParentCid: string | null;
	targetSiblingIndex: number;
}): { request: PrepareBlockTransfer; fixedPreview: TransferBlock[] } {
	if (input.source.object.id === input.target.object.id) throw new TransferPlanError('same_page');
	ready(input.source, 'source', input.kind);
	ready(input.target, 'target', input.kind);
	if (
		!Number.isSafeInteger(input.targetSiblingIndex) ||
		input.targetSiblingIndex < 0 ||
		(input.targetParentCid !== null && !cid.test(input.targetParentCid))
	)
		throw new TransferPlanError('invalid_placement');
	const fixedPreview = selectedTransferBlocks(
		input.sourceBlocks,
		input.rootCid,
		input.kind,
		input.selection
	);
	return {
		request: {
			kind: input.kind,
			target_object_id: input.target.object.id,
			root_block_cid: input.rootCid,
			selection: input.selection,
			target_parent_block_cid: input.targetParentCid,
			target_index: input.targetSiblingIndex,
			expected_source_frontier: input.source.frontier,
			expected_target_frontier: input.target.frontier
		},
		fixedPreview
	};
}
