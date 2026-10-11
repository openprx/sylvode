/**
 * A fake collaboration server for the FP2-M6 suites: a mock `WebSocket`, a `fetch` router for the
 * four REST reads the Flow client makes around a session (object, grants, bootstrap, ticket), and
 * a server that holds a real `loro-crdt` document and speaks the wire shapes of
 * `apps/api/src/flow/collab/frame.rs` in the handshake order of `session.rs::run`.
 *
 * It answers by the contract as the server does since FP2-B6 (`collab-protocol-v1.md` 2026-10-10
 * registration):
 * - a replayed `update` whose `update_id` already committed is answered with the ORIGINAL receipt
 *   (same `head_seq`/`head_frontier`/`event_id`), on any connection, and is never applied twice;
 * - every `rejected` that answers an update carries its `update_id` (the rate refusal included);
 * - an update whose operations depend on ones the document does not have (Loro import leaves them
 *   pending) is refused as `invalid_update` with `details.reason="causal_dependency_missing"` and
 *   gets no seq;
 * - the REST bootstrap admits `view`, like the WebSocket session.
 *
 * Every frame the server receives is recorded, so a suite can assert what reached the wire and in
 * which order. `pendingAccepted` counts accepted updates whose import still produced pending
 * operations; with the causal check it stays zero, and the suites assert that.
 */

import { LoroDoc } from 'loro-crdt';
import {
	DEFAULT_FLOW_LIMITS,
	FLOW_LIMITS_FIELDS,
	FLOW_LIMITS_VERSION,
	wireFieldName
} from '../../src/lib/flow/limits';
import type { Connectivity } from '../../src/lib/flow/object-session';

export const OBJECT_ID = '11111111-1111-4111-8111-111111111111';
export const DOCUMENT_ID = '22222222-2222-4222-8222-222222222222';
export const WORKSPACE_ID = '33333333-3333-4333-8333-333333333333';

export function toBase64(bytes: Uint8Array): string {
	let binary = '';
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return btoa(binary);
}

export function fromBase64(value: string): Uint8Array {
	const binary = atob(value);
	const out = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
	return out;
}

export function limitsPayload(overrides: Record<string, number> = {}): Record<string, unknown> {
	const payload: Record<string, unknown> = { version: FLOW_LIMITS_VERSION };
	for (const field of FLOW_LIMITS_FIELDS)
		payload[wireFieldName(field)] = DEFAULT_FLOW_LIMITS[field];
	return { ...payload, ...overrides };
}

export function tick(ms = 0): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

/** A rejection the policy script can return for one update. */
export interface RejectSpec {
	readonly code: string;
	readonly recoverable?: boolean;
	readonly writeState?: 'not_applied' | 'unknown';
	readonly details?: unknown;
	/** Omit `update_id` from the frame. The contract allows that only for a refusal that answers
	 * no update; a suite uses it to prove such a frame is never attributed to one. */
	readonly withoutUpdateId?: boolean;
}

export type PolicyVerdict =
	| { readonly kind: 'accept' }
	| { readonly kind: 'reject'; readonly reject: RejectSpec }
	/** Commit, then drop the connection before answering. */
	| { readonly kind: 'commit_then_drop' }
	/** Never answer at all. */
	| { readonly kind: 'silence' };

export interface UpdateSeen {
	readonly updateId: string;
	readonly idempotencyKey: unknown;
	readonly baseFrontier: unknown;
	readonly bytes: Uint8Array;
	readonly connection: number;
}

type Listener = (event: unknown) => void;

export class MockWebSocket {
	static readonly CONNECTING = 0;
	static readonly OPEN = 1;
	static readonly CLOSING = 2;
	static readonly CLOSED = 3;
	static instances: MockWebSocket[] = [];
	static server: FakeCollabServer | null = null;

	readonly url: string;
	readyState = MockWebSocket.CONNECTING;
	readonly sent: Array<Record<string, unknown>> = [];
	closedByClient: { code: number | undefined; reason: string | undefined } | null = null;
	readonly id: number;
	private readonly listeners = new Map<string, Listener[]>();

	constructor(url: string) {
		this.url = url;
		this.id = MockWebSocket.instances.length + 1;
		MockWebSocket.instances.push(this);
		const server = MockWebSocket.server;
		setTimeout(() => {
			if (this.readyState !== MockWebSocket.CONNECTING) return;
			if (!server || !server.online) {
				this.serverClose(1006, '');
				return;
			}
			server.attach(this);
			this.readyState = MockWebSocket.OPEN;
			this.emit('open', {});
		}, 0);
	}

	addEventListener(type: string, handler: Listener): void {
		const bucket = this.listeners.get(type) ?? [];
		bucket.push(handler);
		this.listeners.set(type, bucket);
	}

	removeEventListener(type: string, handler: Listener): void {
		this.listeners.set(
			type,
			(this.listeners.get(type) ?? []).filter((candidate) => candidate !== handler)
		);
	}

	send(payload: string): void {
		if (this.readyState !== MockWebSocket.OPEN) return;
		const frame = JSON.parse(payload) as Record<string, unknown>;
		this.sent.push(frame);
		const server = MockWebSocket.server;
		setTimeout(() => server?.receive(this, frame), 0);
	}

	close(code?: number, reason?: string): void {
		if (this.readyState === MockWebSocket.CLOSED) return;
		this.closedByClient = { code, reason };
		this.readyState = MockWebSocket.CLOSED;
		MockWebSocket.server?.detach(this);
		// Like a browser: `close` is delivered asynchronously.
		setTimeout(() => this.emit('close', { code: code ?? 1000, reason: reason ?? '' }), 0);
	}

	/** Server side: deliver one frame to the client. */
	deliver(frame: Record<string, unknown>): void {
		if (this.readyState !== MockWebSocket.OPEN) return;
		this.emit('message', { data: JSON.stringify(frame) });
	}

	/** Server side: close the connection. */
	serverClose(code: number, reason: string): void {
		if (this.readyState === MockWebSocket.CLOSED) return;
		this.readyState = MockWebSocket.CLOSED;
		MockWebSocket.server?.detach(this);
		this.emit('close', { code, reason });
	}

	private emit(type: string, event: unknown): void {
		for (const handler of this.listeners.get(type) ?? []) handler(event);
	}
}

/** A controllable stand-in for `window` + `navigator.onLine`. */
export class FakeConnectivity implements Connectivity {
	offline = false;
	readonly events = new EventTarget();

	isOffline(): boolean {
		return this.offline;
	}

	goOffline(): void {
		this.offline = true;
		this.events.dispatchEvent(new Event('offline'));
	}

	goOnline(): void {
		this.offline = false;
		this.events.dispatchEvent(new Event('online'));
	}
}

interface Connection {
	readonly socket: MockWebSocket;
	readonly sessionId: string;
	/** The user the server authenticated for this connection, put on fanned-out presence as
	 * `actor.user_id` (`collab-protocol-v1.md`); `null` imitates a server that sends no actor. */
	readonly userId: string | null;
}

export class FakeCollabServer {
	readonly doc = new LoroDoc();
	headSeq = 0;
	online = true;
	/** The requesting user's effective level, as `GET .../grants` reports it. */
	effectiveLevel: 'none' | 'view' | 'comment' | 'edit' | 'full_access' = 'edit';
	/** The REST bootstrap's `limits` (the current version's defaults when `null`). */
	bootstrapLimits: Record<string, unknown> | null = null;
	/** Per-update decision; default accepts everything. */
	policy: (seen: UpdateSeen, index: number) => PolicyVerdict = () => ({ kind: 'accept' });
	readonly accepted: Array<{ seq: number; updateId: string; bytes: Uint8Array }> = [];
	readonly updatesSeen: UpdateSeen[] = [];
	readonly presenceSeen: Array<Record<string, unknown>> = [];
	readonly hellos: Array<Record<string, unknown>> = [];
	/** Accepted updates whose import produced only pending (causally incomplete) operations. */
	pendingAccepted = 0;
	/** Replays of an already committed `update_id`, answered with the original receipt. */
	replaysAnswered = 0;
	/** Updates refused as `causal_dependency_missing`. */
	causalRefusals = 0;
	readonly connections = new Map<MockWebSocket, Connection>();
	/** Users assigned to the next connections, in order (see `Connection.userId`). */
	connectionUserIds: string[] = [];
	private connectionCounter = 0;
	private readonly byUpdateId = new Map<string, number>();

	install(): void {
		MockWebSocket.server = this;
	}

	attach(socket: MockWebSocket): void {
		this.connectionCounter += 1;
		this.connections.set(socket, {
			socket,
			sessionId: `00000000-0000-4000-8000-${String(this.connectionCounter).padStart(12, '0')}`,
			userId: this.connectionUserIds.shift() ?? null
		});
	}

	detach(socket: MockWebSocket): void {
		this.connections.delete(socket);
	}

	/** Takes the network down: open sockets drop (1006) and tickets cannot be issued. */
	setOnline(online: boolean): void {
		this.online = online;
		if (!online) for (const socket of [...this.connections.keys()]) socket.serverClose(1006, '');
	}

	snapshotBase64(): string {
		return toBase64(this.doc.export({ mode: 'snapshot' }));
	}

	frontier(): string {
		return `f${this.headSeq}`;
	}

	/** A peer (not the client under test) commits `bytes` directly on the server. */
	commitFromPeer(bytes: Uint8Array): void {
		this.commit(`peer-${this.headSeq + 1}`, bytes, null);
	}

	receive(socket: MockWebSocket, frame: Record<string, unknown>): void {
		const connection = this.connections.get(socket);
		if (!connection) return;
		switch (frame.type) {
			case 'hello':
				this.hellos.push(frame);
				socket.deliver({
					type: 'hello',
					protocol_version: 1,
					capabilities: ['presence'],
					client_id: 'server',
					session_id: connection.sessionId
				});
				return;
			case 'open':
				socket.deliver({
					type: 'snapshot',
					protocol_version: 1,
					document_id: DOCUMENT_ID,
					snapshot_seq: this.headSeq,
					head_seq: this.headSeq,
					snapshot: this.snapshotBase64(),
					tail_updates: [],
					head_frontier: this.frontier()
				});
				return;
			case 'presence':
				this.presenceSeen.push(frame);
				for (const [other, peer] of this.connections) {
					if (other === socket) continue;
					peer.socket.deliver({
						type: 'presence',
						protocol_version: 1,
						document_id: DOCUMENT_ID,
						session_id: connection.sessionId,
						payload: frame.payload,
						ttl_seconds: frame.ttl_seconds ?? 30,
						...(connection.userId ? { actor: { user_id: connection.userId } } : {})
					});
				}
				return;
			case 'update':
				this.onUpdate(connection, frame);
				return;
			case 'pong':
			case 'ack':
			default:
				return;
		}
	}

	private onUpdate(connection: Connection, frame: Record<string, unknown>): void {
		const updateId = String(frame.update_id);
		const seen: UpdateSeen = {
			updateId,
			idempotencyKey: frame.idempotency_key,
			baseFrontier: frame.base_frontier,
			bytes: fromBase64(String(frame.bytes)),
			connection: this.connectionIndex(connection)
		};
		this.updatesSeen.push(seen);
		const prior = this.byUpdateId.get(updateId);
		if (prior !== undefined) {
			this.replaysAnswered += 1;
			connection.socket.deliver(this.acceptedFrame(updateId, prior));
			return;
		}
		const verdict = this.policy(seen, this.updatesSeen.length - 1);
		if (verdict.kind === 'silence') return;
		if (verdict.kind === 'reject') {
			const spec = verdict.reject;
			connection.socket.deliver({
				type: 'rejected',
				protocol_version: 1,
				document_id: DOCUMENT_ID,
				update_id: spec.withoutUpdateId ? null : updateId,
				code: spec.code,
				recoverable: spec.recoverable ?? false,
				write_state: spec.writeState ?? 'not_applied',
				...(spec.details === undefined ? {} : { details: spec.details })
			});
			return;
		}
		// Probed on a fork, so a refused update leaves nothing pending in the real document.
		const probe = this.doc.fork();
		const status = probe.import(seen.bytes);
		probe.free();
		if (status.pending && status.pending.size > 0) {
			this.causalRefusals += 1;
			connection.socket.deliver({
				type: 'rejected',
				protocol_version: 1,
				document_id: DOCUMENT_ID,
				update_id: updateId,
				code: 'invalid_update',
				recoverable: false,
				write_state: 'not_applied',
				details: { reason: 'causal_dependency_missing' }
			});
			return;
		}
		const seq = this.commit(updateId, seen.bytes, connection.socket);
		if (verdict.kind === 'commit_then_drop') {
			connection.socket.serverClose(1006, '');
			return;
		}
		connection.socket.deliver(this.acceptedFrame(updateId, seq));
	}

	private commit(updateId: string, bytes: Uint8Array, from: MockWebSocket | null): number {
		const status = this.doc.import(bytes);
		if (status.pending && status.pending.size > 0) this.pendingAccepted += 1;
		this.headSeq += 1;
		const seq = this.headSeq;
		this.byUpdateId.set(updateId, seq);
		this.accepted.push({ seq, updateId, bytes });
		for (const [socket] of this.connections) {
			if (socket === from) continue;
			socket.deliver({
				type: 'update',
				protocol_version: 1,
				document_id: DOCUMENT_ID,
				update_id: updateId,
				base_frontier: '',
				bytes: toBase64(bytes),
				idempotency_key: null,
				origin: 'peer',
				message: null
			});
			socket.deliver(this.acceptedFrame(updateId, seq));
		}
		return seq;
	}

	private acceptedFrame(updateId: string, seq: number): Record<string, unknown> {
		return {
			type: 'accepted',
			protocol_version: 1,
			document_id: DOCUMENT_ID,
			update_id: updateId,
			head_seq: seq,
			head_frontier: `f${seq}`,
			projection_seq: seq,
			event_id: `evt-${seq}`
		};
	}

	private connectionIndex(connection: Connection): number {
		return Number(connection.sessionId.slice(-12));
	}

	/** The `fetch` stand-in for the REST reads around a session. */
	fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
		const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
		const path = url.replace(/^https?:\/\/[^/]+/, '').split('?')[0];
		const method = (init?.method ?? 'GET').toUpperCase();
		const ok = (data: unknown) =>
			new Response(JSON.stringify({ code: 0, message: 'ok', data }), {
				headers: { 'Content-Type': 'application/json' }
			});
		if (!this.online) throw new TypeError('Failed to fetch');
		if (method === 'POST' && path === '/api/v1/collab/tickets') {
			return ok({ websocket_url: '/api/v1/collab/ws?ticket=t', ticket: 't', expires_at: '' });
		}
		if (method === 'GET' && path === `/api/v1/flow/objects/${OBJECT_ID}/grants`) {
			return ok({
				items: [],
				inherit_from_parent: true,
				inherited: [],
				effective_level: this.effectiveLevel
			});
		}
		if (method === 'GET' && path === `/api/v1/flow/objects/${OBJECT_ID}/bootstrap`) {
			// The REST bootstrap admits `view` (`routes/flow.rs::get_flow_object_bootstrap`),
			// like the WebSocket session.
			if (this.effectiveLevel === 'none') {
				return new Response(
					JSON.stringify({
						code: 404,
						message: 'flow object not found',
						data: null,
						error_code: 'not_found'
					}),
					{ headers: { 'Content-Type': 'application/json' } }
				);
			}
			return ok({
				object_id: OBJECT_ID,
				document_id: DOCUMENT_ID,
				engine: 'loro',
				format_version: '1',
				snapshot_seq: this.headSeq,
				head_seq: this.headSeq,
				snapshot_base64: this.snapshotBase64(),
				tail_updates: [],
				head_frontier: this.frontier(),
				limits: this.bootstrapLimits ?? limitsPayload(),
				websocket_path: '/api/v1/collab/ws'
			});
		}
		if (method === 'GET' && path === `/api/v1/flow/objects/${OBJECT_ID}`) {
			return ok(objectView());
		}
		return new Response(JSON.stringify({ code: 404, message: 'not found', data: null }), {
			headers: { 'Content-Type': 'application/json' }
		});
	};
}

export function objectView(): Record<string, unknown> {
	return {
		id: OBJECT_ID,
		workspace_id: WORKSPACE_ID,
		project_id: null,
		object_type: 'page',
		lifecycle_status: 'active',
		governance_metadata: {},
		title: 'Fixture page',
		semantic_content: null,
		document_id: DOCUMENT_ID,
		document_seq: 0,
		frontier: '',
		projection_seq: 0,
		projection_lag: 0,
		created_at: '',
		updated_at: '',
		archived_at: null,
		parent_id: null
	};
}

/** Installs the fakes globally; returns a restore function. */
export async function installFakes(server: FakeCollabServer): Promise<() => void> {
	const realFetch = globalThis.fetch;
	const realWebSocket = globalThis.WebSocket;
	MockWebSocket.instances = [];
	server.install();
	globalThis.fetch = server.fetch as typeof globalThis.fetch;
	(globalThis as { WebSocket: unknown }).WebSocket = MockWebSocket;
	const { apiClient } = await import('../../src/lib/api/client');
	apiClient.setToken('test-access-token');
	return () => {
		globalThis.fetch = realFetch;
		(globalThis as { WebSocket: unknown }).WebSocket = realWebSocket;
		MockWebSocket.server = null;
	};
}

/** A fresh fake IndexedDB factory per suite (`fake-indexeddb`), installed as `indexedDB`. */
export async function freshIndexedDb(): Promise<IDBFactory> {
	const { IDBFactory } = await import('fake-indexeddb');
	const factory = new IDBFactory();
	(globalThis as { indexedDB: unknown }).indexedDB = factory;
	return factory;
}

/** Edits the client doc the way the editor would: one committed change = one local update. */
export function edit(doc: unknown, text: string): void {
	const loro = doc as LoroDoc;
	const target = loro.getText('body');
	target.insert(target.length, text);
	loro.commit();
}

export function textOf(doc: unknown): string {
	return (doc as LoroDoc).getText('body').toString();
}

/** The outbox database the suites open directly (one user's, `local-data.ts`). */
export const OUTBOX_DB = 'sylvode-flow-outbox-v1:fixture-user';
