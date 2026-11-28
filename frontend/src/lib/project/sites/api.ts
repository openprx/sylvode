import { apiClient, type ApiResult, type PaginatedData } from '#lib/api/client.js';
import type {
	BindingProjection,
	PageInput,
	SettingsInput,
	Site,
	SiteBinding,
	SitePage,
	SiteRole,
	SiteText
} from './wire';

const enc = encodeURIComponent;

export const sitesApi = {
	list(projectId: string): Promise<ApiResult<PaginatedData<Site>>> {
		return apiClient.get<PaginatedData<Site>>(
			`/api/v1/projects/${enc(projectId)}/sites?per_page=200`
		);
	},
	get(id: string): Promise<ApiResult<Site>> {
		return apiClient.get<Site>(`/api/v1/sites/${enc(id)}`);
	},
	create(projectId: string, body: { slug: string; title: SiteText }): Promise<ApiResult<Site>> {
		return apiClient.post<Site>(`/api/v1/projects/${enc(projectId)}/sites`, body);
	},
	/** Title, and the slug while the site is a draft (owner or admin only). */
	update(id: string, body: { title?: SiteText; slug?: string }): Promise<ApiResult<Site>> {
		return apiClient.patch<Site>(`/api/v1/sites/${enc(id)}`, body);
	},
	publish(id: string): Promise<ApiResult<Site>> {
		return apiClient.post<Site>(`/api/v1/sites/${enc(id)}/publish`, {});
	},
	unpublish(id: string): Promise<ApiResult<Site>> {
		return apiClient.post<Site>(`/api/v1/sites/${enc(id)}/unpublish`, {});
	},
	disable(id: string): Promise<ApiResult<Site>> {
		return apiClient.post<Site>(`/api/v1/sites/${enc(id)}/disable`, {});
	},
	remove(id: string): Promise<ApiResult<{ id: string }>> {
		return apiClient.delete<{ id: string }>(`/api/v1/sites/${enc(id)}`);
	},
	putSettings(id: string, settings: SettingsInput): Promise<ApiResult<Site>> {
		return apiClient.put<Site>(`/api/v1/sites/${enc(id)}/settings`, settings);
	},
	pages(id: string): Promise<ApiResult<{ items: SitePage[] }>> {
		return apiClient.get<{ items: SitePage[] }>(`/api/v1/sites/${enc(id)}/pages`);
	},
	createPage(id: string, body: PageInput): Promise<ApiResult<SitePage>> {
		return apiClient.post<SitePage>(`/api/v1/sites/${enc(id)}/pages`, body);
	},
	updatePage(pageId: string, body: PageInput): Promise<ApiResult<SitePage>> {
		return apiClient.patch<SitePage>(`/api/v1/site-pages/${enc(pageId)}`, body);
	},
	removePage(pageId: string): Promise<ApiResult<{ id: string }>> {
		return apiClient.delete<{ id: string }>(`/api/v1/site-pages/${enc(pageId)}`);
	},
	bindings(id: string): Promise<ApiResult<{ items: SiteBinding[] }>> {
		return apiClient.get<{ items: SiteBinding[] }>(`/api/v1/sites/${enc(id)}/bindings`);
	},
	createBinding(
		id: string,
		key: string,
		body: { form_id: string; projection: BindingProjection }
	): Promise<ApiResult<SiteBinding>> {
		return apiClient.post<SiteBinding>(`/api/v1/sites/${enc(id)}/bindings/${enc(key)}`, body);
	},
	putBinding(
		id: string,
		key: string,
		body: { form_id: string; projection: BindingProjection }
	): Promise<ApiResult<SiteBinding>> {
		return apiClient.put<SiteBinding>(`/api/v1/sites/${enc(id)}/bindings/${enc(key)}`, body);
	},
	removeBinding(
		id: string,
		key: string
	): Promise<ApiResult<{ site_id: string; binding_key: string }>> {
		return apiClient.delete<{ site_id: string; binding_key: string }>(
			`/api/v1/sites/${enc(id)}/bindings/${enc(key)}`
		);
	},
	roles(id: string): Promise<ApiResult<{ items: SiteRole[] }>> {
		return apiClient.get<{ items: SiteRole[] }>(`/api/v1/sites/${enc(id)}/roles`);
	},
	putRole(id: string, userId: string, role: SiteRole['role']): Promise<ApiResult<SiteRole>> {
		return apiClient.put<SiteRole>(`/api/v1/sites/${enc(id)}/roles/${enc(userId)}`, { role });
	},
	removeRole(id: string, userId: string): Promise<ApiResult<{ site_id: string; user_id: string }>> {
		return apiClient.delete<{ site_id: string; user_id: string }>(
			`/api/v1/sites/${enc(id)}/roles/${enc(userId)}`
		);
	}
};
