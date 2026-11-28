import { apiClient, type ApiResult } from '#lib/api/client.js';
import type { PackageSummary } from './api';

const enc = encodeURIComponent;

/** A localised text map (`en` always present). */
export type I18nText = Record<string, string>;

export interface IndexSource {
	id: string;
	url: string;
	hidden: boolean;
	fetched_at: string | null;
	checked_at: string | null;
	generated_at: string | null;
	stale: boolean;
	error: string | null;
	warnings: unknown[];
	plugins: number;
	/** Only on the answer of a refresh. */
	outcome?: 'skipped' | 'unchanged' | 'updated' | 'failed';
}

export interface IndexListItem {
	source: string;
	id: string;
	kind: string;
	title: I18nText;
	summary: I18nText;
	icon: string;
	publisher: { id: string; name: I18nText | null; trusted: boolean };
	category: string;
	latest_compatible: string | null;
	labels: string[];
	imported: boolean;
	installed: boolean;
}

export interface IndexPage<T> {
	items: T[];
	total: number;
	page: number;
	per_page: number;
	total_pages: number;
}

export interface IndexListFilter {
	source?: string;
	q?: string;
	kind?: string;
	category?: string;
	label?: string;
	compatible: boolean;
	page: number;
	perPage: number;
}

export interface IndexContents {
	requests: string[];
	custom_code: boolean;
	actions: string[];
	rules: string[];
	binds: string[];
	layouts: { key: string; slots: string[] }[];
	pages: { route: string; audience: string }[];
	forms: string[];
	project_block: boolean;
}

export type IndexVersionStatus = 'listed' | 'yanked' | 'revoked';

export interface IndexVersion {
	version: string;
	published_at: number;
	status: IndexVersionStatus;
	status_reason?: string;
	url: string;
	size: number;
	sha256: string;
	digest: string;
	publisher: string;
	key_id: string;
	sylvode: string;
	contents: IndexContents;
	changelog?: I18nText;
	compatible: boolean;
	imported_digest: string | null;
}

export interface IndexPublisherKey {
	key_id: string;
	public_key: string;
	status: 'active' | 'revoked';
	added_at: number;
	revoked_at?: number;
	reason?: string;
	pinned: boolean;
}

export interface IndexPluginDetail {
	id: string;
	source: string;
	source_url: string;
	kind: string;
	title: I18nText;
	summary: I18nText;
	description: I18nText;
	icon: string;
	screenshots: { path: string; caption: I18nText }[];
	homepage?: string;
	repository?: string;
	license?: string;
	metadata: { category: string; labels: string[]; keywords: string[] };
	channels: { latest?: string };
	versions: IndexVersion[];
	publisher: {
		id: string;
		name: I18nText | null;
		homepage: string | null;
		trusted: boolean;
		keys: IndexPublisherKey[];
	};
	latest_compatible: string | null;
	imported: boolean;
	installed: boolean;
}

export interface IndexEvent {
	id: string | number;
	source: string;
	action: string;
	actor_id: string | null;
	details: Record<string, unknown> | null;
	created_at: string;
}

export interface ImportedPackage extends PackageSummary {
	origin: ImportOrigin;
}

/** `packages.origin` of a package imported from a public index. */
export interface ImportOrigin {
	source: string;
	plugin_id: string;
	publisher?: string;
	name?: string;
	version: string;
	digest?: string;
	url?: string;
	sha256?: string;
	imported_by?: string;
	imported_at?: string;
}

const base = (workspaceId: string) => `/api/v1/workspaces/${enc(workspaceId)}/market/index`;

/** Path of an image inside a plugin directory, each segment encoded (the route takes a wildcard). */
export function assetPath(
	workspaceId: string,
	source: string,
	pluginId: string,
	path: string
): string {
	const [publisher = '', name = ''] = pluginId.split('/');
	const tail = path.split('/').map(enc).join('/');
	return `${base(workspaceId)}/assets/${enc(source)}/${enc(publisher)}/${enc(name)}/${tail}`;
}

export const indexApi = {
	sources(workspaceId: string): Promise<ApiResult<IndexSource[]>> {
		return apiClient.get(`${base(workspaceId)}/sources`);
	},
	plugins(
		workspaceId: string,
		filter: IndexListFilter
	): Promise<ApiResult<IndexPage<IndexListItem>>> {
		const query = new URLSearchParams({
			page: String(filter.page),
			per_page: String(filter.perPage)
		});
		if (filter.source) query.set('source', filter.source);
		if (filter.q) query.set('q', filter.q);
		if (filter.kind) query.set('kind', filter.kind);
		if (filter.category) query.set('category', filter.category);
		if (filter.label) query.set('label', filter.label);
		if (filter.compatible) query.set('compatible', 'true');
		return apiClient.get(`${base(workspaceId)}/plugins?${query.toString()}`);
	},
	plugin(
		workspaceId: string,
		source: string,
		pluginId: string
	): Promise<ApiResult<IndexPluginDetail>> {
		const [publisher = '', name = ''] = pluginId.split('/');
		return apiClient.get(
			`${base(workspaceId)}/plugins/${enc(source)}/${enc(publisher)}/${enc(name)}`
		);
	},
	import(
		workspaceId: string,
		body: { source: string; plugin_id: string; version: string; trust_key_id?: string }
	): Promise<ApiResult<ImportedPackage>> {
		return apiClient.post(`${base(workspaceId)}/import`, body);
	},
	refresh(workspaceId: string, source: string): Promise<ApiResult<IndexSource>> {
		return apiClient.post(`${base(workspaceId)}/sources/${enc(source)}/refresh`, {});
	},
	setHidden(workspaceId: string, source: string, hidden: boolean): Promise<ApiResult<IndexSource>> {
		return apiClient.put(`${base(workspaceId)}/sources/${enc(source)}/hidden`, { hidden });
	},
	events(workspaceId: string, page: number): Promise<ApiResult<IndexPage<IndexEvent>>> {
		return apiClient.get(`${base(workspaceId)}/events?page=${page}&per_page=20`);
	},
	/** Image bytes with the console's auth; the caller turns them into an object URL. */
	asset(workspaceId: string, source: string, pluginId: string, path: string, signal?: AbortSignal) {
		return apiClient.getBinary(assetPath(workspaceId, source, pluginId, path), true, signal);
	}
};
