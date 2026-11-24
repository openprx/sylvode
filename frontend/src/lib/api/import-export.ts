import { apiClient, type ApiResult } from './client';

/** What importing an export file into a project did. */
export interface ImportSummary {
	project_id: string;
	project_key: string;
	issues_created: number;
	comments_created: number;
	/** The identifier each work item had in the file, and the one it has in the project. */
	identifiers: Record<string, string>;
}

export type ExportFormat = 'json' | 'csv';

/** A project export as the server sends it: the file and the name it chose for it. */
export interface ExportFile {
	blob: Blob;
	filename: string;
}

/** The file name of a `Content-Disposition: attachment` header (quoted, plain or RFC 5987 form). */
export function filenameFromDisposition(header: string | null, fallback: string): string {
	if (!header) return fallback;
	const extended = /filename\*\s*=\s*(?:[^']*)'[^']*'([^;]+)/i.exec(header);
	if (extended) {
		try {
			const name = decodeURIComponent(extended[1].trim());
			if (name) return name;
		} catch {
			// fall through to the plain form
		}
	}
	const quoted = /filename\s*=\s*"((?:[^"\\]|\\.)*)"/i.exec(header);
	if (quoted?.[1]) return quoted[1].replace(/\\(.)/g, '$1');
	const plain = /filename\s*=\s*([^;\s]+)/i.exec(header);
	return plain?.[1] || fallback;
}

export const importExportApi = {
	/** Downloads the project's export file; a rejection comes back as the usual envelope. */
	async exportProject(
		projectId: string,
		format: ExportFormat = 'json',
		fallbackName = `project_export.${format}`
	): Promise<ApiResult<ExportFile>> {
		const result = await apiClient.getBinary(
			`/api/v1/projects/${encodeURIComponent(projectId)}/export?format=${format}`
		);
		if (result.code !== 0 || !result.data) {
			return { code: result.code, message: result.message, data: null };
		}
		return {
			code: 0,
			message: result.message,
			data: {
				blob: result.data.blob,
				filename: filenameFromDisposition(
					result.data.headers.get('Content-Disposition'),
					fallbackName
				)
			}
		};
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
