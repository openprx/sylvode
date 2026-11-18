/** Next sub-batch only. No routes or buttons enabled in the frozen sharing runtime. */
export type BlockTransferKind = 'move' | 'copy';
export interface PrepareBlockTransfer {
	kind: BlockTransferKind;
	target_object_id: string;
	root_block_cid: string;
	selection: 'block' | 'current_subtree';
	target_parent_block_cid: string | null;
	/** Native Tree sibling index, never physical-list/preorder position. */
	target_index: number;
	expected_source_frontier: string;
	expected_target_frontier: string;
}
/** A receipt-driven server operation is deliberately not a single-document native Undo item. */
export interface TransferUndoPolicy {
	changedPageIds: readonly string[];
	nativeHistory: 'reset_after_accepted_only';
	pending: 'lock_editing_and_native_undo';
	unknown: 'keep_locked_until_authoritative_receipt';
	notApplied: 'preserve_history';
	inverse: 'explicit_receipt_action_with_head_and_generation_CAS';
}
export function transferUndoPolicy(
	kind: BlockTransferKind,
	sourceId: string,
	targetId: string
): TransferUndoPolicy {
	return {
		changedPageIds: kind === 'copy' ? [targetId] : [sourceId, targetId],
		nativeHistory: 'reset_after_accepted_only',
		pending: 'lock_editing_and_native_undo',
		unknown: 'keep_locked_until_authoritative_receipt',
		notApplied: 'preserve_history',
		inverse: 'explicit_receipt_action_with_head_and_generation_CAS'
	};
}

/** Frozen backend wire, distinct from the controller's normalized internal view model. */
export interface PreparedBlockTransferWire {
	transfer_id: string;
	kind: BlockTransferKind;
	state: 'prepared';
	expires_at: string;
	review: {
		source_object_id: string;
		target_object_id: string;
		source_frontier: string;
		target_frontier: string;
		selection_digest: string;
		block_count: number;
		asset_count: number;
		total_asset_bytes: number;
		affected_share_count: number;
		native_undo_barrier_pages: string[];
	};
	assets: { state: 'ready'; copied_count: number; total_bytes: number };
}
export interface CommitBlockTransfer {
	expected_source_frontier: string;
	expected_target_frontier: string;
	selection_digest: string;
}
export interface UndoBlockTransfer {
	expected_source_frontier: string;
	expected_target_frontier: string;
}
export interface BlockTransferReceiptWire {
	changed_page_ids: string[];
	source: { object_id: string; accepted_seq: number; frontier: string };
	target: { object_id: string; accepted_seq: number; frontier: string };
	mappings: { source_block_cid: string; target_block_cid: string; identity: 'preserved' | 'new' }[];
	source_placeholder_cid: string | null;
	affected_share_count: number;
	undo:
		| { state: 'available'; expected_source_frontier: string; expected_target_frontier: string }
		| { state: 'unavailable' };
}
export interface AcceptedBlockTransferWire {
	transfer_id: string;
	kind: BlockTransferKind;
	state: 'accepted' | 'undone';
	receipt: BlockTransferReceiptWire | null;
}
export interface BlockTransferStatusWire {
	transfer_id: string;
	kind: BlockTransferKind;
	state: 'prepared' | 'accepted' | 'not_applied' | 'undone' | 'expired' | 'cancelled';
	expires_at: string;
	receipt: BlockTransferReceiptWire | null;
	failure_reason: string | null;
}
