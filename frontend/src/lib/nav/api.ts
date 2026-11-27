import { apiClient, type ApiResult } from '#lib/api/client.js';
import type { NavConfig, NavDoc, NavPrefs, NavScope, ResolvedNav } from './types';

const enc = encodeURIComponent;

function base(scope: NavScope, id: string): string {
	return scope === 'workspace' ? `/api/v1/workspaces/${enc(id)}` : `/api/v1/projects/${enc(id)}`;
}

function prefsQuery(workspaceId: string, projectId: string | null): string {
	const query = new URLSearchParams({ workspace_id: workspaceId });
	if (projectId) query.set('project_id', projectId);
	return query.toString();
}

export const navApi = {
	menu(scope: NavScope, id: string, options: { includeUserHidden?: boolean } = {}) {
		const suffix = options.includeUserHidden ? '?include_user_hidden=1' : '';
		return apiClient.get<ResolvedNav>(`${base(scope, id)}/nav${suffix}`);
	},
	config(scope: NavScope, id: string): Promise<ApiResult<NavConfig>> {
		return apiClient.get<NavConfig>(`${base(scope, id)}/nav/config`);
	},
	saveConfig(
		scope: NavScope,
		id: string,
		doc: NavDoc,
		expectedVersion: number
	): Promise<ApiResult<NavConfig>> {
		return apiClient.put<NavConfig>(`${base(scope, id)}/nav/config`, {
			doc,
			expected_version: expectedVersion
		});
	},
	resetConfig(scope: NavScope, id: string): Promise<ApiResult<NavConfig>> {
		return apiClient.delete<NavConfig>(`${base(scope, id)}/nav/config`);
	},
	prefs(workspaceId: string, projectId: string | null) {
		return apiClient.get<{ prefs: NavPrefs; updated_at: string | null }>(
			`/api/v1/me/nav-prefs?${prefsQuery(workspaceId, projectId)}`
		);
	},
	savePrefs(workspaceId: string, projectId: string | null, prefs: NavPrefs) {
		return apiClient.put<{ prefs: NavPrefs; updated_at: string | null }>(
			`/api/v1/me/nav-prefs?${prefsQuery(workspaceId, projectId)}`,
			{ prefs }
		);
	},
	resetPrefs(workspaceId: string, projectId: string | null) {
		return apiClient.delete<{ prefs: NavPrefs; updated_at: string | null }>(
			`/api/v1/me/nav-prefs?${prefsQuery(workspaceId, projectId)}`
		);
	}
};
