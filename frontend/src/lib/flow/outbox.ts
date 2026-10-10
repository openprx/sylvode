// The local outbox and recovery draft stores (`ui-surface-v1.md` "职责边界": "`ObjectRepository`：
// ...IndexedDB accepted cache/本地 outbox"). `ObjectRepository` is the only owner; no component
// imports this module.
//
// `versions/v0.5-collaboration.md` "Web 交互": "IndexedDB outbox 同时保存 update bytes、base
// frontier、idempotency key 和可重放 semantic intent". Every field of `OutboxEntry` below is one of
// those four plus bookkeeping (order, owner object, the peer that produced the bytes). An entry
// leaves the outbox only when the server accepted it, or when it moves -- whole, with the reason it
// could not be replayed -- into the recovery draft store. Nothing is deleted any other way.

import type { FlowErrorCode, SemanticIntent } from './types';

/** One local update waiting for the server's `accepted`. `updateId` doubles as the
 * idempotency key: a replay of the same intent resends both unchanged
 * (`collab-protocol-v1.md`: a retry under `write_state:"unknown"` must reuse the same id). */
export interface OutboxEntry {
	readonly updateId: string;
	readonly idempotencyKey: string;
	readonly objectId: string;
	readonly documentId: string;
	/** Monotonic per store; the replay order. */
	readonly order: number;
	readonly bytes: Uint8Array;
	readonly baseFrontier: string;
	readonly intent: SemanticIntent;
	readonly enqueuedAt: number;
	/** The Loro peer that produced `bytes` (diagnostics; not used for any decision). */
	readonly peerId: string;
}

/** Why an intent could not be replayed. `source` says who decided:
 * - `server`: the server answered this update with `code`;
 * - `precheck`: this client refused to send it (policy re-check, limits pre-check);
 * - `dependency`: an earlier intent this one causally builds on was refused (`rootUpdateId`), so
 *   sending it would only store operations the server can never apply. */
export interface RecoveryDraftReason {
	readonly code: FlowErrorCode;
	readonly source: 'server' | 'precheck' | 'dependency';
	readonly details?: unknown;
	readonly rootUpdateId?: string;
}

export interface RecoveryDraftEntry {
	readonly updateId: string;
	readonly idempotencyKey: string;
	readonly objectId: string;
	readonly order: number;
	readonly bytes: Uint8Array;
	readonly baseFrontier: string;
	readonly intent: SemanticIntent;
	readonly enqueuedAt: number;
	readonly rejectedAt: number;
	readonly reason: RecoveryDraftReason;
}

export interface OutboxStore {
	/** Entries of one object, in replay order. */
	list(objectId: string): Promise<OutboxEntry[]>;
	put(entry: OutboxEntry): Promise<void>;
	delete(updateId: string): Promise<void>;
	listDrafts(objectId: string): Promise<RecoveryDraftEntry[]>;
	/** Moves an intent into the draft store: one transaction writes the draft and deletes the
	 * outbox entry, so a crash between the two can neither lose it nor replay it. */
	moveToDraft(draft: RecoveryDraftEntry): Promise<void>;
	clearDrafts(objectId: string): Promise<void>;
	/** The next free `order` value. */
	nextOrder(): Promise<number>;
}

export const OUTBOX_DB_NAME = 'sylvode-flow-outbox-v1';
const OUTBOX_STORE = 'outbox';
const DRAFT_STORE = 'drafts';
const BY_OBJECT = 'by_object';

function byOrder<T extends { order: number }>(a: T, b: T): number {
	return a.order - b.order;
}

function requestResult<T>(request: IDBRequest<T>): Promise<T> {
	return new Promise((resolve, reject) => {
		request.onsuccess = () => resolve(request.result);
		request.onerror = () => reject(request.error ?? new Error('IndexedDB request failed'));
	});
}

function transactionDone(tx: IDBTransaction): Promise<void> {
	return new Promise((resolve, reject) => {
		tx.oncomplete = () => resolve();
		tx.onabort = () => reject(tx.error ?? new Error('IndexedDB transaction aborted'));
		tx.onerror = () => reject(tx.error ?? new Error('IndexedDB transaction failed'));
	});
}

/** `IndexedDB`-backed outbox. */
export class IndexedDbOutboxStore implements OutboxStore {
	private readonly db: IDBDatabase;

	private constructor(db: IDBDatabase) {
		this.db = db;
	}

	/** Opens (creating on first use) the outbox database, or resolves `null` when the browser
	 * gives this origin no IndexedDB at all. */
	static open(factory: IDBFactory | undefined): Promise<IndexedDbOutboxStore | null> {
		return new Promise((resolve) => {
			if (!factory) {
				resolve(null);
				return;
			}
			let request: IDBOpenDBRequest;
			try {
				request = factory.open(OUTBOX_DB_NAME, 1);
			} catch {
				resolve(null);
				return;
			}
			request.onupgradeneeded = () => {
				const db = request.result;
				const outbox = db.createObjectStore(OUTBOX_STORE, { keyPath: 'updateId' });
				outbox.createIndex(BY_OBJECT, 'objectId', { unique: false });
				const drafts = db.createObjectStore(DRAFT_STORE, { keyPath: 'updateId' });
				drafts.createIndex(BY_OBJECT, 'objectId', { unique: false });
			};
			request.onsuccess = () => resolve(new IndexedDbOutboxStore(request.result));
			request.onerror = () => resolve(null);
		});
	}

	async list(objectId: string): Promise<OutboxEntry[]> {
		const tx = this.db.transaction(OUTBOX_STORE, 'readonly');
		const rows = await requestResult(
			tx.objectStore(OUTBOX_STORE).index(BY_OBJECT).getAll(objectId) as IDBRequest<OutboxEntry[]>
		);
		return rows.sort(byOrder);
	}

	async put(entry: OutboxEntry): Promise<void> {
		const tx = this.db.transaction(OUTBOX_STORE, 'readwrite');
		tx.objectStore(OUTBOX_STORE).put(entry);
		await transactionDone(tx);
	}

	async delete(updateId: string): Promise<void> {
		const tx = this.db.transaction(OUTBOX_STORE, 'readwrite');
		tx.objectStore(OUTBOX_STORE).delete(updateId);
		await transactionDone(tx);
	}

	async listDrafts(objectId: string): Promise<RecoveryDraftEntry[]> {
		const tx = this.db.transaction(DRAFT_STORE, 'readonly');
		const rows = await requestResult(
			tx.objectStore(DRAFT_STORE).index(BY_OBJECT).getAll(objectId) as IDBRequest<
				RecoveryDraftEntry[]
			>
		);
		return rows.sort(byOrder);
	}

	async moveToDraft(draft: RecoveryDraftEntry): Promise<void> {
		const tx = this.db.transaction([OUTBOX_STORE, DRAFT_STORE], 'readwrite');
		tx.objectStore(DRAFT_STORE).put(draft);
		tx.objectStore(OUTBOX_STORE).delete(draft.updateId);
		await transactionDone(tx);
	}

	async clearDrafts(objectId: string): Promise<void> {
		const drafts = await this.listDrafts(objectId);
		const tx = this.db.transaction(DRAFT_STORE, 'readwrite');
		for (const draft of drafts) tx.objectStore(DRAFT_STORE).delete(draft.updateId);
		await transactionDone(tx);
	}

	async nextOrder(): Promise<number> {
		const tx = this.db.transaction([OUTBOX_STORE, DRAFT_STORE], 'readonly');
		const [outbox, drafts] = await Promise.all([
			requestResult(tx.objectStore(OUTBOX_STORE).getAll() as IDBRequest<OutboxEntry[]>),
			requestResult(tx.objectStore(DRAFT_STORE).getAll() as IDBRequest<RecoveryDraftEntry[]>)
		]);
		let max = 0;
		for (const row of [...outbox, ...drafts]) max = Math.max(max, row.order);
		return max + 1;
	}
}

/** In-memory outbox for a browser without IndexedDB (the outbox then lasts as long as the tab). */
export class MemoryOutboxStore implements OutboxStore {
	private readonly outbox = new Map<string, OutboxEntry>();
	private readonly drafts = new Map<string, RecoveryDraftEntry>();
	private order = 0;

	async list(objectId: string): Promise<OutboxEntry[]> {
		return [...this.outbox.values()].filter((e) => e.objectId === objectId).sort(byOrder);
	}

	async put(entry: OutboxEntry): Promise<void> {
		this.outbox.set(entry.updateId, entry);
	}

	async delete(updateId: string): Promise<void> {
		this.outbox.delete(updateId);
	}

	async listDrafts(objectId: string): Promise<RecoveryDraftEntry[]> {
		return [...this.drafts.values()].filter((e) => e.objectId === objectId).sort(byOrder);
	}

	async moveToDraft(draft: RecoveryDraftEntry): Promise<void> {
		this.drafts.set(draft.updateId, draft);
		this.outbox.delete(draft.updateId);
	}

	async clearDrafts(objectId: string): Promise<void> {
		for (const [id, draft] of this.drafts) if (draft.objectId === objectId) this.drafts.delete(id);
	}

	async nextOrder(): Promise<number> {
		this.order += 1;
		let max = this.order;
		for (const row of [...this.outbox.values(), ...this.drafts.values()]) {
			max = Math.max(max, row.order + 1);
		}
		this.order = max;
		return max;
	}
}

/** The outbox store for this browser: IndexedDB when available, memory otherwise. */
export async function openOutboxStore(): Promise<OutboxStore> {
	const factory = typeof indexedDB === 'undefined' ? undefined : indexedDB;
	return (await IndexedDbOutboxStore.open(factory)) ?? new MemoryOutboxStore();
}
