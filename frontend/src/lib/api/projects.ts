import { apiClient, type ApiResult, type PaginatedData } from './client';

export interface IssueCounts {
	by_state: Record<string, number>;
	total: number;
}

export interface Project {
	id: string;
	workspace_id: string;
	name: string;
	key: string;
	description?: string;
	type_key: string;
	type_settings: Record<string, unknown>;
	created_at: string;
	updated_at: string;
	issue_counts?: IssueCounts | null;
}

export interface ProjectType {
	key: string;
	workspace_id?: string | null;
	name: string;
	description: string;
	domain: string;
	default_workflow_id?: string | null;
	enabled_capabilities: unknown[];
	field_schema: Record<string, unknown>;
	artifact_schema: Record<string, unknown>;
	created_at: string;
	updated_at: string;
}

export interface TemplatePackage {
	id: string;
	digest: string;
	package_key: string;
	version: string;
	kind: string;
	tier: string;
	custom_code: boolean;
	summary: Record<string, string> | null;
}

export interface TemplateInstallResult {
	installation_id: string;
	package_key: string;
	version: string;
	forms: { bind: string; form_id: string }[];
}

export interface ProjectAgentPolicy {
	project_id: string;
	project_type: string;
	capabilities: unknown[];
	action_classes: Record<string, unknown>;
	mcp?: {
		workspace_scope_required?: boolean;
		project_context_required?: boolean;
		tool_registry?: {
			source?: string;
			groups?: Record<string, string[]>;
			enabled_tools?: string[];
		};
	};
}

export interface ProjectContext {
	project: Project;
	project_type?: ProjectType | null;
	governance?: unknown;
	workflow?: unknown;
	recent_decisions?: unknown[];
	agent_policy: ProjectAgentPolicy;
}

export const projectsApi = {
	list(
		workspaceId: string,
		params?: { page?: number; per_page?: number }
	): Promise<ApiResult<PaginatedData<Project>>> {
		const query = new URLSearchParams();
		if (params?.page) query.append('page', params.page.toString());
		if (params?.per_page) query.append('per_page', params.per_page.toString());

		const endpoint = query.size
			? `/api/v1/workspaces/${workspaceId}/projects?${query.toString()}`
			: `/api/v1/workspaces/${workspaceId}/projects`;

		return apiClient.get<PaginatedData<Project>>(endpoint);
	},

	get(projectId: string): Promise<ApiResult<Project>> {
		return apiClient.get<Project>(`/api/v1/projects/${projectId}`);
	},

	getContext(projectId: string): Promise<ApiResult<ProjectContext>> {
		return apiClient.get<ProjectContext>(`/api/v1/projects/${projectId}/context`);
	},

	getAgentPolicy(projectId: string): Promise<ApiResult<ProjectAgentPolicy>> {
		return apiClient.get<ProjectAgentPolicy>(`/api/v1/projects/${projectId}/agent-policy`);
	},

	create(
		workspaceId: string,
		data: {
			name: string;
			key: string;
			description?: string;
			type_key?: string;
			type_settings?: Record<string, unknown>;
			/** Key of a built-in template package (owners and admins), for example `sv/contract-review`. */
			template_package?: string;
		}
	): Promise<ApiResult<Project>> {
		return apiClient.post<Project>(`/api/v1/workspaces/${workspaceId}/projects`, data);
	},

	update(
		projectId: string,
		data: {
			name?: string;
			description?: string;
			type_key?: string;
			type_settings?: Record<string, unknown>;
			expected_updated_at?: string;
		}
	): Promise<ApiResult<Project>> {
		return apiClient.put<Project>(`/api/v1/projects/${projectId}`, data);
	},

	delete(projectId: string): Promise<ApiResult<null>> {
		return apiClient.delete<null>(`/api/v1/projects/${projectId}`);
	},

	listTypes(workspaceId: string): Promise<ApiResult<PaginatedData<ProjectType>>> {
		return apiClient.get<PaginatedData<ProjectType>>(
			`/api/v1/workspaces/${workspaceId}/project-types`
		);
	},

	createType(
		workspaceId: string,
		data: {
			key: string;
			name: string;
			description?: string;
			domain?: string;
		}
	): Promise<ApiResult<ProjectType>> {
		return apiClient.post<ProjectType>(`/api/v1/workspaces/${workspaceId}/project-types`, data);
	},

	updateType(
		key: string,
		data: {
			name?: string;
			description?: string;
			domain?: string;
		}
	): Promise<ApiResult<ProjectType>> {
		return apiClient.patch<ProjectType>(`/api/v1/project-types/${key}`, data);
	},

	/** The built-in and private packages of kind form-template. */
	listTemplatePackages(workspaceId: string): Promise<ApiResult<PaginatedData<TemplatePackage>>> {
		return apiClient.get<PaginatedData<TemplatePackage>>(
			`/api/v1/workspaces/${workspaceId}/packages?kind=form-template&per_page=200`
		);
	},

	/** Installs a built-in template package into a project and approves it (owners and admins). */
	installTemplate(
		projectId: string,
		packageKey: string
	): Promise<ApiResult<TemplateInstallResult>> {
		return apiClient.post<TemplateInstallResult>(`/api/v1/projects/${projectId}/template-install`, {
			package_key: packageKey
		});
	}
};
