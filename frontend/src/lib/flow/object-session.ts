// `ObjectSession`: ticket/WS, ack/resume/backoff, presence, accepted/rejected/resync state machine
// (`contracts/ui-surface-v1.md` "职责边界"). Speaks the exact wire shape the server implements
// (`apps/api/src/flow/collab/frame.rs`: one JSON text WebSocket message per frame, tagged `type`,
// base64 byte fields) and the handshake order `apps/api/src/flow/collab/session.rs::run` actually
// executes: client `hello` -> server `hello` -> client `open` -> server `snapshot` -> steady state.
//
// v0.4 scope note (documented, not silent): full mid-stream mismatch resync
// (`collab-protocol-v1.md` "客户端仅在 accepted.seq == last_applied_seq+1 时应用...立即停止该
// document apply并请求 resync/bootstrap") is implemented here as "close and reconnect", which gets
// a fresh, consistent `snapshot` from the same loader REST `Bootstrap` would use
// (`rest-api-v1.md`: "WS `open` 与 REST endpoint 使用同一 loader"). It does not attempt to splice a
// partial tail into a live document. `GET .../bootstrap` IS routed server-side at this repo's
// baseline now (`ObjectRepository.bootstrap`/`replaceWithAccepted` use it directly for the
// recovery-draft flow), but reusing it here instead of a WS reconnect would only change which
// transport re-delivers the same snapshot+tail -- the actually-missing piece for a true
// `stale_frontier/resync_required` "persist intent -> bootstrap -> replaceWithAccepted -> replay"
// flow is a local pending-update outbox to replay, which is explicitly out of scope this round:
// Offline pending/outbox is out of scope (`ui-surface-v1.md` "v0.5：presence/cursor、offline
// pending...").

import type { CollabTicket } from '$lib/api/flow';
import { apiClient, type ApiResult } from '$lib/api/client';
import { writable, type Readable } from 'svelte/store';
import type {
	EngineUpdate,
	FlowError,
	FlowLimitsNegotiation,
	FlowLimitsV1,
	ObjectHandle,
	ObjectSessionContract,
	SemanticIntent,
	SyncState
} from './types';
import { DEFAULT_FLOW_LIMITS, checkUpdateBytes } from './limits';
import {
	SERVER_REJECTED_CLOSE_CODE,
	SERVER_REJECTED_STREAK_LIMIT,
	clientError,
	drainDisposition,
	flowErrorFromCloseCode,
	flowErrorFromEnvelope,
	flowErrorFromRejectedFrame,
	parseRetryAfterMs as retryAfterMs
} from './errors';
import {
	PRESENCE_MIN_INTERVAL_MS,
	PRESENCE_REFRESH_MS,
	PRESENCE_TTL_SECONDS_DEFAULT,
	PresenceThrottle,
	RemotePresenceMap,
	buildPresencePayload,
	presencePayloadBytes,
	type PresenceClock,
	type PresenceCursor,
	type PresenceIdentity,
	type PresencePayload,
	type RemotePresence
} from './presence';

export interface AcceptedNotice {
	readonly headSeq: number;
	readonly headFrontier: string;
	readonly projectionSeq: number;
	readonly eventId: string;
	/** Present only for updates this connection did not itself submit (remote/other-session commits). */
	readonly remoteBytes?: Uint8Array;
}

export interface SnapshotPayload {
	readonly snapshotSeq: number;
	readonly headSeq: number;
	readonly snapshotBytes: Uint8Array;
	readonly tailUpdates: ReadonlyArray<{ seq: number; bytes: Uint8Array }>;
	readonly headFrontier: string;
}

export type DisconnectReason = 'manual' | 'network' | 'stale_frontier' | 'auth_expired';

interface SessionHooks {
	onSnapshot(payload: SnapshotPayload): void;
	onAccepted(notice: AcceptedNotice): void;
	onFlowError(error: FlowError): void;
	/** A (re)connection reached `snapshot`: the outbox may replay. */
	onConnected?(): void;
	/** The session was given up for good (three consecutive `server_rejected`, close 4500). */
	onTerminal?(error: FlowError): void;
	/** Access was revoked on a live connection (close 4403): the session is now read-only. */
	onRevoked?(error: FlowError): void;
}

/** Test seams. Production uses the defaults. */
export interface SessionOptions {
	readonly clock?: PresenceClock;
	/** How long a sent `update` may go unanswered before the connection is treated as lost. */
	readonly answerTimeoutMs?: number;
	/** Where `online`/`offline` come from. Defaults to `window` + `navigator.onLine`. */
	readonly connectivity?: Connectivity | null;
}

/** The browser's network status: `isOffline()` plus `online`/`offline` events. */
export interface Connectivity {
	isOffline(): boolean;
	readonly events: Pick<EventTarget, 'addEventListener' | 'removeEventListener'>;
}

function browserConnectivity(): Connectivity | null {
	if (typeof window === 'undefined') return null;
	return {
		isOffline: () => typeof navigator !== 'undefined' && navigator.onLine === false,
		events: window
	};
}

/** Why a local presence value was not sent. */
export interface PresenceRefusal {
	readonly limitKind: 'presence_payload_bytes';
	readonly limit: number;
	readonly observed: number;
}

const PROTOCOL_VERSION = 1;
/** `collab-protocol-v1.md` "hello: capabilities": the one capability this client uses. */
export const CLIENT_CAPABILITIES = ['presence'] as const;
const DEFAULT_ANSWER_TIMEOUT_MS = 20_000;
/** How long `connect()` waits for the first `snapshot` before giving up on this attempt. */
const CONNECT_TIMEOUT_MS = 15_000;
const BACKOFF_STEPS_MS = [250, 500, 1000, 2000, 5000];

function toBase64(bytes: Uint8Array): string {
	let binary = '';
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return btoa(binary);
}

function fromBase64(value: string): Uint8Array {
	const binary = atob(value);
	const out = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
	return out;
}

function wsBaseUrl(): string {
	const configured = import.meta.env.VITE_API_BASE_URL as string | undefined;
	if (configured) {
		return configured.replace(/^http/, 'ws');
	}
	if (typeof window === 'undefined') return '';
	return window.location.origin.replace(/^http/, 'ws');
}

type PendingResolve = {
	resolve: () => void;
	reject: (error: FlowError) => void;
	timer: ReturnType<typeof setTimeout> | null;
};

/** The subset of `CommandService` `ObjectSession` needs for ticket issuance -- kept narrow so
 * this module depends on an interface, not the concrete class, matching `types.ts`'s existing
 * "narrowed to what this v0.4 delivery actually implements" convention. */
export interface TicketIssuer {
	createTicket(input: {
		workspace_id: string;
		document_id: string;
		client_id: string;
		origin: string;
	}): Promise<ApiResult<CollabTicket>>;
}

export class LoroObjectSession implements ObjectSessionContract {
	private readonly hooks: SessionHooks;
	private readonly commandService: TicketIssuer;
	private readonly clientId: string;
	private socket: WebSocket | null = null;
	private handle: ObjectHandle | null = null;
	private readonly pending = new Map<string, PendingResolve>();
	private reconnectAttempt = 0;
	private lastReconnectDelayMs: number | null = null;
	private connectTimeout: ReturnType<typeof setTimeout> | null = null;
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private disposed = false;
	private manualClose = false;
	private readonly stateStore = writable<SyncState>('local');
	/** Effective ceilings for this session's client-side pre-checks. Starts at the pre-bootstrap
	 * fallback and is replaced wholesale the first time a real `Bootstrap.limits` payload is
	 * negotiated (`adoptLimits`); never merged field-by-field. */
	private limits: FlowLimitsV1 = DEFAULT_FLOW_LIMITS;
	/** True once a `Bootstrap.limits` payload was rejected by version negotiation. Local writes
	 * are blocked while set (`limits-v1.md`: "Client 不得自行放宽或缓存跨 `version` limits"). */
	private unknownLimitsVersion: Extract<
		FlowLimitsNegotiation,
		{ outcome: 'unknownVersion' }
	> | null = null;
	/** Resolved/rejected the first time this connection reaches `snapshot` or a terminal failure.
	 * `connect()` awaits this so callers (`ObjectRepository.open`) never hand a doc to
	 * `EditorAdapter`/a navigator reorder before the doc actually has server content imported --
	 * without this, `LoroSyncPlugin` can initialize a ProseMirror doc against a still-empty local
	 * Loro doc and `doc.subscribeLocalUpdates` ships that empty-init as a spurious `update` frame
	 * with `base_frontier: ""` before the real snapshot has even arrived. Found via manual E2E,
	 * not by inspection -- see the delivery report. */
	private firstSync: { resolve: () => void; reject: (error: FlowError) => void } | null = null;

	// ---- v0.5 ----
	private readonly clock: PresenceClock;
	private readonly answerTimeoutMs: number;
	/** True between this connection's `snapshot` and its close. */
	private connected = false;
	/** Given up for good: never reconnects again (`server_rejected` streak, close 4500). */
	private terminal = false;
	/** `view`/`comment` on this object, or a 4403 revocation: no local write is sent. */
	private permissionReadOnly = false;
	/** `collab-protocol-v1.md` "连续 3 次": consecutive `update` frames this connection saw
	 * answered `server_rejected`. Any other answer to an update resets it; frames that are not
	 * answers to an update (`presence`, `ping`, a `rejected` with no `update_id`) do not touch it. */
	private serverRejectedStreak = 0;
	/** Highest accepted seq applied to the doc on this connection. */
	private lastAppliedSeq = 0;
	/** A peer's `update` bytes, held until its paired `accepted` (same `update_id`) arrives. */
	private readonly remoteUpdates = new Map<string, Uint8Array>();
	private readonly remotePresence: RemotePresenceMap;
	private readonly presenceThrottle: PresenceThrottle<PresencePayload>;
	private presenceIdentity: PresenceIdentity | null = null;
	private localPresence: PresencePayload | null = null;
	private presenceRefreshTimer: ReturnType<typeof setInterval> | null = null;
	private presenceSweepTimer: ReturnType<typeof setInterval> | null = null;
	private lastPresenceRefusal: PresenceRefusal | null = null;
	private presenceFramesSent = 0;
	/** Sockets this class closed itself; their `close` events are not connection losses. */
	private readonly intentionallyClosed = new WeakSet<WebSocket>();
	/** Access was revoked on a live connection (4403): never reconnect automatically. */
	private revoked = false;
	private readonly onWindowOnline = () => this.handleOnline();
	private readonly onWindowOffline = () => this.handleOffline();
	private readonly connectivity: Connectivity | null;

	constructor(hooks: SessionHooks, commandService: TicketIssuer, options: SessionOptions = {}) {
		this.hooks = hooks;
		this.commandService = commandService;
		this.clientId =
			typeof crypto !== 'undefined' && 'randomUUID' in crypto
				? crypto.randomUUID()
				: `client-${Math.random().toString(36).slice(2)}`;
		this.clock = options.clock ?? { now: () => Date.now() };
		this.answerTimeoutMs = options.answerTimeoutMs ?? DEFAULT_ANSWER_TIMEOUT_MS;
		this.remotePresence = new RemotePresenceMap(this.clock);
		this.presenceThrottle = new PresenceThrottle<PresencePayload>(
			(payload) => this.sendPresence(payload),
			PRESENCE_MIN_INTERVAL_MS,
			this.clock
		);
		this.connectivity =
			options.connectivity === undefined ? browserConnectivity() : options.connectivity;
		this.connectivity?.events.addEventListener('online', this.onWindowOnline);
		this.connectivity?.events.addEventListener('offline', this.onWindowOffline);
	}

	get state(): Readable<SyncState> {
		return { subscribe: this.stateStore.subscribe };
	}

	/** Remote peers on this document (ephemeral; never part of the engine doc). */
	get presence(): Readable<ReadonlyArray<RemotePresence>> {
		return this.remotePresence.peers;
	}

	/** Current unexpired peers (tests, diagnostics). */
	remotePresenceList(): ReadonlyArray<RemotePresence> {
		return this.remotePresence.list();
	}

	/** The last local presence value refused by the payload pre-check, if any. */
	get presenceRefusal(): PresenceRefusal | null {
		return this.lastPresenceRefusal;
	}

	/** How many `presence` frames this session has put on the wire. */
	get presenceFrameCount(): number {
		return this.presenceFramesSent;
	}

	private isOffline(): boolean {
		return this.connectivity?.isOffline() ?? false;
	}

	get isConnected(): boolean {
		return this.connected && !!this.socket && this.socket.readyState === WebSocket.OPEN;
	}

	get isTerminal(): boolean {
		return this.terminal;
	}

	get consecutiveServerRejections(): number {
		return this.serverRejectedStreak;
	}

	/**
	 * The one place the sync indicator changes. While the session is read-only (permission, or an
	 * unknown limits version) the "everything is fine" states read as `read_only`: a `view` user
	 * who receives a peer's snapshot must not see `local`/`saved` next to a canvas they cannot
	 * edit.
	 */
	private setState(next: SyncState): void {
		const readOnly = this.permissionReadOnly || this.unknownLimitsVersion !== null;
		if (
			readOnly &&
			(next === 'local' || next === 'saved' || next === 'saving' || next === 'resyncing')
		) {
			this.stateStore.set('read_only');
			return;
		}
		this.stateStore.set(next);
	}

	/** Puts the session in (or takes it out of) permission read-only: the object's effective
	 * level for this user is `view`/`comment` (`ADR-0012`: "`edit` 起可写、`view` 只读"). */
	setPermissionReadOnly(readOnly: boolean): void {
		this.permissionReadOnly = readOnly;
		if (readOnly) this.setState('read_only');
		else if (this.connected) this.setState('local');
	}

	get isPermissionReadOnly(): boolean {
		return this.permissionReadOnly;
	}

	/** Adopts (or refuses) a live `Bootstrap.limits` payload for this session.
	 *
	 * On `supported` the server's numbers replace whatever this session was using -- including
	 * undoing an earlier read-only degradation, because a later bootstrap that negotiates cleanly
	 * is proof the mismatch is gone. On `unknownVersion` the session drops to the `read_only`
	 * sync state and `submit()` refuses every local write from then on, rather than falling back
	 * to this build's compiled-in ceilings. */
	adoptLimits(negotiation: FlowLimitsNegotiation): void {
		if (negotiation.outcome === 'supported') {
			this.limits = negotiation.limits;
			this.unknownLimitsVersion = null;
			return;
		}
		this.unknownLimitsVersion = negotiation;
		this.setState('read_only');
	}

	/** The ceilings this session currently pre-checks against. */
	get effectiveLimits(): FlowLimitsV1 {
		return this.limits;
	}

	/** Whether local writes are blocked because `Bootstrap.limits` failed version negotiation. */
	get isReadOnly(): boolean {
		return this.unknownLimitsVersion !== null || this.permissionReadOnly;
	}

	async connect(handle: ObjectHandle): Promise<void> {
		this.handle = handle;
		this.manualClose = false;
		const firstSync = new Promise<void>((resolve, reject) => {
			this.firstSync = { resolve, reject };
		});
		const timeout = new Promise<void>((_, reject) => {
			// A connect that never completes is a LOCAL condition, so it is marked as such. It used
			// to reject with a hand-built `server_draining{reason:'contention'}` -- a required,
			// server-produced discriminator manufactured by the client, which would have shown a
			// server-contention banner for what is really "this browser got no answer", and would
			// have made every "the UI honoured the server's reason" assertion pass vacuously.
			this.connectTimeout = setTimeout(
				() => reject(clientError('resync_required')),
				CONNECT_TIMEOUT_MS
			);
		});
		await this.openSocket();
		try {
			await Promise.race([firstSync, timeout]);
		} finally {
			this.firstSync = null;
			if (this.connectTimeout) {
				clearTimeout(this.connectTimeout);
				this.connectTimeout = null;
			}
		}
	}

	private async openSocket(): Promise<void> {
		if (this.disposed || !this.handle || this.terminal || this.revoked) return;
		if (this.isOffline()) {
			// No network at all: wait for the `online` event instead of burning backoff steps on
			// tickets that cannot be issued.
			this.setState('offline');
			return;
		}
		this.setState('reconnecting');

		const clientToken = apiClient.getToken();
		if (!clientToken) {
			this.setState('auth_required');
			this.firstSync?.reject({ code: 'unauthenticated', recoverable: true });
			return;
		}

		let ticketResult = await this.commandService.createTicket({
			workspace_id: this.handle.workspaceId,
			document_id: this.handle.documentId,
			client_id: this.clientId,
			origin: typeof window !== 'undefined' ? window.location.origin : 'sylvode-flow-web'
		});

		if (ticketResult.code === 401) {
			// `ui-surface-v1.md` "连接、refresh 与恢复状态机" step 3: "ticket 401：最多一次
			// `ensureFreshAccessToken` + 新 ticket；仍失败进入 `auth_required`，停止自动 loop."
			const refreshed = await apiClient.ensureFreshAccessToken();
			if (refreshed) {
				ticketResult = await this.commandService.createTicket({
					workspace_id: this.handle.workspaceId,
					document_id: this.handle.documentId,
					client_id: this.clientId,
					origin: typeof window !== 'undefined' ? window.location.origin : 'sylvode-flow-web'
				});
			}
		}

		if (ticketResult.code === 401) {
			this.setState('auth_required');
			this.firstSync?.reject({ code: 'unauthenticated', recoverable: true });
			return;
		}
		if (ticketResult.code === 403) {
			// `forbidden`/`feature_disabled`: not recoverable by retrying (`error-mapping-v1.md`
			// UI invariant "auth/feature/forbidden 不自动无限重连"), unlike a transient failure.
			// Both map to HTTP-equivalent 403, so the stable `error_code` -- not the status, and
			// never the message -- decides which one this is.
			this.setState('error');
			const error: FlowError = flowErrorFromEnvelope(ticketResult) ?? {
				code: 'forbidden',
				recoverable: false
			};
			this.hooks.onFlowError(error);
			this.firstSync?.reject(error);
			return;
		}
		if (ticketResult.code !== 0 || !ticketResult.data) {
			// A typed rejection on the ticket endpoint is the REST half of the same producer the
			// WebSocket `rejected` frame carries -- most importantly `server_draining`, whose
			// required `details.reason` decides whether this client waits out a drain or retries a
			// contention. Reconnecting blindly here would discard that discriminator.
			const rejection = flowErrorFromEnvelope(ticketResult);
			if (rejection) {
				this.applyFlowError(rejection);
				return;
			}
			this.scheduleReconnect();
			return;
		}

		const url = `${wsBaseUrl()}${ticketResult.data.websocket_url}`;
		const socket = new WebSocket(url);
		this.socket = socket;

		socket.addEventListener('open', () => {
			this.sendFrame({
				type: 'hello',
				protocol_version: PROTOCOL_VERSION,
				capabilities: [...CLIENT_CAPABILITIES],
				client_id: this.clientId,
				session_id: this.clientId
			});
		});

		let sentOpen = false;
		socket.addEventListener('message', (event: MessageEvent<string>) => {
			let frame: Record<string, unknown>;
			try {
				frame = JSON.parse(event.data) as Record<string, unknown>;
			} catch {
				return;
			}

			if (frame.type === 'hello' && !sentOpen) {
				sentOpen = true;
				this.sendFrame({
					type: 'open',
					protocol_version: PROTOCOL_VERSION,
					document_id: this.handle?.documentId,
					known_seq: null,
					known_frontier: null
				});
				return;
			}

			this.handleFrame(frame);
		});

		socket.addEventListener('close', (event) => {
			const wasCurrent = this.socket === socket;
			if (wasCurrent) this.socket = null;
			// A socket this class closed on purpose already ran its own cleanup, and a stale
			// socket's late close must not touch the connection that replaced it (a real
			// browser delivers `close` asynchronously, after the replacement may be open).
			if (this.intentionallyClosed.has(socket) || !wasCurrent) return;
			this.onConnectionLost();
			if (this.manualClose || this.disposed || this.terminal) return;
			if (event.code === SERVER_REJECTED_CLOSE_CODE) {
				this.terminate(
					flowErrorFromCloseCode(event.code, event.reason ?? '') ?? clientError('server_rejected')
				);
				return;
			}
			if (event.code === 4403) {
				this.revoke(flowErrorFromCloseCode(event.code, '') ?? clientError('forbidden'));
				return;
			}
			if (this.isOffline()) {
				this.setState('offline');
				return;
			}
			// The frozen WS close codes (`error-mapping-v1.md`'s "WS close/control" column) are
			// machine-readable discriminators in their own right, including 4410's drain payload.
			const closed = flowErrorFromCloseCode(event.code, event.reason ?? '');
			if (closed) {
				this.applyFlowError(closed);
				return;
			}
			this.scheduleReconnect();
		});

		socket.addEventListener('error', () => {
			// Handled by the subsequent `close` event; nothing actionable here.
		});
	}

	private handleFrame(frame: Record<string, unknown>): void {
		switch (frame.type) {
			case 'snapshot': {
				this.setState('resyncing');
				const tailUpdates = (frame.tail_updates as Array<Record<string, unknown>>).map((u) => ({
					seq: u.seq as number,
					bytes: fromBase64(u.bytes as string)
				}));
				this.hooks.onSnapshot({
					snapshotSeq: frame.snapshot_seq as number,
					headSeq: frame.head_seq as number,
					snapshotBytes: fromBase64(frame.snapshot as string),
					tailUpdates,
					headFrontier: frame.head_frontier as string
				});
				this.reconnectAttempt = 0;
				this.lastAppliedSeq = frame.head_seq as number;
				this.connected = true;
				this.remoteUpdates.clear();
				this.serverRejectedStreak = 0;
				// `snapshot` establishes the baseline document state, not a save confirmation.
				// `contracts/ui-surface-v1.md` "连接、refresh 与恢复状态机": "只有带 matching
				// update id 和 events-v1.md event id 的 accepted frame 可进入 saved" -- entering
				// `saved` here would tell the user an edit was durably persisted before this
				// connection has ever received a matching `accepted` frame for one. `local`
				// reflects "loaded and mirrors the server as of this snapshot, no in-flight or
				// confirmed local edit" without making that false claim.
				this.setState('local');
				this.firstSync?.resolve();
				this.startPresenceTimers();
				// Re-announce this user's own presence on the new connection: the server dropped
				// the old entry when the old connection closed.
				if (this.localPresence) this.presenceThrottle.push(this.localPresence);
				this.hooks.onConnected?.();
				break;
			}
			case 'update': {
				// A peer's committed update. Its paired `accepted` (same `update_id`) follows
				// (`apps/api/src/flow/collab/write.rs::broadcast_committed_update`); the bytes are
				// applied only then, and only in seq order.
				if (typeof frame.update_id === 'string' && typeof frame.bytes === 'string') {
					try {
						this.remoteUpdates.set(frame.update_id, fromBase64(frame.bytes));
					} catch {
						// Undecodable bytes: the paired `accepted` falls back to a resync.
					}
				}
				break;
			}
			case 'presence': {
				if (typeof frame.session_id === 'string') {
					this.remotePresence.receive(
						frame.session_id,
						frame.payload,
						typeof frame.ttl_seconds === 'number' ? frame.ttl_seconds : PRESENCE_TTL_SECONDS_DEFAULT
					);
				}
				break;
			}
			case 'accepted': {
				const updateId = frame.update_id as string;
				const waiter = this.pending.get(updateId);
				const headSeq = frame.head_seq as number;
				if (waiter) {
					this.settle(updateId);
					// An answer to an update that is not `server_rejected`: the streak ends.
					this.serverRejectedStreak = 0;
					this.lastAppliedSeq = Math.max(this.lastAppliedSeq, headSeq);
					waiter.resolve();
					this.hooks.onAccepted({
						headSeq,
						headFrontier: frame.head_frontier as string,
						projectionSeq: frame.projection_seq as number,
						eventId: frame.event_id as string
					});
					this.setState('saved');
				} else if (this.remoteUpdates.has(updateId)) {
					const bytes = this.remoteUpdates.get(updateId) as Uint8Array;
					this.remoteUpdates.delete(updateId);
					if (headSeq <= this.lastAppliedSeq) break; // idempotent duplicate
					if (headSeq !== this.lastAppliedSeq + 1) {
						// `collab-protocol-v1.md`: a gap is never buffered or guessed across.
						this.setState('resyncing');
						void this.reconnect('stale_frontier');
						break;
					}
					this.lastAppliedSeq = headSeq;
					this.hooks.onAccepted({
						headSeq,
						headFrontier: frame.head_frontier as string,
						projectionSeq: frame.projection_seq as number,
						eventId: frame.event_id as string,
						remoteBytes: bytes
					});
				} else {
					// This `accepted` is for an update THIS connection did not submit -- i.e. another
					// session committed a change. `apps/api/src/flow/collab/session.rs` broadcasts
					// `Frame::Accepted` (head_seq/frontier/event_id only, no `bytes`) to every other
					// session on the document; there is no second broadcast frame carrying the actual
					// update bytes at this repo's baseline. A full reconnect (fresh `snapshot`) is
					// therefore the only way this client can pick up someone else's content -- the
					// same recovery path `resync_required` already uses. Documented backend gap, not
					// a client bug: true incremental multi-session fan-out needs a server-side change
					// outside `frontend/**`.
					this.setState('resyncing');
					void this.reconnect('stale_frontier');
				}
				break;
			}
			case 'rejected': {
				// `error-mapping-v1.md`: a consumer that does not recognise a `RejectedCode` treats
				// it as non-retryable. Before this, an unknown code returned `null` here and the
				// update's waiter was never settled -- the write hung forever.
				const error: FlowError = flowErrorFromRejectedFrame(frame) ?? {
					code: 'invalid_update',
					recoverable: false,
					origin: 'server'
				};
				const updateId = this.answeredUpdateId(frame, error);
				if (updateId) {
					// Only an answer to an `update` drives the streak (`collab-protocol-v1.md`
					// "「被答复的」这个限定词是承重的").
					this.serverRejectedStreak =
						frame.code === 'server_rejected' ? this.serverRejectedStreak + 1 : 0;
					const waiter = this.pending.get(updateId);
					if (waiter) {
						this.settle(updateId);
						waiter.reject(error);
					}
				}
				if (this.serverRejectedStreak >= SERVER_REJECTED_STREAK_LIMIT) {
					this.hooks.onFlowError(error);
					this.terminate(error);
					break;
				}
				if (error.code === 'limit_exceeded' && retryAfterMs(error.details) !== null) {
					// A rate refusal: the outbox resends the same update after `retry_after_ms`.
					// Not a sync error and not a banner.
					break;
				}
				this.applyFlowError(error);
				break;
			}
			case 'resync': {
				this.setState('resyncing');
				this.reconnect('stale_frontier');
				break;
			}
			case 'ping': {
				this.sendFrame({ type: 'pong', protocol_version: PROTOCOL_VERSION, nonce: frame.nonce });
				break;
			}
			default:
				break;
		}
	}

	/** The single place a `FlowError` -- from a REST envelope, a `rejected` control frame or a
	 * close code -- turns into a sync state and a user-visible report.
	 *
	 * `server_draining`'s two reasons share a stable code and MUST diverge here, in exactly the
	 * ways `error-mapping-v1.md` freezes: `drain` gives up this connection and waits out the
	 * advertised `retry_after_ms` before reconnecting; `contention` keeps the socket and the
	 * accepted head untouched and lets the caller retry the write. A `server_draining` with a
	 * missing or unknown `details.reason` is a protocol violation, not a third behaviour: it is
	 * reported and NOT acted on, because both possible guesses are harmful (retrying into a
	 * draining instance, or showing maintenance for a lock conflict). */
	private applyFlowError(error: FlowError): void {
		if (error.code === 'server_draining') {
			const disposition = drainDisposition(error);
			this.hooks.onFlowError(error);
			// An error that arrives before this connection ever reached `snapshot` means the OPEN
			// itself failed, whatever the reason says about retrying afterwards. Resolving that
			// distinction here is what stops `connect()` from hanging until its timeout on a drain.
			this.firstSync?.reject(error);
			if (!disposition) return;
			this.setState(disposition.syncState);
			if (!disposition.keepConnection) this.closeCurrentSocket();
			if (disposition.reconnects) this.scheduleReconnect(disposition.retryAfterMs);
			return;
		}
		if (error.code === 'resync_required' || error.code === 'stale_frontier') {
			// Both mean "this connection's frontier is behind": a fresh connection brings the
			// accepted snapshot, and the outbox replays from `onConnected`.
			this.setState('resyncing');
			void this.reconnect('stale_frontier');
			return;
		}
		if (error.code === 'unauthenticated') {
			this.setState('auth_required');
			this.firstSync?.reject(error);
			return;
		}
		this.setState('error');
		this.hooks.onFlowError(error);
		this.firstSync?.reject(error);
	}

	/** Closes this connection on purpose.
	 *
	 * The resulting `close` event must NOT run the generic lost-connection path: that path
	 * schedules a plain jittered backoff, and because `scheduleReconnect` is a no-op while a timer
	 * is already pending, it would win the race against the drain's own `retry_after_ms` floor and
	 * silently reconnect ~60 ms into a 1.5 s drain. Found by the UI state test, not by reading. */
	private closeCurrentSocket(code?: number, reason?: string): void {
		const socket = this.socket;
		this.socket = null;
		if (!socket) return;
		this.intentionallyClosed.add(socket);
		this.manualClose = true;
		if (code === undefined) socket.close();
		else socket.close(code, reason);
		this.manualClose = false;
		this.onConnectionLost();
	}

	private sendFrame(frame: Record<string, unknown>): void {
		if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return;
		this.socket.send(JSON.stringify(frame));
	}

	async submit(
		update: EngineUpdate,
		_intent: SemanticIntent,
		options: { updateId?: string } = {}
	): Promise<void> {
		if (this.terminal) {
			throw { code: 'server_rejected', recoverable: false, origin: 'client' } satisfies FlowError;
		}
		if (!this.socket || this.socket.readyState !== WebSocket.OPEN || !this.handle) {
			// No socket is a local condition, not a server rejection -- see `clientError`.
			throw clientError('resync_required');
		}
		// `Bootstrap.limits` failed version negotiation: this client cannot know the real
		// ceilings, and `limits-v1.md` forbids writing under its own cached/compiled-in ones
		// ("Client 不得自行放宽或缓存跨 `version` limits"). Refuse locally instead of sending.
		if (this.permissionReadOnly) {
			throw {
				code: 'policy_rejected',
				recoverable: false,
				origin: 'client',
				details: { reason: 'read_only' }
			} satisfies FlowError;
		}
		const unknownVersion = this.unknownLimitsVersion;
		if (unknownVersion) {
			throw {
				code: 'policy_rejected',
				recoverable: false,
				details: {
					reason: 'unknown_limits_version',
					limits_version: unknownVersion.version,
					negotiation: unknownVersion.reason
				}
			} satisfies FlowError;
		}
		// Client-side pre-check (`ui-surface-v1.md` "Session/CommandService 在编码前按
		// limits-v1.md 检查 update/frame...超限不进入 IndexedDB outbox、不发网络请求"): reject an
		// obviously over-budget update before spending a round trip. This does not replace the
		// server's own re-validation of the same limit.
		const violation = checkUpdateBytes(update.bytes, this.limits);
		if (violation) {
			throw {
				code: 'limit_exceeded',
				recoverable: false,
				details: {
					limit_kind: violation.limitKind,
					limit: violation.limit,
					observed: violation.observed
				}
			} satisfies FlowError;
		}
		// A replay passes the outbox entry's id so the retry is the SAME update to the server
		// (`collab-protocol-v1.md` `write_state:"unknown"`: "必须复用同一 `update_id` 重试").
		const updateId =
			options.updateId ??
			(typeof crypto !== 'undefined' && 'randomUUID' in crypto
				? crypto.randomUUID()
				: `${Date.now()}-${Math.random()}`);

		this.setState('saving');
		return new Promise<void>((resolve, reject) => {
			// An update nobody ever answers would block the ordered outbox for good; after the
			// timeout the connection is treated as lost and the update is replayed under the same
			// id on the next one.
			const timer = setTimeout(() => {
				const waiter = this.pending.get(updateId);
				if (!waiter) return;
				this.settle(updateId);
				waiter.reject(clientError('resync_required'));
				void this.reconnect('network');
			}, this.answerTimeoutMs);
			this.pending.set(updateId, { resolve, reject, timer });
			this.sendFrame({
				type: 'update',
				protocol_version: PROTOCOL_VERSION,
				document_id: this.handle?.documentId,
				update_id: updateId,
				base_frontier: update.baseFrontier,
				bytes: toBase64(update.bytes),
				idempotency_key: updateId,
				origin: 'local',
				message: null
			});
		});
	}

	/** Who this user is, for the presence payload (id, display name, colour). */
	setPresenceIdentity(identity: PresenceIdentity): void {
		this.presenceIdentity = identity;
	}

	/**
	 * Reports this user's cursor/selection (`null`: no caret in this document). Throttled to at
	 * most one frame per `PRESENCE_MIN_INTERVAL_MS`; a payload over `presence_payload_bytes_max` is
	 * refused locally and never sent. Presence never blocks or delays an edit.
	 */
	setPresence(cursor: PresenceCursor | null): void {
		if (!this.presenceIdentity) return;
		const payload = buildPresencePayload(this.presenceIdentity, cursor);
		const observed = presencePayloadBytes(payload);
		const limit = this.limits.presencePayloadBytesMax;
		if (observed > limit) {
			this.lastPresenceRefusal = { limitKind: 'presence_payload_bytes', limit, observed };
			return;
		}
		this.lastPresenceRefusal = null;
		this.localPresence = payload;
		this.presenceThrottle.push(payload);
	}

	private sendPresence(payload: PresencePayload): void {
		if (!this.isConnected || !this.handle) return;
		this.presenceFramesSent += 1;
		this.sendFrame({
			type: 'presence',
			protocol_version: PROTOCOL_VERSION,
			document_id: this.handle.documentId,
			// Required by the frame's wire shape; the server keys the entry by its own id for
			// this connection and ignores this value.
			session_id: this.clientId,
			payload,
			ttl_seconds: PRESENCE_TTL_SECONDS_DEFAULT
		});
	}

	private startPresenceTimers(): void {
		if (!this.presenceRefreshTimer) {
			this.presenceRefreshTimer = setInterval(() => {
				if (this.localPresence) this.presenceThrottle.push(this.localPresence);
			}, PRESENCE_REFRESH_MS);
		}
		if (!this.presenceSweepTimer) {
			this.presenceSweepTimer = setInterval(() => this.remotePresence.sweep(), 1000);
		}
	}

	private stopPresenceTimers(): void {
		if (this.presenceRefreshTimer) clearInterval(this.presenceRefreshTimer);
		if (this.presenceSweepTimer) clearInterval(this.presenceSweepTimer);
		this.presenceRefreshTimer = null;
		this.presenceSweepTimer = null;
	}

	/** The update a `rejected` frame answers: its `update_id`, or -- for the server's rate
	 * refusal, which is sent without one (`session.rs::limit_exceeded_frame` sets `update_id:
	 * None`) -- the single update in flight, when there is exactly one. */
	private answeredUpdateId(frame: Record<string, unknown>, error: FlowError): string | null {
		if (typeof frame.update_id === 'string') return frame.update_id;
		if (error.code !== 'limit_exceeded' || this.pending.size !== 1) return null;
		const details = error.details as { limit_kind?: unknown } | undefined;
		if (details?.limit_kind !== 'update_rate') return null;
		return [...this.pending.keys()][0] ?? null;
	}

	private settle(updateId: string): void {
		const waiter = this.pending.get(updateId);
		if (waiter?.timer) clearTimeout(waiter.timer);
		this.pending.delete(updateId);
	}

	/** The connection is gone (closed by either side, or by going offline). Every update still in
	 * flight has an unknown `write_state`; its waiter is rejected as a local transport error so
	 * the outbox replays it under the same id on the next connection. */
	private onConnectionLost(): void {
		this.connected = false;
		this.remoteUpdates.clear();
		this.remotePresence.clear();
		this.stopPresenceTimers();
		for (const [updateId, waiter] of [...this.pending]) {
			this.settle(updateId);
			waiter.reject(clientError('resync_required'));
		}
	}

	/** Three consecutive permanent refusals: close 4500 and never reconnect. */
	private terminate(error: FlowError): void {
		if (this.terminal) return;
		this.terminal = true;
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		this.closeCurrentSocket(SERVER_REJECTED_CLOSE_CODE, 'server_rejected');
		this.stateStore.set('error');
		this.firstSync?.reject(error);
		this.hooks.onTerminal?.(error);
	}

	/** Access revoked on a live connection (4403): read-only, no automatic reconnect. */
	private revoke(error: FlowError): void {
		this.revoked = true;
		this.permissionReadOnly = true;
		this.stateStore.set('read_only');
		this.hooks.onFlowError(error);
		this.firstSync?.reject(error);
		this.hooks.onRevoked?.(error);
	}

	private handleOffline(): void {
		if (this.disposed || this.terminal) return;
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		this.closeCurrentSocket();
		this.setState('offline');
	}

	private handleOnline(): void {
		if (this.disposed || this.terminal || this.revoked || this.socket || !this.handle) return;
		// `ui-surface-v1.md` step 6: "online event 可提前触发一次".
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		this.reconnectAttempt = 0;
		void this.openSocket();
	}

	async reconnect(_reason: DisconnectReason): Promise<void> {
		if (this.terminal || this.disposed || this.revoked) return;
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		this.closeCurrentSocket();
		await this.openSocket();
	}

	/** `ui-surface-v1.md` step 6: full-jitter exponential backoff 250ms->5s, with
	 * `server_draining.details.retry_after_ms` acting as a LOWER bound when the server supplied
	 * one -- reconnecting sooner than the server asked is exactly what a drain is trying to
	 * prevent, so the advertised floor wins over a shorter jittered delay. */
	private scheduleReconnect(retryAfterMs: number | null = null): void {
		if (this.disposed || this.reconnectTimer) return;
		this.setState('reconnecting');
		const step = BACKOFF_STEPS_MS[Math.min(this.reconnectAttempt, BACKOFF_STEPS_MS.length - 1)];
		const jitter = Math.random() * step;
		const delay = Math.max(jitter, retryAfterMs ?? 0);
		this.reconnectAttempt += 1;
		this.lastReconnectDelayMs = delay;
		this.reconnectTimer = setTimeout(() => {
			this.reconnectTimer = null;
			void this.openSocket();
		}, delay);
	}

	/** The delay the most recent reconnect was scheduled with. Read by the UI state tests to
	 * prove `retry_after_ms` was honoured as a floor rather than silently dropped. */
	get lastScheduledReconnectDelayMs(): number | null {
		return this.lastReconnectDelayMs;
	}

	async dispose(): Promise<void> {
		this.disposed = true;
		this.manualClose = true;
		this.presenceThrottle.cancel();
		this.stopPresenceTimers();
		this.connectivity?.events.removeEventListener('online', this.onWindowOnline);
		this.connectivity?.events.removeEventListener('offline', this.onWindowOffline);
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		if (this.connectTimeout) {
			clearTimeout(this.connectTimeout);
			this.connectTimeout = null;
		}
		for (const [updateId, waiter] of [...this.pending]) {
			// Teardown, not a server drain: the caller navigated away or the route was destroyed.
			this.settle(updateId);
			waiter.reject(clientError('resync_required'));
		}
		this.closeCurrentSocket();
	}
}
