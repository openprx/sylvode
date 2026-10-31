// Collection view controller: table/board/record panel/schema mode state for one Collection
// (`versions/v0.6-collections.md` "Web 交互"; `ui-surface-v1.md` "职责边界" and "Renderer
// registry"). Components render this state and call its methods; they keep only ephemeral UI
// state (focus, an open popover, a drag in progress).
//
// Rules this module owns:
// - reads go through `CollectionReads` (the `ObjectRepository`), writes through
//   `CollectionWrites` (the `CommandService`); nothing here builds a URL;
// - a record's document is opened ONLY when its record panel opens and closed when the panel
//   closes or moves to another record. Loading, paging, scrolling, editing a cell and dragging a
//   card never open one ("大集合只打开 viewport/正在编辑的 record documents");
// - schema design is an explicit mode: every `field_*` / `view_*` command is refused here, with
//   zero requests, unless the mode is `schema` ("schema design 是显式模式");
// - a value edit is one `record_patch` for one field with that record's `expected_frontier`; a
//   `stale_frontier` keeps the user's draft, re-reads the records and asks for a resubmit;
// - uncommitted drafts (cells and record bodies) live here, keyed by record and field, so
//   switching table/board or closing and reopening the panel does not lose them;
// - the draft filter/sort/group is ephemeral (a query parameter only); only "save view" in
//   schema mode writes it, with `view_update`;
// - one intent = one idempotency key: a retry of the same write reuses its key, a different
//   value, frontier or target is a new intent;
// - records are read with the active view's `view_id`, so the server applies the saved
//   filter/sort/group; only an ephemeral draft query is sent explicitly, without `view_id`;
// - an `invalid_update` with `details.reason="invalid_field_value"` is shown on the cell its
//   `details.field_id` names, with that reason's wording;
// - under field secrecy a record's `semantic_content` is withheld whole (`{}`), body included
//   (`rest-api-v1.md` v0.6 "field secrecy 下的 `semantic_content`"): the body is then shown as
//   unreadable and cannot be edited, never as an empty text that a save would overwrite;
// - archiving a record (`record_archive`) needs the user's explicit confirmation.

import { writable, type Readable } from 'svelte/store';
import type {
	AcceptedChange,
	CreateCollectionRecordInput,
	FlowCollectionDescription,
	FlowCollectionFieldType,
	FlowCollectionRecordsQuery,
	FlowCollectionRecordsResponse,
	FlowCommandInput,
	FlowObjectView,
	FlowRecordFilter,
	FlowRecordSort
} from '#lib/api/flow.js';
import type { ApiResult } from '#lib/api/client.js';
import {
	flowErrorDetailKey,
	flowErrorFromEnvelope,
	flowErrorI18nKey,
	invalidUpdateDisposition
} from './errors';
import type { FlowError } from './types';
import {
	COLLECTION_LABEL_MAX_CHARS,
	CREATABLE_FIELD_TYPES,
	ENCODE_FAILURE_KEYS,
	clampColumnWidth,
	encodeValue,
	parseCollection,
	reorderIndex,
	type CollectionField,
	type CollectionView,
	type CollectionViewKind,
	type DraftValue,
	type ParsedCollection
} from './collection-values';

export const RECORD_PAGE_SIZE = 50;
/** Upper bound on the pages a standalone record page walks to find its row. */
export const RECORD_LOOKUP_MAX_PAGES = 20;

export type CollectionMode = 'entry' | 'schema';

export interface CollectionReads {
	getCollection(collectionId: string): Promise<ApiResult<FlowCollectionDescription>>;
	/** `GET /flow/objects/{id}/schema`: for a Record, its owning Collection's schema. */
	getSchema(objectId: string): Promise<ApiResult<FlowCollectionDescription>>;
	listRecords(
		collectionId: string,
		query: FlowCollectionRecordsQuery
	): Promise<ApiResult<FlowCollectionRecordsResponse>>;
	openRecord(recordId: string): Promise<FlowObjectView>;
	closeRecord(recordId: string): void;
}

export interface CollectionViewOptions {
	/** A Record opened on its own route: the schema is read through that record
	 * (`GET /flow/objects/{record}/schema`, readable by whoever can read the record and its
	 * Collection) instead of `GET /flow/collections/{id}`. */
	readonly schemaFromRecord?: string;
}

export interface CollectionWrites {
	execute(
		objectId: string,
		command: FlowCommandInput,
		options?: { idempotencyKey?: string; expectedFrontier?: string; message?: string }
	): Promise<AcceptedChange>;
	createRecord(
		collectionId: string,
		input: Omit<CreateCollectionRecordInput, 'idempotency_key'>,
		options?: { idempotencyKey?: string }
	): Promise<AcceptedChange>;
}

export interface RecordRow {
	readonly recordId: string;
	readonly frontier: string;
	readonly values: Readonly<Record<string, unknown>>;
}

export type CellStatus =
	| { readonly state: 'saving' }
	| { readonly state: 'conflict' }
	| { readonly state: 'error'; readonly key: string; readonly code: string };

export interface RecordPanelState {
	readonly recordId: string;
	readonly status: 'opening' | 'open' | 'error';
	/** The stored body (`record:body`), `null` when the record has none. */
	readonly body: string | null;
	/** The server withheld the record's `semantic_content` under field secrecy: the body cannot
	 * be read, so it is neither shown as empty nor editable. */
	readonly bodyWithheld: boolean;
	readonly errorKey: string | null;
	readonly bodyStatus: CellStatus | null;
}

export interface DraftQuery {
	readonly filter: FlowRecordFilter | null;
	readonly sort: FlowRecordSort | null;
	readonly groupFieldId: string | null;
}

export interface CollectionNotice {
	readonly key: string;
	readonly code: string | null;
}

export interface CollectionViewState {
	readonly status: 'loading' | 'ready' | 'error';
	readonly errorKey: string | null;
	readonly collection: ParsedCollection | null;
	readonly activeViewId: string | null;
	readonly mode: CollectionMode;
	readonly rows: readonly RecordRow[];
	readonly nextCursor: string | null;
	readonly loadingRecords: boolean;
	/** The ephemeral query override; `null` follows the saved view. */
	readonly draftQuery: DraftQuery | null;
	/** Ephemeral column widths, per view, on top of the saved `config.column_widths`. */
	readonly columnWidths: Readonly<Record<string, Readonly<Record<string, number>>>>;
	readonly panel: RecordPanelState | null;
	/** Uncommitted cell drafts: record id -> field id -> editor value. */
	readonly drafts: Readonly<Record<string, Readonly<Record<string, DraftValue>>>>;
	/** Uncommitted record body drafts: record id -> text. */
	readonly bodyDrafts: Readonly<Record<string, string>>;
	/** `${recordId}:${fieldId}` -> status of the last commit of that cell. */
	readonly cellStatus: Readonly<Record<string, CellStatus>>;
	readonly schemaBusy: boolean;
	readonly notice: CollectionNotice | null;
}

export type SchemaOutcome =
	| { readonly ok: true }
	| {
			readonly ok: false;
			readonly key: string;
			readonly code: string | null;
			/** The field a refusal names (`invalid_field_value`'s `details.field_id`). */
			readonly fieldId?: string;
	  };

export type ArchiveRecordOutcome =
	| { readonly kind: 'needs_confirmation' }
	| { readonly kind: 'archived' }
	| { readonly kind: 'failed'; readonly key: string; readonly code: string };

export const SCHEMA_MODE_REQUIRED_KEY = 'flow.collection.schema.required';
export const RECORD_STALE_KEY = 'flow.collection.record.stale';

function newKey(): string {
	return typeof crypto !== 'undefined' && 'randomUUID' in crypto
		? crypto.randomUUID()
		: `idem-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function newUuid(): string {
	return typeof crypto !== 'undefined' && 'randomUUID' in crypto
		? crypto.randomUUID()
		: '00000000-0000-4000-8000-000000000000'.replace(/0/g, () =>
				Math.floor(Math.random() * 16).toString(16)
			);
}

function asFlowError(error: unknown): FlowError {
	if (typeof error === 'object' && error !== null && 'code' in error) {
		return error as FlowError;
	}
	return { code: 'invalid_update', recoverable: false, origin: 'client' };
}

function cellKey(recordId: string, fieldId: string): string {
	return `${recordId}:${fieldId}`;
}

function recordBody(view: FlowObjectView): string | null {
	// Only the `record:body` block is read here. Property values are never taken from
	// `semantic_content`: `values_by_field_id` is the one redacted representation (`ADR-0018`
	// NR-7), and a second source could disagree with it.
	const content = view.semantic_content;
	if (typeof content !== 'object' || content === null) return null;
	const nodes = (content as { nodes?: unknown }).nodes;
	if (typeof nodes !== 'object' || nodes === null) return null;
	const node = (nodes as Record<string, unknown>)['record:body'];
	if (typeof node !== 'object' || node === null) return null;
	if ((node as { deleted?: unknown }).deleted === true) return null;
	const properties = (node as { properties?: unknown }).properties;
	if (typeof properties !== 'object' || properties === null) return null;
	const body = (properties as Record<string, unknown>).body;
	return typeof body === 'string' ? body : null;
}

/** Whether the server withheld this record's content: field secrecy is on for the Collection
 * and `semantic_content` came back as the empty object. A record that merely has no body still
 * carries its `nodes`. */
function bodyWithheld(view: FlowObjectView, collection: ParsedCollection | null): boolean {
	const secret =
		collection !== null &&
		(collection.fieldSecrecyEnabled || collection.fields.some((field) => field.restricted));
	if (!secret) return false;
	const content = view.semantic_content;
	return (
		typeof content === 'object' &&
		content !== null &&
		!Array.isArray(content) &&
		Object.keys(content).length === 0
	);
}

/** `details.field_id` of an `invalid_field_value` refusal, else `null`. */
function invalidFieldId(error: FlowError): string | null {
	if (invalidUpdateDisposition(error)?.reason !== 'invalid_field_value') return null;
	const fieldId = (error.details as { field_id?: unknown } | undefined)?.field_id;
	return typeof fieldId === 'string' && fieldId.length > 0 ? fieldId : null;
}

/** A refusal's status for a cell: the named reason's key and code when it has one. */
function cellError(error: FlowError): CellStatus {
	const reason = invalidUpdateDisposition(error)?.reason;
	return { state: 'error', key: flowErrorDetailKey(error), code: reason ?? error.code };
}

function rowFromItem(item: FlowCollectionRecordsResponse['items'][number]): RecordRow {
	return {
		recordId: item.record.id,
		frontier: item.record.frontier,
		values: item.values_by_field_id ?? {}
	};
}

interface WriteIntent {
	readonly signature: string;
	readonly key: string;
}

const INITIAL_STATE: CollectionViewState = {
	status: 'loading',
	errorKey: null,
	collection: null,
	activeViewId: null,
	mode: 'entry',
	rows: [],
	nextCursor: null,
	loadingRecords: false,
	draftQuery: null,
	columnWidths: {},
	panel: null,
	drafts: {},
	bodyDrafts: {},
	cellStatus: {},
	schemaBusy: false,
	notice: null
};

export class CollectionViewController {
	readonly collectionId: string;
	private readonly reads: CollectionReads;
	private readonly schemaFromRecord: string | null;
	private readonly writes: CollectionWrites;
	private readonly store = writable<CollectionViewState>(INITIAL_STATE);
	private current: CollectionViewState = INITIAL_STATE;
	/** Latest known frontier per record, from whichever read or accepted write is newest. */
	private readonly frontiers = new Map<string, string>();
	private readonly intents = new Map<string, WriteIntent>();
	/** Client-minted field/view ids of a create intent still unconfirmed, so a retry of the same
	 * create resends the same id (the server's replay identity includes it). */
	private readonly pendingIds = new Map<string, string>();
	private recordsGeneration = 0;
	private openedRecordId: string | null = null;
	private disposed = false;

	constructor(
		collectionId: string,
		reads: CollectionReads,
		writes: CollectionWrites,
		options: CollectionViewOptions = {}
	) {
		this.collectionId = collectionId;
		this.reads = reads;
		this.writes = writes;
		this.schemaFromRecord = options.schemaFromRecord ?? null;
	}

	get state(): Readable<CollectionViewState> {
		return { subscribe: this.store.subscribe };
	}

	snapshot(): CollectionViewState {
		return this.current;
	}

	private patch(update: Partial<CollectionViewState>): void {
		this.current = { ...this.current, ...update };
		this.store.set(this.current);
	}

	// ---- derived reads (the same pure functions the components use on a state snapshot) ------

	activeView(): CollectionView | null {
		return activeViewIn(this.current);
	}

	field(fieldId: string): CollectionField | null {
		return this.current.collection?.fields.find((field) => field.fieldId === fieldId) ?? null;
	}

	effectiveQuery(): DraftQuery {
		return effectiveQueryIn(this.current);
	}

	boardGroupField(): CollectionField | null {
		return boardGroupFieldIn(this.current);
	}

	columnWidth(fieldId: string): number | null {
		return columnWidthIn(this.current, fieldId);
	}

	draftFor(recordId: string, fieldId: string): DraftValue | null {
		return draftIn(this.current, recordId, fieldId);
	}

	// ---- loading ------------------------------------------------------------------------------

	async load(preferredViewId: string | null = null): Promise<void> {
		this.patch({ status: 'loading', errorKey: null });
		const ok = await this.reloadSchema(preferredViewId);
		if (!ok) return;
		await this.reloadRecords();
	}

	private async reloadSchema(preferredViewId: string | null = null): Promise<boolean> {
		const result = this.schemaFromRecord
			? await this.reads.getSchema(this.schemaFromRecord)
			: await this.reads.getCollection(this.collectionId);
		if (this.disposed) return false;
		if (result.code !== 0 || !result.data) {
			const error = flowErrorFromEnvelope(result);
			this.patch({
				status: 'error',
				errorKey: error ? flowErrorI18nKey(error) : 'flow.collection.loadFailed'
			});
			return false;
		}
		const collection = parseCollection(result.data);
		const wanted = preferredViewId ?? this.current.activeViewId;
		const activeViewId =
			collection.views.find((view) => view.viewId === wanted)?.viewId ??
			collection.views[0]?.viewId ??
			null;
		this.patch({ status: 'ready', collection, activeViewId });
		return true;
	}

	private recordsQuery(cursor: string | null): FlowCollectionRecordsQuery {
		const query = this.effectiveQuery();
		const view = this.activeView();
		const group =
			view?.kind === 'board' ? (this.boardGroupField()?.fieldId ?? undefined) : undefined;
		const page = { cursor: cursor ?? undefined, limit: RECORD_PAGE_SIZE };
		if (view && !this.current.draftQuery) {
			// The saved view: the server reads its filter/sort/group by `view_id`. A board whose
			// saved group does not name a live select field falls back to another one here, and
			// only that fallback is sent explicitly.
			return {
				view_id: view.viewId,
				...(group !== undefined && group !== view.query.groupFieldId ? { group } : {}),
				...page
			};
		}
		// An ephemeral draft (or no view at all): everything explicit and no `view_id`, so a part
		// the draft cleared is not filled back in from the saved view.
		return {
			filter: query.filter ?? undefined,
			sort: query.sort ?? undefined,
			group,
			...page
		};
	}

	/** Reads the first page for the effective query, replacing the rows. */
	async reloadRecords(): Promise<void> {
		const generation = ++this.recordsGeneration;
		this.patch({ loadingRecords: true });
		const result = await this.reads.listRecords(this.collectionId, this.recordsQuery(null));
		if (this.disposed || generation !== this.recordsGeneration) return;
		if (result.code !== 0 || !result.data) {
			const error = flowErrorFromEnvelope(result);
			this.patch({
				loadingRecords: false,
				notice: {
					key: error ? flowErrorI18nKey(error) : 'flow.collection.loadFailed',
					code: error?.code ?? null
				}
			});
			return;
		}
		const rows = result.data.items.map(rowFromItem);
		for (const row of rows) this.frontiers.set(row.recordId, row.frontier);
		this.patch({
			rows,
			nextCursor: result.data.next_cursor ?? null,
			loadingRecords: false
		});
	}

	/** Reads the next page with the same query (the cursor is bound to it server-side). */
	async loadMore(): Promise<void> {
		const cursor = this.current.nextCursor;
		if (!cursor || this.current.loadingRecords) return;
		const generation = this.recordsGeneration;
		this.patch({ loadingRecords: true });
		const result = await this.reads.listRecords(this.collectionId, this.recordsQuery(cursor));
		if (this.disposed || generation !== this.recordsGeneration) return;
		if (result.code !== 0 || !result.data) {
			const error = flowErrorFromEnvelope(result);
			this.patch({
				loadingRecords: false,
				notice: {
					key: error ? flowErrorI18nKey(error) : 'flow.collection.loadFailed',
					code: error?.code ?? null
				}
			});
			return;
		}
		const known = new Set(this.current.rows.map((row) => row.recordId));
		const added = result.data.items.map(rowFromItem).filter((row) => !known.has(row.recordId));
		for (const row of added) this.frontiers.set(row.recordId, row.frontier);
		this.patch({
			rows: [...this.current.rows, ...added],
			nextCursor: result.data.next_cursor ?? null,
			loadingRecords: false
		});
	}

	/** Pages through the records until `recordId` is loaded (a standalone record page). */
	async findRecord(recordId: string): Promise<RecordRow | null> {
		for (let page = 0; page < RECORD_LOOKUP_MAX_PAGES; page += 1) {
			const row = this.current.rows.find((candidate) => candidate.recordId === recordId);
			if (row) return row;
			if (!this.current.nextCursor) return null;
			await this.loadMore();
		}
		return this.current.rows.find((candidate) => candidate.recordId === recordId) ?? null;
	}

	// ---- view, mode and ephemeral query --------------------------------------------------------

	async setActiveView(viewId: string): Promise<void> {
		if (viewId === this.current.activeViewId) return;
		if (!this.current.collection?.views.some((view) => view.viewId === viewId)) return;
		// The panel and every draft stay: they are keyed by record, not by view.
		this.patch({ activeViewId: viewId, draftQuery: null });
		await this.reloadRecords();
	}

	setMode(mode: CollectionMode): void {
		this.patch({ mode, notice: null });
	}

	async setDraftQuery(update: Partial<DraftQuery>): Promise<void> {
		const base = this.effectiveQuery();
		this.patch({ draftQuery: { ...base, ...update } });
		await this.reloadRecords();
	}

	async clearDraftQuery(): Promise<void> {
		if (!this.current.draftQuery) return;
		this.patch({ draftQuery: null });
		await this.reloadRecords();
	}

	setColumnWidth(fieldId: string, width: number): void {
		// A Collection without views still resizes, under the '' key (nothing to save it to).
		const viewKey = this.activeView()?.viewId ?? '';
		const forView = { ...(this.current.columnWidths[viewKey] ?? {}) };
		forView[fieldId] = clampColumnWidth(width);
		this.patch({ columnWidths: { ...this.current.columnWidths, [viewKey]: forView } });
	}

	dismissNotice(): void {
		this.patch({ notice: null });
	}

	// ---- value edits --------------------------------------------------------------------------

	setDraft(recordId: string, fieldId: string, value: DraftValue): void {
		const forRecord = { ...(this.current.drafts[recordId] ?? {}), [fieldId]: value };
		const cellStatus = { ...this.current.cellStatus };
		const status = cellStatus[cellKey(recordId, fieldId)];
		if (status && status.state === 'error') delete cellStatus[cellKey(recordId, fieldId)];
		this.patch({ drafts: { ...this.current.drafts, [recordId]: forRecord }, cellStatus });
	}

	cancelDraft(recordId: string, fieldId: string): void {
		const forRecord = { ...(this.current.drafts[recordId] ?? {}) };
		delete forRecord[fieldId];
		const drafts = { ...this.current.drafts };
		if (Object.keys(forRecord).length === 0) delete drafts[recordId];
		else drafts[recordId] = forRecord;
		const cellStatus = { ...this.current.cellStatus };
		delete cellStatus[cellKey(recordId, fieldId)];
		this.patch({ drafts, cellStatus });
	}

	private intentKey(scope: string, signature: string): string {
		const existing = this.intents.get(scope);
		if (existing && existing.signature === signature) return existing.key;
		const key = newKey();
		this.intents.set(scope, { signature, key });
		return key;
	}

	private setCellStatus(recordId: string, fieldId: string, status: CellStatus | null): void {
		const cellStatus = { ...this.current.cellStatus };
		if (status) cellStatus[cellKey(recordId, fieldId)] = status;
		else delete cellStatus[cellKey(recordId, fieldId)];
		this.patch({ cellStatus });
	}

	private applyAccepted(
		recordId: string,
		change: AcceptedChange,
		values: Record<string, unknown>
	): void {
		const frontier = change.head_frontier || change.object.frontier;
		this.frontiers.set(recordId, frontier);
		this.patch({
			rows: this.current.rows.map((row) =>
				row.recordId === recordId
					? { recordId, frontier, values: { ...row.values, ...values } }
					: row
			)
		});
	}

	/**
	 * Commits one cell: one `record_patch` with one property and the record's latest known
	 * frontier as `expected_frontier`. Returns whether the value was accepted.
	 */
	async commitCell(recordId: string, fieldId: string, value?: DraftValue): Promise<boolean> {
		if (value !== undefined) this.setDraft(recordId, fieldId, value);
		const draft = this.draftFor(recordId, fieldId);
		const field = this.field(fieldId);
		if (draft === null || !field) return false;
		const encoded = encodeValue(field, draft);
		if (!encoded.ok) {
			this.setCellStatus(recordId, fieldId, {
				state: 'error',
				key: ENCODE_FAILURE_KEYS[encoded.reason],
				code: encoded.reason
			});
			return false;
		}
		const frontier = this.frontiers.get(recordId);
		if (frontier === undefined) return false;
		const properties = { [fieldId]: encoded.value };
		const key = this.intentKey(
			`cell:${recordId}:${fieldId}`,
			JSON.stringify([frontier, encoded.value])
		);
		this.setCellStatus(recordId, fieldId, { state: 'saving' });
		try {
			const change = await this.writes.execute(
				this.collectionId,
				{ type: 'record_patch', payload: { record_id: recordId, properties } },
				{ idempotencyKey: key, expectedFrontier: frontier }
			);
			if (this.disposed) return false;
			this.intents.delete(`cell:${recordId}:${fieldId}`);
			this.applyAccepted(recordId, change, properties);
			this.cancelDraft(recordId, fieldId);
			return true;
		} catch (raw) {
			const error = asFlowError(raw);
			if (error.code === 'stale_frontier') {
				// Someone else wrote this record since it was read. Keep the draft, re-read the
				// records (fresh frontier and values), and let the user resubmit deliberately.
				this.setCellStatus(recordId, fieldId, { state: 'conflict' });
				this.patch({ notice: { key: RECORD_STALE_KEY, code: error.code } });
				await this.reloadRecords();
				return false;
			}
			// `invalid_field_value` names the refused field; its cell shows the reason.
			const target = invalidFieldId(error) ?? fieldId;
			if (target !== fieldId) this.setCellStatus(recordId, fieldId, null);
			this.setCellStatus(recordId, target, cellError(error));
			return false;
		}
	}

	/** Board drag (or its keyboard alternative): set the grouping `select` field of one record. */
	async moveCard(recordId: string, toOption: string | null): Promise<boolean> {
		const field = this.boardGroupField();
		if (!field) return false;
		const row = this.current.rows.find((candidate) => candidate.recordId === recordId);
		if (!row) return false;
		const currentValue = row.values[field.fieldId];
		if ((currentValue ?? null) === toOption) return true;
		return this.commitCell(recordId, field.fieldId, toOption ?? '');
	}

	// ---- record create -----------------------------------------------------------------------

	/** `POST .../records` with `values_by_field_id` (only fields the caller set). */
	async createRecord(values: Readonly<Record<string, DraftValue>> = {}): Promise<SchemaOutcome> {
		const encodedValues: Record<string, unknown> = {};
		for (const [fieldId, draft] of Object.entries(values)) {
			const field = this.field(fieldId);
			if (!field) continue;
			const encoded = encodeValue(field, draft);
			if (!encoded.ok) {
				return { ok: false, key: ENCODE_FAILURE_KEYS[encoded.reason], code: encoded.reason };
			}
			if (encoded.value !== null) encodedValues[fieldId] = encoded.value;
		}
		const key = this.intentKey('record_create', JSON.stringify(encodedValues));
		try {
			await this.writes.createRecord(
				this.collectionId,
				{ values_by_field_id: encodedValues },
				{ idempotencyKey: key }
			);
		} catch (raw) {
			const error = asFlowError(raw);
			const status = cellError(error);
			const fieldId = invalidFieldId(error);
			return {
				ok: false,
				key: status.state === 'error' ? status.key : flowErrorI18nKey(error),
				code: status.state === 'error' ? status.code : error.code,
				...(fieldId ? { fieldId } : {})
			};
		}
		this.intents.delete('record_create');
		await this.reloadRecords();
		return { ok: true };
	}

	// ---- record panel -------------------------------------------------------------------------

	/** Opens the record panel: the only place a record document is opened. */
	async openPanel(recordId: string): Promise<void> {
		if (this.openedRecordId === recordId && this.current.panel?.status !== 'error') return;
		this.closeOpenedRecord();
		this.openedRecordId = recordId;
		this.patch({
			panel: {
				recordId,
				status: 'opening',
				body: null,
				bodyWithheld: false,
				errorKey: null,
				bodyStatus: null
			}
		});
		try {
			const view = await this.reads.openRecord(recordId);
			if (this.disposed || this.openedRecordId !== recordId) return;
			// The panel's read is the newest one of this record.
			this.frontiers.set(recordId, view.frontier);
			this.patch({
				panel: {
					recordId,
					status: 'open',
					body: recordBody(view),
					bodyWithheld: bodyWithheld(view, this.current.collection),
					errorKey: null,
					bodyStatus: null
				}
			});
		} catch (raw) {
			if (this.openedRecordId !== recordId) return;
			const error = asFlowError(raw);
			// A failed open holds no document: release the reference right away.
			this.reads.closeRecord(recordId);
			this.openedRecordId = null;
			this.patch({
				panel: {
					recordId,
					status: 'error',
					body: null,
					bodyWithheld: false,
					errorKey: flowErrorI18nKey(error),
					bodyStatus: null
				}
			});
		}
	}

	closePanel(): void {
		this.closeOpenedRecord();
		this.patch({ panel: null });
	}

	private closeOpenedRecord(): void {
		if (this.openedRecordId) {
			this.reads.closeRecord(this.openedRecordId);
			this.openedRecordId = null;
		}
	}

	setBodyDraft(recordId: string, text: string): void {
		this.patch({ bodyDrafts: { ...this.current.bodyDrafts, [recordId]: text } });
	}

	discardBodyDraft(recordId: string): void {
		const bodyDrafts = { ...this.current.bodyDrafts };
		delete bodyDrafts[recordId];
		this.patch({ bodyDrafts });
	}

	private setBodyStatus(recordId: string, bodyStatus: CellStatus | null, body?: string): void {
		const panel = this.current.panel;
		if (!panel || panel.recordId !== recordId) return;
		this.patch({
			panel: { ...panel, bodyStatus, body: body === undefined ? panel.body : body }
		});
	}

	/** Saves the record body draft: one `record_patch {record_id, body}`. */
	async commitBody(recordId: string): Promise<boolean> {
		const draft = this.current.bodyDrafts[recordId];
		const frontier = this.frontiers.get(recordId);
		if (draft === undefined || frontier === undefined) return false;
		// A withheld body is not known to be empty: saving would overwrite what the user cannot see.
		const panel = this.current.panel;
		if (panel && panel.recordId === recordId && panel.bodyWithheld) return false;
		const key = this.intentKey(`body:${recordId}`, JSON.stringify([frontier, draft]));
		this.setBodyStatus(recordId, { state: 'saving' });
		try {
			const change = await this.writes.execute(
				this.collectionId,
				{ type: 'record_patch', payload: { record_id: recordId, body: draft } },
				{ idempotencyKey: key, expectedFrontier: frontier }
			);
			if (this.disposed) return false;
			this.intents.delete(`body:${recordId}`);
			this.applyAccepted(recordId, change, {});
			this.discardBodyDraft(recordId);
			this.setBodyStatus(recordId, null, draft);
			return true;
		} catch (raw) {
			const error = asFlowError(raw);
			if (error.code === 'stale_frontier') {
				this.setBodyStatus(recordId, { state: 'conflict' });
				await this.refreshRecordFrontier(recordId);
				return false;
			}
			this.setBodyStatus(recordId, {
				state: 'error',
				key: flowErrorI18nKey(error),
				code: error.code
			});
			return false;
		}
	}

	/** Re-reads an open record after a body conflict (fresh frontier and stored body). */
	private async refreshRecordFrontier(recordId: string): Promise<void> {
		if (this.openedRecordId !== recordId) return;
		try {
			// Reopen-then-close keeps the reference count balanced while forcing a fresh read.
			this.reads.closeRecord(recordId);
			const view = await this.reads.openRecord(recordId);
			this.frontiers.set(recordId, view.frontier);
			const panel = this.current.panel;
			if (panel && panel.recordId === recordId) {
				this.patch({
					panel: {
						...panel,
						body: recordBody(view),
						bodyWithheld: bodyWithheld(view, this.current.collection)
					}
				});
			}
		} catch (raw) {
			this.openedRecordId = null;
			const error = asFlowError(raw);
			this.setBodyStatus(recordId, {
				state: 'error',
				key: flowErrorI18nKey(error),
				code: error.code
			});
		}
	}

	/**
	 * Archives one record with `record_archive` -- only after the user confirmed (`confirmed`
	 * comes from their action, never a default). A retry of the same archive reuses its key. On
	 * success the record leaves the list and its panel closes.
	 */
	async archiveRecord(recordId: string, confirmed: boolean): Promise<ArchiveRecordOutcome> {
		if (confirmed !== true) return { kind: 'needs_confirmation' };
		const key = this.intentKey(`archive:${recordId}`, recordId);
		try {
			await this.writes.execute(
				this.collectionId,
				{ type: 'record_archive', payload: { record_id: recordId } },
				{ idempotencyKey: key }
			);
		} catch (raw) {
			const error = asFlowError(raw);
			const status = cellError(error);
			return {
				kind: 'failed',
				key: status.state === 'error' ? status.key : flowErrorI18nKey(error),
				code: error.code
			};
		}
		if (this.disposed) return { kind: 'archived' };
		this.intents.delete(`archive:${recordId}`);
		if (this.current.panel?.recordId === recordId) this.closePanel();
		const drafts = { ...this.current.drafts };
		delete drafts[recordId];
		const bodyDrafts = { ...this.current.bodyDrafts };
		delete bodyDrafts[recordId];
		this.frontiers.delete(recordId);
		this.patch({
			rows: this.current.rows.filter((row) => row.recordId !== recordId),
			drafts,
			bodyDrafts
		});
		await this.reloadRecords();
		return { kind: 'archived' };
	}

	// ---- schema mode --------------------------------------------------------------------------

	private async schemaCommand(scope: string, command: FlowCommandInput): Promise<SchemaOutcome> {
		// Hard gate: no `field_*` / `view_*` command leaves this client outside schema mode.
		if (this.current.mode !== 'schema') {
			return { ok: false, key: SCHEMA_MODE_REQUIRED_KEY, code: null };
		}
		const collection = this.current.collection;
		if (!collection) return { ok: false, key: 'flow.collection.loadFailed', code: null };
		const key = this.intentKey(`schema:${scope}`, JSON.stringify([collection.frontier, command]));
		this.patch({ schemaBusy: true, notice: null });
		try {
			await this.writes.execute(this.collectionId, command, {
				idempotencyKey: key,
				expectedFrontier: collection.frontier
			});
			this.intents.delete(`schema:${scope}`);
		} catch (raw) {
			const error = asFlowError(raw);
			const outcome: SchemaOutcome = { ok: false, key: flowErrorI18nKey(error), code: error.code };
			if (error.code === 'stale_frontier') await this.reloadSchema();
			this.patch({ schemaBusy: false, notice: { key: outcome.key, code: error.code } });
			return outcome;
		}
		await this.reloadSchema();
		this.patch({ schemaBusy: false });
		return { ok: true };
	}

	private labelProblem(label: string): SchemaOutcome | null {
		const trimmed = label.trim();
		if (trimmed === '')
			return { ok: false, key: 'flow.collection.schema.labelRequired', code: null };
		if ([...trimmed].length > COLLECTION_LABEL_MAX_CHARS) {
			return { ok: false, key: 'flow.collection.schema.labelTooLong', code: null };
		}
		return null;
	}

	async createField(
		label: string,
		type: FlowCollectionFieldType,
		options: readonly string[] = []
	): Promise<SchemaOutcome> {
		if (this.current.mode !== 'schema') {
			return { ok: false, key: SCHEMA_MODE_REQUIRED_KEY, code: null };
		}
		const problem = this.labelProblem(label);
		if (problem) return problem;
		if (!(CREATABLE_FIELD_TYPES as readonly string[]).includes(type)) {
			return { ok: false, key: 'flow.collection.schema.typeUnsupported', code: null };
		}
		const config: Record<string, unknown> =
			type === 'select' || type === 'multi_select' ? { options: [...options] } : {};
		const scope = `field_create:${JSON.stringify([label.trim(), type, config])}`;
		const fieldId = this.pendingIds.get(scope) ?? newUuid();
		this.pendingIds.set(scope, fieldId);
		const outcome = await this.schemaCommand(scope, {
			type: 'field_create',
			payload: {
				field_id: fieldId,
				label: label.trim(),
				field_type: type,
				config,
				// Appended after every existing root node (fields and views share the root).
				index: this.current.collection?.rootOrder.length ?? 0
			}
		});
		if (outcome.ok) this.pendingIds.delete(scope);
		return outcome;
	}

	async renameField(fieldId: string, label: string): Promise<SchemaOutcome> {
		if (this.current.mode !== 'schema') {
			return { ok: false, key: SCHEMA_MODE_REQUIRED_KEY, code: null };
		}
		const problem = this.labelProblem(label);
		if (problem) return problem;
		return this.schemaCommand(`field_update:${fieldId}`, {
			type: 'field_update',
			payload: { field_id: fieldId, label: label.trim() }
		});
	}

	/** Replaces a select/multi_select field's options, keeping every other config key. */
	async setFieldOptions(fieldId: string, options: readonly string[]): Promise<SchemaOutcome> {
		const field = this.field(fieldId);
		if (!field) return { ok: false, key: 'flow.collection.loadFailed', code: null };
		const cleaned: string[] = [];
		for (const option of options) {
			const trimmed = option.trim();
			if (trimmed !== '' && !cleaned.includes(trimmed)) cleaned.push(trimmed);
		}
		return this.schemaCommand(`field_options:${fieldId}`, {
			type: 'field_update',
			payload: { field_id: fieldId, config: { ...field.config, options: cleaned } }
		});
	}

	/** Sends the type change as asked; the server decides (it refuses any change after creation,
	 * `invalid_update`), and the refusal is shown by `error_code`. */
	async changeFieldType(fieldId: string, type: FlowCollectionFieldType): Promise<SchemaOutcome> {
		return this.schemaCommand(`field_type:${fieldId}`, {
			type: 'field_update',
			payload: { field_id: fieldId, field_type: type }
		});
	}

	async archiveField(fieldId: string): Promise<SchemaOutcome> {
		return this.schemaCommand(`field_archive:${fieldId}`, {
			type: 'field_archive',
			payload: { field_id: fieldId }
		});
	}

	/** Moves a field one step up (-1) or down (+1) among the fields. */
	async moveField(fieldId: string, step: -1 | 1): Promise<SchemaOutcome> {
		const collection = this.current.collection;
		if (!collection) return { ok: false, key: 'flow.collection.loadFailed', code: null };
		const index = collection.fields.findIndex((field) => field.fieldId === fieldId);
		const neighbour = collection.fields[index + step];
		if (index < 0 || !neighbour) return { ok: true };
		const target = reorderIndex(collection.rootOrder, fieldId, neighbour.fieldId);
		if (target === null)
			return { ok: false, key: 'flow.collection.schema.reorderUnavailable', code: null };
		return this.schemaCommand(`field_reorder:${fieldId}`, {
			type: 'field_reorder',
			payload: { field_id: fieldId, index: target }
		});
	}

	async createView(name: string, kind: CollectionViewKind): Promise<SchemaOutcome> {
		if (this.current.mode !== 'schema') {
			return { ok: false, key: SCHEMA_MODE_REQUIRED_KEY, code: null };
		}
		const problem = this.labelProblem(name);
		if (problem) return problem;
		const scope = `view_create:${JSON.stringify([name.trim(), kind])}`;
		const viewId = this.pendingIds.get(scope) ?? newUuid();
		this.pendingIds.set(scope, viewId);
		const outcome = await this.schemaCommand(scope, {
			type: 'view_create',
			payload: {
				view_id: viewId,
				name: name.trim(),
				view_type: kind,
				config: {},
				index: this.current.collection?.rootOrder.length ?? 0
			}
		});
		if (outcome.ok) {
			this.pendingIds.delete(scope);
			await this.setActiveView(viewId);
		}
		return outcome;
	}

	async renameView(viewId: string, name: string): Promise<SchemaOutcome> {
		if (this.current.mode !== 'schema') {
			return { ok: false, key: SCHEMA_MODE_REQUIRED_KEY, code: null };
		}
		const problem = this.labelProblem(name);
		if (problem) return problem;
		return this.schemaCommand(`view_update:${viewId}`, {
			type: 'view_update',
			payload: { view_id: viewId, name: name.trim() }
		});
	}

	async moveView(viewId: string, step: -1 | 1): Promise<SchemaOutcome> {
		const collection = this.current.collection;
		if (!collection) return { ok: false, key: 'flow.collection.loadFailed', code: null };
		const index = collection.views.findIndex((view) => view.viewId === viewId);
		const neighbour = collection.views[index + step];
		if (index < 0 || !neighbour) return { ok: true };
		const target = reorderIndex(collection.rootOrder, viewId, neighbour.viewId);
		if (target === null)
			return { ok: false, key: 'flow.collection.schema.reorderUnavailable', code: null };
		return this.schemaCommand(`view_reorder:${viewId}`, {
			type: 'view_reorder',
			payload: { view_id: viewId, index: target }
		});
	}

	/** "Save view": writes the effective filter/sort/group and the column widths into the active
	 * view's config with `view_update` (schema mode only), keeping every other config key. */
	async saveActiveView(): Promise<SchemaOutcome> {
		const view = this.activeView();
		if (!view) return { ok: false, key: 'flow.collection.loadFailed', code: null };
		const query = this.effectiveQuery();
		const widths = {
			...view.query.columnWidths,
			...(this.current.columnWidths[view.viewId] ?? {})
		};
		const config: Record<string, unknown> = { ...view.config };
		config.filter = query.filter;
		config.sort = query.sort;
		if (view.kind === 'board') config.group_field_id = this.boardGroupField()?.fieldId ?? null;
		config.column_widths = widths;
		const outcome = await this.schemaCommand(`view_config:${view.viewId}`, {
			type: 'view_update',
			payload: { view_id: view.viewId, config }
		});
		if (outcome.ok) {
			const columnWidths = { ...this.current.columnWidths };
			delete columnWidths[view.viewId];
			this.patch({ draftQuery: null, columnWidths });
		}
		return outcome;
	}

	// ---- teardown -----------------------------------------------------------------------------

	dispose(): void {
		this.closeOpenedRecord();
		this.disposed = true;
	}
}

// ---- pure helpers used by the renderers ---------------------------------------------------------

export function activeViewIn(state: CollectionViewState): CollectionView | null {
	const collection = state.collection;
	if (!collection) return null;
	return collection.views.find((view) => view.viewId === state.activeViewId) ?? null;
}

/** The query the record list is read with: the draft override, else the saved view config. */
export function effectiveQueryIn(state: CollectionViewState): DraftQuery {
	if (state.draftQuery) return state.draftQuery;
	const view = activeViewIn(state);
	return {
		filter: view?.query.filter ?? null,
		sort: view?.query.sort ?? null,
		groupFieldId: view?.query.groupFieldId ?? null
	};
}

/** The board's grouping field: the effective group if it names a live `select` field, else the
 * first `select` field, else `null` (the board is then disabled). */
export function boardGroupFieldIn(state: CollectionViewState): CollectionField | null {
	const fields = state.collection?.fields ?? [];
	const selects = fields.filter((field) => field.type === 'select' && !field.restricted);
	const wanted = effectiveQueryIn(state).groupFieldId;
	return selects.find((field) => field.fieldId === wanted) ?? selects[0] ?? null;
}

/** A column's width: the ephemeral resize, else the saved `config.column_widths`, else `null`. */
export function columnWidthIn(state: CollectionViewState, fieldId: string): number | null {
	const view = activeViewIn(state);
	if (!view) return state.columnWidths['']?.[fieldId] ?? null;
	const local = state.columnWidths[view.viewId]?.[fieldId];
	if (local !== undefined) return local;
	return view.query.columnWidths[fieldId] ?? null;
}

/** The uncommitted draft of one cell, `null` when there is none. */
export function draftIn(
	state: CollectionViewState,
	recordId: string,
	fieldId: string
): DraftValue | null {
	return state.drafts[recordId]?.[fieldId] ?? null;
}

export interface VirtualWindow {
	readonly start: number;
	readonly end: number;
	readonly padTop: number;
	readonly padBottom: number;
}

/** Rows to render for a scroll position: only the visible slice plus `overscan` on each side,
 * never the whole list. */
export function virtualWindow(input: {
	readonly scrollTop: number;
	readonly viewportHeight: number;
	readonly rowHeight: number;
	readonly rowCount: number;
	readonly overscan?: number;
}): VirtualWindow {
	const overscan = input.overscan ?? 6;
	const rowHeight = Math.max(1, input.rowHeight);
	const count = Math.max(0, input.rowCount);
	const first = Math.floor(Math.max(0, input.scrollTop) / rowHeight);
	const visible = Math.ceil(Math.max(0, input.viewportHeight) / rowHeight) + 1;
	const start = Math.min(count, Math.max(0, first - overscan));
	const end = Math.min(count, first + visible + overscan);
	return {
		start,
		end,
		padTop: start * rowHeight,
		padBottom: (count - end) * rowHeight
	};
}

export interface BoardColumn {
	/** The option value, `null` for "no value". */
	readonly value: string | null;
	readonly rows: readonly RecordRow[];
}

/** Board columns for a `select` field: one per option in option order, then values that are not
 * (or no longer) options as their own columns, then "no value". */
export function boardColumns(rows: readonly RecordRow[], field: CollectionField): BoardColumn[] {
	const columns = new Map<string | null, RecordRow[]>();
	for (const option of field.options) columns.set(option, []);
	const extra: string[] = [];
	const empty: RecordRow[] = [];
	for (const row of rows) {
		const value = row.values[field.fieldId];
		if (typeof value !== 'string' || value === '') {
			empty.push(row);
			continue;
		}
		if (!columns.has(value)) {
			columns.set(value, []);
			extra.push(value);
		}
		columns.get(value)?.push(row);
	}
	const out: BoardColumn[] = field.options.map((option) => ({
		value: option,
		rows: columns.get(option) ?? []
	}));
	for (const value of extra) out.push({ value, rows: columns.get(value) ?? [] });
	out.push({ value: null, rows: empty });
	return out;
}

/** Grid keyboard navigation: the next `[row, column]` for a key, clamped to the grid. */
export function nextGridCell(
	position: readonly [number, number],
	key: string,
	rowCount: number,
	columnCount: number
): [number, number] | null {
	const [row, column] = position;
	switch (key) {
		case 'ArrowDown':
			return [Math.min(rowCount - 1, row + 1), column];
		case 'ArrowUp':
			return [Math.max(0, row - 1), column];
		case 'ArrowRight':
			return [row, Math.min(columnCount - 1, column + 1)];
		case 'ArrowLeft':
			return [row, Math.max(0, column - 1)];
		case 'Home':
			return [row, 0];
		case 'End':
			return [row, Math.max(0, columnCount - 1)];
		default:
			return null;
	}
}
