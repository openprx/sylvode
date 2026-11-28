import type { ApiResult } from '#lib/api/client.js';
export interface FlowRouteFailure {
	errorCode?: string;
	errorReason?: string;
	error?: string;
}
export function routeFailure(
	result: Pick<ApiResult<unknown>, 'code' | 'error_code' | 'details'>
): FlowRouteFailure {
	const reason =
		typeof result.details === 'object' && result.details !== null && 'reason' in result.details
			? result.details.reason
			: undefined;
	return {
		errorCode:
			result.error_code ??
			(result.code === 401
				? 'unauthenticated'
				: result.code === 403
					? 'forbidden'
					: result.code === 404
						? 'not_found'
						: result.code >= 500
							? 'network_error'
							: undefined),
		errorReason: typeof reason === 'string' ? reason : undefined
	};
}
export function routeFailureCopy(data: FlowRouteFailure & { workspaceId?: string }): {
	title: string;
	body: string;
} {
	if (data.errorCode === 'unauthenticated')
		return { title: 'flow.route.authTitle', body: 'flow.errors.unauthenticated' };
	if (data.errorCode === 'forbidden')
		return { title: 'flow.route.forbiddenTitle', body: 'flow.errors.forbidden' };
	if (data.errorCode === 'not_found')
		return { title: 'flow.route.notFoundTitle', body: 'flow.route.notFoundBody' };
	if (data.errorCode === 'network_error')
		return { title: 'flow.personal.unavailable', body: 'flow.route.networkBody' };
	if (data.errorCode === 'feature_disabled') {
		if (
			data.workspaceId &&
			(!data.errorReason || ['flow_disabled', 'workspace_flow_disabled'].includes(data.errorReason))
		)
			return { title: 'flow.route.disabledTitle', body: 'flow.route.disabledBody' };
		if (!data.workspaceId || data.errorReason === 'personal_creation_disabled')
			return { title: 'flow.personal.unavailable', body: 'flow.route.personalBody' };
	}
	return { title: 'flow.personal.unavailable', body: 'flow.route.unavailableBody' };
}
