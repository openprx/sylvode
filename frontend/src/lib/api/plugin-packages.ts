import { apiClient } from './client';

export const pluginCapabilities = ['amount.preview', 'amount.apply'] as const;
export type PluginCapability = (typeof pluginCapabilities)[number];

export type PluginSlot = 'form.record.detail' | 'form.record.actions';

export interface PluginContribution {
	id: string;
	slot: PluginSlot;
	label: string;
}

export interface PluginPlacement {
	id: string;
	enabled: boolean;
	position: number;
}

export interface PluginManifest {
	key: string;
	name: string;
	version: string;
	publisher: string;
	host_protocol: { min: number; max: number };
	wasm_abi: string;
	ui: {
		bridge_version: number;
		slot: string;
		form_key: string;
		contributions?: PluginContribution[];
	};
	capabilities: { name: PluginCapability; permission: string }[];
}

export interface PluginUpload {
	manifest: PluginManifest;
	wasm_base64: string;
	web_base64: string;
}

export interface InstalledPluginPackage {
	id: string;
	manifest: PluginManifest;
	package_sha256: string;
	installed_by: string;
	installed_at?: string;
	placements?: PluginPlacement[];
	library_source_id?: string | null;
}

export interface PluginBinding {
	key: string;
	package_id: string | null;
	generation: number;
	grants: PluginCapability[];
	placements?: PluginPlacement[];
}

export interface PluginBindingEvent extends PluginBinding {
	request_id: string;
	actor_id: string;
	created_at: string;
	previous_package_id: string | null;
	previous_version: string | null;
	previous_grants: PluginCapability[];
	previous_placements?: PluginPlacement[];
	version: string | null;
}

export interface PluginInvocationEvent {
	request_id: string;
	actor_id: string;
	record_id: string;
	package_id: string;
	generation: number;
	capability: PluginCapability;
	preview_request_id: string | null;
	status: string;
	error_kind: string | null;
	created_at: string;
	key?: string;
	version?: string;
}

export interface PluginPanel {
	package_id: string;
	key: string;
	version: string;
	name: string;
	generation: number;
	grants: PluginCapability[];
	html: string;
	web_sha256: string;
	package_sha256: string;
	contributions?: (PluginContribution & { position: number })[];
}

export interface PluginInvocation {
	request_id: string;
	package_id: string;
	version: string;
	generation: number;
	amount_cents: number;
}

const packages = (projectId: string) =>
	`/api/v1/projects/${encodeURIComponent(projectId)}/plugin-packages`;
const panels = (recordId: string) =>
	`/api/v1/form-records/${encodeURIComponent(recordId)}/plugin-panels`;

export const pluginPackagesApi = {
	list: (projectId: string) =>
		apiClient.get<{
			items: InstalledPluginPackage[];
			bindings: PluginBinding[];
			binding_history?: PluginBindingEvent[];
			layout_supported?: boolean;
		}>(packages(projectId)),
	invocations: (projectId: string) =>
		apiClient.get<PluginInvocationEvent[]>(`${packages(projectId)}/invocations`),
	install: (projectId: string, upload: PluginUpload) =>
		apiClient.post<InstalledPluginPackage>(packages(projectId), upload),
	setBinding: (
		projectId: string,
		key: string,
		binding: Omit<PluginBinding, 'key' | 'generation'> & {
			expected_generation: number;
			request_id: string;
		}
	) =>
		apiClient.post<PluginBinding>(
			`${packages(projectId)}/${encodeURIComponent(key)}/binding`,
			binding
		),
	discover: (recordId: string) => apiClient.get<PluginPanel[]>(panels(recordId)),
	invoke: (
		recordId: string,
		panel: PluginPanel,
		method: PluginCapability,
		requestId: string,
		previewRequestId?: string
	) =>
		apiClient.post<PluginInvocation>(
			`${panels(recordId)}/${encodeURIComponent(panel.package_id)}/invoke`,
			{
				generation: panel.generation,
				request_id: requestId,
				capability: method,
				...(previewRequestId ? { preview_request_id: previewRequestId } : {}),
				input: {}
			}
		)
};

export interface LibraryProject {
	id: string;
	workspace_id: string;
	workspace_name: string;
	can_manage: boolean;
	name: string;
	key: string;
}

export interface PersonalPluginPackage extends InstalledPluginPackage {
	archived_at?: string | null;
	project_links: {
		project_id: string;
		package_id: string | null;
		active: boolean;
		status: 'pending' | 'attached' | 'rejected' | 'revoked';
	}[];
}

export interface ProjectPluginRequest {
	source_id: string;
	owner_id: string;
	manifest: PluginManifest;
	package_sha256: string;
	submitted_at: string;
}

const requests = (projectId: string) => `${packages(projectId)}/library-requests`;
const library = '/api/v1/me/form-plugin-packages';
const link = (projectId: string, packageId: string) =>
	`${packages(projectId)}/library/${encodeURIComponent(packageId)}`;

export const pluginLibraryApi = {
	list: () =>
		apiClient.get<{ items: PersonalPluginPackage[]; projects: LibraryProject[] }>(library),
	panel: (packageId: string) =>
		apiClient.get<Pick<PluginPanel, 'html' | 'web_sha256'>>(
			`${library}/${encodeURIComponent(packageId)}/panel`
		),
	save: (upload: PluginUpload) => apiClient.post<PersonalPluginPackage>(library, upload),
	preview: (packageId: string, quantity: number, unitPriceCents: number) =>
		apiClient.post<{ amount_cents: number }>(
			`${library}/${encodeURIComponent(packageId)}/preview`,
			{
				quantity,
				unit_price_cents: unitPriceCents
			}
		),
	connect: (projectId: string, packageId: string) =>
		apiClient.post<{ status: 'pending' } | { status: 'attached'; package: InstalledPluginPackage }>(
			link(projectId, packageId),
			{}
		),
	revoke: (projectId: string, packageId: string) =>
		apiClient.delete<{ revoked: boolean }>(link(projectId, packageId)),
	requests: (projectId: string) =>
		apiClient.get<{ items: ProjectPluginRequest[] }>(requests(projectId)),
	review: (projectId: string, sourceId: string, approve: boolean) =>
		apiClient.post<unknown>(`${requests(projectId)}/${encodeURIComponent(sourceId)}`, { approve })
};
