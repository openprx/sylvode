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
	schema_version?: 'openpr.plugin.v2';
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

export type PluginSlotV3 =
	| 'form.record.detail'
	| 'form.record.actions'
	| 'form.list.toolbar'
	| 'project.view';
export type PluginKind = 'panel' | 'action' | 'view';
export type PluginGrant = PluginCapability | 'ui.display' | `read:${string}`;

export interface PluginContributionV3 {
	id: string;
	kind: PluginKind;
	slot: PluginSlotV3;
	label: string;
	view: string;
	route: string | null;
	form_key: string | null;
}

export interface PluginReadV3 {
	id: string;
	form_key: string;
	fields: string[];
}

export interface PluginManifestV3 {
	schema_version: 'openpr.plugin.v3';
	key: string;
	name: string;
	version: string;
	publisher: string;
	host_protocol: { min: 3; max: 3 };
	profile: 'ui' | 'wasm';
	bridge_version: 2;
	entry: { path: 'panel.html'; size: number; sha256: string };
	wasm: null | {
		abi: 'openpr.plugin.v1';
		path: 'plugin.wasm';
		size: number;
		sha256: string;
		form_key: string;
	};
	contributions: PluginContributionV3[];
	reads: PluginReadV3[];
	capabilities: {
		name: PluginCapability;
		permission: string;
		input_schema: unknown;
		output_schema: unknown;
	}[];
}

export type AnyPluginManifest = PluginManifest | PluginManifestV3;
export const isV3 = (m: AnyPluginManifest): m is PluginManifestV3 =>
	m.schema_version === 'openpr.plugin.v3';

export interface PluginUpload {
	manifest: PluginManifest;
	wasm_base64: string;
	web_base64: string;
}

export interface PluginUploadV3 {
	manifest: PluginManifestV3;
	web_base64: string;
	wasm_base64?: string;
}

export interface InstalledPluginPackage {
	id: string;
	manifest: AnyPluginManifest;
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
	grants: PluginGrant[];
	placements?: PluginPlacement[];
}

export interface PluginBindingEvent extends PluginBinding {
	request_id: string;
	actor_id: string;
	created_at: string;
	previous_package_id: string | null;
	previous_version: string | null;
	previous_grants: PluginGrant[];
	previous_placements?: PluginPlacement[];
	version: string | null;
	reason?: string | null;
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
			extension_supported?: boolean;
		}>(packages(projectId)),
	invocations: (projectId: string) =>
		apiClient.get<PluginInvocationEvent[]>(`${packages(projectId)}/invocations`),
	install: (projectId: string, upload: PluginUpload | PluginUploadV3) =>
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
	manifest: AnyPluginManifest;
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
	save: (upload: PluginUpload | PluginUploadV3) =>
		apiClient.post<PersonalPluginPackage>(library, upload),
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

export interface PluginPreference {
	hidden: boolean;
	collapsed: boolean;
	position: number | null;
}

export interface PluginContributionItem {
	package_id: string;
	key: string;
	version: string;
	name: string;
	profile: 'ui' | 'wasm';
	generation: number;
	package_sha256: string;
	web_sha256: string;
	contribution: PluginContributionV3;
	project_position: number;
	preference: PluginPreference;
	effective_position: number;
	grants: PluginGrant[];
}

export type PluginDataRequest =
	| { op: 'form.describe'; read: string }
	| { op: 'records.query'; read: string; limit?: number; cursor?: string | null }
	| { op: 'record.get'; read: string; record_id: string };

const contributionsBase = (projectId: string) =>
	`/api/v1/projects/${encodeURIComponent(projectId)}/plugin-contributions`;
const contribution = (projectId: string, key: string, contributionId: string) =>
	`${contributionsBase(projectId)}/${encodeURIComponent(key)}/${encodeURIComponent(contributionId)}`;
const preferences = (projectId: string) =>
	`/api/v1/projects/${encodeURIComponent(projectId)}/plugin-preferences`;

export const pluginExtensionsApi = {
	projectContributions: (projectId: string) =>
		apiClient.get<{ items: PluginContributionItem[] }>(
			`${contributionsBase(projectId)}?surface=project`
		),
	formContributions: (projectId: string, formId: string) =>
		apiClient.get<{ items: PluginContributionItem[] }>(
			`${contributionsBase(projectId)}?surface=form&form_id=${encodeURIComponent(formId)}`
		),
	recordContributions: (recordId: string) =>
		apiClient.get<{ items: PluginContributionItem[] }>(
			`/api/v1/form-records/${encodeURIComponent(recordId)}/plugin-contributions`
		),
	entry: (
		projectId: string,
		key: string,
		contributionId: string,
		ctx: { recordId?: string; formId?: string }
	) => {
		const q = new URLSearchParams();
		if (ctx.recordId) q.set('record_id', ctx.recordId);
		if (ctx.formId) q.set('form_id', ctx.formId);
		const qs = q.toString();
		return apiClient.get<{ item: PluginContributionItem; html: string }>(
			`${contribution(projectId, key, contributionId)}/entry${qs ? `?${qs}` : ''}`
		);
	},
	data: (
		projectId: string,
		key: string,
		contributionId: string,
		body: PluginDataRequest & { package_id: string; generation: number; request_id: string }
	) => apiClient.post<unknown>(`${contribution(projectId, key, contributionId)}/data`, body),
	invoke: (
		recordId: string,
		key: string,
		contributionId: string,
		body: {
			package_id: string;
			generation: number;
			request_id: string;
			preview_request_id?: string;
			capability: PluginCapability;
			input: Record<string, never>;
		}
	) =>
		apiClient.post<PluginInvocation>(
			`/api/v1/form-records/${encodeURIComponent(recordId)}/plugin-contributions/${encodeURIComponent(key)}/${encodeURIComponent(contributionId)}/invoke`,
			body
		),
	setPreference: (projectId: string, key: string, contributionId: string, pref: PluginPreference) =>
		apiClient.put<PluginPreference & { key: string; contribution_id: string; updated_at: string }>(
			`${preferences(projectId)}/${encodeURIComponent(key)}/${encodeURIComponent(contributionId)}`,
			pref
		),
	resetPreference: (projectId: string, key: string, contributionId: string) =>
		apiClient.delete<{ reset: true }>(
			`${preferences(projectId)}/${encodeURIComponent(key)}/${encodeURIComponent(contributionId)}`
		),
	resetAllPreferences: (projectId: string) =>
		apiClient.delete<{ reset: number }>(preferences(projectId)),
	resetConfiguration: (
		projectId: string,
		key: string,
		body: { expected_generation: number; request_id: string }
	) =>
		apiClient.post<PluginBinding>(
			`${packages(projectId)}/${encodeURIComponent(key)}/configuration/reset`,
			body
		)
};
