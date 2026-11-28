import type { ApiResult } from '#lib/api/client.js';
import type { FlowCapabilities } from '#lib/api/flow.js';
export function sharedBlocksSupported(result: ApiResult<FlowCapabilities>): boolean {
	return result.code === 0 && !!result.data;
}
