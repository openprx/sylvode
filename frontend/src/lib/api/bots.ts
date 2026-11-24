import { apiClient, type ApiResult } from './client';
import type { BotTransportSurface } from '#lib/bots/transport-surfaces.js';

export interface Bot {
	id: string;
	workspace_id: string;
	name: string;
	token_prefix: string;
	permissions: string[];
	/** The one surface the API accepts this token on. A string, not the union: a newer server
	 * may report a surface this client does not know yet. */
	transport_surface: string;
	is_active: boolean;
	last_used_at: string | null;
	expires_at: string | null;
	created_at: string;
	/** Set when a project bot connection manages this token (read it with `parseManagedBy`). */
	managed_by?: unknown;
	/** `workspace` (the token permissions apply to every project) or `projects`; read it with
	 * `parseProjectAccess` rules, not as the union: a newer server may add a value. */
	project_scope?: string;
	/** The level in projects without a grant: `none`, `read` or `comment`. */
	default_project_level?: string;
	/** How many projects have a grant. */
	grant_count?: number;
}

/** Body and answer of `project-access`. */
export interface BotProjectAccess {
	project_scope: string;
	default_level: string;
	grants: Array<{ project_id: string; level: string }>;
}

export interface CreateBotResponse extends Bot {
	token: string;
}

export interface CreateBotData {
	name: string;
	permissions?: string[];
	/** Always sent by this client: the API's default when it is omitted is `rest`. */
	transport_surface: BotTransportSurface;
	expires_at?: string;
}

export const botsApi = {
	list(workspaceId: string): Promise<ApiResult<Bot[]>> {
		return apiClient.get<Bot[]>(`/api/v1/workspaces/${workspaceId}/bots`);
	},

	create(workspaceId: string, data: CreateBotData): Promise<ApiResult<CreateBotResponse>> {
		return apiClient.post<CreateBotResponse>(`/api/v1/workspaces/${workspaceId}/bots`, data);
	},

	/** Owners and admins; 409 for a bot a project connection manages. */
	getProjectAccess(workspaceId: string, botId: string): Promise<ApiResult<BotProjectAccess>> {
		return apiClient.get<BotProjectAccess>(
			`/api/v1/workspaces/${workspaceId}/bots/${botId}/project-access`
		);
	},

	/** Replaces the scope, the default level and every grant in one step. */
	putProjectAccess(
		workspaceId: string,
		botId: string,
		access: BotProjectAccess
	): Promise<ApiResult<BotProjectAccess>> {
		return apiClient.put<BotProjectAccess>(
			`/api/v1/workspaces/${workspaceId}/bots/${botId}/project-access`,
			access
		);
	},

	revoke(workspaceId: string, botId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/workspaces/${workspaceId}/bots/${botId}`);
	}
};
