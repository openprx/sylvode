import { apiClient, type ApiResult } from './client';
import type { BotTransportSurface } from '$lib/bots/transport-surfaces';

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

	revoke(workspaceId: string, botId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/workspaces/${workspaceId}/bots/${botId}`);
	}
};
