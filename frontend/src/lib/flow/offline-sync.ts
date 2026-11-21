// Offline editing, ordered replay and the recovery draft for one open document
// (`ui-surface-v1.md` "连接、refresh 与恢复状态机" steps 4-5, `versions/v0.5-collaboration.md`
// "Web 交互"). Composed by `ObjectRepository`, which owns the outbox (`outbox.ts`) and the engine
// doc; every update still leaves through `ObjectSession.submit` ("同一 update 只由 Session 发出").
//
// The rules this class enforces, each of which a test in `tests/flow-outbox.test.ts` or
// `tests/flow-offline-recovery.test.ts` turns red when broken:
//
// 1. Every local update is persisted before it is sent, and leaves the outbox only on `accepted`
//    or by moving -- whole, with its reason -- into the recovery draft. Nothing is dropped.
// 2. A replay reuses the entry's `updateId`/idempotency key. A fresh key would walk past the
//    server's `(document_id, update_id)` dedup and apply the same operations twice.
// 3. Updates go out one at a time, in enqueue order. A Loro change always builds on the earlier
//    changes of the same peer, so sending update N+1 before N is accepted can only store
//    operations the server cannot apply yet.
// 4. After a reconnect (or any rejection that ends the straight-line path): bootstrap ->
//    `replaceWithAccepted` -> re-check policy -> replay what is still allowed, in order, under the
//    same keys. An entry whose first send committed before the connection dropped is replayed
//    too: the server answers a replayed `update_id` with the original receipt
//    (`collab-protocol-v1.md` "同 `update_id` 重放必须被答复"), so the client never has to guess
//    from the snapshot whether it was already accepted.
// 5. A load refusal (`server_draining` / `apply_busy`, `not_applied`) is retried inside caps.
//    `retry_mode = resend` sends the same entry under the same id (at most `RESEND_SENDS_MAX`
//    sends). `retry_mode = rebase` means the update was built on a frontier that lacked a peer's
//    commit, so resending can never pass the server's sequential gate: the unsent local edits are
//    re-encoded as ONE new update on top of the head and sent under a new id (at most
//    `REBASE_MAX` rebases per edit). After a cap the entry takes the permanent-refusal path
//    below. The old ids may be dropped because the refusal is `not_applied` and the entries
//    after the head were never sent, so nothing in the queue is on the server.
// 6. `policy_rejected` / `invalid_update` / `server_rejected` / an unreplayable limit do not change
//    the accepted head. The refused intent, and every later intent that causally depends on it,
//    goes to the recovery draft; the live document is then rebuilt from the accepted snapshot so
//    it equals what the server accepted. `policy_rejected` is final for the connection's writer
//    (`error-mapping-v1.md`: "保留 intent，不自动重放"): the server re-sources a stale authorization
//    epoch in place, so a refusal means the level really is below `edit`.

import { writable, type Readable } from 'svelte/store';
import type { FlowBootstrap } from '#lib/api/flow.js';
import type { OutboxEntry, OutboxStore, RecoveryDraftEntry, RecoveryDraftReason } from './outbox';
import { applyBusyRetryMode, type RetryMode } from './errors';
import type { EngineUpdate, FlowError, SemanticIntent } from './types';

/** What a scratch replica says about one update relative to the accepted state it was built
 * from: `new` (would add operations), `known` (already contained), `pending` (depends on
 * operations the accepted state does not have). Only `pending` changes what is sent: such an
 * update can never be accepted (`causal_dependency_missing`), so it is drafted without reaching
 * the wire. `known` is sent like `new`; the server answers the replay with its original receipt. */
export type ScratchVerdict = 'new' | 'known' | 'pending';

export interface ScratchReplica {
	classify(bytes: Uint8Array): ScratchVerdict;
	/** Applies bytes the server accepted, so later classifications see them. */
	apply(bytes: Uint8Array): void;
	dispose(): void;
}

/** The slice of `ObjectSession` this class drives. */
export interface OfflineSyncTransport {
	readonly isConnected: boolean;
	readonly isTerminal: boolean;
	submit(
		update: EngineUpdate,
		intent: SemanticIntent,
		options: { updateId: string }
	): Promise<void>;
	/** The write was given up after its retry caps: leave the `saving` indicator. */
	markUnsaved?(): void;
}

/** The outcome of `OfflineSyncHost.rebaseUnsent`. */
export type RebaseResult =
	/** `bytes` is the one new update; `null` when the head already holds every unsent edit. */
	| { readonly ok: true; readonly bytes: Uint8Array | null }
	/** The rebase could not be built; `error` (when set) is reported instead of the final busy text. */
	| { readonly ok: false; readonly error?: FlowError };

/** What `ObjectRepository` provides around the doc and the REST reads. */
export interface OfflineSyncHost {
	readonly objectId: string;
	readonly documentId: string;
	peerId(): string;
	/** A local limits pre-check (`ui-surface-v1.md`: "超限不进入 IndexedDB outbox"). */
	limitViolation(bytes: Uint8Array): FlowError | null;
	/** A fresh policy re-check: may this user still write this object? */
	canWrite(): Promise<boolean>;
	/**
	 * The accepted snapshot+tail over REST, or `null` when this user may not read it there (the
	 * REST bootstrap admits `view`, like the WebSocket session, so `null` means access is gone).
	 */
	bootstrap(): Promise<FlowBootstrap | null>;
	replaceWithAccepted(bootstrap: FlowBootstrap): Promise<void>;
	/** Rebuilds the live doc from exactly `bootstrap` (accepted state only). */
	rebuildFromAccepted(bootstrap: FlowBootstrap): Promise<void>;
	/** Rebuilds the live doc from the session's next `snapshot` (the same loader as the REST
	 * bootstrap), for a user the REST bootstrap refuses. */
	rebuildFromSessionSnapshot(): Promise<void>;
	/** Makes sure the live doc contains `bytes` (an entry persisted by an earlier page load). */
	importIntoLive(bytes: Uint8Array): void;
	/**
	 * Re-encodes the live doc's unsent local changes (everything `entries` carries) as one update
	 * on top of everything else the live doc holds, under a fresh peer id, and rebinds the editor
	 * to the resulting doc. Synchronous: no other local edit may interleave.
	 */
	rebaseUnsent(entries: readonly OutboxEntry[]): RebaseResult;
	scratchFrom(bootstrap: FlowBootstrap): Promise<ScratchReplica>;
	/** The user lost write access: the session and editor go read-only. */
	enterReadOnly(): void;
	reportError(error: FlowError): void;
}

export interface OfflineSyncOptions {
	readonly now?: () => number;
	readonly sleep?: (ms: number) => Promise<void>;
	readonly newId?: () => string;
}

type Classified =
	| { kind: 'transport' }
	| { kind: 'retry'; delayMs: number }
	| { kind: 'busy_resend'; retryAfterMs: number }
	| { kind: 'busy_rebase'; retryAfterMs: number }
	| { kind: 'resync' }
	| { kind: 'policy' }
	| { kind: 'permanent' };

const MIN_RETRY_DELAY_MS = 250;
/** Sends of one update (the first included) before a `resend` refusal becomes final. */
export const RESEND_SENDS_MAX = 5;
/** Rebases of one edit before a `rebase` refusal becomes final. */
export const REBASE_MAX = 3;
const RESEND_BACKOFF_BASE_MS = 250;
const RESEND_BACKOFF_CAP_MS = 4000;
const REBASE_BACKOFF_BASE_MS = 100;
const REBASE_BACKOFF_CAP_MS = 1000;
const REBASE_PERSIST_SETTLE_ROUNDS = 10;

/** Full jitter: uniform in [0, min(cap, base * 2^(attempt-1))]. */
function fullJitter(base: number, cap: number, attempt: number): number {
	return Math.random() * Math.min(cap, base * 2 ** Math.max(0, attempt - 1));
}

function retryAfter(details: unknown): number | null {
	if (typeof details !== 'object' || details === null) return null;
	const value = (details as { retry_after_ms?: unknown }).retry_after_ms;
	return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
}

/** How one rejected `submit` must be handled. Exported for the tests. */
export function classifySubmitError(error: FlowError): Classified {
	if (error.origin === 'client' && error.code === 'resync_required') return { kind: 'transport' };
	switch (error.code) {
		case 'server_draining': {
			const reason = (error.details as { reason?: unknown } | undefined)?.reason;
			if (reason === 'apply_busy') {
				// A missing or unknown `retry_mode` is a protocol violation: fail closed.
				const mode: RetryMode | null = applyBusyRetryMode(error);
				if (mode === null) return { kind: 'permanent' };
				const delay = retryAfter(error.details) ?? 0;
				return mode === 'resend'
					? { kind: 'busy_resend', retryAfterMs: delay }
					: { kind: 'busy_rebase', retryAfterMs: delay };
			}
			return {
				kind: 'retry',
				delayMs: Math.max(retryAfter(error.details) ?? 0, MIN_RETRY_DELAY_MS)
			};
		}
		case 'authorization_churn':
			return {
				kind: 'retry',
				delayMs: Math.max(retryAfter(error.details) ?? 0, MIN_RETRY_DELAY_MS)
			};
		case 'limit_exceeded': {
			// A rate refusal carries `retry_after_ms` and is about *when*, not *what*: the same
			// update under the same key is resent. A size refusal has none and can never pass.
			const delay = retryAfter(error.details);
			return delay === null
				? { kind: 'permanent' }
				: { kind: 'retry', delayMs: Math.max(delay, MIN_RETRY_DELAY_MS) };
		}
		case 'stale_frontier':
		case 'resync_required':
		case 'unauthenticated':
			return { kind: 'resync' };
		case 'policy_rejected':
			return { kind: 'policy' };
		default:
			return { kind: 'permanent' };
	}
}

function defaultId(): string {
	return typeof crypto !== 'undefined' && 'randomUUID' in crypto
		? crypto.randomUUID()
		: `upd-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export class FlowOfflineSync {
	private readonly store: OutboxStore;
	private readonly transport: OfflineSyncTransport;
	private readonly host: OfflineSyncHost;
	private readonly now: () => number;
	private readonly sleep: (ms: number) => Promise<void>;
	private readonly newId: () => string;

	private queue: OutboxEntry[] = [];
	private drafts: RecoveryDraftEntry[] = [];
	private readonly pendingStore = writable(0);
	private persisting = 0;
	private readonly recoveryStore = writable(false);
	private readonly draftStore = writable<ReadonlyArray<RecoveryDraftEntry>>([]);
	/** Serialises every pump/recovery pass; there is never more than one update in flight. */
	private chain: Promise<void> = Promise.resolve();
	private pumpQueued = false;
	private recoveryQueued = false;
	private readOnly = false;
	private disposed = false;
	/** Set whenever an intent moves to the draft: the live doc still holds its operations, so it
	 * has to be rebuilt from the accepted state on the next recovery pass. */
	private needsRebuild = false;
	private nextOrder = 1;
	private enqueueChain: Promise<void> = Promise.resolve();
	/** Busy answers seen per update id (in memory; a reload starts the count again). */
	private readonly busyAnswers = new Map<string, number>();
	/** Rebases of the current edit lineage; carried over to the rebased entry, reset on accept. */
	private rebases = 0;

	constructor(
		store: OutboxStore,
		transport: OfflineSyncTransport,
		host: OfflineSyncHost,
		options: OfflineSyncOptions = {}
	) {
		this.store = store;
		this.transport = transport;
		this.host = host;
		this.now = options.now ?? (() => Date.now());
		this.sleep = options.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
		this.newId = options.newId ?? defaultId;
	}

	/** How many local updates are waiting for the server. */
	get pending(): Readable<number> {
		return { subscribe: this.pendingStore.subscribe };
	}

	/** Intents that could not be replayed, with their reasons. */
	/** The live Page can contain a prefix while an old outbox is replaying. */
	get recovering(): Readable<boolean> {
		return { subscribe: this.recoveryStore.subscribe };
	}

	get recoveryDraft(): Readable<ReadonlyArray<RecoveryDraftEntry>> {
		return { subscribe: this.draftStore.subscribe };
	}

	get pendingEntries(): ReadonlyArray<OutboxEntry> {
		return [...this.queue];
	}

	get draftEntries(): ReadonlyArray<RecoveryDraftEntry> {
		return [...this.drafts];
	}

	/** Loads what an earlier page load left behind. Call once before the first enqueue. */
	async load(): Promise<void> {
		this.nextOrder = await this.store.nextOrder();
		this.queue = await this.store.list(this.host.objectId);
		this.drafts = await this.store.listDrafts(this.host.objectId);
		this.recoveryStore.set(this.queue.length > 0);
		this.publish();
	}

	/** Read-only (by permission or an unknown limits version): local updates are not sent. */
	setReadOnly(readOnly: boolean): void {
		this.readOnly = readOnly;
	}

	/**
	 * One local engine update. Persisted first (`ui-surface-v1.md` step 4: "先持久化 intent"),
	 * then sent in order. An update over a local limit is refused before it reaches the outbox and
	 * goes to the recovery draft instead, and the doc is rebuilt from the accepted state, because
	 * every later local update would causally depend on it.
	 */
	enqueue(bytes: Uint8Array, baseFrontier: string, intent: SemanticIntent): Promise<void> {
		if (this.disposed || this.readOnly) return Promise.resolve();
		// Order and id are fixed synchronously, in the order the doc emitted the updates; the
		// persistence chain below keeps that order through the asynchronous IndexedDB writes.
		const updateId = this.newId();
		const order = this.nextOrder;
		this.nextOrder += 1;
		const enqueuedAt = this.now();
		const peerId = this.host.peerId();
		this.persisting += 1;
		this.publishPending();
		const next = this.enqueueChain.then(async () => {
			let persisted: OutboxEntry | null = null;
			try {
				persisted = await this.persist({
					updateId,
					order,
					bytes,
					baseFrontier,
					intent,
					enqueuedAt,
					peerId
				});
			} finally {
				this.persisting -= 1;
				if (persisted) this.queue.push(persisted);
				this.publishPending();
			}
			if (persisted) this.schedulePump();
		});
		this.enqueueChain = next.catch(() => undefined);
		return next;
	}

	private async persist(input: {
		updateId: string;
		order: number;
		bytes: Uint8Array;
		baseFrontier: string;
		intent: SemanticIntent;
		enqueuedAt: number;
		peerId: string;
	}): Promise<OutboxEntry | null> {
		const { updateId, order, bytes, baseFrontier, intent, enqueuedAt, peerId } = input;
		const entry: OutboxEntry = {
			updateId,
			idempotencyKey: updateId,
			objectId: this.host.objectId,
			documentId: this.host.documentId,
			order,
			bytes,
			baseFrontier,
			intent,
			enqueuedAt,
			peerId
		};
		const violation = this.host.limitViolation(bytes);
		if (violation) {
			this.host.reportError(violation);
			await this.toDraft(entry, {
				code: violation.code,
				source: 'precheck',
				details: violation.details
			});
			this.scheduleRecovery();
			return null;
		}
		await this.store.put(entry);
		return entry;
	}

	/** The session reached `snapshot` on a (re)connection. */
	onConnected(): void {
		if (this.queue.length > 0 || this.needsRebuild) this.scheduleRecovery();
	}

	/** The session was closed permanently (three consecutive `server_rejected`, close 4500). */
	async onTerminal(reason: RecoveryDraftReason): Promise<void> {
		await this.run(async () => {
			await this.draftRemaining(reason);
		});
	}

	/** Write access was revoked mid-session (close 4403): every pending intent goes to the draft. */
	async onRevoked(reason: RecoveryDraftReason): Promise<void> {
		this.readOnly = true;
		await this.run(async () => {
			await this.draftRemaining(reason);
		});
	}

	/** Changes edit permission while a legitimate view stream remains connected. */
	async onAuthorizationChange(canWrite: boolean): Promise<boolean> {
		if (!canWrite) this.readOnly = true;
		// Include already emitted, not-yet-persisted keystrokes before drafting the queue.
		await this.enqueueChain;
		await this.run(async () => {
			if (!canWrite) {
				this.readOnly = true;
				await this.draftRemaining({
					code: 'policy_rejected',
					source: 'server',
					details: { reason: 'authorization_changed' }
				});
				if (this.needsRebuild) await this.recover();
			} else if (this.readOnly) {
				// The draft stays a draft. Rebuild accepted state before new edits can depend on it.
				if (this.needsRebuild) await this.recover();
				if (!this.needsRebuild) this.readOnly = false;
			}
		});
		return !this.disposed && !this.needsRebuild;
	}

	/** Forgets the drafts this instance holds (the ones its user saw); drafts another tab added
	 * to the shared store since stay there for that tab. */
	async clearDraft(): Promise<void> {
		const cleared = this.drafts.map((draft) => draft.updateId);
		await this.store.deleteDrafts(cleared);
		this.drafts = this.drafts.filter((draft) => !cleared.includes(draft.updateId));
		this.publish();
	}

	/** Resolves when every scheduled pass has finished (tests, and teardown). */
	idle(): Promise<void> {
		return this.enqueueChain.then(() => this.chain);
	}

	dispose(): void {
		this.disposed = true;
	}

	// ---- scheduling -------------------------------------------------------------------------

	private run(op: () => Promise<void>): Promise<void> {
		const next = this.chain.then(op).catch((error: unknown) => {
			this.host.reportError(
				(error as FlowError)?.code
					? (error as FlowError)
					: { code: 'resync_required', recoverable: true, origin: 'client' }
			);
		});
		this.chain = next;
		return next;
	}

	private schedulePump(): void {
		if (this.pumpQueued) return;
		this.pumpQueued = true;
		void this.run(async () => {
			this.pumpQueued = false;
			await this.pump();
		});
	}

	private scheduleRecovery(): void {
		if (this.recoveryQueued) return;
		this.recoveryQueued = true;
		this.recoveryStore.set(true);
		void this.run(async () => {
			this.recoveryQueued = false;
			await this.recover();
		});
	}

	// ---- the straight-line path ---------------------------------------------------------------

	/** Sends queued entries one at a time while connected. Hands anything that is not a plain
	 * accept or a timed retry over to `recover`. */
	private async pump(): Promise<void> {
		while (!this.disposed && this.queue.length > 0) {
			if (this.readOnly) return;
			if (this.transport.isTerminal) return;
			if (!this.transport.isConnected) return; // `onConnected` resumes with a recovery pass
			const entry = this.queue[0];
			const outcome = await this.send(entry);
			if (outcome === 'accepted') continue;
			if (outcome === 'retried' || outcome === 'rebased') continue;
			if (outcome === 'stop') return;
			// `recover`: anything that needs the accepted snapshot to decide what comes next.
			this.scheduleRecovery();
			return;
		}
	}

	/** One send of the head entry. */
	private async send(
		entry: OutboxEntry
	): Promise<'accepted' | 'retried' | 'rebased' | 'stop' | 'recover'> {
		try {
			await this.transport.submit(
				{ bytes: entry.bytes, baseFrontier: entry.baseFrontier },
				entry.intent,
				{ updateId: entry.updateId }
			);
		} catch (raw) {
			const error = raw as FlowError;
			const classified = classifySubmitError(error);
			switch (classified.kind) {
				case 'transport':
					return 'stop';
				case 'retry':
					await this.sleep(classified.delayMs);
					return 'retried';
				case 'busy_resend': {
					const sends = (this.busyAnswers.get(entry.updateId) ?? 0) + 1;
					this.busyAnswers.set(entry.updateId, sends);
					if (sends >= RESEND_SENDS_MAX) return this.giveUpBusy(entry, error);
					await this.sleep(
						Math.max(
							classified.retryAfterMs,
							fullJitter(RESEND_BACKOFF_BASE_MS, RESEND_BACKOFF_CAP_MS, sends)
						)
					);
					return 'retried';
				}
				case 'busy_rebase': {
					if (this.rebases >= REBASE_MAX) return this.giveUpBusy(entry, error);
					this.rebases += 1;
					await this.sleep(
						Math.max(
							classified.retryAfterMs,
							fullJitter(REBASE_BACKOFF_BASE_MS, REBASE_BACKOFF_CAP_MS, this.rebases)
						)
					);
					return this.rebase(entry, error);
				}
				case 'resync':
					// The session reconnects on its own for these; the recovery pass runs from
					// `onConnected` once it is back.
					return 'stop';
				case 'policy':
					await this.handlePolicyRejection(error);
					return 'recover';
				case 'permanent':
					this.host.reportError(error);
					await this.toDraft(entry, { code: error.code, source: 'server', details: error.details });
					return 'recover';
			}
		}
		await this.acknowledge(entry);
		return 'accepted';
	}

	/** The retry cap of an `apply_busy` refusal is reached: the permanent-refusal path (recovery
	 * draft, rebuild from the accepted state), reported with the final-failure text. */
	private async giveUpBusy(
		entry: OutboxEntry,
		busy: FlowError,
		failure?: FlowError
	): Promise<'recover'> {
		const details = {
			...(typeof busy.details === 'object' && busy.details !== null ? busy.details : {}),
			reason: 'apply_busy',
			final: true
		};
		this.host.reportError(
			failure ?? { code: 'server_draining', recoverable: false, origin: 'server', details }
		);
		this.transport.markUnsaved?.();
		await this.toDraft(entry, { code: 'server_draining', source: 'server', details });
		this.busyAnswers.delete(entry.updateId);
		this.rebases = 0;
		return 'recover';
	}

	/**
	 * `retry_mode = rebase`: synchronise to the head, re-encode every unsent local edit as one new
	 * sequential update, replace the queue (in memory and, in one IndexedDB transaction, on disk)
	 * by it and let the pump send it under its new id.
	 */
	private async rebase(entry: OutboxEntry, busy: FlowError): Promise<'rebased' | 'recover'> {
		if (this.needsRebuild) {
			// Drafted intents still have their operations in the live doc; they must leave it
			// before anything is re-encoded as "unsent".
			const clean = await this.host.bootstrap();
			if (!clean) return this.dropForPolicy();
			await this.host.rebuildFromAccepted(clean);
			this.needsRebuild = false;
			for (const queued of this.queue) this.host.importIntoLive(queued.bytes);
		}
		const accepted = await this.host.bootstrap();
		if (!accepted) return this.dropForPolicy();
		await this.host.replaceWithAccepted(accepted);
		// Every emitted local update has to be in the queue before it is re-encoded.
		for (let round = 0; round < REBASE_PERSIST_SETTLE_ROUNDS; round += 1) {
			await this.enqueueChain;
			if (this.persisting === 0) break;
		}
		if (this.persisting > 0) return this.giveUpBusy(entry, busy);
		// No `await` from here to the queue swap: nothing else may touch the doc or the queue.
		const unsent = [...this.queue];
		const head = unsent[0];
		const result = this.host.rebaseUnsent(unsent);
		if (!result.ok) return this.giveUpBusy(entry, busy, result.error);
		const oldIds = unsent.map((e) => e.updateId);
		let replacement: OutboxEntry | null = null;
		if (result.bytes) {
			const updateId = this.newId();
			replacement = {
				updateId,
				idempotencyKey: updateId,
				objectId: head.objectId,
				documentId: head.documentId,
				order: head.order,
				bytes: result.bytes,
				baseFrontier: accepted.head_frontier,
				intent: head.intent,
				enqueuedAt: head.enqueuedAt,
				peerId: this.host.peerId()
			};
		}
		this.queue = replacement ? [replacement] : [];
		for (const id of oldIds) this.busyAnswers.delete(id);
		// On the enqueue chain, so later edits (on the rebased doc) persist after the swap.
		const swap = this.enqueueChain.then(() =>
			replacement ? this.store.replace(oldIds, replacement) : this.deleteAll(oldIds)
		);
		this.enqueueChain = swap.catch(() => undefined);
		this.publish();
		await swap;
		return 'rebased';
	}

	private async deleteAll(updateIds: readonly string[]): Promise<void> {
		for (const id of updateIds) await this.store.delete(id);
	}

	/** The REST bootstrap refused this user mid-rebase: the existing policy path. */
	private async dropForPolicy(): Promise<'recover'> {
		this.readOnly = true;
		this.host.enterReadOnly();
		await this.draftRemaining({ code: 'policy_rejected', source: 'precheck' });
		return 'recover';
	}

	/**
	 * `policy_rejected` on a sent update (`error-mapping-v1.md`: "保留 intent，不自动重放"). The server
	 * re-evaluates a connection whose authorization epoch moved before refusing
	 * (`collab-protocol-v1.md`), so this refusal means this user may no longer write here: the
	 * session goes read-only and the refused intent, with every later one, is kept in the recovery
	 * draft. Nothing is reconnected or replayed.
	 */
	private async handlePolicyRejection(error: FlowError): Promise<void> {
		this.host.reportError(error);
		this.readOnly = true;
		this.host.enterReadOnly();
		await this.draftRemaining({
			code: 'policy_rejected',
			source: 'server',
			details: error.details
		});
	}

	// ---- recovery ---------------------------------------------------------------------------

	/**
	 * `ui-surface-v1.md` step 4: bootstrap -> `replaceWithAccepted` -> replay what policy still
	 * allows, in order, under the same keys. Then, if anything went to the draft, rebuild the live
	 * doc from the accepted state so it holds exactly what the server accepted.
	 */
	private async recover(): Promise<void> {
		if (this.disposed) return;
		this.recoveryStore.set(true);
		if (this.queue.length > 0) {
			if (this.transport.isTerminal || !this.transport.isConnected) return;
			const accepted = await this.host.bootstrap();
			if (accepted) await this.host.replaceWithAccepted(accepted);
			const writable = accepted !== null && (await this.host.canWrite().catch(() => false));
			if (!writable || !accepted) {
				this.readOnly = true;
				this.host.enterReadOnly();
				await this.draftRemaining({ code: 'policy_rejected', source: 'precheck' });
			} else {
				const finished = await this.replay(accepted);
				if (!finished) return; // transport lost mid-replay; resumes on the next connect
			}
		}
		if (this.needsRebuild) {
			const accepted = await this.host.bootstrap();
			if (accepted) await this.host.rebuildFromAccepted(accepted);
			else await this.host.rebuildFromSessionSnapshot();
			this.needsRebuild = false;
		}
		if (!this.recoveryQueued) this.recoveryStore.set(false);
		if (this.queue.length > 0) this.schedulePump();
	}

	/** Replays the queue against `accepted`. Returns `false` if the transport went away. */
	private async replay(accepted: FlowBootstrap): Promise<boolean> {
		let scratch = await this.host.scratchFrom(accepted);
		const acceptedBytes: Uint8Array[] = [];
		let root: { updateId: string; code: RecoveryDraftReason['code'] } | null = null;
		try {
			while (this.queue.length > 0) {
				const entry = this.queue[0];
				if (root) {
					await this.toDraft(entry, {
						code: root.code,
						source: 'dependency',
						rootUpdateId: root.updateId
					});
					continue;
				}
				const violation = this.host.limitViolation(entry.bytes);
				if (violation) {
					await this.toDraft(entry, {
						code: violation.code,
						source: 'precheck',
						details: violation.details
					});
					root = { updateId: entry.updateId, code: violation.code };
					continue;
				}
				// A `known` entry (committed before the connection dropped) is sent like any other:
				// the server answers the replayed `update_id` with the original receipt.
				const verdict = scratch.classify(entry.bytes);
				if (verdict === 'pending') {
					await this.toDraft(entry, { code: 'invalid_update', source: 'dependency' });
					continue;
				}
				this.host.importIntoLive(entry.bytes);
				if (!this.transport.isConnected || this.transport.isTerminal) return false;
				const outcome = await this.send(entry);
				if (outcome === 'accepted') {
					scratch.apply(entry.bytes);
					acceptedBytes.push(entry.bytes);
					continue;
				}
				if (outcome === 'retried') {
					continue;
				}
				// The queue is now the rebased entry (plus later edits), built on a newer head
				// than `accepted`: `recover` schedules the pump, which sends it.
				if (outcome === 'rebased') return true;
				if (outcome === 'stop') return false;
				// Refused for good (already in the draft). The scratch replica may hold its
				// operations from `classify`; rebuild it from the accepted state plus what the
				// server accepted since, so the dependents are judged against the truth.
				const last = this.drafts[this.drafts.length - 1];
				root = last ? { updateId: last.updateId, code: last.reason.code } : null;
				scratch.dispose();
				scratch = await this.host.scratchFrom(accepted);
				for (const bytes of acceptedBytes) scratch.apply(bytes);
				if (this.readOnly) {
					await this.draftRemaining({ code: 'policy_rejected', source: 'precheck' });
				}
			}
			return true;
		} finally {
			scratch.dispose();
		}
	}

	// ---- bookkeeping --------------------------------------------------------------------------

	private async acknowledge(entry: OutboxEntry): Promise<void> {
		await this.store.delete(entry.updateId);
		this.queue = this.queue.filter((e) => e.updateId !== entry.updateId);
		this.busyAnswers.delete(entry.updateId);
		this.rebases = 0;
		this.publish();
	}

	private async toDraft(entry: OutboxEntry, reason: RecoveryDraftReason): Promise<void> {
		const draft: RecoveryDraftEntry = {
			updateId: entry.updateId,
			idempotencyKey: entry.idempotencyKey,
			objectId: entry.objectId,
			order: entry.order,
			bytes: entry.bytes,
			baseFrontier: entry.baseFrontier,
			intent: entry.intent,
			enqueuedAt: entry.enqueuedAt,
			rejectedAt: this.now(),
			reason
		};
		await this.store.moveToDraft(draft);
		this.queue = this.queue.filter((e) => e.updateId !== entry.updateId);
		this.drafts = [...this.drafts, draft];
		// The live doc still holds this intent's operations; the next recovery pass rebuilds it
		// from the accepted state.
		this.needsRebuild = true;
		this.publish();
	}

	private async draftRemaining(reason: RecoveryDraftReason): Promise<void> {
		for (const entry of [...this.queue]) await this.toDraft(entry, reason);
	}

	private publishPending(): void {
		this.pendingStore.set(this.queue.length + this.persisting);
	}

	private publish(): void {
		this.publishPending();
		this.draftStore.set([...this.drafts]);
	}
}
