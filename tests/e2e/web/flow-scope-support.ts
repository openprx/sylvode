import type { APIRequestContext, APIResponse } from '@playwright/test';

type Headers = Record<string, string>;

/** The Flow scope bound to a workspace: the workspace-level Flow routes hang off it. */
export async function flowScopeId(
	request: APIRequestContext,
	workspaceId: string,
	headers: Headers
): Promise<string> {
	const response = await request.get(`/api/v2/flow/workspaces/${workspaceId}/scope`, { headers });
	const body = (await response.json()) as { data?: { scope?: { id?: string } } };
	const id = body.data?.scope?.id;
	if (!response.ok() || !id) throw new Error(`workspace ${workspaceId} has no Flow scope`);
	return id;
}

/** The v2 prefix of a workspace's scope routes, e.g. `.../scopes/{id}/search`. */
export async function flowScopePath(
	request: APIRequestContext,
	workspaceId: string,
	headers: Headers
): Promise<string> {
	return `/api/v2/flow/scopes/${await flowScopeId(request, workspaceId, headers)}`;
}

/** Creates a Flow object in a workspace's scope. */
export async function postWorkspaceFlowObject(
	request: APIRequestContext,
	workspaceId: string,
	options: { headers: Headers; data: Record<string, unknown> }
): Promise<APIResponse> {
	const scope_id = await flowScopeId(request, workspaceId, options.headers);
	return request.post('/api/v2/flow/objects', {
		...options,
		data: { ...options.data, scope_id }
	});
}
