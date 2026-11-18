import type { ApiResult } from '#lib/api/client.js';
import type { FlowCapabilities } from '#lib/api/flow.js';
import { FLOW_PROCESS_READER_COMPATIBILITY } from '../product-mode';
export function sharedBlocksSupported(
	result: ApiResult<FlowCapabilities>,
	compatibility = FLOW_PROCESS_READER_COMPATIBILITY
): boolean {
	return (
		result.code === 0 &&
		result.data?.api_version === 2 &&
		result.data.block_shares_supported === true &&
		compatibility >= 5 &&
		result.data.minimum_reader_version >= 5 &&
		result.data.minimum_reader_version <= compatibility
	);
}
