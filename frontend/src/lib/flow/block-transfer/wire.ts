/** Transfer DTO checks are scoped to the original authorized Page pair, never the currently active Tab. */
import type {
	PreparedBlockTransferWire,
	PrepareBlockTransfer,
	BlockTransferReceiptWire,
	BlockTransferStatusWire,
	BlockTransferKind
} from './contracts';
import type { ReviewedBlockTransfer, BlockTransferReceipt, TransferDecision } from './controller';
const uuid = /^[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}$/i,
	cid = /^cid:\d+@\d+:Map$/;
function record(value: unknown): Record<string, unknown> {
	if (!value || typeof value !== 'object' || Array.isArray(value))
		throw new Error('invalid_transfer_response');
	return value as Record<string, unknown>;
}
function keys(value: Record<string, unknown>, allowed: string[]): void {
	if (Object.keys(value).some((key) => !allowed.includes(key)))
		throw new Error('invalid_transfer_response');
}
const number = (value: unknown) => Number.isSafeInteger(value) && Number(value) >= 0;
const string = (value: unknown) => typeof value === 'string' && value.length > 0;
function pagesMatch(
	value: unknown,
	source: string,
	target: string,
	kind: BlockTransferKind
): boolean {
	return (
		Array.isArray(value) &&
		value.every((id) => typeof id === 'string') &&
		value.length === (kind === 'copy' ? 1 : 2) &&
		new Set(value).size === value.length &&
		value.includes(target) &&
		(kind === 'copy' || value.includes(source))
	);
}
export function parsePreparedTransfer(
	value: unknown,
	sourceId: string,
	input: PrepareBlockTransfer
): { wire: PreparedBlockTransferWire; review: ReviewedBlockTransfer } {
	const root = record(value);
	keys(root, ['transfer_id', 'kind', 'state', 'expires_at', 'review', 'assets']);
	const review = record(root.review),
		assets = record(root.assets);
	keys(review, [
		'source_object_id',
		'target_object_id',
		'source_frontier',
		'target_frontier',
		'selection_digest',
		'block_count',
		'asset_count',
		'total_asset_bytes',
		'affected_share_count',
		'native_undo_barrier_pages'
	]);
	keys(assets, ['state', 'copied_count', 'total_bytes']);
	if (
		!uuid.test(String(root.transfer_id)) ||
		root.kind !== input.kind ||
		root.state !== 'prepared' ||
		typeof root.expires_at !== 'string' ||
		!Number.isFinite(Date.parse(root.expires_at)) ||
		review.source_object_id !== sourceId ||
		review.target_object_id !== input.target_object_id ||
		review.source_frontier !== input.expected_source_frontier ||
		review.target_frontier !== input.expected_target_frontier ||
		!string(review.selection_digest) ||
		!number(review.block_count) ||
		Number(review.block_count) < 1 ||
		!number(review.asset_count) ||
		!number(review.total_asset_bytes) ||
		!number(review.affected_share_count) ||
		!pagesMatch(review.native_undo_barrier_pages, sourceId, input.target_object_id, input.kind) ||
		assets.state !== 'ready' ||
		!number(assets.copied_count) ||
		!number(assets.total_bytes)
	)
		throw new Error('invalid_transfer_review');
	const wire = JSON.parse(JSON.stringify(value)) as PreparedBlockTransferWire;
	return {
		wire,
		review: {
			id: wire.transfer_id,
			kind: wire.kind,
			sourcePageId: sourceId,
			targetPageId: input.target_object_id,
			sourceFrontier: wire.review.source_frontier,
			targetFrontier: wire.review.target_frontier,
			digest: wire.review.selection_digest,
			expiresAt: Date.parse(wire.expires_at),
			memberCount: wire.review.block_count,
			assetCount: wire.review.asset_count,
			affectedShareCount: wire.review.affected_share_count,
			assetsReady: wire.assets.state === 'ready'
		}
	};
}
function parseReceipt(
	value: unknown,
	review: ReviewedBlockTransfer,
	undone: boolean
): BlockTransferReceipt {
	const receipt = record(value);
	keys(receipt, [
		'changed_page_ids',
		'source',
		'target',
		'mappings',
		'source_placeholder_cid',
		'affected_share_count',
		'undo'
	]);
	const source = record(receipt.source),
		target = record(receipt.target),
		undo = record(receipt.undo);
	for (const page of [source, target]) {
		keys(page, ['object_id', 'accepted_seq', 'frontier']);
		if (!number(page.accepted_seq) || !string(page.frontier))
			throw new Error('invalid_transfer_head');
	}
	keys(
		undo,
		undone ? ['state'] : ['state', 'expected_source_frontier', 'expected_target_frontier']
	);
	if (
		source.object_id !== review.sourcePageId ||
		target.object_id !== review.targetPageId ||
		!pagesMatch(receipt.changed_page_ids, review.sourcePageId, review.targetPageId, review.kind) ||
		!Array.isArray(receipt.mappings) ||
		!receipt.mappings.length ||
		!number(receipt.affected_share_count) ||
		(receipt.source_placeholder_cid !== null &&
			!cid.test(String(receipt.source_placeholder_cid))) ||
		(undone
			? undo.state !== 'unavailable'
			: undo.state !== 'available' ||
				!string(undo.expected_source_frontier) ||
				!string(undo.expected_target_frontier))
	)
		throw new Error('invalid_transfer_receipt');
	for (const raw of receipt.mappings) {
		const mapping = record(raw);
		keys(mapping, ['source_block_cid', 'target_block_cid', 'identity']);
		if (
			!cid.test(String(mapping.source_block_cid)) ||
			!cid.test(String(mapping.target_block_cid)) ||
			mapping.identity !== (review.kind === 'move' ? 'preserved' : 'new')
		)
			throw new Error('invalid_transfer_mapping');
	}
	const typed = receipt as unknown as BlockTransferReceiptWire;
	return {
		id: review.id,
		kind: review.kind,
		sourcePageId: review.sourcePageId,
		targetPageId: review.targetPageId,
		sourcePostFrontier: typed.source.frontier,
		targetPostFrontier: typed.target.frontier,
		firstTargetBlockCid: typed.mappings[0]?.target_block_cid ?? null
	};
}
export function transferDecision(value: unknown, review: ReviewedBlockTransfer): TransferDecision {
	const root = record(value);
	keys(root, ['transfer_id', 'kind', 'state', 'expires_at', 'receipt', 'failure_reason']);
	if (root.transfer_id !== review.id || root.kind !== review.kind)
		throw new Error('transfer_scope_mismatch');
	if (root.state === 'accepted' || root.state === 'undone')
		return {
			state: root.state === 'accepted' ? 'applied' : 'undone',
			id: review.id,
			kind: review.kind,
			receipt:
				root.receipt === null ? null : parseReceipt(root.receipt, review, root.state === 'undone')
		};
	if (['not_applied', 'expired', 'cancelled'].includes(String(root.state)))
		return {
			state: 'not_applied',
			reason: typeof root.failure_reason === 'string' ? root.failure_reason : String(root.state)
		};
	if (root.state === 'prepared') return { state: 'pending' };
	throw new Error('invalid_transfer_status');
}
export function metadataOnlyStatus(value: unknown): BlockTransferStatusWire | null {
	try {
		const root = record(value);
		keys(root, ['transfer_id', 'kind', 'state', 'expires_at', 'receipt', 'failure_reason']);
		if (
			!uuid.test(String(root.transfer_id)) ||
			!['move', 'copy'].includes(String(root.kind)) ||
			!['prepared', 'accepted', 'not_applied', 'undone', 'expired', 'cancelled'].includes(
				String(root.state)
			) ||
			root.receipt !== null ||
			typeof root.expires_at !== 'string' ||
			(root.failure_reason !== null && typeof root.failure_reason !== 'string')
		)
			return null;
		return JSON.parse(JSON.stringify(value)) as BlockTransferStatusWire;
	} catch {
		return null;
	}
}
