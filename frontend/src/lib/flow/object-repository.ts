// `ObjectRepository`: snapshot+tail load, engine lifecycle, IndexedDB accepted cache, reference
// counting (`contracts/ui-surface-v1.md` "职责边界"). This is the ONLY place that constructs a
// document's Loro engine doc ("同一 object 的 engine doc 只由 Repository 创建").
//
// v0.4 baseline note: `GET .../bootstrap` is not routed server-side yet (see `api/flow.ts`'s
// header comment), so `bootstrap()`/hydration in this package rides the WebSocket `snapshot`
// frame via a composed `ObjectSession` rather than a separate REST call -- there is exactly one
// WebSocket connection per open object, and this class is what owns starting it. This is a
// disclosed adaptation to the actual backend surface at this repo's baseline, not a second engine
// doc or a second update-sending path: `ObjectSession` (composed here, never constructed by a
// component) remains the only thing that ever calls `.submit()`.
//
// `loro-crdt` is imported dynamically, same rule as `editor-adapter.ts`: this keeps the engine
// chunk out of every route that isn't Flow.

import { readonly as readonlyStore, writable, type Readable } from 'svelte/store';
import {
	flowApi,
	type FlowAdminHealth,
	type FlowAdminIntegrity,
	type FlowAdminLag,
	type FlowBootstrap,
	type FlowCollectionDescription,
	type FlowCollectionQueryInput,
	type FlowCollectionRecordsQuery,
	type FlowCollectionRecordsResponse,
	type FlowExportJob,
	type FlowGrantsView,
	type FlowObjectView,
	type FlowPackageImportReport,
	type FlowProjectionLag,
	type FlowReferenceList,
	type FlowRelationListResponse,
	type FlowRelationsQuery,
	type FlowSearchQuery,
	type FlowSearchResponse
} from '$lib/api/flow';
import type { ApiResult } from '$lib/api/client';
import { FlowCommandService } from './command-service';
import {
	LoroObjectSession,
	type AcceptedNotice,
	type SessionOptions,
	type SnapshotPayload
} from './object-session';
import { LiveProjectionStore } from './projection-store';
import { checkUpdateBytes, negotiateFlowLimitsVersion } from './limits';
import { clientError, flowErrorFromEnvelope } from './errors';
import { openOutboxStore, type OutboxStore, type RecoveryDraftEntry } from './outbox';
import {
	FlowOfflineSync,
	type OfflineSyncHost,
	type OfflineSyncOptions,
	type ScratchReplica,
	type ScratchVerdict
} from './offline-sync';
import { buildRecoveryDraftDocument, recoveryDraftText } from './recovery-draft';
import type { RemotePresence } from './presence';
import type { EngineDiff, FlowError, ObjectHandle, ObjectProjection, SyncState } from './types';
// `LoroDocType` (from `loro-prosemirror`, type-only) is the specific `{doc, data}` container
// shape `LoroSyncPlugin`/`LoroUndoPlugin` require; `loro-crdt`'s own `LoroDoc` generic defaults
// to an unconstrained container-shape record. Both describe the same runtime object -- Loro
// containers are created on demand by string key, so this is a nominal typing gap between two
// packages, not a real structural mismatch. The single `as unknown as LoroDocType` cast below,
// at the one place the doc is constructed, resolves it without weakening any other signature in
// this module to `any`.
import type { LoroDocType } from 'loro-prosemirror';

const DB_NAME = 'sylvode-flow-cache-v1';
const STORE_NAME = 'snapshots';

interface CachedSnapshot {
	objectId: string;
	snapshotBytes: Uint8Array;
	headSeq: number;
	headFrontier: string;
	cachedAt: number;
}

function openIndexedDb(): Promise<IDBDatabase | null> {
	return new Promise((resolve) => {
		if (typeof indexedDB === 'undefined') {
			resolve(null);
			return;
		}
		const request = indexedDB.open(DB_NAME, 1);
		request.onupgradeneeded = () => {
			request.result.createObjectStore(STORE_NAME, { keyPath: 'objectId' });
		};
		request.onsuccess = () => resolve(request.result);
		request.onerror = () => resolve(null);
	});
}

async function readCachedSnapshot(objectId: string): Promise<CachedSnapshot | null> {
	const db = await openIndexedDb();
	if (!db) return null;
	return new Promise((resolve) => {
		const tx = db.transaction(STORE_NAME, 'readonly');
		const req = tx.objectStore(STORE_NAME).get(objectId);
		req.onsuccess = () => resolve((req.result as CachedSnapshot | undefined) ?? null);
		req.onerror = () => resolve(null);
	});
}

async function writeCachedSnapshot(entry: CachedSnapshot): Promise<void> {
	const db = await openIndexedDb();
	if (!db) return;
	await new Promise<void>((resolve) => {
		const tx = db.transaction(STORE_NAME, 'readwrite');
		tx.objectStore(STORE_NAME).put(entry);
		tx.oncomplete = () => resolve();
		tx.onerror = () => resolve();
	});
}

/** Structural shape of a Loro map, wide enough to cover any container name (see `setTitle`'s
 * comment for why `LoroDocType.getMap` needs widening for non-`doc`/`data` containers). */
export interface NamedLoroMap {
	get(key: string): unknown;
	set(key: string, value: unknown): void;
	delete(key: string): void;
	getShallowValue(): Record<string, unknown>;
}

/** Reads/creates a Loro map container by name outside the two well-known ones `LoroDocType`
 * exposes. Used for the Navigator document's `order` container (`domain-model-v1.md`: "Navigator
 * 一个 workspace/project 的导航排序与显示元数据") and the shared `meta` container `setTitle` uses. */
export function getNamedMap(doc: LoroDocType, name: string): NamedLoroMap {
	return (doc as unknown as { getMap(key: string): NamedLoroMap }).getMap(name);
}

export interface OpenFlowObjectResult {
	readonly handle: ObjectHandle;
	readonly session: LoroObjectSession;
	readonly projection: LiveProjectionStore;
	/** The CURRENT engine doc. A recovery rebuild replaces it (see `docGeneration`), so read it
	 * when needed rather than caching it. */
	readonly doc: LoroDocType;
	/** Bumps whenever `doc` was replaced by a rebuild from the accepted snapshot; a canvas bound to
	 * the old doc remounts on it. */
	readonly docGeneration: Readable<number>;
	readonly flowError: Readable<FlowError | null>;
	/** Local updates waiting for the server (the indicator's pending count). */
	readonly pending: Readable<number>;
	/** Intents that could not be replayed, with their reasons. */
	readonly recoveryDraft: Readable<ReadonlyArray<RecoveryDraftEntry>>;
	/** The user may not write this object (`view`/`comment`, or revoked mid-session). */
	readonly readOnly: Readable<boolean>;
	/** Remote peers on this document (ephemeral, never in the engine doc). */
	readonly presence: Readable<ReadonlyArray<RemotePresence>>;
	/** The outbox controller (diagnostics and tests; components use the stores above). */
	readonly offline: FlowOfflineSync;
}

/** Test seams for `FlowObjectRepository`. Production passes nothing. */
export interface FlowObjectRepositoryOptions {
	readonly outboxStore?: OutboxStore;
	readonly session?: SessionOptions;
	readonly offlineSync?: OfflineSyncOptions;
}

/** Effective levels that may write a document (`ADR-0012`: "`edit` 起可写、`view` 只读"). */
const WRITABLE_LEVELS: ReadonlySet<string> = new Set(['edit', 'full_access']);

class OpenEntry implements OpenFlowObjectResult {
	readonly handle: ObjectHandle;
	readonly session: LoroObjectSession;
	readonly projection: LiveProjectionStore;
	readonly flowError: Readable<FlowError | null>;
	readonly docGeneration: Readable<number>;
	readonly pending: Readable<number>;
	readonly recoveryDraft: Readable<ReadonlyArray<RecoveryDraftEntry>>;
	readonly readOnly: Readable<boolean>;
	readonly presence: Readable<ReadonlyArray<RemotePresence>>;
	readonly offline: FlowOfflineSync;
	readonly generationStore: ReturnType<typeof writable<number>>;
	readonly readOnlyStore: ReturnType<typeof writable<boolean>>;
	refCount = 1;
	current: LoroDocType;
	/** Docs replaced by a rebuild; freed on close, once nothing can still be bound to them. */
	readonly retired: LoroDocType[] = [];
	unsubscribeLocal: (() => void) | null = null;
	/** The next session `snapshot` replaces the doc instead of merging into it. */
	rebuildOnSnapshot = false;
	rebuildDone: (() => void) | null = null;

	constructor(input: {
		handle: ObjectHandle;
		session: LoroObjectSession;
		projection: LiveProjectionStore;
		flowError: Readable<FlowError | null>;
		doc: LoroDocType;
		offline: FlowOfflineSync;
		generationStore: ReturnType<typeof writable<number>>;
		readOnlyStore: ReturnType<typeof writable<boolean>>;
	}) {
		this.handle = input.handle;
		this.session = input.session;
		this.projection = input.projection;
		this.flowError = input.flowError;
		this.current = input.doc;
		this.offline = input.offline;
		this.generationStore = input.generationStore;
		this.readOnlyStore = input.readOnlyStore;
		this.docGeneration = readonlyStore(input.generationStore);
		this.readOnly = readonlyStore(input.readOnlyStore);
		this.pending = input.offline.pending;
		this.recoveryDraft = input.offline.recoveryDraft;
		this.presence = input.session.presence;
	}

	get doc(): LoroDocType {
		return this.current;
	}
}

interface OpenRecordEntry {
	refCount: number;
	view: Promise<FlowObjectView>;
}

async function loadLoro(): Promise<typeof import('loro-crdt')> {
	return import('loro-crdt');
}

function fromBase64(value: string): Uint8Array {
	const binary = atob(value);
	const out = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
	return out;
}

// Deliberately NOT `implements ObjectRepositoryContract`: `open()` here returns the richer
// `OpenFlowObjectResult` (handle + session + projection + doc + flowError), not the frozen
// interface's bare `ObjectHandle` -- callers (`FlowNavigator`/route pages) need the session and
// projection in the same call, not a second lookup. Every OTHER member below matches the contract
// signature exactly (`bootstrap`/`replaceWithAccepted`/`exportRecoveryDraft`/`close`/
// `getProjection`), so the shape is still the frozen one, just not a formal `implements` given
// TypeScript's structural typing would reject `open`'s covariant return.
export class FlowObjectRepository {
	private readonly open_ = new Map<string, OpenEntry>();
	private readonly options: FlowObjectRepositoryOptions;
	private outboxStore: Promise<OutboxStore> | null = null;

	constructor(options: FlowObjectRepositoryOptions = {}) {
		this.options = options;
	}

	private outbox(): Promise<OutboxStore> {
		if (!this.outboxStore) {
			this.outboxStore = this.options.outboxStore
				? Promise.resolve(this.options.outboxStore)
				: openOutboxStore();
		}
		return this.outboxStore;
	}
	// Record documents opened by a record panel (`openRecord`). A Record is server-only
	// (`flow/collab/ticket.rs::object_type_admits_collab_session` refuses a collab session and
	// `query.rs::get_bootstrap` refuses its bootstrap), so "opening" its document is the one REST
	// read of the record object, ref-counted here like `open_`; it never builds an engine doc.
	private readonly records_ = new Map<string, OpenRecordEntry>();
	// Ticket issuance is the only thing `ObjectSession` needs from `CommandService`
	// (`command-service.ts`'s `TicketIssuer`); owned here since `CommandService` has no
	// per-request state of its own and every open session shares one workspace-agnostic instance.
	private readonly commandService = new FlowCommandService();
	// Objects created by a surface other than the navigator itself (a Page's slash-menu
	// Collection embed). The navigator lives in the layout and loads its list once, so this is how
	// it learns that a new child exists without polling.
	private readonly createdObjects_ = writable<{ objectId: string; parentId: string | null } | null>(
		null
	);
	readonly createdObjects: Readable<{ objectId: string; parentId: string | null } | null> =
		readonlyStore(this.createdObjects_);

	/** Announces an object a non-navigator surface created (see `createdObjects`). */
	notifyObjectCreated(objectId: string, parentId: string | null): void {
		this.createdObjects_.set({ objectId, parentId });
	}

	/** Pulls the accepted snapshot+tail of an OPEN object over REST and merges it into its doc.
	 * Used after a server-side command changed the document (e.g. `create_collection_embed`
	 * writing the embed block), so the canvas shows the real block without waiting for the
	 * WebSocket's own resync. Returns whether the merge happened. */
	async resyncAccepted(objectId: string): Promise<boolean> {
		if (!this.open_.has(objectId)) return false;
		try {
			await this.replaceWithAccepted(await this.bootstrap(objectId));
			return true;
		} catch {
			return false;
		}
	}

	async open(input: {
		workspaceId: string;
		objectId: string;
		signal: AbortSignal;
		/** The object as the caller already read it (`get`), so the route page that dispatches on
		 * `object_type` first does not read it twice. */
		object?: FlowObjectView;
	}): Promise<OpenFlowObjectResult> {
		const existing = this.open_.get(input.objectId);
		if (existing) {
			existing.refCount += 1;
			return existing;
		}

		let object: FlowObjectView;
		if (input.object && input.object.id === input.objectId) {
			object = input.object;
		} else {
			const objectResult = await flowApi.getObject(input.objectId);
			if (objectResult.code !== 0 || !objectResult.data) {
				throw {
					code: objectResult.code === 404 ? 'not_found' : 'forbidden',
					recoverable: false
				} satisfies FlowError;
			}
			object = objectResult.data;
		}
		if (input.signal.aborted) {
			throw { code: 'not_found', recoverable: false } satisfies FlowError;
		}

		const { LoroDoc } = await loadLoro();
		const doc = new LoroDoc() as unknown as LoroDocType;

		const projection = new LiveProjectionStore(objectToProjection(object));
		const flowErrorStore = writable<FlowError | null>(null);
		const generationStore = writable(0);
		const readOnlyStore = writable(false);
		let lastKnownFrontierBase64 = '';
		// Assigned right below; the session hooks only run after `connect`, by which time it is set.
		let entry: OpenEntry | null = null;
		const liveDoc = (): LoroDocType => entry?.doc ?? doc;

		const session = new LoroObjectSession(
			{
				onSnapshot: (payload: SnapshotPayload) => {
					if (entry?.rebuildOnSnapshot) {
						// A rebuild requested by the outbox for a user the REST bootstrap refuses:
						// this snapshot (the same loader) becomes the whole new doc.
						entry.rebuildOnSnapshot = false;
						void this.rebuildFromSnapshotPayload(entry, payload);
						lastKnownFrontierBase64 = payload.headFrontier;
						return;
					}
					const target = liveDoc();
					target.import(payload.snapshotBytes);
					for (const update of payload.tailUpdates) {
						target.import(update.bytes);
					}
					lastKnownFrontierBase64 = payload.headFrontier;
					projection.applyEngineDiff({ documentId: object.document_id } satisfies EngineDiff, payload.headSeq);
					void writeCachedSnapshot({
						objectId: input.objectId,
						snapshotBytes: target.export({ mode: 'snapshot' }),
						headSeq: payload.headSeq,
						headFrontier: payload.headFrontier,
						cachedAt: Date.now()
					});
				},
				onAccepted: (notice: AcceptedNotice) => {
					// A peer's committed update arrives with its bytes and is applied in seq order
					// (the session already checked `seq == last+1`). `import` is not a local
					// change, so it never re-enters the outbox below.
					if (notice.remoteBytes) liveDoc().import(notice.remoteBytes);
					lastKnownFrontierBase64 = notice.headFrontier;
					projection.applyEngineDiff({ documentId: object.document_id } satisfies EngineDiff, notice.headSeq);
				},
				onFlowError: (error: FlowError) => {
					flowErrorStore.set(error);
				},
				onConnected: () => {
					entry?.offline.onConnected();
				},
				onTerminal: (error: FlowError) => {
					void entry?.offline.onTerminal({
						code: 'server_rejected',
						source: 'server',
						details: error.details
					});
				},
				onRevoked: (error: FlowError) => {
					readOnlyStore.set(true);
					void entry?.offline.onRevoked({ code: error.code, source: 'server' });
				}
			},
			this.commandService,
			this.options.session
		);

		const handle: ObjectHandle = {
			workspaceId: input.workspaceId,
			objectId: input.objectId,
			objectType: object.object_type,
			documentId: object.document_id,
			object
		};

		const host: OfflineSyncHost = {
			objectId: input.objectId,
			documentId: object.document_id,
			peerId: () => liveDoc().peerIdStr,
			limitViolation: (bytes) => {
				const violation = checkUpdateBytes(bytes, session.effectiveLimits);
				return violation
					? clientError('limit_exceeded', {
							limit_kind: violation.limitKind,
							limit: violation.limit,
							observed: violation.observed
						})
					: null;
			},
			canWrite: () => this.canWrite(input.objectId),
			bootstrap: () => this.acceptedBootstrap(input.objectId),
			replaceWithAccepted: (bootstrap) => this.replaceWithAccepted(bootstrap),
			rebuildFromAccepted: (bootstrap) => this.rebuildFromAccepted(bootstrap),
			rebuildFromSessionSnapshot: async () => {
				if (!entry) return;
				const target = entry;
				const rebuilt = new Promise<void>((resolve) => {
					target.rebuildDone = resolve;
				});
				target.rebuildOnSnapshot = true;
				await session.reconnect('stale_frontier');
				// Resolves on the swap; bounded so a connection that never comes back cannot
				// park the outbox (the next connect's snapshot still performs the rebuild).
				await Promise.race([rebuilt, new Promise<void>((resolve) => setTimeout(resolve, 15_000))]);
			},
			importIntoLive: (bytes) => {
				liveDoc().import(bytes);
			},
			scratchFrom: (bootstrap) => scratchReplicaFrom(bootstrap),
			enterReadOnly: () => {
				session.setPermissionReadOnly(true);
				readOnlyStore.set(true);
			},
			reportError: (error) => flowErrorStore.set(error)
		};
		const offline = new FlowOfflineSync(await this.outbox(), session, host, this.options.offlineSync);
		await offline.load();

		entry = new OpenEntry({
			handle,
			session,
			projection,
			flowError: flowErrorStore,
			doc,
			offline,
			generationStore,
			readOnlyStore
		});
		const opened = entry;
		// Every local mutation of the doc -- whether from `EditorAdapter`'s ProseMirror binding, a
		// navigator reorder, or a title edit -- funnels through this single subscription into the
		// outbox, which sends through the session, so there is exactly one path that ever calls
		// `session.submit()` no matter which UI surface produced the change ("同一 update 只由
		// Session 发出").
		this.subscribeLocal(opened, () => lastKnownFrontierBase64);

		// Cold-start hint only: paints the last known state immediately while the real WS
		// snapshot loads, then gets overwritten wholesale by the first `onSnapshot` above. Not a
		// second writable source -- `doc` is not mutated from this branch, only read for a
		// display-only best-effort seed value that the projection re-derives from the real doc.
		const cached = await readCachedSnapshot(input.objectId);
		if (cached && !input.signal.aborted) {
			projection.applyEngineDiff({ documentId: object.document_id } satisfies EngineDiff, cached.headSeq);
		}

		// The permission re-check runs beside the connect: a `view`/`comment` user gets a
		// read-only canvas before the editor ever mounts (`ADR-0012`: "`view` 只读").
		const [writable_] = await Promise.all([this.canWrite(input.objectId), session.connect(handle)]);
		if (!writable_) {
			session.setPermissionReadOnly(true);
			offline.setReadOnly(true);
			readOnlyStore.set(true);
		}

		this.open_.set(input.objectId, opened);
		return opened;
	}

	/** Routes the current doc's local updates into the outbox. Re-run after every rebuild. */
	private subscribeLocal(entry: OpenEntry, frontier: () => string): void {
		entry.unsubscribeLocal?.();
		const target = entry.doc;
		const unsubscribe = target.subscribeLocalUpdates((bytes: Uint8Array) => {
			void entry.offline.enqueue(bytes, frontier(), { kind: 'content_edit' });
		});
		entry.unsubscribeLocal = unsubscribe;
		this.frontiers.set(entry.handle.objectId, frontier);
	}

	private readonly frontiers = new Map<string, () => string>();

	/**
	 * The policy re-check the outbox and the read-only mode use: this user's own effective level on
	 * the object (`GET .../grants` returns it to every caller with at least `view`). A transport
	 * failure answers `true` -- the server stays the authority and will refuse a write it does not
	 * allow; a pre-check must not turn a network blip into a read-only canvas.
	 */
	async canWrite(objectId: string): Promise<boolean> {
		const result = await flowApi.getGrants(objectId);
		if (result.code === 0 && result.data) return WRITABLE_LEVELS.has(result.data.effective_level);
		const typed = flowErrorFromEnvelope(result);
		if (typed && (typed.code === 'forbidden' || typed.code === 'not_found' || typed.code === 'policy_rejected')) {
			return false;
		}
		return true;
	}

	/** The REST bootstrap for the outbox, or `null` when this user may not read it there (the
	 * server requires `edit` for it, `routes/flow.rs::get_flow_object_bootstrap`). */
	private async acceptedBootstrap(objectId: string): Promise<FlowBootstrap | null> {
		try {
			return await this.bootstrap(objectId);
		} catch (error) {
			const code = (error as FlowError).code;
			if (code === 'forbidden' || code === 'not_found') return null;
			throw error;
		}
	}

	private async rebuildFromSnapshotPayload(entry: OpenEntry, payload: SnapshotPayload): Promise<void> {
		const { LoroDoc } = await loadLoro();
		const fresh = new LoroDoc() as unknown as LoroDocType;
		fresh.import(payload.snapshotBytes);
		for (const update of payload.tailUpdates) fresh.import(update.bytes);
		this.swapDoc(entry, fresh, payload.headSeq, () => payload.headFrontier);
		entry.rebuildDone?.();
		entry.rebuildDone = null;
	}

	private swapDoc(entry: OpenEntry, fresh: LoroDocType, headSeq: number, fallbackFrontier: () => string): void {
		entry.retired.push(entry.current);
		entry.current = fresh;
		const frontier = this.frontiers.get(entry.handle.objectId) ?? fallbackFrontier;
		this.subscribeLocal(entry, frontier);
		entry.projection.applyEngineDiff({ documentId: entry.handle.documentId } satisfies EngineDiff, headSeq);
		entry.generationStore.update((n) => n + 1);
	}

	/**
	 * Rebuilds an open document's engine doc from exactly one accepted bootstrap, so the local doc
	 * equals what the server accepted (`versions/v0.5-collaboration.md`: "重连拒绝后以 accepted
	 * snapshot 重建"). A refused intent's operations live only in the recovery draft afterwards.
	 * The old doc is retired (the canvas still bound to it remounts on `docGeneration`) and freed
	 * on close.
	 */
	async rebuildFromAccepted(bootstrap: FlowBootstrap): Promise<void> {
		const entry = this.open_.get(bootstrap.object_id);
		if (!entry) return;
		const { LoroDoc } = await loadLoro();
		const fresh = new LoroDoc() as unknown as LoroDocType;
		fresh.import(fromBase64(bootstrap.snapshot_base64));
		for (const update of bootstrap.tail_updates) fresh.import(fromBase64(update.bytes));
		this.swapDoc(entry, fresh, bootstrap.head_seq, () => bootstrap.head_frontier);
	}

	async close(objectId: string): Promise<void> {
		const entry = this.open_.get(objectId);
		if (!entry) return;
		entry.refCount -= 1;
		if (entry.refCount > 0) return;
		this.open_.delete(objectId);
		this.frontiers.delete(objectId);
		entry.offline.dispose();
		entry.unsubscribeLocal?.();
		await entry.session.dispose();
		entry.doc.free();
		for (const retired of entry.retired) retired.free();
	}

	getProjection(objectId: string): Readable<ObjectProjection> {
		const entry = this.open_.get(objectId);
		if (!entry) {
			throw new Error(`FlowObjectRepository.getProjection: ${objectId} is not open`);
		}
		return entry.projection.object;
	}

	getSyncState(objectId: string): Readable<SyncState> | null {
		return this.open_.get(objectId)?.session.state ?? null;
	}

	/** Sets `meta.title` directly on the Loro doc (canonical source, `ADR-0002`) and lets the
	 * existing `subscribeLocalUpdates` hook above ship it out like any other local edit. `meta` is
	 * a server-defined container (`crates/collab-core::engine::LoroCollabEngine::set_title`)
	 * outside `loro-prosemirror`'s narrowed `{doc, data}` container names. */
	setTitle(objectId: string, title: string): void {
		const entry = this.open_.get(objectId);
		if (!entry || entry.session.isReadOnly) return;
		getNamedMap(entry.doc, 'meta').set('title', title);
		entry.doc.commit();
		entry.projection.updateObjectMeta({ title });
	}

	/** `exportRecoveryDraft` (`ui-surface-v1.md`): the recovery draft as a JSON document -- every
	 * intent that could not be replayed, with its reason and its raw update bytes, plus the local
	 * document content -- so a refused edit is never silently lost. */
	async exportRecoveryDraft(objectId: string): Promise<Blob> {
		const document = this.recoveryDraftDocument(objectId);
		return new Blob([JSON.stringify(document, null, 2)], { type: 'application/json' });
	}

	/** The same draft as copyable text, labelled through the caller's translator. */
	recoveryDraftText(
		objectId: string,
		translate: (key: string, values?: Record<string, string | number>) => string
	): string {
		return recoveryDraftText(this.recoveryDraftDocument(objectId), translate);
	}

	/** Forgets the recovery draft of an open object (the user took it away). */
	async clearRecoveryDraft(objectId: string): Promise<void> {
		await this.open_.get(objectId)?.offline.clearDraft();
	}

	private recoveryDraftDocument(objectId: string) {
		const entry = this.open_.get(objectId);
		if (!entry) throw new Error(`FlowObjectRepository.exportRecoveryDraft: ${objectId} is not open`);
		return buildRecoveryDraftDocument({
			objectId,
			documentId: entry.handle.documentId,
			entries: entry.offline.draftEntries,
			localDocument: entry.doc.toJSON()
		});
	}

	/** `GET .../bootstrap` (`ObjectRepositoryContract.bootstrap`). Independent of the live
	 * WebSocket session -- callers that need a fresh snapshot without tearing down the current
	 * connection (e.g. a manual "reload from server" recovery action) use this instead of
	 * `ObjectSession.reconnect`. */
	async bootstrap(objectId: string, known?: { seq: number; frontier: string }): Promise<FlowBootstrap> {
		const result = await flowApi.getBootstrap(objectId, known ? { known_seq: known.seq, known_frontier: known.frontier } : {});
		if (result.code !== 0 || !result.data) {
			throw {
				code: result.code === 404 ? 'not_found' : result.code === 409 ? 'resync_required' : 'forbidden',
				recoverable: result.code === 409
			} satisfies FlowError;
		}
		this.adoptBootstrapLimits(result.data);
		return result.data;
	}

	/** Feeds a fresh `Bootstrap.limits` payload through version negotiation and hands the outcome
	 * to the matching open session. This is the only place the client adopts server ceilings: a
	 * `supported` payload replaces the session's pre-bootstrap fallback with the server's real
	 * numbers, an unrecognised `version` drops that session to read-only
	 * (`contracts/limits-v1.md`: "Client 不得自行放宽或缓存跨 `version` limits"). */
	private adoptBootstrapLimits(bootstrap: FlowBootstrap): void {
		const entry = this.open_.get(bootstrap.object_id);
		if (!entry) return;
		const negotiation = negotiateFlowLimitsVersion(bootstrap.limits);
		entry.session.adoptLimits(negotiation);
		if (negotiation.outcome !== 'supported') entry.offline.setReadOnly(true);
	}

	/** `ObjectRepositoryContract.replaceWithAccepted`: imports a bootstrap's snapshot+tail into the
	 * matching open document (`bootstrap.object_id`), wholesale. Loro's `import` is CRDT-merge, not
	 * destructive replace, so this only ever advances the doc toward the server's accepted state --
	 * it never rolls back content the doc already has that the server has since superseded. */
	async replaceWithAccepted(bootstrap: FlowBootstrap): Promise<void> {
		const entry = this.open_.get(bootstrap.object_id);
		if (!entry) return;
		entry.doc.import(fromBase64(bootstrap.snapshot_base64));
		for (const update of bootstrap.tail_updates) {
			entry.doc.import(fromBase64(update.bytes));
		}
		entry.projection.applyEngineDiff({ documentId: entry.handle.documentId } satisfies EngineDiff, bootstrap.head_seq);
	}

	/** One object's current server view (`GET /flow/objects/{id}`) without opening a collaboration
	 * session or an engine doc. Used by surfaces that only need the accepted head (title, type,
	 * `document_seq`, `frontier`), e.g. the conversion wizard. Returns the raw `ApiResult`. */
	get(objectId: string): Promise<ApiResult<FlowObjectView>> {
		return flowApi.getObject(objectId);
	}

	// v0.8 workspace operations reads (`surface-coverage-v1.md`: admin health / lag / integrity
	// dashboards and the projection-lag badge are `adapter:ObjectRepository` consumers). They
	// return the raw `ApiResult` so the operations panel can branch on `error_code` itself.

	getAdminHealth(workspaceId: string): Promise<ApiResult<FlowAdminHealth>> {
		return flowApi.getAdminHealth(workspaceId);
	}

	getAdminLag(workspaceId: string): Promise<ApiResult<FlowAdminLag>> {
		return flowApi.getAdminLag(workspaceId);
	}

	getAdminIntegrity(
		workspaceId: string,
		query: { scope: 'summary' | 'documents'; limit?: number }
	): Promise<ApiResult<FlowAdminIntegrity>> {
		return flowApi.getAdminIntegrity(workspaceId, query);
	}

	getProjectionLag(
		workspaceId: string,
		query: { cursor?: string; limit?: number } = {}
	): Promise<ApiResult<FlowProjectionLag>> {
		return flowApi.getProjectionLag(workspaceId, query);
	}

	// v0.5 Navigator / command palette full-text search (`surface-coverage-v1.md`: search is an
	// `adapter:ObjectRepository` consumer). A read only: a hit is never opened from its snippet,
	// the object page GETs the current object itself. Raw `ApiResult`, so `navigator-search.ts`
	// branches on `error_code` (`require_current` -> `stale_frontier`).

	search(
		workspaceId: string,
		query: FlowSearchQuery,
		signal?: AbortSignal
	): Promise<ApiResult<FlowSearchResponse>> {
		return flowApi.searchObjects(workspaceId, query, signal);
	}

	// v0.8 package round-trip reads (export job status, the finished artifact, the import job
	// report). Typed reads only: no engine doc, no authorization decision.

	getExportJob(jobId: string): Promise<ApiResult<FlowExportJob>> {
		return flowApi.getExportJob(jobId);
	}

	downloadExportArtifact(
		downloadUrl: string
	): Promise<ApiResult<{ blob: Blob; sha256: string | null }>> {
		return flowApi.downloadExportArtifact(downloadUrl);
	}

	getImportReport(
		workspaceId: string,
		importJobId: string
	): Promise<ApiResult<FlowPackageImportReport>> {
		return flowApi.getPackageImport(workspaceId, importJobId);
	}

	// v0.5 object panel reads (`ui-surface-v1.md` "职责边界": the Share panel reads through
	// `ObjectRepository`, the Relations panel is a lazy-loaded repository read). Raw `ApiResult`s,
	// so the panel controllers (`share-panel.ts`, `relations-panel.ts`) branch on `error_code`.

	// v0.6 Collections (`rest-api-v1.md` v0.6 table). Schema, collection and record-list reads are
	// typed projections; none of them opens an engine doc. Raw `ApiResult`s, so the collection
	// view controller (`collection-view.ts`) branches on `error_code`.

	getSchema(objectId: string): Promise<ApiResult<FlowCollectionDescription>> {
		return flowApi.getObjectSchema(objectId);
	}

	getCollection(collectionId: string): Promise<ApiResult<FlowCollectionDescription>> {
		return flowApi.getCollection(collectionId);
	}

	listRecords(
		collectionId: string,
		query: FlowCollectionRecordsQuery = {}
	): Promise<ApiResult<FlowCollectionRecordsResponse>> {
		return flowApi.listCollectionRecords(collectionId, query);
	}

	queryRecords(
		collectionId: string,
		input: FlowCollectionQueryInput
	): Promise<ApiResult<FlowCollectionRecordsResponse>> {
		return flowApi.queryCollectionRecords(collectionId, input);
	}

	/**
	 * Opens one Record's document for a record panel (`versions/v0.6-collections.md`: "大集合只
	 * 打开 viewport/正在编辑的 record documents"). Ref-counted: a second open of the same record
	 * shares the first read. Rejects with a typed `FlowError`. Every open must be paired with
	 * `closeRecord`.
	 */
	async openRecord(recordId: string): Promise<FlowObjectView> {
		const existing = this.records_.get(recordId);
		if (existing) {
			existing.refCount += 1;
			return existing.view;
		}
		const view = flowApi.getObject(recordId).then((result) => {
			if (result.code !== 0 || !result.data) {
				this.records_.delete(recordId);
				throw (
					flowErrorFromEnvelope(result) ??
					({
						code: result.code === 404 ? 'not_found' : 'forbidden',
						recoverable: false
					} satisfies FlowError)
				);
			}
			return result.data;
		});
		this.records_.set(recordId, { refCount: 1, view });
		return view;
	}

	closeRecord(recordId: string): void {
		const entry = this.records_.get(recordId);
		if (!entry) return;
		entry.refCount -= 1;
		if (entry.refCount <= 0) this.records_.delete(recordId);
	}

	/** Record documents currently open (diagnostics and tests). */
	openRecordIds(): string[] {
		return [...this.records_.keys()];
	}

	/** v0.7 Reference cards / Embed views (`GET .../references`): the bridge's request-time
	 * resolution of every live reference on the Page, with each target's current
	 * `permission_state`. Raw `ApiResult`, so `slash-bridge.ts` branches on `error_code`. */
	listReferences(objectId: string): Promise<ApiResult<FlowReferenceList>> {
		return flowApi.listReferences(objectId);
	}

	getGrants(objectId: string): Promise<ApiResult<FlowGrantsView>> {
		return flowApi.getGrants(objectId);
	}

	getRelations(
		objectId: string,
		query: FlowRelationsQuery = {}
	): Promise<ApiResult<FlowRelationListResponse>> {
		return flowApi.getRelations(objectId, query);
	}
}

function objectToProjection(object: FlowObjectView): ObjectProjection {
	return {
		objectId: object.id,
		objectType: object.object_type,
		title: object.title,
		documentSeq: object.document_seq,
		frontier: object.frontier,
		projectionSeq: object.projection_seq,
		parentId: object.parent_id ?? null
	};
}

/** A throwaway Loro replica of one accepted bootstrap, used by the outbox to tell whether a
 * queued update is new to the server, already accepted, or dependent on something the server
 * never accepted (`offline-sync.ts`). Never bound to an editor, never sent anywhere. */
async function scratchReplicaFrom(bootstrap: FlowBootstrap): Promise<ScratchReplica> {
	const { LoroDoc } = await loadLoro();
	const replica = new LoroDoc();
	replica.import(fromBase64(bootstrap.snapshot_base64));
	for (const update of bootstrap.tail_updates) replica.import(fromBase64(update.bytes));
	return {
		classify(bytes: Uint8Array): ScratchVerdict {
			const status = replica.import(bytes);
			if (status.pending && status.pending.size > 0) return 'pending';
			return status.success.size > 0 ? 'new' : 'known';
		},
		apply(bytes: Uint8Array): void {
			replica.import(bytes);
		},
		dispose(): void {
			replica.free();
		}
	};
}
