import { apiClient, type ApiResult } from './client';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

/** What importing an export file into a project did. */
export interface ImportSummary {
	project_id: string;
	project_key: string;
	issues_created: number;
	comments_created: number;
	/** The identifier each work item had in the file, and the one it has in the project. */
	identifiers: Record<string, string>;
}

interface ExportPayload {
	format: string;
	filename: string;
	content: string;
}

export const importExportApi = {
	async exportProject(projectId: string): Promise<ApiResult<unknown>> {
		const result = await apiClient.get<ExportPayload>(
			`/api/v1/export/project/${projectId}?format=json`
		);
		if (result.code !== 0) {
			return {
				code: result.code,
				message: result.message || get(t)('project.exportFailed'),
				data: null
			};
		}

		if (result.data?.format === 'json' && typeof result.data.content === 'string') {
			try {
				return {
					code: 0,
					message: result.message,
					data: JSON.parse(result.data.content) as unknown
				};
			} catch {
				return { code: 0, message: result.message, data: result.data.content };
			}
		}

		return { code: 0, message: result.message, data: result.data };
	},

	/** Adds the work items and comments of an export file to the existing project `projectKey`. */
	importProject(
		workspaceId: string,
		projectKey: string,
		document: unknown
	): Promise<ApiResult<ImportSummary>> {
		return apiClient.post<ImportSummary>(`/api/v1/workspaces/${workspaceId}/import/project`, {
			project_key: projectKey,
			document
		});
	}
};
