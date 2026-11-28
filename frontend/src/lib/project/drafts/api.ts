import { apiClient, type ApiResult, type PaginatedData } from '#lib/api/client.js';
import type { Draft, DraftKind, DraftStatus, DraftSummary } from './wire';

const enc = encodeURIComponent;

export const draftsApi = {
	list(
		projectId: string,
		filter: { status?: DraftStatus; kind?: DraftKind; page?: number; perPage?: number } = {}
	): Promise<ApiResult<PaginatedData<DraftSummary>>> {
		const query = new URLSearchParams({
			project_id: projectId,
			per_page: String(filter.perPage ?? 50)
		});
		if (filter.status) query.set('status', filter.status);
		if (filter.kind) query.set('kind', filter.kind);
		if (filter.page) query.set('page', String(filter.page));
		return apiClient.get<PaginatedData<DraftSummary>>(`/api/v1/drafts?${query.toString()}`);
	},
	get(id: string): Promise<ApiResult<Draft>> {
		return apiClient.get<Draft>(`/api/v1/drafts/${enc(id)}`);
	},
	/** Sends the digest the reviewer saw; the server refuses `draft_stale` when the state moved. */
	approve(id: string, reviewDigest: string, comment?: string): Promise<ApiResult<Draft>> {
		return apiClient.post<Draft>(`/api/v1/drafts/${enc(id)}/approve`, {
			review_digest: reviewDigest,
			...(comment ? { comment } : {})
		});
	},
	reject(id: string, comment: string): Promise<ApiResult<Draft>> {
		return apiClient.post<Draft>(`/api/v1/drafts/${enc(id)}/reject`, { comment });
	}
};
