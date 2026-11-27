import { apiClient, type ApiResult } from '#lib/api/client.js';

const enc = encodeURIComponent;

export type InstallationStatus = 'pending' | 'active' | 'disabled' | 'revoked';
export type GenerationStatus = 'pending' | 'approved' | 'rejected' | 'superseded';
export type Risk = 'read' | 'write' | 'destructive' | 'admin' | 'secret' | 'unknown';

export interface GrantSelector {
	project?: string;
	form?: string;
}

export interface InstallationGeneration {
	generation: number;
	status: GenerationStatus;
	package: { digest: string; version: string };
	requested_capabilities: string[];
	bind_map: Record<string, { form_id?: string; create?: boolean }>;
	requested_by: { kind: 'user' | 'bot'; id: string };
	requested_at: string;
	reviewed_by: string | null;
	reviewed_at: string | null;
}

export interface Installation {
	id: string;
	workspace_id: string;
	scope: { kind: 'workspace' | 'project'; project_id: string | null };
	package_key: string;
	status: InstallationStatus;
	disabled_reason: 'admin' | 'approver_lost' | 'key_revoked' | null;
	active_generation: number | null;
	/** False when the administrator who approved the active generation no longer is one. */
	approver_active: boolean | null;
	generations: InstallationGeneration[];
	grants: { capability: string; selector: GrantSelector }[];
	/** Grants whose capability changed since they were approved: not honoured until approved again. */
	changed_grants: { capability: string; selector: GrantSelector }[];
	created_at: string;
	/** Present on the answer of disable and revoke. */
	forms_using_rules?: { id: string; project_id: string; key: string; name: string }[];
}

export interface PreviewError {
	code: string;
	path: string;
	message: string;
}

export interface PreviewRequest {
	capability: string;
	selector: { form: string | null };
	/** The package bind a grant selector names as `$bind.<name>` when the form is created on approval. */
	form_bind: string | null;
	risk: Risk;
	label_key: string;
	new: boolean;
}

export interface InstallPreview {
	package: {
		digest: string;
		package_key: string;
		version: string;
		kind: string;
		custom_code: boolean;
	};
	generation: number;
	valid: boolean;
	errors: PreviewError[];
	binds: {
		name: string;
		resolution: 'create' | 'existing';
		form_id: string | null;
		missing_fields: string[];
		type_mismatches: { field: string; expected: string; actual: string }[];
	}[];
	creates: {
		forms: string[];
		layouts: string[];
		placements: { slot_id: string; scope_kind: string }[];
		pages: string[];
	};
	actions: { name: string; label: Record<string, string> | null }[];
	rules: { name: string; stages: string[]; field_types: string[] }[];
	requests: PreviewRequest[];
	existing: { installation_id: string | null; active_generation: number | null };
	/** Only on a review. */
	installation?: Installation;
	pending?: boolean;
}

export interface PackageSummary {
	id: string;
	digest: string;
	package_key: string;
	version: string;
	kind: string;
	tier: string;
	custom_code: boolean;
	summary: Record<string, string> | null;
}

export interface ApproveGrant {
	capability: string;
	selector: GrantSelector | { form: string };
}

export interface InvocationRow {
	id: string;
	generation: number;
	world: string;
	entry: string;
	outcome: string;
	error_code: string | null;
	duration_ms: number;
	host_calls: number;
	created_at: string;
}

export type BindChoice = { form_id: string } | { create: true };

export const marketApi = {
	packages(workspaceId: string): Promise<ApiResult<{ items: PackageSummary[] }>> {
		return apiClient.get(`/api/v1/workspaces/${enc(workspaceId)}/packages?per_page=200`);
	},
	list(workspaceId: string): Promise<ApiResult<{ items: Installation[] }>> {
		return apiClient.get(`/api/v1/workspaces/${enc(workspaceId)}/installations`);
	},
	preview(
		workspaceId: string,
		body: {
			package_digest: string;
			scope: { kind: 'workspace' | 'project'; project_id?: string };
			bind_map: Record<string, BindChoice>;
		}
	): Promise<ApiResult<InstallPreview>> {
		return apiClient.post(`/api/v1/workspaces/${enc(workspaceId)}/installations/preview`, body);
	},
	request(
		workspaceId: string,
		body: {
			package_digest: string;
			scope: { kind: 'workspace' | 'project'; project_id?: string };
			bind_map: Record<string, BindChoice>;
		}
	): Promise<ApiResult<Installation>> {
		return apiClient.post(`/api/v1/workspaces/${enc(workspaceId)}/installations`, body);
	},
	review(installationId: string): Promise<ApiResult<InstallPreview>> {
		return apiClient.get(`/api/v1/installations/${enc(installationId)}/review`);
	},
	approve(
		installationId: string,
		generation: number,
		grants: ApproveGrant[]
	): Promise<ApiResult<Installation>> {
		return apiClient.post(`/api/v1/installations/${enc(installationId)}/approve`, {
			generation,
			grants
		});
	},
	reject(installationId: string, generation: number): Promise<ApiResult<Installation>> {
		return apiClient.post(`/api/v1/installations/${enc(installationId)}/reject`, { generation });
	},
	disable(installationId: string): Promise<ApiResult<Installation>> {
		return apiClient.post(`/api/v1/installations/${enc(installationId)}/disable`, {});
	},
	enable(installationId: string): Promise<ApiResult<Installation>> {
		return apiClient.post(`/api/v1/installations/${enc(installationId)}/enable`, {});
	},
	revoke(installationId: string): Promise<ApiResult<Installation>> {
		return apiClient.post(`/api/v1/installations/${enc(installationId)}/revoke`, {});
	},
	invocations(installationId: string): Promise<ApiResult<{ items: InvocationRow[] }>> {
		return apiClient.get(`/api/v1/installations/${enc(installationId)}/invocations`);
	}
};

/** The grants an approval sends for the requests the administrator ticked. A selector that names a
 * bind the package creates is sent as `$bind.<name>`; the server substitutes the created form. */
export function grantsFor(requests: PreviewRequest[], ticked: readonly boolean[]): ApproveGrant[] {
	return requests
		.filter((_, index) => ticked[index] === true)
		.map((request) => {
			if (request.selector.form) {
				return { capability: request.capability, selector: { form: request.selector.form } };
			}
			if (request.form_bind) {
				return {
					capability: request.capability,
					selector: { form: `$bind.${request.form_bind}` }
				};
			}
			return { capability: request.capability, selector: {} };
		});
}

/** Everything the package asks for is ticked by default. */
export function allRequests(requests: PreviewRequest[]): boolean[] {
	return requests.map(() => true);
}

export interface PackageDetail extends PackageSummary {
	analysis: {
		forms: { bind: string; create: boolean; requires_fields: Record<string, string> }[];
	};
}

export const marketDetailApi = {
	package(workspaceId: string, digest: string): Promise<ApiResult<PackageDetail>> {
		return apiClient.get(`/api/v1/workspaces/${enc(workspaceId)}/packages/${enc(digest)}`);
	},
	get(installationId: string): Promise<ApiResult<Installation>> {
		return apiClient.get(`/api/v1/installations/${enc(installationId)}`);
	}
};
