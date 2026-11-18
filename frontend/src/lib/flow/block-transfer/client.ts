/** Next-batch wire adapter only. No source/destination snapshots or raw updates are accepted. */
import { apiClient, authSessionChanged, type ApiResult } from '#lib/api/client.js';
import type {
	PrepareBlockTransfer,
	PreparedBlockTransferWire,
	CommitBlockTransfer,
	UndoBlockTransfer,
	AcceptedBlockTransferWire,
	BlockTransferStatusWire
} from './contracts';
export const blockTransferApi = {
	prepare(
		sourceId: string,
		input: PrepareBlockTransfer,
		key: string,
		epoch: number,
		signal?: AbortSignal
	): Promise<ApiResult<PreparedBlockTransferWire | BlockTransferStatusWire>> {
		if (!apiClient.isAuthSessionEpoch(epoch)) return Promise.resolve(authSessionChanged());
		return apiClient.request(
			'POST',
			`/api/v2/flow/objects/${encodeURIComponent(sourceId)}/block-transfers/prepare`,
			input,
			true,
			{ 'Idempotency-Key': key },
			signal
		);
	},
	commit(
		transferId: string,
		input: CommitBlockTransfer,
		key: string,
		epoch: number,
		signal?: AbortSignal
	): Promise<ApiResult<AcceptedBlockTransferWire | BlockTransferStatusWire>> {
		if (!apiClient.isAuthSessionEpoch(epoch)) return Promise.resolve(authSessionChanged());
		return apiClient.request(
			'POST',
			`/api/v2/flow/block-transfers/${encodeURIComponent(transferId)}/commit`,
			input,
			true,
			{ 'Idempotency-Key': key },
			signal
		);
	},
	receipt(
		transferId: string,
		epoch: number,
		signal?: AbortSignal
	): Promise<ApiResult<BlockTransferStatusWire>> {
		if (!apiClient.isAuthSessionEpoch(epoch)) return Promise.resolve(authSessionChanged());
		return apiClient.request(
			'GET',
			`/api/v2/flow/block-transfers/${encodeURIComponent(transferId)}`,
			undefined,
			true,
			{},
			signal
		);
	},
	cancel(
		transferId: string,
		key: string,
		epoch: number,
		signal?: AbortSignal
	): Promise<ApiResult<BlockTransferStatusWire>> {
		if (!apiClient.isAuthSessionEpoch(epoch)) return Promise.resolve(authSessionChanged());
		return apiClient.request(
			'POST',
			`/api/v2/flow/block-transfers/${encodeURIComponent(transferId)}/cancel`,
			{},
			true,
			{ 'Idempotency-Key': key },
			signal
		);
	},

	undo(
		transferId: string,
		input: UndoBlockTransfer,
		key: string,
		epoch: number,
		signal?: AbortSignal
	): Promise<ApiResult<AcceptedBlockTransferWire | BlockTransferStatusWire>> {
		if (!apiClient.isAuthSessionEpoch(epoch)) return Promise.resolve(authSessionChanged());
		return apiClient.request(
			'POST',
			`/api/v2/flow/block-transfers/${encodeURIComponent(transferId)}/undo`,
			input,
			true,
			{ 'Idempotency-Key': key },
			signal
		);
	}
};
