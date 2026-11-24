import { apiClient, type ApiResult, type PaginatedData } from './client';

export type SprintStatus = 'planned' | 'active' | 'completed' | 'cancelled';

/** What completing or cancelling a sprint did with its unfinished issues. */
export interface SprintRollover {
	moved_count: number;
	/** The planned sprint the issues went to; `null` is the backlog. */
	target_sprint_id: string | null;
}

export interface SprintProgress {
	total: number;
	done: number;
}

export interface Sprint {
	id: string;
	project_id: string;
	name: string;
	description?: string;
	start_date: string | null;
	end_date: string | null;
	status: SprintStatus;
	created_at: string;
	updated_at: string;
	/** Items counted by the server for this sprint (`done` follows the finished rule of the
	 * project). Present on the sprint list. */
	progress?: SprintProgress;
	/** Only on the answer that completed or cancelled the sprint. */
	rollover?: SprintRollover;
}

export interface CreateSprintData {
	name: string;
	description?: string;
	start_date: string;
	end_date: string;
	status?: 'planned' | 'active';
}

export interface UpdateSprintData {
	name?: string;
	description?: string;
	start_date?: string;
	end_date?: string;
	status?: SprintStatus;
	/** With `completed` or `cancelled`: the planned sprint that takes the unfinished issues. */
	rollover_sprint_id?: string | null;
}

export const sprintsApi = {
	list(projectId: string): Promise<ApiResult<PaginatedData<Sprint>>> {
		return apiClient.get<PaginatedData<Sprint>>(`/api/v1/projects/${projectId}/sprints`);
	},

	get(id: string): Promise<ApiResult<Sprint>> {
		return apiClient.get<Sprint>(`/api/v1/sprints/${id}`);
	},

	create(projectId: string, data: CreateSprintData): Promise<ApiResult<Sprint>> {
		return apiClient.post<Sprint>(`/api/v1/projects/${projectId}/sprints`, data);
	},

	update(id: string, data: UpdateSprintData): Promise<ApiResult<Sprint>> {
		return apiClient.put<Sprint>(`/api/v1/sprints/${id}`, data);
	},

	delete(id: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/sprints/${id}`);
	}
};
