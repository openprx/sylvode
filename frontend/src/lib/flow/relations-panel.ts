// Relations panel (`contracts/ui-surface-v1.md` "后续版本 UI 派生" v0.5; `versions/v0.5-
// collaboration.md` "Web 交互"; `rest-api-v1.md` `GET /flow/objects/{object_id}/relations` and the
// `RelationView` union).
//
// Rules this module owns:
// - pages are loaded lazily with the server's opaque `next_cursor`; a filter change starts over;
// - an `unavailable` row becomes a row with NO identifier at all, so nothing downstream can link
//   to it or copy a reference from it;
// - any relation the client cannot prove belongs to the root's workspace (a visible row naming
//   another workspace, a malformed row, an unknown `visibility`) and any rejected read is an
//   integrity failure for the whole panel: every row already shown is dropped and only the error
//   is rendered ("跨 workspace relation 作为 integrity failure 整次 fail closed"). The server
//   reports a cross-workspace relation as `invalid_update` with `details.reason=
//   "cross_workspace_relation"` (`flow/relations.rs::record_relation_integrity`), which gets its
//   own integrity copy here; any other `invalid_update` keeps the generic integrity copy. This
//   read is the only producer of that reason: a create/move whose target is in another
//   workspace answers `not_found` and is handled where the write is made;
// - errors are classified by `error_code` only.

import type { ApiResult } from '$lib/api/client';
import type {
	FlowRelationDirection,
	FlowRelationListResponse,
	FlowRelationsQuery
} from '$lib/api/flow';
import { flowErrorFromEnvelope, flowErrorI18nKey, invalidUpdateDisposition } from './errors';
import { FlowObjectRepository } from './object-repository';

export const RELATION_PAGE_SIZE = 50;
export const RELATION_DIRECTIONS = [
	'both',
	'outgoing',
	'incoming'
] as const satisfies readonly FlowRelationDirection[];

export const RELATION_DIRECTION_KEYS: Readonly<Record<FlowRelationDirection, string>> = {
	both: 'flow.relations.direction.both',
	outgoing: 'flow.relations.direction.outgoing',
	incoming: 'flow.relations.direction.incoming'
};

export const RELATION_INTEGRITY_KEY = 'flow.relations.error.integrity';
export const RELATION_CROSS_WORKSPACE_KEY = 'flow.relations.error.crossWorkspace';

export type RelationRow =
	| {
			readonly kind: 'visible';
			readonly relationId: string;
			readonly relationType: string;
			readonly direction: 'outgoing' | 'incoming';
			readonly targetId: string;
			readonly title: string;
			readonly objectType: string;
			readonly lifecycleStatus: string;
	  }
	| { readonly kind: 'unavailable' };

/** The object a row may navigate to, or `null`. Unavailable rows never have one. */
export function relationTarget(row: RelationRow): string | null {
	return row.kind === 'visible' ? row.targetId : null;
}

export interface RelationFilter {
	readonly direction: FlowRelationDirection;
	/** `''` means every relation type. */
	readonly relationType: string;
}

export interface RelationsSnapshot {
	readonly rows: readonly RelationRow[];
	readonly loading: boolean;
	readonly loaded: boolean;
	readonly hasMore: boolean;
	readonly failureKey: string | null;
	readonly filter: RelationFilter;
	/** Relation types seen so far, for the type filter. */
	readonly relationTypes: readonly string[];
}

export interface RelationsPanelDeps {
	readonly reads: Pick<FlowObjectRepository, 'getRelations'>;
	readonly pageSize: number;
}

class RelationIntegrityError extends Error {}

function isString(value: unknown): value is string {
	return typeof value === 'string' && value.length > 0;
}

/** Parses one wire row. Throws `RelationIntegrityError` for anything that is not a well-formed
 * row of the root's workspace. */
export function parseRelationRow(raw: unknown, workspaceId: string): RelationRow {
	if (typeof raw !== 'object' || raw === null) throw new RelationIntegrityError('row');
	const row = raw as Record<string, unknown>;
	if (row.visibility === 'unavailable') return { kind: 'unavailable' };
	if (row.visibility !== 'visible') throw new RelationIntegrityError('visibility');
	const other = row.other_object;
	if (typeof other !== 'object' || other === null) throw new RelationIntegrityError('other');
	const target = other as Record<string, unknown>;
	// `RelatedObjectView` carries no workspace; a row that names one must name the root's.
	if ('workspace_id' in target && target.workspace_id !== workspaceId) {
		throw new RelationIntegrityError('workspace');
	}
	if ('workspace_id' in row && row.workspace_id !== workspaceId) {
		throw new RelationIntegrityError('workspace');
	}
	if (
		!isString(row.relation_id) ||
		!isString(row.relation_type) ||
		(row.direction !== 'outgoing' && row.direction !== 'incoming') ||
		!isString(target.id) ||
		typeof target.title !== 'string' ||
		!isString(target.object_type)
	) {
		throw new RelationIntegrityError('shape');
	}
	return {
		kind: 'visible',
		relationId: row.relation_id,
		relationType: row.relation_type,
		direction: row.direction,
		targetId: target.id,
		title: target.title,
		objectType: target.object_type,
		lifecycleStatus: typeof target.lifecycle_status === 'string' ? target.lifecycle_status : ''
	};
}

function failureKeyOf(result: ApiResult<unknown>): string {
	const flowError = flowErrorFromEnvelope(result);
	if (flowError) {
		if (flowError.code !== 'invalid_update') return flowErrorI18nKey(flowError);
		return invalidUpdateDisposition(flowError)?.reason === 'cross_workspace_relation'
			? RELATION_CROSS_WORKSPACE_KEY
			: RELATION_INTEGRITY_KEY;
	}
	switch (result.code) {
		case 401:
			return 'flow.error.unauthenticated';
		case 403:
			return 'flow.error.forbidden';
		case 404:
			return 'flow.error.not_found';
		default:
			return 'flow.relations.error.unavailable';
	}
}

export class FlowRelationsController {
	private readonly deps: RelationsPanelDeps;
	private rows: RelationRow[] = [];
	private cursor: string | null = null;
	private loading = false;
	private loaded = false;
	private failureKey: string | null = null;
	private filter: RelationFilter = { direction: 'both', relationType: '' };
	private readonly types = new Set<string>();
	private generation = 0;

	constructor(
		private readonly objectId: string,
		private readonly workspaceId: string,
		private readonly onChange: (snapshot: RelationsSnapshot) => void = () => {},
		deps: Partial<RelationsPanelDeps> = {}
	) {
		this.deps = {
			reads: deps.reads ?? new FlowObjectRepository(),
			pageSize: deps.pageSize ?? RELATION_PAGE_SIZE
		};
	}

	get snapshot(): RelationsSnapshot {
		return {
			rows: [...this.rows],
			loading: this.loading,
			loaded: this.loaded,
			hasMore: this.failureKey === null && this.cursor !== null,
			failureKey: this.failureKey,
			filter: this.filter,
			relationTypes: [...this.types].sort()
		};
	}

	/** First page for the current filter; a no-op once loaded. */
	async load(): Promise<RelationsSnapshot> {
		if (this.loaded || this.loading) return this.snapshot;
		return this.fetchPage(null);
	}

	async loadMore(): Promise<RelationsSnapshot> {
		if (this.loading || this.failureKey !== null || this.cursor === null) return this.snapshot;
		return this.fetchPage(this.cursor);
	}

	async setFilter(filter: RelationFilter): Promise<RelationsSnapshot> {
		this.filter = filter;
		this.rows = [];
		this.cursor = null;
		this.loaded = false;
		this.loading = false;
		this.failureKey = null;
		this.generation += 1;
		return this.fetchPage(null);
	}

	/** Retries after a failure, from the first page. */
	async reload(): Promise<RelationsSnapshot> {
		return this.setFilter(this.filter);
	}

	private query(cursor: string | null): FlowRelationsQuery {
		return {
			direction: this.filter.direction,
			...(this.filter.relationType ? { relation_type: this.filter.relationType } : {}),
			...(cursor ? { cursor } : {}),
			limit: this.deps.pageSize
		};
	}

	private async fetchPage(cursor: string | null): Promise<RelationsSnapshot> {
		const generation = this.generation;
		this.loading = true;
		this.emit();
		const result: ApiResult<FlowRelationListResponse> = await this.deps.reads.getRelations(
			this.objectId,
			this.query(cursor)
		);
		if (generation !== this.generation) return this.snapshot;
		this.loading = false;
		this.loaded = true;
		if (result.code !== 0 || !result.data || !Array.isArray(result.data.items)) {
			this.failClosed(result.code === 0 ? RELATION_INTEGRITY_KEY : failureKeyOf(result));
			return this.snapshot;
		}
		let page: RelationRow[];
		try {
			page = result.data.items.map((item) => parseRelationRow(item, this.workspaceId));
		} catch {
			this.failClosed(RELATION_INTEGRITY_KEY);
			return this.snapshot;
		}
		this.rows = [...this.rows, ...page];
		for (const row of page) if (row.kind === 'visible') this.types.add(row.relationType);
		this.cursor = isString(result.data.next_cursor) ? result.data.next_cursor : null;
		this.emit();
		return this.snapshot;
	}

	private failClosed(key: string): void {
		this.rows = [];
		this.cursor = null;
		this.failureKey = key;
		this.emit();
	}

	private emit(): void {
		this.onChange(this.snapshot);
	}
}
