// Presence: who else is on this document, and where their cursor/selection is
// (`versions/v0.5-collaboration.md` "Web 交互": "显示其他用户 cursor/selection 和 block presence，
// 但不把 presence 当锁").
//
// The wire is `collab-protocol-v1.md`'s `presence` frame (`document_id, ephemeral payload,
// ttl_seconds?`, the same shape both ways) and `limits-v1.md`'s presence rows: payload at most
// `presence_payload_bytes_max` (8 KiB) of cursor/selection/identity summary, `ttl_seconds` in
// `1..30` (30 when omitted), and "presence 永不进入 CRDT、history、event 或投递". This module holds
// no engine doc and imports nothing that could reach one: the map below is the whole of a
// document's presence state on the client, and it lives beside `ProjectionStore`, not inside it.
//
// `ObjectSession` owns sending and receiving (`ui-surface-v1.md` "ObjectSession：ticket/WS...
// presence"); this module is the pure part -- payload shape, byte budget, throttle and TTL --
// so it can be tested without a socket.

import { writable, type Readable } from 'svelte/store';

/** The frozen wire version of this client's presence payload. A peer payload with any other
 * `v` is ignored rather than half-read. */
export const PRESENCE_PAYLOAD_VERSION = 1;

/** `limits-v1.md`: "Presence `ttl_seconds` 省略时由服务端取 30，允许范围 `1..30`". */
export const PRESENCE_TTL_SECONDS_DEFAULT = 30;

/** At most four presence frames per second (the task's "节流 ≤ 4 次/s"), comfortably inside
 * `frames_per_connection_per_second` = 30 even while typing. */
export const PRESENCE_MIN_INTERVAL_MS = 250;

/** The local presence is re-sent this often while it is set, so the server-side entry (expiry
 * counted from the server's accept, `limits-v1.md`) never lapses for a peer who is merely idle. */
export const PRESENCE_REFRESH_MS = 10_000;

/** Who a presence entry belongs to. Summary only: an id, the display name and a colour. */
export interface PresenceIdentity {
	readonly userId: string;
	readonly name: string;
	readonly color: string;
}

/** A position inside one block: the block's stable id (the Loro container id of its node, the
 * same on every peer) and character offsets inside that block. `anchor === head` is a caret. */
export interface PresenceCursor {
	readonly blockId: string;
	readonly anchor: number;
	readonly head: number;
}

/** The `presence.payload` this client sends and accepts. */
export interface PresencePayload {
	readonly v: typeof PRESENCE_PAYLOAD_VERSION;
	readonly user: PresenceIdentity;
	readonly cursor: PresenceCursor | null;
}

/** One remote peer, as rendered. `sessionId` is the server's id for the peer's connection.
 * `user.userId` is the frame's server-attested `actor.user_id` (`collab-protocol-v1.md` "presence
 * 身份由服务端背书"); only a frame without `actor` (an older server) falls back to the payload's
 * self-reported id, and then `attested` is `false` and the UI never attributes the peer to that id.
 * `user.name` is sanitised plain text and `user.color` is always `#rrggbb`. */
export interface RemotePresence {
	readonly sessionId: string;
	readonly user: PresenceIdentity;
	readonly attested: boolean;
	readonly cursor: PresenceCursor | null;
	readonly expiresAt: number;
}

export function buildPresencePayload(
	user: PresenceIdentity,
	cursor: PresenceCursor | null
): PresencePayload {
	return { v: PRESENCE_PAYLOAD_VERSION, user, cursor };
}

/** The payload's size the way the server measures it: the UTF-8 bytes of its JSON encoding
 * (`session.rs` checks `serde_json::to_vec(&payload).len()` against the ceiling). */
export function presencePayloadBytes(payload: PresencePayload): number {
	return new TextEncoder().encode(JSON.stringify(payload)).length;
}

function isFiniteNonNegative(value: unknown): value is number {
	return typeof value === 'number' && Number.isFinite(value) && value >= 0;
}

/** `collab-protocol-v1.md`: a peer's display name is plain text of at most 64 characters. */
export const PRESENCE_NAME_MAX_CHARS = 64;

const PRESENCE_COLOR_PATTERN = /^#[0-9a-f]{6}$/i;

/** `collab-protocol-v1.md`: a peer's colour is only ever `#rrggbb`. */
export function isPresenceColor(value: unknown): value is string {
	return typeof value === 'string' && PRESENCE_COLOR_PATTERN.test(value);
}

/** The colour to render for a peer: its own when it is `#rrggbb`, the palette colour of
 * `userId` otherwise. Every place a peer colour reaches a style goes through this. */
export function safePresenceColor(color: unknown, userId: string): string {
	return isPresenceColor(color) ? color : presenceColor(userId);
}

/** A peer's display name as plain text: markup-like `<...>` runs, control and bidi/format
 * characters removed, whitespace collapsed, at most `PRESENCE_NAME_MAX_CHARS` characters. */
export function sanitizePresenceName(raw: string): string {
	const text = raw
		.replace(/<[^>]*>?/g, '')
		.replace(/[<>]/g, '')
		.replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, '')
		.replace(/\s+/g, ' ')
		.trim();
	return Array.from(text).slice(0, PRESENCE_NAME_MAX_CHARS).join('').trim();
}

/** The frame's server-attested actor id, or `null` when the frame carries none. */
export function presenceActorId(actor: unknown): string | null {
	if (typeof actor !== 'object' || actor === null) return null;
	const userId = (actor as { user_id?: unknown }).user_id;
	return typeof userId === 'string' && userId.length > 0 ? userId : null;
}

/** Reads a peer's `presence.payload`, or `null` for anything that is not this client's v1 shape.
 * A malformed peer payload is dropped, never partially rendered. The returned name is already
 * sanitised (a name with no text left drops the payload); the colour is returned as received and
 * must be rendered through `safePresenceColor`. */
export function parsePresencePayload(raw: unknown): PresencePayload | null {
	if (typeof raw !== 'object' || raw === null) return null;
	const value = raw as Record<string, unknown>;
	if (value.v !== PRESENCE_PAYLOAD_VERSION) return null;
	const user = value.user as Record<string, unknown> | null | undefined;
	if (
		typeof user !== 'object' ||
		user === null ||
		typeof user.userId !== 'string' ||
		typeof user.name !== 'string' ||
		typeof user.color !== 'string'
	) {
		return null;
	}
	const name = sanitizePresenceName(user.name);
	if (name.length === 0) return null;
	let cursor: PresenceCursor | null = null;
	if (value.cursor !== null && value.cursor !== undefined) {
		const c = value.cursor as Record<string, unknown>;
		if (
			typeof c !== 'object' ||
			typeof c.blockId !== 'string' ||
			!isFiniteNonNegative(c.anchor) ||
			!isFiniteNonNegative(c.head)
		) {
			return null;
		}
		cursor = { blockId: c.blockId, anchor: c.anchor, head: c.head };
	}
	return {
		v: PRESENCE_PAYLOAD_VERSION,
		user: { userId: user.userId, name, color: user.color },
		cursor
	};
}

/** A stable colour per user id, so a peer keeps the same colour on every screen. */
const PRESENCE_COLORS = [
	'#2563eb',
	'#db2777',
	'#16a34a',
	'#ea580c',
	'#7c3aed',
	'#0891b2',
	'#ca8a04',
	'#dc2626'
] as const;

export function presenceColor(userId: string): string {
	let hash = 0;
	for (let i = 0; i < userId.length; i += 1) hash = (hash * 31 + userId.charCodeAt(i)) >>> 0;
	return PRESENCE_COLORS[hash % PRESENCE_COLORS.length];
}

/** Short initials for a block-level avatar. */
export function presenceInitials(name: string): string {
	const parts = name.trim().split(/\s+/).filter(Boolean);
	if (parts.length === 0) return '?';
	const first = Array.from(parts[0])[0] ?? '?';
	const second = parts.length > 1 ? (Array.from(parts[parts.length - 1])[0] ?? '') : '';
	return `${first}${second}`.toUpperCase();
}

/** Clock seam for tests. */
export interface PresenceClock {
	now(): number;
}

const SYSTEM_CLOCK: PresenceClock = { now: () => Date.now() };

/**
 * The ephemeral map of remote peers on one document, keyed by the server's session id
 * (`limits-v1.md`: "Entry key 固定为 `(document_id,session_id)`：同 key frame 只刷新
 * payload/expiry，不增加计数").
 *
 * Expiry is local and absolute: an entry whose `ttl_seconds` (30 when the frame omits it) passes
 * without a refresh disappears on the next read or sweep, so a peer whose tab crashed stops
 * showing a cursor within the same 30 s the server keeps it.
 */
export class RemotePresenceMap {
	private readonly entries = new Map<string, RemotePresence>();
	private readonly store = writable<ReadonlyArray<RemotePresence>>([]);
	private readonly clock: PresenceClock;

	constructor(clock: PresenceClock = SYSTEM_CLOCK) {
		this.clock = clock;
	}

	get peers(): Readable<ReadonlyArray<RemotePresence>> {
		return { subscribe: this.store.subscribe };
	}

	/** Upserts one peer from a received frame (`actor` is the frame's server-attested
	 * `actor`). Returns whether the frame was usable. */
	receive(
		sessionId: string,
		payload: unknown,
		ttlSeconds: number | null | undefined,
		actor?: unknown
	): boolean {
		const parsed = parsePresencePayload(payload);
		if (!parsed) return false;
		const actorId = presenceActorId(actor);
		const userId = actorId ?? parsed.user.userId;
		const ttl =
			typeof ttlSeconds === 'number' && Number.isFinite(ttlSeconds) && ttlSeconds > 0
				? Math.min(ttlSeconds, PRESENCE_TTL_SECONDS_DEFAULT)
				: PRESENCE_TTL_SECONDS_DEFAULT;
		this.entries.set(sessionId, {
			sessionId,
			user: {
				userId,
				name: parsed.user.name,
				color: safePresenceColor(parsed.user.color, userId)
			},
			attested: actorId !== null,
			cursor: parsed.cursor,
			expiresAt: this.clock.now() + ttl * 1000
		});
		this.publish();
		return true;
	}

	/** Drops every entry whose TTL has passed. Returns how many were removed. */
	sweep(): number {
		const now = this.clock.now();
		let removed = 0;
		for (const [id, entry] of this.entries) {
			if (entry.expiresAt <= now) {
				this.entries.delete(id);
				removed += 1;
			}
		}
		if (removed > 0) this.publish();
		return removed;
	}

	/** Current, unexpired peers. */
	list(): ReadonlyArray<RemotePresence> {
		this.sweep();
		return [...this.entries.values()];
	}

	get size(): number {
		return this.list().length;
	}

	clear(): void {
		if (this.entries.size === 0) return;
		this.entries.clear();
		this.publish();
	}

	private publish(): void {
		this.store.set([...this.entries.values()]);
	}
}

/**
 * Leading-and-trailing throttle for outgoing presence: the first change goes out at once, later
 * changes inside the window collapse into one trailing send of the latest value. Never more than
 * one send per `intervalMs`.
 */
export class PresenceThrottle<T> {
	private lastSentAt = Number.NEGATIVE_INFINITY;
	private pendingValue: { value: T } | null = null;
	private timer: ReturnType<typeof setTimeout> | null = null;
	private readonly intervalMs: number;
	private readonly send: (value: T) => void;
	private readonly clock: PresenceClock;

	constructor(
		send: (value: T) => void,
		intervalMs = PRESENCE_MIN_INTERVAL_MS,
		clock: PresenceClock = SYSTEM_CLOCK
	) {
		this.send = send;
		this.intervalMs = intervalMs;
		this.clock = clock;
	}

	push(value: T): void {
		const now = this.clock.now();
		const wait = this.lastSentAt + this.intervalMs - now;
		if (wait <= 0 && !this.timer) {
			this.lastSentAt = now;
			this.send(value);
			return;
		}
		this.pendingValue = { value };
		if (!this.timer) {
			this.timer = setTimeout(() => this.flush(), Math.max(wait, 0));
		}
	}

	/** Sends the trailing value now (used by the timer and by tests). */
	flush(): void {
		if (this.timer) {
			clearTimeout(this.timer);
			this.timer = null;
		}
		const pending = this.pendingValue;
		this.pendingValue = null;
		if (!pending) return;
		this.lastSentAt = this.clock.now();
		this.send(pending.value);
	}

	cancel(): void {
		if (this.timer) clearTimeout(this.timer);
		this.timer = null;
		this.pendingValue = null;
	}
}
