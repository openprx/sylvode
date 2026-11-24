import { apiClient, type ApiResult, type PaginatedData } from './client';

export type MilestoneStatus = 'open' | 'closed';

/** Items of a milestone in one project; `done` follows the finished-state rule of the project. */
export interface MilestoneProjectProgress {
	project_id: string;
	project_key: string;
	total: number;
	done: number;
}

export interface MilestoneProgress {
	total: number;
	done: number;
	/** Ordered by project key. */
	by_project: MilestoneProjectProgress[];
}

export interface Milestone {
	id: string;
	workspace_id: string;
	name: string;
	description: string;
	/** `YYYY-MM-DD`, or `null` when the milestone has no target date. */
	target_date: string | null;
	status: MilestoneStatus;
	created_at: string;
	updated_at: string;
	progress: MilestoneProgress;
}

export interface MilestoneItem {
	id: string;
	/** The short identifier, `KEY-n`. */
	identifier: string;
	title: string;
	state: string;
	done: boolean;
}

export interface MilestoneProjectItems {
	project_id: string;
	project_key: string;
	items: MilestoneItem[];
}

export interface MilestoneDetail extends Milestone {
	items_by_project: MilestoneProjectItems[];
	/** True when the milestone has more items than the 500 listed. */
	truncated: boolean;
}

export interface CreateMilestoneData {
	name: string;
	description?: string;
	target_date?: string;
}

/** A field that is left out is unchanged; `null` on `target_date` clears the date. */
export interface UpdateMilestoneData {
	name?: string;
	description?: string;
	status?: MilestoneStatus;
	target_date?: string | null;
}

export interface ListMilestonesParams {
	status?: MilestoneStatus;
	page?: number;
	per_page?: number;
}

export const milestonesApi = {
	list(
		workspaceId: string,
		params?: ListMilestonesParams
	): Promise<ApiResult<PaginatedData<Milestone>>> {
		const query = new URLSearchParams();
		if (params?.status) query.set('status', params.status);
		if (params?.page) query.set('page', String(params.page));
		if (params?.per_page) query.set('per_page', String(params.per_page));
		const suffix = query.size > 0 ? `?${query.toString()}` : '';
		return apiClient.get<PaginatedData<Milestone>>(
			`/api/v1/workspaces/${workspaceId}/milestones${suffix}`
		);
	},

	get(id: string): Promise<ApiResult<MilestoneDetail>> {
		return apiClient.get<MilestoneDetail>(`/api/v1/milestones/${id}`);
	},

	create(workspaceId: string, data: CreateMilestoneData): Promise<ApiResult<Milestone>> {
		return apiClient.post<Milestone>(`/api/v1/workspaces/${workspaceId}/milestones`, data);
	},

	update(id: string, data: UpdateMilestoneData): Promise<ApiResult<Milestone>> {
		return apiClient.put<Milestone>(`/api/v1/milestones/${id}`, data);
	},

	delete(id: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/milestones/${id}`);
	}
};
