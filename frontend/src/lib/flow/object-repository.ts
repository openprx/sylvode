import { apiClient, authSessionChanged } from '#lib/api/client.js';
import { clearBodyClipboard } from './body/clipboard';
import { PageBodyJobs, pageBodyJobs, disposePageBodyJobs } from './body/jobs';
import { applyBodyIntent, bodyBlockPresent } from './body/doc-intents';
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

import {
	editorLocalUpdate,
	registerEditorRecovery,
	pauseEditorWrites,
	holdEditorWrites
} from './editor-batch';
import { detachEditorUndo, releaseEditorSession } from './editor-session';
import { get, derived, readonly as readonlyStore, writable, type Readable } from 'svelte/store';
import {
	flowApi,
	clearFlowAppearanceCache,
	type FlowAdminHealth,
	type FlowAdminIntegrity,
	type FlowAdminLag,
	type FlowBootstrap,
	type FlowCollectionDescription,
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
} from '#lib/api/flow.js';
import type { ApiResult } from '#lib/api/client.js';
import { authStore } from '#lib/stores/auth.js';
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
import {
	openOutboxStore,
	type OutboxEntry,
	type OutboxStore,
	type RecoveryDraftEntry
} from './outbox';
import { CACHE_DB_PREFIX, flowDatabaseName } from './local-data';
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

const STORE_NAME = 'snapshots';

interface CachedSnapshot {
	objectId: string;
	snapshotBytes: Uint8Array;
	headSeq: number;
	headFrontier: string;
	cachedAt: number;
}

/** Opens `userId`'s snapshot cache (`local-data.ts`: one database per user), or `null` without a
 * user or without IndexedDB. Each caller closes the connection when its transaction ends. */
function openIndexedDb(userId: string | null): Promise<IDBDatabase | null> {
	return new Promise((resolve) => {
		const name = flowDatabaseName(CACHE_DB_PREFIX, userId);
		if (!name || typeof indexedDB === 'undefined') {
			resolve(null);
			return;
		}
		let request: IDBOpenDBRequest;
		try {
			request = indexedDB.open(name, 1);
		} catch {
			resolve(null);
			return;
		}
		request.onupgradeneeded = () => {
			request.result.createObjectStore(STORE_NAME, { keyPath: 'objectId' });
		};
		request.onsuccess = () => {
			const db = request.result;
			db.onversionchange = () => db.close();
			resolve(db);
		};
		request.onerror = () => resolve(null);
	});
}

async function readCachedSnapshot(
	userId: string | null,
	objectId: string
): Promise<CachedSnapshot | null> {
	const db = await openIndexedDb(userId);
	if (!db) return null;
	return new Promise((resolve) => {
		const finish = (value: CachedSnapshot | null) => {
			db.close();
			resolve(value);
		};
		try {
			const tx = db.transaction(STORE_NAME, 'readonly');
			const req = tx.objectStore(STORE_NAME).get(objectId);
			req.onsuccess = () => finish((req.result as CachedSnapshot | undefined) ?? null);
			req.onerror = () => finish(null);
		} catch {
			finish(null);
		}
	});
}

async function writeCachedSnapshot(userId: string | null, entry: CachedSnapshot): Promise<void> {
	const db = await openIndexedDb(userId);
	if (!db) return;
	await new Promise<void>((resolve) => {
		const finish = () => {
			db.close();
			resolve();
		};
		try {
			const tx = db.transaction(STORE_NAME, 'readwrite');
			tx.objectStore(STORE_NAME).put(entry);
			tx.oncomplete = finish;
			tx.onerror = finish;
			tx.onabort = finish;
		} catch {
			finish();
		}
	});
}

async function deleteCachedSnapshot(userId: string | null, objectId: string): Promise<void> {
	const db = await openIndexedDb(userId);
	if (!db) return;
	await new Promise<void>((resolve) => {
		const finish = () => {
			db.close();
			resolve();
		};
		try {
			const tx = db.transaction(STORE_NAME, 'readwrite');
			tx.objectStore(STORE_NAME).delete(objectId);
			tx.oncomplete = finish;
			tx.onerror = finish;
			tx.onabort = finish;
		} catch {
			finish();
		}
	});
}

/** Structural shape of a Loro map, wide enough to cover any container name (see `setTitle`'s
 * comment for why `LoroDocType.getMap` needs widening for non-`doc`/`data` containers). */
/** At most this many rebuilds of a doc that cannot import server bytes, within `POISON_WINDOW_MS`. */
const POISON_REBUILDS_MAX = 2;
const POISON_WINDOW_MS = 60_000;
/** Marks the error a replay throws when its doc was just discarded; it is not shown. */
const DOC_REBUILD_REASON = 'doc_rebuild';

function unreadableRemoteUpdate(): FlowError {
	return {
		code: 'invalid_update',
		recoverable: false,
		origin: 'client',
		details: { reason: 'remote_update_unreadable' }
	};
}

/**
 * The ONE place remote bytes enter a doc that is (or may become) bound to the editor. Returns
 * false when the engine threw: a failed `import` can panic the WASM module (a tree op under a
 * parent that does not exist), after which every call on that doc throws too, so the caller must
 * treat the doc as poisoned and never touch it again. A try/catch only: no copy, no decoding.
 */
function importRemote(doc: LoroDocType, chunks: readonly Uint8Array[]): boolean {
	try {
		for (const bytes of chunks) doc.import(bytes);
		return true;
	} catch {
		return false;
	}
}

/** A fresh doc holding exactly `chunks`; one the engine cannot import is freed and refused. */
async function freshDocFrom(chunks: readonly Uint8Array[]): Promise<LoroDocType> {
	const { LoroDoc } = await loadLoro();
	const fresh = new LoroDoc() as unknown as LoroDocType;
	if (importRemote(fresh, chunks)) return fresh;
	try {
		fresh.free();
	} catch {
		// Nothing left to release.
	}
	throw unreadableRemoteUpdate();
}

function docRebuildStarted(): FlowError {
	return {
		code: 'resync_required',
		recoverable: true,
		origin: 'client',
		details: { reason: DOC_REBUILD_REASON }
	};
}

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
	readonly metadata?: Readable<FlowObjectView>;
	readonly flowError: Readable<FlowError | null>;
	/** Local updates waiting for the server (the indicator's pending count). */
	readonly pending: Readable<number>;
	readonly bodyPending?: Readable<number>;
	/** Intents that could not be replayed, with their reasons. */
	readonly recoveryDraft: Readable<ReadonlyArray<RecoveryDraftEntry>>;
	/** The user may not write this object (`view`/`comment`, or revoked mid-session). */
	readonly readOnly: Readable<boolean>;
	readonly permittedActions: Readable<readonly string[] | null>;
	/** Remote peers on this document (ephemeral, never in the engine doc). */
	readonly presence: Readable<ReadonlyArray<RemotePresence>>;
	/** The outbox controller (diagnostics and tests; components use the stores above). */
	readonly offline: FlowOfflineSync;
}

/** Test seams for `FlowObjectRepository`. Production passes nothing. */
export interface FlowObjectRepositoryOptions {
	readonly outboxStore?: OutboxStore;
	/** Whose local databases (outbox, drafts, snapshot cache) this repository uses; defaults to
	 * the signed-in user (`authStore`). `null` persists nothing. */
	readonly currentUserId?: () => string | null;
	readonly session?: SessionOptions;
	readonly offlineSync?: OfflineSyncOptions;
}

/** Effective levels that may write a document (`ADR-0012`: "`edit` 起可写、`view` 只读"). */
const WRITABLE_LEVELS: ReadonlySet<string> = new Set(['edit', 'full_access']);

class OpenEntry implements OpenFlowObjectResult {
	readonly handle: ObjectHandle;
	readonly ownerUserId: string | null;
	readonly ownerAuthEpoch: number;
	readonly session: LoroObjectSession;
	readonly projection: LiveProjectionStore;
	readonly metadataStore: ReturnType<typeof writable<FlowObjectView>>;
	readonly metadata: Readable<FlowObjectView>;
	metadataRefresh: Promise<unknown> | null = null;
	readonly flowError: Readable<FlowError | null>;
	readonly docGeneration: Readable<number>;
	readonly pending: Readable<number>;
	readonly bodyPendingStore = writable(0);
	readonly bodyPending = readonlyStore(this.bodyPendingStore);
	readonly localEnqueues = new Set<Promise<void>>();
	localEnqueueFailed = false;
	readonly recoveryDraft: Readable<ReadonlyArray<RecoveryDraftEntry>>;
	readonly readOnly: Readable<boolean>;
	readonly operationHeldStore = writable(false);
	readonly operationHolds = new Set<string>();
	readonly permittedActions: Readable<readonly string[] | null>;
	readonly presence: Readable<ReadonlyArray<RemotePresence>>;
	readonly offline: FlowOfflineSync;
	readonly generationStore: ReturnType<typeof writable<number>>;
	readonly readOnlyStore: ReturnType<typeof writable<boolean>>;
	refCount = 1;
	editorPaused = false;
	readonly lifecycleAbort = new AbortController();
	readonly reportEditorFailure: (error: FlowError) => void;
	current: LoroDocType;
	/** Docs replaced by a rebuild; freed on close, once nothing can still be bound to them. */
	readonly retired: LoroDocType[] = [];
	unsubscribeLocal: (() => void) | null = null;
	/** The next session `snapshot` replaces the doc instead of merging into it. */
	rebuildOnSnapshot = false;
	rebuildDone: (() => void) | null = null;
	/** Times (ms) of the recent rebuilds of a doc that could not import server bytes. */
	poisonRebuilds: number[] = [];

	constructor(input: {
		ownerUserId: string | null;
		ownerAuthEpoch: number;
		handle: ObjectHandle;
		session: LoroObjectSession;
		projection: LiveProjectionStore;
		flowError: Readable<FlowError | null>;
		reportEditorFailure: (error: FlowError) => void;
		doc: LoroDocType;
		offline: FlowOfflineSync;
		generationStore: ReturnType<typeof writable<number>>;
		readOnlyStore: ReturnType<typeof writable<boolean>>;
		permissionsStore: ReturnType<typeof writable<readonly string[] | null>>;
	}) {
		this.handle = input.handle;
		this.metadataStore = writable(input.handle.object);
		this.metadata = readonlyStore(this.metadataStore);
		this.ownerUserId = input.ownerUserId;
		this.ownerAuthEpoch = input.ownerAuthEpoch;
		this.session = input.session;
		this.projection = input.projection;
		this.flowError = input.flowError;
		this.reportEditorFailure = input.reportEditorFailure;
		this.current = input.doc;
		this.offline = input.offline;
		this.generationStore = input.generationStore;
		this.readOnlyStore = input.readOnlyStore;
		this.docGeneration = readonlyStore(input.generationStore);
		this.readOnly = derived(
			[input.readOnlyStore, this.operationHeldStore],
			([permission, held]) => permission || held
		);
		this.permittedActions = readonlyStore(input.permissionsStore);
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
	private readonly transferHolds = new Map<string, Set<string>>();
	private readonly options: FlowObjectRepositoryOptions;
	private outboxStore: Promise<OutboxStore> | null = null;
	private outboxUser: string | null = null;

	constructor(options: FlowObjectRepositoryOptions = {}) {
		this.options = options;
	}

	private currentUserId(): string | null {
		return this.options.currentUserId
			? this.options.currentUserId()
			: (get(authStore).user?.id ?? null);
	}

	/** The outbox of `userId`: reopened when the signed-in user changed, so one user's entries
	 * are never listed, replayed or drafted under another's session. */
	private outbox(userId: string | null): Promise<OutboxStore> {
		if (this.options.outboxStore) return Promise.resolve(this.options.outboxStore);
		if (!this.outboxStore || this.outboxUser !== userId) {
			this.outboxUser = userId;
			this.outboxStore = openOutboxStore(userId);
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
		scopeId: string;
		objectId: string;
		signal: AbortSignal;
		/** The object as the caller already read it (`get`), so the route page that dispatches on
		 * `object_type` first does not read it twice. */
		object?: FlowObjectView;
	}): Promise<OpenFlowObjectResult> {
		const authEpoch = apiClient.getAuthSessionEpoch();
		const existing = this.open_.get(input.objectId);
		if (existing) {
			if (!apiClient.isAuthSessionEpoch(existing.ownerAuthEpoch))
				throw { code: 'unauthenticated', recoverable: true } satisfies FlowError;
			existing.refCount += 1;
			return existing;
		}
		// The user whose local databases this open reads and writes, fixed for its lifetime.
		const userId = this.currentUserId();
		if (userId && typeof localStorage !== 'undefined') {
			const { transferSessions } = await import('./block-transfer/session');
			transferSessions(this, userId, () => this.currentUserId());
		}

		let object: FlowObjectView;
		if (input.object && input.object.id === input.objectId) {
			object = input.object;
		} else {
			const objectResult = await flowApi.getObject(input.objectId);
			if (objectResult.code !== 0 || !objectResult.data) {
				throw {
					...(flowErrorFromEnvelope(objectResult) ?? {
						code: objectResult.code === 404 ? 'not_found' : 'forbidden',
						recoverable: false
					})
				} satisfies FlowError;
			}
			object = objectResult.data;
		}
		if (input.signal.aborted) {
			throw { code: 'not_found', recoverable: false } satisfies FlowError;
		}

		const loro = await loadLoro();
		const { LoroDoc } = loro;
		if (!apiClient.isAuthSessionEpoch(authEpoch) || this.currentUserId() !== userId)
			throw { code: 'unauthenticated', recoverable: true } satisfies FlowError;
		const doc = new LoroDoc() as unknown as LoroDocType;

		const projection = new LiveProjectionStore(objectToProjection(object));
		const flowErrorStore = writable<FlowError | null>(null);
		const generationStore = writable(0);
		const permissionsStore = writable<readonly string[] | null>(object.permitted_actions ?? null);
		const readOnlyStore = writable(
			object.permitted_actions ? !object.permitted_actions.includes('edit') : false
		);
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
					if (
						!importRemote(target, [
							payload.snapshotBytes,
							...payload.tailUpdates.map((u) => u.bytes)
						])
					)
						throw unreadableRemoteUpdate();
					lastKnownFrontierBase64 = payload.headFrontier;
					projection.applyEngineDiff(
						{ documentId: object.document_id } satisfies EngineDiff,
						payload.headSeq
					);
					void writeCachedSnapshot(userId, {
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
					if (notice.remoteBytes && !importRemote(liveDoc(), [notice.remoteBytes]))
						throw unreadableRemoteUpdate();
					lastKnownFrontierBase64 = notice.headFrontier;
					if (
						entry &&
						entry.handle.objectType === 'page' &&
						getNamedMap(liveDoc(), 'flow_editor_structure_meta').get('model') === 'rich_page_v5' &&
						(get(entry.metadataStore).minimum_editor_model ?? 1) < 5 &&
						!entry.metadataRefresh
					) {
						const captured = entry;
						captured.metadataRefresh = this.get(input.objectId).finally(
							() => (captured.metadataRefresh = null)
						);
					}
					projection.applyEngineDiff(
						{ documentId: object.document_id } satisfies EngineDiff,
						notice.headSeq
					);
				},
				onFlowError: (error: FlowError) => {
					flowErrorStore.set(error);
				},
				onImportFailure: () => {
					// The import of server bytes threw, so the doc may be dead. Before the entry
					// exists (the first snapshot) nothing is mounted: report the failure.
					if (entry) void this.recoverUnreadableDoc(entry);
					else flowErrorStore.set(unreadableRemoteUpdate());
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
				onAuthorizationPending: (actions: readonly string[]) => {
					permissionsStore.set([...actions]);
					if (!actions.includes('edit')) entry?.offline.setReadOnly(true);
				},
				onAuthorizationChange: async (actions: readonly string[]) => {
					permissionsStore.set([...actions]);
					return entry ? entry.offline.onAuthorizationChange(actions.includes('edit')) : false;
				},
				onPermissionState: (readOnly: boolean) =>
					readOnlyStore.set(readOnly || entry?.editorPaused === true),
				onRevoked: (error: FlowError) => {
					permissionsStore.set([]);
					readOnlyStore.set(true);
					void deleteCachedSnapshot(userId, input.objectId);
					clearFlowAppearanceCache(input.objectId);
					void entry?.offline.onRevoked({ code: error.code, source: 'server' });
				}
			},
			this.commandService,
			{ ...this.options.session, authEpoch }
		);

		const handle: ObjectHandle = {
			scopeId: object.scope_id,
			objectId: input.objectId,
			objectType: object.object_type,
			documentId: object.document_id,
			object
		};

		const limitViolation = (bytes: Uint8Array): FlowError | null => {
			const violation = checkUpdateBytes(bytes, session.effectiveLimits);
			return violation
				? clientError('limit_exceeded', {
						limit_kind: violation.limitKind,
						limit: violation.limit,
						observed: violation.observed
					})
				: null;
		};
		const host: OfflineSyncHost = {
			objectId: input.objectId,
			documentId: object.document_id,
			peerId: () => liveDoc().peerIdStr,
			limitViolation,
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
				if (!importRemote(liveDoc(), [bytes])) {
					// Replay stops; the rebuild below reconnects, and the next connect replays the
					// queue onto the fresh doc.
					if (entry) void this.recoverUnreadableDoc(entry);
					throw docRebuildStarted();
				}
			},
			commitPending: () => {
				try {
					liveDoc().commit();
				} catch {
					// A dead doc is handled by the rebuild paths; nothing to commit.
				}
			},
			hasUncommittedEdits: () => {
				try {
					return (liveDoc() as unknown as RawLoroDoc).getPendingTxnLength() > 0;
				} catch {
					return false;
				}
			},
			sessionHeadFrontier: () => (lastKnownFrontierBase64 === '' ? null : lastKnownFrontierBase64),
			prepareRebase: (queued) => {
				if (!entry) return { ok: false };
				const target = entry;
				const rebased = rebaseUnsentChanges(loro, liveDoc() as unknown as RawLoroDoc, queued);
				if (!rebased.ok) return { ok: false, error: rebased.error };
				if (!rebased.bytes) {
					rebased.doc.free();
					return {
						ok: true,
						bytes: null,
						unreplayable: rebased.unreplayable,
						adopt: () => undefined
					};
				}
				const violation = limitViolation(rebased.bytes);
				if (violation) {
					rebased.doc.free();
					return { ok: false, error: violation };
				}
				return {
					ok: true,
					bytes: rebased.bytes,
					unreplayable: rebased.unreplayable,
					adopt: () => {
						// The editor rebinds to the new doc (the cursor may move); the old doc is
						// retired like after any rebuild. It is never swapped back.
						this.swapDoc(
							target,
							rebased.doc as unknown as LoroDocType,
							get(target.projection.object).documentSeq,
							() => lastKnownFrontierBase64
						);
					}
				};
			},
			clearPausedNotice: () =>
				flowErrorStore.update((shown) =>
					(shown?.details as { reason?: string } | undefined)?.reason === 'sending_paused'
						? null
						: shown
				),
			scratchFrom: (bootstrap) => scratchReplicaFrom(bootstrap),
			enterReadOnly: () => {
				session.setPermissionReadOnly(true);
				readOnlyStore.set(true);
			},
			reportError: (error) => {
				if ((error.details as { reason?: string } | undefined)?.reason !== DOC_REBUILD_REASON)
					flowErrorStore.set(error);
			}
		};
		const offline = new FlowOfflineSync(
			await this.outbox(userId),
			session,
			host,
			this.options.offlineSync
		);
		await offline.load();

		entry = new OpenEntry({
			ownerUserId: userId,
			ownerAuthEpoch: authEpoch,
			handle,
			session,
			projection,
			flowError: flowErrorStore,
			reportEditorFailure: (error) => flowErrorStore.set(error),
			doc,
			offline,
			generationStore,
			readOnlyStore,
			permissionsStore
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
		const cached = await readCachedSnapshot(userId, input.objectId);
		if (cached && !input.signal.aborted) {
			projection.applyEngineDiff(
				{ documentId: object.document_id } satisfies EngineDiff,
				cached.headSeq
			);
		}

		// The permission re-check runs beside the connect: a `view`/`comment` user gets a
		// read-only canvas before the editor ever mounts (`ADR-0012`: "`view` 只读").
		let writable_: boolean;
		try {
			[writable_] = await Promise.all([this.canWrite(input.objectId), session.connect(handle)]);
			if (!apiClient.isAuthSessionEpoch(authEpoch) || this.currentUserId() !== userId)
				throw { code: 'unauthenticated', recoverable: true } satisfies FlowError;
		} catch (error) {
			opened.lifecycleAbort.abort();
			offline.dispose();
			opened.unsubscribeLocal?.();
			await session.dispose();
			doc.free();
			throw error;
		}
		if (!session.hasBoundAuthorization && !writable_) {
			session.setPermissionReadOnly(true);
			offline.setReadOnly(true);
			readOnlyStore.set(true);
		}

		this.open_.set(input.objectId, opened);
		for (const hold of this.transferHolds.get(input.objectId) ?? [])
			this.attachTransferHold(opened, hold);
		return opened;
	}

	/** Routes the current doc's local updates into the outbox. Re-run after every rebuild. */
	private subscribeLocal(entry: OpenEntry, frontier: () => string): void {
		try {
			entry.unsubscribeLocal?.();
		} catch {
			// The previous doc is dead (see `recoverUnreadableDoc`): nothing is left to detach.
		}
		const target = entry.doc;
		if (entry.editorPaused) {
			entry.unsubscribeLocal = null;
			this.frontiers.set(entry.handle.objectId, frontier);
			return;
		}
		const deliver = (bytes: Uint8Array) => {
			if (!entry.editorPaused) {
				const queued = entry.offline.enqueue(bytes, frontier(), { kind: 'content_edit' });
				entry.localEnqueues.add(queued);
				void queued
					.catch(() => {
						entry.localEnqueueFailed = true;
						entry.reportEditorFailure({
							code: 'network_error',
							recoverable: true,
							origin: 'client',
							details: { reason: 'local_outbox_persistence' }
						});
					})
					.finally(() => entry.localEnqueues.delete(queued));
			}
		};
		const unsubscribe = target.subscribeLocalUpdates((bytes: Uint8Array) =>
			editorLocalUpdate(target, bytes, deliver)
		);
		const recovery = registerEditorRecovery(target, (preimage) => {
			if (entry.doc !== target) {
				preimage.free();
				return;
			}
			entry.editorPaused = true;
			pauseEditorWrites(preimage);
			entry.unsubscribeLocal?.();
			entry.unsubscribeLocal = null;
			entry.offline.setReadOnly(true);
			entry.offline.dispose();
			entry.session.setPermissionReadOnly(true);
			void entry.session.dispose();
			detachEditorUndo(target);
			entry.readOnlyStore.set(true);
			this.swapDoc(
				entry,
				preimage as LoroDocType,
				get(entry.projection.object).documentSeq,
				frontier
			);
			entry.reportEditorFailure({
				code: 'invalid_update',
				recoverable: false,
				origin: 'client',
				details: { reason: 'editor_transaction_aborted' }
			});
		});
		entry.unsubscribeLocal = () => {
			unsubscribe();
			recovery();
		};
		this.frontiers.set(entry.handle.objectId, frontier);
	}

	/** Read-only lifecycle observability; no document text or credentials are exposed. */
	editorLifecycle(
		objectId: string
	): { refs: number; generation: number; retired: number; paused: boolean } | null {
		const entry = this.open_.get(objectId);
		return entry
			? {
					refs: entry.refCount,
					generation: get(entry.generationStore),
					retired: entry.retired.length,
					paused: entry.editorPaused
				}
			: null;
	}

	bodyJobs(objectId: string): PageBodyJobs | null {
		const entry = this.open_.get(objectId);
		if (!entry) return null;
		return pageBodyJobs(entry, () => {
			const allowed = () =>
				this.open_.get(objectId) === entry &&
				this.currentUserId() === entry.ownerUserId &&
				apiClient.isAuthSessionEpoch(entry.ownerAuthEpoch) &&
				!entry.editorPaused &&
				!get(entry.readOnly) &&
				!entry.localEnqueueFailed;
			const jobs = new PageBodyJobs(
				{
					objectId,
					allowed,
					ensure: () => this.ensureBodyStructure(entry, entry.doc),
					insert: (bookmark, intent) => {
						const doc = entry.doc;
						return applyBodyIntent(
							doc,
							bookmark,
							intent,
							() => allowed() && entry.doc === doc
						).catch(() => null);
					},
					saved: (signal) => this.waitBodySaved(entry, entry.doc, signal),
					contains: (blockId) => bodyBlockPresent(entry.doc, blockId),
					failureReason: () => {
						const error = get(entry.flowError);
						return error?.details && typeof error.details === 'object'
							? String((error.details as { reason?: unknown }).reason ?? '')
							: null;
					},
					prepare: (title, key, signal) =>
						allowed()
							? flowApi.prepareChildPage(objectId, title, key, signal)
							: Promise.resolve(authSessionChanged()),
					cancelReservation: (id) =>
						this.currentUserId() === entry.ownerUserId &&
						apiClient.isAuthSessionEpoch(entry.ownerAuthEpoch)
							? flowApi.cancelChildReservation(objectId, id)
							: Promise.resolve({ code: 401, message: 'Session changed', data: null }),
					reservationStatus: (id, signal) =>
						allowed()
							? flowApi.childReservationStatus(objectId, id, signal)
							: Promise.resolve(authSessionChanged()),
					childReady: (id) => this.notifyObjectCreated(id, objectId)
				},
				{
					limits: (id, signal) =>
						allowed() ? flowApi.bodyAssetLimits(id, signal) : Promise.resolve(authSessionChanged()),
					upload: (id, kind, file, key, signal, progress) =>
						allowed()
							? flowApi.uploadBodyAsset(id, kind, file, key, signal, progress)
							: Promise.resolve(authSessionChanged()),
					discard: (id, asset) =>
						this.currentUserId() === entry.ownerUserId &&
						apiClient.isAuthSessionEpoch(entry.ownerAuthEpoch)
							? flowApi.discardBodyAsset(id, asset)
							: Promise.resolve({ code: 401, message: 'Session changed', data: null })
				}
			);
			jobs.subscribe((snapshot) => entry.bodyPendingStore.set(snapshot.pending));
			return jobs;
		});
	}
	private async ensureBodyStructure(entry: OpenEntry, doc: LoroDocType): Promise<boolean> {
		if (
			this.open_.get(entry.handle.objectId) !== entry ||
			entry.doc !== doc ||
			entry.editorPaused ||
			!apiClient.isAuthSessionEpoch(entry.ownerAuthEpoch) ||
			get(entry.readOnly)
		)
			return false;
		if (getNamedMap(doc, 'flow_editor_structure_meta').get('initialized') === true) return true;
		try {
			await Promise.all([...entry.localEnqueues]);
			if (
				get(entry.pending) > 0 &&
				!(await this.waitBodySaved(entry, doc, entry.lifecycleAbort.signal))
			)
				return false;
			if (!apiClient.isAuthSessionEpoch(entry.ownerAuthEpoch)) return false;
			const object = await flowApi.getObject(entry.handle.objectId);
			if (
				object.code !== 0 ||
				!object.data ||
				!apiClient.isAuthSessionEpoch(entry.ownerAuthEpoch) ||
				this.open_.get(entry.handle.objectId) !== entry ||
				entry.doc !== doc ||
				get(entry.readOnly)
			)
				return false;
			const result = await flowApi.executeCommand(entry.handle.objectId, {
				command: { type: 'initialize_editor_doc', payload: { model: 'rich_page_v4' } },
				expected_frontier: object.data.frontier,
				idempotency_key: crypto.randomUUID()
			});
			if (result.code !== 0 || entry.doc !== doc || this.open_.get(entry.handle.objectId) !== entry)
				return false;
			await this.replaceWithAccepted(await this.bootstrap(entry.handle.objectId));
			return (
				entry.doc === doc &&
				getNamedMap(doc, 'flow_editor_structure_meta').get('initialized') === true
			);
		} catch {
			return false;
		}
	}
	/** Counts only the current actor's existing outbox/jobs; no hidden Page is opened to sync it. */
	async transferPendingSummary(objectId: string): Promise<{
		known: boolean;
		pending: number;
		drafts: number;
		bodyJobs: number;
		enqueues: number;
		open: boolean;
		paused: boolean;
	}> {
		const actor = this.currentUserId(),
			epoch = apiClient.getAuthSessionEpoch();
		if (!actor)
			return {
				known: false,
				pending: 0,
				drafts: 0,
				bodyJobs: 0,
				enqueues: 0,
				open: false,
				paused: false
			};
		try {
			const store = await this.outbox(actor),
				[pending, drafts] = await Promise.all([store.list(objectId), store.listDrafts(objectId)]);
			const entry = this.open_.get(objectId);
			if (!apiClient.isAuthSessionEpoch(epoch) || this.currentUserId() !== actor)
				return {
					known: false,
					pending: 0,
					drafts: 0,
					bodyJobs: 0,
					enqueues: 0,
					open: false,
					paused: false
				};
			return {
				known: true,
				pending: Math.max(pending.length, entry ? get(entry.pending) : 0),
				drafts: drafts.length,
				bodyJobs: entry ? get(entry.bodyPending) : 0,
				enqueues: entry?.localEnqueues.size ?? 0,
				open: !!entry,
				paused: entry?.editorPaused ?? false
			};
		} catch {
			return {
				known: false,
				pending: 0,
				drafts: 0,
				bodyJobs: 0,
				enqueues: 0,
				open: false,
				paused: false
			};
		}
	}
	/** Merge the accepted transfer head into the live CRDT; never replace a personal unsent draft. */
	async mergeTransferAccepted(objectId: string): Promise<boolean> {
		const entry = this.open_.get(objectId);
		if (!entry) return false;
		const doc = entry.doc,
			epoch = entry.ownerAuthEpoch;
		try {
			const bootstrap = await this.bootstrap(objectId);
			if (
				this.open_.get(objectId) !== entry ||
				entry.doc !== doc ||
				!apiClient.isAuthSessionEpoch(epoch)
			)
				return false;
			if (!importRemote(doc, bootstrapChunks(bootstrap))) {
				void this.recoverUnreadableDoc(entry);
				return false;
			}
			entry.projection.applyEngineDiff(
				{ documentId: entry.handle.documentId } satisfies EngineDiff,
				bootstrap.head_seq
			);
			this.frontiers.set(objectId, () => bootstrap.head_frontier);
			return true;
		} catch {
			return false;
		}
	}

	transferEditingHeld(objectId: string): boolean {
		return !!this.transferHolds.get(objectId)?.size;
	}
	openTransferEntry(objectId: string): OpenFlowObjectResult | null {
		return this.open_.get(objectId) ?? null;
	}
	private attachTransferHold(entry: OpenEntry, operationId: string): void {
		if (entry.operationHolds.has(operationId)) return;
		entry.operationHolds.add(operationId);
		entry.operationHeldStore.set(true);
		let releaseDoc = holdEditorWrites(entry.doc, operationId);
		const stop = entry.docGeneration.subscribe(() => {
			releaseDoc();
			releaseDoc = holdEditorWrites(entry.doc, operationId);
		});
		const release = () => {
			stop();
			releaseDoc();
			entry.operationHolds.delete(operationId);
			entry.operationHeldStore.set(entry.operationHolds.size > 0);
		};
		entry.lifecycleAbort.signal.addEventListener('abort', release, { once: true });
		this.transferHoldReleases.set(`${entry.handle.objectId}:${operationId}`, () => {
			entry.lifecycleAbort.signal.removeEventListener('abort', release);
			release();
		});
	}
	private readonly transferHoldReleases = new Map<string, () => void>();
	holdTransferEditing(objectIds: readonly string[], operationId: string): () => void {
		for (const id of objectIds) {
			let holds = this.transferHolds.get(id);
			if (!holds) {
				holds = new Set();
				this.transferHolds.set(id, holds);
			}
			holds.add(operationId);
			const entry = this.open_.get(id);
			if (entry) this.attachTransferHold(entry, operationId);
		}
		return () => {
			for (const id of objectIds) {
				const holds = this.transferHolds.get(id);
				holds?.delete(operationId);
				if (!holds?.size) this.transferHolds.delete(id);
				const key = `${id}:${operationId}`;
				this.transferHoldReleases.get(key)?.();
				this.transferHoldReleases.delete(key);
			}
		};
	}

	blockShareManagementAllowed(objectId: string): boolean | null {
		const entry = this.open_.get(objectId);
		if (!entry) return null;
		return (
			entry.ownerUserId === this.currentUserId() &&
			(get(entry.permittedActions) ?? entry.handle.object.permitted_actions ?? []).includes(
				'manage'
			)
		);
	}

	/** Review only the original open source Page, after accepted local writes and a matching head. */
	async reviewBlockShare(
		objectId: string,
		rootCid: string,
		selection: 'block' | 'current_subtree',
		signal: AbortSignal
	): Promise<
		| { ok: true; review: import('./shared-blocks/review').BlockShareReview }
		| {
				ok: false;
				reason: 'not_saved' | 'source_changed' | 'unsupported' | 'limit' | 'unavailable';
		  }
	> {
		const entry = this.open_.get(objectId);
		if (!entry || get(entry.readOnly) || entry.editorPaused)
			return { ok: false, reason: 'unavailable' };
		const doc = entry.doc;
		if (!doc.getMap('doc').get('children')) return { ok: false, reason: 'unsupported' };
		if (!(await this.ensureBodyStructure(entry, doc)) || signal.aborted)
			return { ok: false, reason: 'unavailable' };
		try {
			await Promise.all([...entry.localEnqueues]);
		} catch {
			return { ok: false, reason: 'not_saved' };
		}
		if (get(entry.pending) > 0 && !(await this.waitBodySaved(entry, doc, signal)))
			return { ok: false, reason: 'not_saved' };
		if (
			signal.aborted ||
			entry.localEnqueueFailed ||
			!apiClient.isAuthSessionEpoch(entry.ownerAuthEpoch)
		)
			return { ok: false, reason: 'unavailable' };
		// Clean Local is not a synchronization claim: a fresh native-VV/head equality below is mandatory.
		const fresh = await flowApi.getObject(objectId, undefined, signal);
		if (
			signal.aborted ||
			fresh.code !== 0 ||
			!fresh.data ||
			this.open_.get(objectId) !== entry ||
			entry.doc !== doc ||
			get(entry.readOnly)
		)
			return { ok: false, reason: 'unavailable' };
		const { captureBlockShareReview, ShareReviewError } = await import('./shared-blocks/review');
		try {
			return {
				ok: true,
				review: captureBlockShareReview(doc, rootCid, selection, fresh.data.frontier)
			};
		} catch (error) {
			return {
				ok: false,
				reason: error instanceof ShareReviewError ? error.reason : 'unsupported'
			};
		}
	}
	async blockShareReviewCurrent(
		objectId: string,
		review: import('./shared-blocks/review').BlockShareReview,
		signal: AbortSignal
	): Promise<boolean> {
		const current = await this.reviewBlockShare(
			objectId,
			review.root_block_cid,
			review.selection,
			signal
		);
		return current.ok && current.review.expected_frontier === review.expected_frontier;
	}

	private async waitBodySaved(
		entry: OpenEntry,
		doc: LoroDocType,
		signal: AbortSignal
	): Promise<boolean> {
		try {
			await Promise.all([...entry.localEnqueues]);
		} catch {
			return false;
		}
		if (signal.aborted) return false;
		return new Promise((resolve) => {
			let settled = false;
			const releases: (() => void)[] = [];
			const finish = (value: boolean) => {
				if (settled) return;
				settled = true;
				queueMicrotask(() => {
					for (const release of releases) release();
					signal.removeEventListener('abort', aborted);
					entry.lifecycleAbort.signal.removeEventListener('abort', aborted);
				});
				resolve(value);
			};
			const aborted = () => finish(false);
			const inspect = () => {
				if (
					this.open_.get(entry.handle.objectId) !== entry ||
					entry.doc !== doc ||
					entry.editorPaused ||
					get(entry.readOnly) ||
					entry.localEnqueueFailed ||
					signal.aborted
				)
					return finish(false);
				const state = get(entry.session.state);
				if (state === 'saved' && get(entry.pending) === 0) finish(true);
				else if (['error', 'auth_required', 'read_only'].includes(state)) finish(false);
			};
			signal.addEventListener('abort', aborted, { once: true });
			entry.lifecycleAbort.signal.addEventListener('abort', aborted, { once: true });
			releases.push(
				entry.pending.subscribe(inspect),
				entry.session.state.subscribe(inspect),
				entry.docGeneration.subscribe(inspect),
				entry.readOnly.subscribe(inspect)
			);
			inspect();
		});
	}

	private readonly frontiers = new Map<string, () => string>();

	/**
	 * The policy re-check the outbox and the read-only mode use: this user's own effective level on
	 * the object (`GET .../grants` returns it to every caller with at least `view`). A transport
	 * failure answers `true` -- the server stays the authority and will refuse a write it does not
	 * allow; a pre-check must not turn a network blip into a read-only canvas.
	 */
	async canWrite(objectId: string): Promise<boolean> {
		const object = await flowApi.getObject(objectId);
		if (object.code === 0 && object.data?.permitted_actions)
			return object.data.permitted_actions.includes('edit');
		const objectError = flowErrorFromEnvelope(object);
		if (objectError && (objectError.code === 'forbidden' || objectError.code === 'not_found'))
			return false;
		const result = await flowApi.getGrants(objectId);
		if (result.code === 0 && result.data) return WRITABLE_LEVELS.has(result.data.effective_level);
		const typed = flowErrorFromEnvelope(result);
		if (
			typed &&
			(typed.code === 'forbidden' || typed.code === 'not_found' || typed.code === 'policy_rejected')
		) {
			return false;
		}
		return true;
	}

	/** The REST bootstrap for the outbox, or `null` when this user may not read it there
	 * (`forbidden`/`not_found`; the server admits `view`, same as the WebSocket session). */
	private async acceptedBootstrap(objectId: string): Promise<FlowBootstrap | null> {
		try {
			return await this.bootstrap(objectId);
		} catch (error) {
			const code = (error as FlowError).code;
			if (code === 'forbidden' || code === 'not_found') return null;
			throw error;
		}
	}

	private async rebuildFromSnapshotPayload(
		entry: OpenEntry,
		payload: SnapshotPayload
	): Promise<void> {
		let fresh: LoroDocType;
		try {
			fresh = await freshDocFrom([
				payload.snapshotBytes,
				...payload.tailUpdates.map((u) => u.bytes)
			]);
		} catch {
			// The snapshot itself cannot be read; the bounded recovery decides between one more
			// rebuild and stopping.
			entry.rebuildDone?.();
			entry.rebuildDone = null;
			await this.recoverUnreadableDoc(entry);
			return;
		}
		this.swapDoc(entry, fresh, payload.headSeq, () => payload.headFrontier);
		entry.rebuildDone?.();
		entry.rebuildDone = null;
	}

	private swapDoc(
		entry: OpenEntry,
		fresh: LoroDocType,
		headSeq: number,
		fallbackFrontier: () => string
	): void {
		entry.retired.push(entry.current);
		entry.current = fresh;
		const frontier = this.frontiers.get(entry.handle.objectId) ?? fallbackFrontier;
		this.subscribeLocal(entry, frontier);
		entry.projection.applyEngineDiff(
			{ documentId: entry.handle.documentId } satisfies EngineDiff,
			headSeq
		);
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
		const fresh = await freshDocFrom(bootstrapChunks(bootstrap));
		this.swapDoc(entry, fresh, bootstrap.head_seq, () => bootstrap.head_frontier);
	}

	/**
	 * The doc threw while importing bytes the server provided. A failed `import` can panic the WASM
	 * engine (a tree op under a parent that does not exist), after which every call on the doc
	 * throws, so the doc is discarded: a fresh one is built from the REST bootstrap (or, when this
	 * user may not read it there, from the next session snapshot, as the outbox does), the editor
	 * rebinds to it (`swapDoc` bumps the generation) and the session reconnects, which replays the
	 * unsent queue exactly as every other resync does. At most `POISON_REBUILDS_MAX` rebuilds per
	 * `POISON_WINDOW_MS`; after that the editor shows its load error instead of looping.
	 */
	async recoverUnreadableDoc(entry: OpenEntry): Promise<void> {
		const now = Date.now();
		entry.poisonRebuilds = entry.poisonRebuilds.filter((at) => now - at < POISON_WINDOW_MS);
		if (entry.poisonRebuilds.length >= POISON_REBUILDS_MAX) {
			this.stopOnUnreadableDoc(entry);
			return;
		}
		entry.poisonRebuilds.push(now);
		const objectId = entry.handle.objectId;
		try {
			const bootstrap = await this.acceptedBootstrap(objectId);
			if (this.open_.get(objectId) !== entry) return;
			if (bootstrap) await this.rebuildFromAccepted(bootstrap);
			else entry.rebuildOnSnapshot = true;
			await entry.session.reconnect('stale_frontier');
		} catch {
			this.stopOnUnreadableDoc(entry);
		}
	}

	/** The rebuild budget is spent: show the error, stop editing and sending, keep the queue. The
	 * unsent edits stay in the outbox (IndexedDB) and the recovery draft; the rest of the app runs. */
	private stopOnUnreadableDoc(entry: OpenEntry): void {
		entry.offline.setReadOnly(true);
		entry.readOnlyStore.set(true);
		entry.reportEditorFailure(unreadableRemoteUpdate());
	}

	async close(objectId: string): Promise<void> {
		const entry = this.open_.get(objectId);
		if (!entry) return;
		entry.refCount -= 1;
		if (entry.refCount > 0) return;
		entry.lifecycleAbort.abort();
		this.open_.delete(objectId);
		this.frontiers.delete(objectId);
		disposePageBodyJobs(entry);
		entry.offline.dispose();
		entry.unsubscribeLocal?.();
		await entry.session.dispose();
		// The current doc may be the one the engine panicked on (after the rebuild budget ran out):
		// releasing it must not stop the rest of the teardown.
		try {
			clearBodyClipboard(entry.doc);
			releaseEditorSession(entry.doc);
			entry.doc.free();
		} catch {
			// Nothing left to release.
		}
		for (const retired of entry.retired) {
			// A retired doc may be one the engine panicked on; releasing it must not stop the rest.
			try {
				clearBodyClipboard(retired);
				releaseEditorSession(retired);
				retired.free();
			} catch {
				// Nothing left to release.
			}
		}
	}

	/** App/account teardown: no cached document or socket may survive into another account. */
	async disposeAll(): Promise<void> {
		for (const [id, entry] of [...this.open_.entries()]) {
			entry.refCount = 1;
			await this.close(id);
		}
		this.records_.clear();
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
		if (!entry || entry.session.isReadOnly || get(entry.readOnly)) return;
		getNamedMap(entry.doc, 'meta').set('title', title);
		entry.doc.commit();
		entry.projection.updateObjectMeta({ title });
	}

	/** `exportRecoveryDraft` (`ui-surface-v1.md`): the recovery draft as a JSON document -- every
	 * intent that could not be replayed, with its reason and its raw update bytes, plus the local
	 * document content -- so a refused edit is never silently lost. */
	async exportRecoveryDraft(objectId: string): Promise<Blob> {
		const document = this.recoveryDraftDocument(objectId);
		const entry = this.open_.get(objectId);
		const exported = entry?.editorPaused
			? {
					...document,
					preserved_snapshot_base64: btoa(
						Array.from(entry.doc.export({ mode: 'snapshot' }), (byte) =>
							String.fromCharCode(byte)
						).join('')
					),
					undo_history_reset: true
				}
			: document;
		return new Blob([JSON.stringify(exported, null, 2)], { type: 'application/json' });
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
		if (!entry)
			throw new Error(`FlowObjectRepository.exportRecoveryDraft: ${objectId} is not open`);
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
	async bootstrap(
		objectId: string,
		known?: { seq: number; frontier: string }
	): Promise<FlowBootstrap> {
		const result = await flowApi.getBootstrap(
			objectId,
			known ? { known_seq: known.seq, known_frontier: known.frontier } : {}
		);
		if (result.code !== 0 || !result.data) {
			throw {
				code:
					result.code === 404 ? 'not_found' : result.code === 409 ? 'resync_required' : 'forbidden',
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
		if (negotiation.outcome !== 'supported') {
			// Both halves: the outbox stops sending AND the canvas stops editing, so no edit is
			// made that could neither be sent nor kept ("Nothing is dropped").
			entry.offline.setReadOnly(true);
			entry.readOnlyStore.set(true);
		}
	}

	/** `ObjectRepositoryContract.replaceWithAccepted`: imports a bootstrap's snapshot+tail into the
	 * matching open document (`bootstrap.object_id`), wholesale. Loro's `import` is CRDT-merge, not
	 * destructive replace, so this only ever advances the doc toward the server's accepted state --
	 * it never rolls back content the doc already has that the server has since superseded. */
	async replaceWithAccepted(bootstrap: FlowBootstrap): Promise<void> {
		const entry = this.open_.get(bootstrap.object_id);
		if (!entry) return;
		if (!importRemote(entry.doc, bootstrapChunks(bootstrap))) {
			void this.recoverUnreadableDoc(entry);
			throw docRebuildStarted();
		}
		entry.projection.applyEngineDiff(
			{ documentId: entry.handle.documentId } satisfies EngineDiff,
			bootstrap.head_seq
		);
	}

	/** One object's current server view (`GET /flow/objects/{id}`) without opening a collaboration
	 * session or an engine doc. Used by surfaces that only need the accepted head (title, type,
	 * `document_seq`, `frontier`), e.g. the conversion wizard. Returns the raw `ApiResult`. */
	async get(objectId: string): Promise<ApiResult<FlowObjectView>> {
		const result = await flowApi.getObject(objectId);
		const entry = this.open_.get(objectId);
		if (result.code === 0 && result.data && entry && this.currentUserId() === entry.ownerUserId) {
			entry.metadataStore.set(result.data);
			Object.assign(entry.handle.object, result.data);
		}
		return result;
	}

	// v0.8 workspace operations reads (`surface-coverage-v1.md`: admin health / lag / integrity
	// dashboards and the projection-lag badge are `adapter:ObjectRepository` consumers). They
	// return the raw `ApiResult` so the operations panel can branch on `error_code` itself.

	getAdminHealth(scopeId: string): Promise<ApiResult<FlowAdminHealth>> {
		return flowApi.getAdminHealth(scopeId);
	}

	getAdminLag(
		scopeId: string,
		query: { cursor?: string; limit?: number } = {}
	): Promise<ApiResult<FlowAdminLag>> {
		return flowApi.getAdminLag(scopeId, query);
	}

	getAdminIntegrity(
		scopeId: string,
		query: { scope: 'summary' | 'documents'; cursor?: string; limit?: number }
	): Promise<ApiResult<FlowAdminIntegrity>> {
		return flowApi.getAdminIntegrity(scopeId, query);
	}

	getWorkspaceProjectionLag(
		workspaceId: string,
		query: { cursor?: string; limit?: number } = {}
	): Promise<ApiResult<FlowProjectionLag>> {
		return flowApi.getWorkspaceProjectionLag(workspaceId, query);
	}

	getProjectionLag(
		scopeId: string,
		query: { cursor?: string; limit?: number } = {}
	): Promise<ApiResult<FlowProjectionLag>> {
		return flowApi.getProjectionLag(scopeId, query);
	}

	// v0.5 Navigator / command palette full-text search (`surface-coverage-v1.md`: search is an
	// `adapter:ObjectRepository` consumer). A read only: a hit is never opened from its snippet,
	// the object page GETs the current object itself. Raw `ApiResult`, so `navigator-search.ts`
	// branches on `error_code` (`require_current` -> `stale_frontier`).

	search(
		scopeId: string,
		query: FlowSearchQuery,
		signal?: AbortSignal
	): Promise<ApiResult<FlowSearchResponse>> {
		return flowApi.searchObjects(scopeId, query, signal);
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
		scopeId: string,
		importJobId: string
	): Promise<ApiResult<FlowPackageImportReport>> {
		return flowApi.getPackageImport(scopeId, importJobId);
	}

	// v0.5 object panel reads (`ui-surface-v1.md` "职责边界": the Share panel reads through
	// `ObjectRepository`, the Relations panel is a lazy-loaded repository read). Raw `ApiResult`s,
	// so the panel controllers (`share-panel.ts`, `relations-panel.ts`) branch on `error_code`.

	// v0.6 Collections (`rest-api-v1.md` v0.6 table). Schema, collection and record-list reads are
	// typed projections; none of them opens an engine doc. Raw `ApiResult`s, so the collection
	// view controller (`collection-view.ts`) branches on `error_code`.

	/** `GET /flow/objects/{id}/schema`. A Record answers its owning Collection's schema; the
	 * standalone record route reads it this way (`CollectionViewOptions.schemaFromRecord`). */
	getRecordContext(recordId: string) {
		return flowApi.getRecordContext(recordId);
	}

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
function bootstrapChunks(bootstrap: FlowBootstrap): Uint8Array[] {
	return [
		fromBase64(bootstrap.snapshot_base64),
		...bootstrap.tail_updates.map((update) => fromBase64(update.bytes))
	];
}

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

type LoroModule = typeof import('loro-crdt');
type RawLoroDoc = import('loro-crdt').LoroDoc;

/** The complete state of a doc as a comparable value: every container resolved, text containers
 * as their delta (so the marks, `toJSON` renders a rich text as its plain string), and the empty
 * root containers dropped: reading a root container (`getMap`) makes it appear without it
 * holding any operation, and such a container never syncs. */
function stateJson(loro: LoroModule, doc: RawLoroDoc): unknown {
	const sorted = (value: unknown): unknown => {
		if (Array.isArray(value)) return value.map(sorted);
		if (typeof value !== 'object' || value === null) return value;
		const record = value as Record<string, unknown>;
		return Object.fromEntries(
			Object.keys(record)
				.sort()
				.map((k) => [k, sorted(record[k])])
		);
	};
	const json = sorted(
		doc.toJsonWithReplacer((_key, value) =>
			value instanceof loro.LoroText ? (value.toDelta() as never) : value
		)
	) as Record<string, unknown>;
	const out: Record<string, unknown> = {};
	for (const key of Object.keys(json)) {
		const value = json[key];
		const empty =
			(Array.isArray(value) && value.length === 0) ||
			(typeof value === 'object' &&
				value !== null &&
				!Array.isArray(value) &&
				Object.keys(value).length === 0) ||
			value === '';
		if (!empty) out[key] = value;
	}
	return out;
}

function randomPeerId(): `${number}` {
	const words = new Uint32Array(2);
	crypto.getRandomValues(words);
	// 52 random bits: a decimal peer id that is also a safe JS integer, never the reserved max.
	return String(words[0] * 0x100000 + (words[1] >>> 12)) as `${number}`;
}

/**
 * Re-encodes the unsent local changes of `live` as one update on a fork of everything else `live`
 * holds (`FLOW-RETRYABLE-BUSY-DESIGN.md` 6.3). Loro changes are immutable and keep the
 * dependencies they were made with, so "on top of the head" means new operations under a fresh
 * peer id: `base` is `live` minus the changes the queued updates carry (including remote updates
 * that arrived meanwhile, which are not ours to re-send); the fork at `base` gets the diff from
 * `base` to `live` applied. The complete state (text marks included) is compared with `live`
 * before anything is returned; on any difference the fork is freed and nothing changes.
 *
 * Nothing here may commit: the caller has committed the editor's open transaction and settled
 * every update it emitted before this synchronous section. `unreplayable` names the queued
 * entries whose operations the live doc does not hold (they depend on an intent that left the
 * doc for the recovery draft): the re-encoding cannot carry them, so the caller drafts them.
 */
function rebaseUnsentChanges(
	loro: LoroModule,
	live: RawLoroDoc,
	queued: readonly OutboxEntry[]
):
	| { ok: true; doc: RawLoroDoc; bytes: Uint8Array | null; unreplayable: ReadonlySet<string> }
	| { ok: false; error: FlowError } {
	let fork: RawLoroDoc | null = null;
	let target: RawLoroDoc | null = null;
	const mismatch = () => ({
		ok: false as const,
		error: clientError('invalid_update', { reason: 'rebase_state_mismatch' })
	});
	try {
		const liveVersion = live.oplogVersion().toJSON();
		const base = new Map(liveVersion);
		const unreplayable = new Set<string>();
		let partial = false;
		for (const entry of queued) {
			const meta = loro.decodeImportBlobMeta(entry.bytes, false);
			let missing = false;
			let applied = false;
			for (const [peer, end] of meta.partialEndVersionVector.toJSON()) {
				if ((liveVersion.get(peer) ?? 0) < end) missing = true;
			}
			for (const [peer, start] of meta.partialStartVersionVector.toJSON()) {
				const current = base.get(peer);
				if (current !== undefined && start < current) {
					applied = true;
					if (start === 0) base.delete(peer);
					else base.set(peer, start);
				}
			}
			if (missing) {
				unreplayable.add(entry.updateId);
				// Some of its changes are in the live doc: they must not be re-encoded, because
				// the entry as a whole goes to the draft and restoring it would apply them twice.
				if (applied) partial = true;
			}
		}
		const baseFrontiers = live.vvToFrontiers(new loro.VersionVector(base));
		fork = live.forkAt(baseFrontiers);
		fork.setPeerId(randomPeerId());
		const before = fork.oplogVersion();
		let source = live;
		if (partial) {
			// The state to carry is the base plus the entries that go with it. An entry that
			// cannot be applied on top of that (it builds on an excluded one) is excluded too.
			for (let round = 0; round <= queued.length && target === null; round += 1) {
				const candidate = live.forkAt(baseFrontiers);
				let blocked: string | null = null;
				for (const entry of queued) {
					if (unreplayable.has(entry.updateId)) continue;
					const status = candidate.import(entry.bytes);
					if (status.pending && status.pending.size > 0) {
						blocked = entry.updateId;
						break;
					}
				}
				if (blocked === null) target = candidate;
				else {
					candidate.free();
					unreplayable.add(blocked);
				}
			}
			if (target === null) {
				fork.free();
				return mismatch();
			}
			source = target;
		}
		fork.applyDiff(source.diff(baseFrontiers, source.frontiers(), false), { fullState: true });
		fork.commit();
		const same = JSON.stringify(stateJson(loro, fork)) === JSON.stringify(stateJson(loro, source));
		target?.free();
		if (!same) {
			fork.free();
			return mismatch();
		}
		if (fork.oplogVersion().compare(before) === 0) {
			return { ok: true, doc: fork, bytes: null, unreplayable };
		}
		return {
			ok: true,
			doc: fork,
			bytes: fork.export({ mode: 'update', from: before }),
			unreplayable
		};
	} catch {
		target?.free();
		fork?.free();
		return mismatch();
	}
}
