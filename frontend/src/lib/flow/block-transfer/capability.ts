import type { ApiResult } from '#lib/api/client.js';
import type { FlowCapabilities } from '#lib/api/flow.js';
import { FLOW_PROCESS_READER_COMPATIBILITY } from '../product-mode';
export function blockTransfersSupported(result: ApiResult<FlowCapabilities>): boolean {
	return (
		result.code === 0 &&
		result.data?.api_version === 2 &&
		result.data.block_transfers_supported === true &&
		result.data.minimum_reader_version >= 5 &&
		result.data.minimum_reader_version <= FLOW_PROCESS_READER_COMPATIBILITY
	);
}
