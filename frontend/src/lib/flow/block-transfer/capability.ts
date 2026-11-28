import type { ApiResult } from '#lib/api/client.js';
import type { FlowCapabilities } from '#lib/api/flow.js';
export function blockTransfersSupported(result: ApiResult<FlowCapabilities>): boolean {
	return result.code === 0 && result.data?.block_transfers_enabled === true;
}
