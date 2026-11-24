import { apiClient, type ApiResult } from './client';

/** `apps/api/src/services/issue_view.rs::PageLink`. */
export type PageLinkAccess = 'visible' | 'restricted';
export type PageLinkLifecycle = 'active' | 'archived';

export interface PageLink {
	object_id: string;
	/** The page's current title; `null` unless the caller may read the page. Never invented. */
	title: string | null;
	access: PageLinkAccess;
	lifecycle: PageLinkLifecycle;
	linked_at: string;
}

/** One work item linked to a page, as the page side shows it. */
export interface PageWorkItem {
	id: string;
	/** The short identifier, `KEY-n`. */
	identifier: string;
	title: string;
	state: string;
	project_id: string;
}

/** `GET /api/v2/flow/objects/{id}/work-items`. */
export interface PageWorkItems {
	items: PageWorkItem[];
	/** Linked items the caller may not read: counted, never described. */
	hidden_count: number;
	/** More links exist than the server examined (500). */
	truncated: boolean;
}

export const pageLinksApi = {
	list(issueId: string): Promise<ApiResult<PageLink[]>> {
		return apiClient.get<PageLink[]>(`/api/v1/issues/${encodeURIComponent(issueId)}/pages`);
	},

	link(issueId: string, objectId: string): Promise<ApiResult<PageLink>> {
		return apiClient.post<PageLink>(`/api/v1/issues/${encodeURIComponent(issueId)}/pages`, {
			object_id: objectId
		});
	},

	unlink(issueId: string, objectId: string): Promise<ApiResult<Record<string, never>>> {
		return apiClient.delete<Record<string, never>>(
			`/api/v1/issues/${encodeURIComponent(issueId)}/pages/${encodeURIComponent(objectId)}`
		);
	},

	workItemsOf(objectId: string): Promise<ApiResult<PageWorkItems>> {
		return apiClient.get<PageWorkItems>(
			`/api/v2/flow/objects/${encodeURIComponent(objectId)}/work-items`
		);
	}
};
