import { routeFailure } from '#lib/flow/route-failure.js';
import { flowApi } from '#lib/api/flow.js';
import { requireRouteParam } from '#lib/utils/route-params.js';
import type { LayoutLoad } from './$types';
export const load: LayoutLoad = async ({ params }) => {
	const workspaceId = requireRouteParam(params.workspaceId, 'workspaceId');
	const result = await flowApi.getWorkspaceScope(workspaceId);
	return {
		workspaceId,
		scopeId: result.data?.scope.id,
		flowEnabled: result.code === 0 && !!result.data,
		...(result.code !== 0 ? routeFailure(result) : {})
	};
};
