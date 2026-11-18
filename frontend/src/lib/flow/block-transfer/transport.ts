import { blockTransferApi } from './client';
import { parsePreparedTransfer, transferDecision } from './wire';
import type {
	TransferOperationTransport,
	ReviewedBlockTransfer,
	TransferDecision,
	BlockTransferReceipt
} from './controller';
import type { ApiResult } from '#lib/api/client.js';
/** Captured epoch is part of request ownership, never a server privilege supplied by the client. */
export function blockTransferTransport(epoch: number): TransferOperationTransport {
	const reviews = new Map<string, ReviewedBlockTransfer>();
	const decision = (
		result: ApiResult<unknown>,
		review: ReviewedBlockTransfer
	): TransferDecision => {
		if (result.code === 0 && result.data) return transferDecision(result.data, review);
		const details =
			result.details && typeof result.details === 'object'
				? (result.details as Record<string, unknown>)
				: null;
		return details?.write_state === 'not_applied'
			? {
					state: 'not_applied',
					reason: String(details.reason ?? result.error_code ?? 'not_applied')
				}
			: { state: 'unknown' };
	};
	return {
		remember: (review) => {
			reviews.set(review.id, review);
		},
		async prepare(source, input, key, signal) {
			const result = await blockTransferApi.prepare(source, input, key, epoch, signal);
			if (result.code !== 0 || !result.data) throw new Error('transfer_prepare_failed');
			const parsed = parsePreparedTransfer(result.data, source, input);
			reviews.set(parsed.review.id, parsed.review);
			return parsed.review;
		},
		async commit(review, key, signal) {
			reviews.set(review.id, review);
			return decision(
				await blockTransferApi.commit(
					review.id,
					{
						expected_source_frontier: review.sourceFrontier,
						expected_target_frontier: review.targetFrontier,
						selection_digest: review.digest
					},
					key,
					epoch,
					signal
				),
				review
			);
		},
		async receipt(id, signal) {
			const review = reviews.get(id);
			if (!review) return { state: 'unknown' };
			return decision(await blockTransferApi.receipt(id, epoch, signal), review);
		},
		async undo(receipt: BlockTransferReceipt, key, signal) {
			const review = reviews.get(receipt.id);
			if (!review) return { state: 'unknown' };
			return decision(
				await blockTransferApi.undo(
					receipt.id,
					{
						expected_source_frontier: receipt.sourcePostFrontier,
						expected_target_frontier: receipt.targetPostFrontier
					},
					key,
					epoch,
					signal
				),
				review
			);
		},
		async cancel(id, key, signal) {
			const review = reviews.get(id);
			if (!review) return { state: 'unknown' };
			return decision(await blockTransferApi.cancel(id, key, epoch, signal), review);
		}
	};
}
