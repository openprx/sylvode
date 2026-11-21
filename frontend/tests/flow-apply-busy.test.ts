/**
 * Retryable "server busy" (`apply_busy`) on the Flow write path, against the real repository,
 * session and outbox over `fake-indexeddb` and the fake collaboration server, whose `applyBusy`
 * verdict produces the answer exactly as the wire contract specifies:
 *
 *   rejected{code:"server_draining", recoverable:true, write_state:"not_applied", update_id,
 *            details:{reason:"apply_busy", retry_after_ms, retry_mode:"resend"|"rebase", cause}}
 *
 * - `resend`: the same update id again after the delay, at most 5 sends;
 * - `rebase`: synchronise to the head, re-encode the unsent edits as ONE new sequential update
 *   under a new id, at most 3 rebases per edit;
 * - after a cap the entry takes the permanent-refusal path (recovery draft, banner) with the
 *   final-failure text; a content refusal stays permanent at once; the session is silent while
 *   the outbox retries.
 *
 * Run standalone: `bun tests/flow-apply-busy.test.ts`
 */

import { LoroDoc } from 'loro-crdt';
import { get } from 'svelte/store';
import { Suite, assert, assertDeepEqual, assertEqual, finish, waitFor } from './support/harness';
import {
	FakeCollabServer,
	FakeConnectivity,
	OBJECT_ID,
	OUTBOX_DB,
	WORKSPACE_ID,
	applyBusy,
	edit,
	freshIndexedDb,
	installFakes,
	objectView,
	textOf,
	type PolicyVerdict
} from './support/collab-fakes';
import { IndexedDbOutboxStore, type OutboxEntry } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';
import type { FlowError } from '../src/lib/flow/types';
import { classifySubmitError } from '../src/lib/flow/offline-sync';
import { flowErrorI18nKey } from '../src/lib/flow/errors';
import en from '../src/lib/i18n/en.json';
import zh from '../src/lib/i18n/zh.json';

const suite = new Suite('flow_apply_busy');

const ACCEPT: PolicyVerdict = { kind: 'accept' };
const BUSY_KEY = 'flow.error.server_draining.apply_busy';

async function open(server: FakeCollabServer, factory?: IDBFactory) {
	const idb = factory ?? (await freshIndexedDb());
	const restore = await installFakes(server);
	const store = await IndexedDbOutboxStore.open(idb, OUTBOX_DB);
	assert(store, 'fake IndexedDB opens');
	const connectivity = new FakeConnectivity();
	const { FlowObjectRepository } = await import('../src/lib/flow/object-repository');
	const repository = new FlowObjectRepository({
		outboxStore: store,
		session: { connectivity, answerTimeoutMs: 1500 },
		offlineSync: { sleep: (ms) => new Promise((r) => setTimeout(r, Math.min(ms, 20))) }
	});
	const entry = await repository.open({
		scopeId: WORKSPACE_ID,
		objectId: OBJECT_ID,
		signal: new AbortController().signal,
		object: objectView() as unknown as FlowObjectView
	});
	const states: string[] = [];
	entry.session.state.subscribe((s) => states.push(s));
	let pending = 0;
	entry.pending.subscribe((n) => (pending = n));
	const errors: Array<FlowError | null> = [];
	entry.flowError.subscribe((e) => errors.push(e));
	return {
		idb,
		store,
		connectivity,
		repository,
		entry,
		states,
		errors,
		pending: () => pending,
		lastError: () => errors.at(-1) ?? null,
		async close() {
			await repository.close(OBJECT_ID);
			restore();
		}
	};
}

type Ctx = Awaited<ReturnType<typeof open>>;

/** JSON of a doc without the empty root containers a read creates (they hold no operation). */
function stateOf(doc: unknown): string {
	const json = (doc as LoroDoc).toJSON() as Record<string, unknown>;
	const sorted = (v: unknown): unknown => {
		if (Array.isArray(v)) return v.map(sorted);
		if (v && typeof v === 'object') {
			const r = v as Record<string, unknown>;
			return Object.fromEntries(
				Object.keys(r)
					.sort()
					.map((k) => [k, sorted(r[k])])
			);
		}
		return v;
	};
	const out: Record<string, unknown> = {};
	for (const [k, v] of Object.entries(sorted(json) as Record<string, unknown>)) {
		const empty =
			v === '' ||
			(Array.isArray(v) && v.length === 0) ||
			(v && typeof v === 'object' && !Array.isArray(v) && Object.keys(v).length === 0);
		if (!empty) out[k] = v;
	}
	return JSON.stringify(out);
}

function serverText(server: FakeCollabServer): string {
	return server.doc.getText('body').toString();
}

/** A peer (not the client) commits `text` at the start of the body on the server. */
function peerCommit(server: FakeCollabServer, text: string): void {
	const peer = new LoroDoc();
	peer.import(server.doc.export({ mode: 'snapshot' }));
	peer.setPeerId('7777');
	const before = peer.oplogVersion();
	peer.getText('body').insert(0, text);
	peer.commit();
	server.commitFromPeer(peer.export({ mode: 'update', from: before }));
}

function occurrences(haystack: string, needle: string): number {
	return haystack.split(needle).length - 1;
}

function rawOutbox(idb: IDBFactory): Promise<OutboxEntry[]> {
	return new Promise((resolve, reject) => {
		const request = idb.open(OUTBOX_DB, 1);
		request.onsuccess = () => {
			const tx = request.result.transaction('outbox', 'readonly');
			const all = tx.objectStore('outbox').getAll();
			all.onsuccess = () => {
				request.result.close();
				resolve(all.result as OutboxEntry[]);
			};
			all.onerror = () => reject(all.error);
		};
		request.onerror = () => reject(request.error);
	});
}

function entryOf(over: Partial<OutboxEntry>): OutboxEntry {
	return {
		updateId: 'u',
		idempotencyKey: 'u',
		objectId: OBJECT_ID,
		documentId: 'd',
		order: 1,
		bytes: new Uint8Array([1]),
		baseFrontier: 'f0',
		intent: { kind: 'content_edit' },
		enqueuedAt: 0,
		peerId: '1',
		...over
	};
}

// 1. resend success
await suite.checkAsync(
	'resend: two busy answers, then accepted under the same update id',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = (_seen, i) => (i < 2 ? applyBusy('resend') : ACCEPT);
			edit(ctx.entry.doc, 'resend-me');
			await waitFor(() => server.accepted.length === 1, 'accepted after two busy answers');
			await ctx.entry.offline.idle();
			assertEqual(server.updatesSeen.length, 3, 'three sends');
			assert(
				server.updatesSeen.every((s) => s.updateId === server.updatesSeen[0].updateId),
				'all sends share one update_id'
			);
			assert(
				server.updatesSeen.every((s) => s.idempotencyKey === s.updateId),
				'and one idempotency key'
			);
			assert(
				ctx.errors.every((e) => e === null),
				'no banner'
			);
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'nothing drafted');
			assertEqual(stateOf(ctx.entry.doc), stateOf(server.doc), 'client doc == server doc');
			assertEqual(ctx.pending(), 0, 'outbox drained');
		} finally {
			await ctx.close();
		}
	}
);

// 2. resend cap
await suite.checkAsync(
	'resend cap: always busy gives exactly 5 sends, then the recovery draft',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = () => applyBusy('resend');
			edit(ctx.entry.doc, 'never-saved');
			await waitFor(() => ctx.entry.offline.draftEntries.length === 1, 'drafted after the cap');
			await ctx.entry.offline.idle();
			assertEqual(server.updatesSeen.length, 5, 'exactly 5 sends');
			assert(
				server.updatesSeen.every((s) => s.updateId === server.updatesSeen[0].updateId),
				'all with the same id'
			);
			const draft = ctx.entry.offline.draftEntries[0];
			assertEqual(draft.updateId, server.updatesSeen[0].updateId, 'the entry is in the draft');
			const error = ctx.lastError();
			assert(error !== null, 'the final failure is reported');
			assertEqual(flowErrorI18nKey(error), BUSY_KEY, 'with the apply_busy text key');
			assertEqual(error.recoverable, false, 'as a permanent refusal');
			assertEqual(ctx.pending(), 0, 'nothing pending');
			assertEqual(serverText(server), '', 'nothing committed');
			assertEqual(textOf(ctx.entry.doc), '', 'the live doc is rebuilt from the accepted state');
			assert(ctx.states.at(-1) !== 'saving', 'the indicator leaves saving');
		} finally {
			await ctx.close();
		}
	}
);

// 3. rebase success, state equality
await suite.checkAsync(
	'rebase: a peer commit lands first; one new sequential update is accepted',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = (seen, i) => {
				if (i === 0) peerCommit(server, 'PEER|');
				return server.isSequential(seen.bytes) ? ACCEPT : applyBusy('rebase');
			};
			edit(ctx.entry.doc, 'mine');
			await waitFor(() => server.accepted.length === 2, 'peer commit and rebased update accepted');
			await ctx.entry.offline.idle();
			assertEqual(server.updatesSeen.length, 2, 'the old update, then one rebased update');
			const [first, second] = server.updatesSeen;
			assert(first.updateId !== second.updateId, 'the rebased update has a new id');
			assert(
				!server.accepted.some((a) => a.updateId === first.updateId),
				'the old id is never accepted'
			);
			assertEqual(second.idempotencyKey, second.updateId, 'new idempotency key = new id');
			assertEqual(occurrences(serverText(server), 'PEER|'), 1, 'the peer text appears once');
			assertEqual(occurrences(serverText(server), 'mine'), 1, 'our text appears once');
			assertEqual(serverText(server), textOf(ctx.entry.doc), 'texts equal on both sides');
			assertEqual(stateOf(ctx.entry.doc), stateOf(server.doc), 'client doc == server doc');
			assertEqual(server.pendingAccepted, 0, 'no pending operations on the server');
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'nothing drafted');
			assert(
				ctx.errors.every((e) => e === null),
				'no banner'
			);
			assertEqual(ctx.pending(), 0, 'outbox drained');
			assertEqual(get(ctx.entry.docGeneration), 1, 'the editor rebound once (doc swap)');
		} finally {
			await ctx.close();
		}
	}
);

// 4. another client commits again in between
await suite.checkAsync(
	'rebase: the head moves once more, a second rebase is accepted',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = (seen, i) => {
				if (i === 0) peerCommit(server, 'ONE|');
				if (i === 1) peerCommit(server, 'TWO|');
				return server.isSequential(seen.bytes) ? ACCEPT : applyBusy('rebase');
			};
			edit(ctx.entry.doc, 'mine');
			await waitFor(() => server.accepted.length === 3, 'two peer commits and our update');
			await ctx.entry.offline.idle();
			assertEqual(server.updatesSeen.length, 3, 'original plus two rebased sends');
			assertEqual(new Set(server.updatesSeen.map((s) => s.updateId)).size, 3, 'three distinct ids');
			assertEqual(occurrences(serverText(server), 'mine'), 1, 'our text once');
			assertEqual(occurrences(serverText(server), 'ONE|'), 1, 'first peer text once');
			assertEqual(occurrences(serverText(server), 'TWO|'), 1, 'second peer text once');
			assertEqual(stateOf(ctx.entry.doc), stateOf(server.doc), 'client doc == server doc');
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'nothing drafted');
		} finally {
			await ctx.close();
		}
	}
);

// 5. rebase cap
await suite.checkAsync(
	'rebase cap: three rebases, then the recovery draft and the final text',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = () => applyBusy('rebase');
			edit(ctx.entry.doc, 'too-contended');
			await waitFor(() => ctx.entry.offline.draftEntries.length === 1, 'drafted after the cap');
			await ctx.entry.offline.idle();
			assertEqual(server.updatesSeen.length, 4, 'the original send plus three rebased sends');
			assertEqual(
				new Set(server.updatesSeen.map((s) => s.updateId)).size,
				4,
				'each under a new id'
			);
			const error = ctx.lastError();
			assert(error !== null, 'banner reported');
			assertEqual(flowErrorI18nKey(error), BUSY_KEY, 'with the apply_busy text key');
			assertEqual(ctx.pending(), 0, 'nothing pending');
			assertEqual(serverText(server), '', 'nothing committed');
			assert(
				occurrences(textOf(ctx.entry.doc), 'too-contended') === 0,
				'the live doc no longer holds the drafted edit'
			);
			assertEqual((await rawOutbox(ctx.idb)).length, 0, 'the outbox store is empty');
		} finally {
			await ctx.close();
		}
	}
);

// 6. a permanent content error stays permanent
await suite.checkAsync('a content refusal is permanent at once: one send, no retry', async () => {
	const server = new FakeCollabServer();
	const ctx = await open(server);
	try {
		server.policy = () => ({
			kind: 'reject',
			reject: {
				code: 'limit_exceeded',
				details: { limit_kind: 'decode_apply_cpu_ms', limit: 50, observed: 51 }
			}
		});
		edit(ctx.entry.doc, 'heavy');
		await waitFor(() => ctx.entry.offline.draftEntries.length === 1, 'drafted at once');
		await ctx.entry.offline.idle();
		assertEqual(server.updatesSeen.length, 1, 'sent once');
		const error = ctx.lastError();
		assert(error !== null, 'reported');
		assertEqual(error.code, 'limit_exceeded', 'with the content code');
		assertEqual(flowErrorI18nKey(error), 'flow.error.limit_exceeded', 'and its own text');
		assertEqual(ctx.entry.offline.draftEntries[0].reason.code, 'limit_exceeded', 'draft reason');
	} finally {
		await ctx.close();
	}
});

// 7a. mixed queue: resend keeps the order
await suite.checkAsync(
	'mixed queue: a busy head is resent before the next entry goes out',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			ctx.connectivity.goOffline();
			await waitFor(() => ctx.states.at(-1) === 'offline', 'offline');
			edit(ctx.entry.doc, 'one|');
			edit(ctx.entry.doc, 'two|');
			edit(ctx.entry.doc, 'three|');
			await ctx.entry.offline.idle();
			const ids = ctx.entry.offline.pendingEntries.map((e) => e.updateId);
			assertEqual(ids.length, 3, 'three queued');
			let busyLeft = 1;
			server.policy = (seen) => {
				if (seen.updateId === ids[0] && busyLeft > 0) {
					busyLeft -= 1;
					return applyBusy('resend');
				}
				return ACCEPT;
			};
			ctx.connectivity.goOnline();
			await waitFor(() => server.accepted.length === 3, 'all three accepted');
			await ctx.entry.offline.idle();
			assertDeepEqual(
				server.updatesSeen.map((s) => s.updateId),
				[ids[0], ids[0], ids[1], ids[2]],
				'the head is resent first; later entries never overtake it'
			);
			assertDeepEqual(
				server.accepted.map((a) => a.updateId),
				ids,
				'accepted in enqueue order'
			);
			assertEqual(serverText(server), 'one|two|three|', 'text in order');
		} finally {
			await ctx.close();
		}
	}
);

// 7b. mixed queue: a rebase folds the queue into one update
await suite.checkAsync(
	'mixed queue: a rebase folds the unsent edits into one new update, then later edits follow',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			ctx.connectivity.goOffline();
			await waitFor(() => ctx.states.at(-1) === 'offline', 'offline');
			edit(ctx.entry.doc, 'one|');
			edit(ctx.entry.doc, 'two|');
			edit(ctx.entry.doc, 'three|');
			await ctx.entry.offline.idle();
			const oldIds = ctx.entry.offline.pendingEntries.map((e) => e.updateId);
			assertEqual(oldIds.length, 3, 'three queued');
			peerCommit(server, 'PEER|'); // not connected: no fan-out, the head just moves
			server.policy = (seen) => (server.isSequential(seen.bytes) ? ACCEPT : applyBusy('rebase'));
			ctx.connectivity.goOnline();
			await waitFor(() => server.accepted.length === 2, 'peer commit and one folded update');
			await ctx.entry.offline.idle();
			assertEqual(server.updatesSeen.length, 2, 'the old head, then the folded update');
			assertEqual(server.updatesSeen[0].updateId, oldIds[0], 'first send was the old head');
			assert(!oldIds.includes(server.updatesSeen[1].updateId), 'the folded update has a new id');
			assert(
				!server.updatesSeen.some((s) => s.updateId === oldIds[1] || s.updateId === oldIds[2]),
				'the later old entries were never sent'
			);
			const text = serverText(server);
			for (const part of ['one|', 'two|', 'three|', 'PEER|'])
				assertEqual(occurrences(text, part), 1, `${part} appears once`);
			assertEqual(stateOf(ctx.entry.doc), stateOf(server.doc), 'client doc == server doc');
			// A later edit, made on the rebased doc, is sent after it.
			edit(ctx.entry.doc, 'four|');
			await waitFor(() => server.accepted.length === 3, 'a later edit is accepted');
			await ctx.entry.offline.idle();
			assertEqual(occurrences(serverText(server), 'four|'), 1, 'later edit once');
			assertEqual(stateOf(ctx.entry.doc), stateOf(server.doc), 'still equal');
			assertEqual((await rawOutbox(ctx.idb)).length, 0, 'outbox store empty');
		} finally {
			await ctx.close();
		}
	}
);

// 8. IndexedDB atomic replacement
await suite.checkAsync(
	'IndexedDB: replace swaps entries in one transaction, all or nothing',
	async () => {
		const idb = await freshIndexedDb();
		const store = await IndexedDbOutboxStore.open(idb, OUTBOX_DB);
		assert(store, 'opens');
		await store.put(entryOf({ updateId: 'a', idempotencyKey: 'a', order: 1 }));
		await store.put(entryOf({ updateId: 'b', idempotencyKey: 'b', order: 2 }));
		// A write that cannot be stored (no key) must leave the old entries alone.
		let failed = false;
		try {
			await store.replace(['a', 'b'], entryOf({ updateId: undefined as unknown as string }));
		} catch {
			failed = true;
		}
		assert(failed, 'the invalid replacement is refused');
		assertDeepEqual(
			(await rawOutbox(idb)).map((e) => e.updateId).sort(),
			['a', 'b'],
			'a failed replace deletes nothing'
		);
		await store.replace(['a', 'b'], entryOf({ updateId: 'c', idempotencyKey: 'c', order: 1 }));
		assertDeepEqual(
			(await rawOutbox(idb)).map((e) => e.updateId),
			['c'],
			'a successful replace leaves exactly the new entry'
		);
		// Through the repository: while the rebased update is unanswered the store holds only it.
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = (_seen, i) => (i === 0 ? applyBusy('rebase') : { kind: 'silence' });
			edit(ctx.entry.doc, 'x');
			await waitFor(() => server.updatesSeen.length === 2, 'rebased update sent');
			const rows = await rawOutbox(ctx.idb);
			assertEqual(rows.length, 1, 'one row after the rebase');
			assertEqual(rows[0].updateId, server.updatesSeen[1].updateId, 'it is the rebased entry');
			assert(rows[0].updateId !== server.updatesSeen[0].updateId, 'the old entry is gone');
		} finally {
			await ctx.close();
		}
	}
);

// 9. session silence
await suite.checkAsync(
	'session: apply_busy raises no error while retrying; contention behaves as before',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = (seen, i) => {
				if (i === 0) return applyBusy('resend');
				if (i === 1) return applyBusy('rebase');
				return server.isSequential(seen.bytes) ? ACCEPT : applyBusy('rebase');
			};
			edit(ctx.entry.doc, 'quiet');
			await waitFor(() => server.accepted.length === 1, 'accepted');
			await ctx.entry.offline.idle();
			assert(
				ctx.errors.every((e) => e === null),
				'onFlowError never fired'
			);
			assert(!ctx.states.includes('error'), 'never in the error state');
			assert(!ctx.states.includes('reconnecting'), 'the connection was kept');
			assertEqual(server.hellos.length, 1, 'no reconnect');
		} finally {
			await ctx.close();
		}
		const other = new FakeCollabServer();
		const ctx2 = await open(other);
		try {
			other.policy = (_seen, i) =>
				i === 0
					? {
							kind: 'reject',
							reject: {
								code: 'server_draining',
								recoverable: true,
								details: { reason: 'contention', retry_after_ms: 10 }
							}
						}
					: ACCEPT;
			edit(ctx2.entry.doc, 'contended');
			await waitFor(() => other.accepted.length === 1, 'accepted after contention');
			const shown = ctx2.errors.find((e) => e !== null);
			assert(shown, 'contention still reports');
			assertEqual(flowErrorI18nKey(shown), 'flow.error.server_draining.contention', 'as before');
		} finally {
			await ctx2.close();
		}
	}
);

// 10. final-failure text
await suite.checkAsync(
	'final failure text is in both locales and is what the banner key resolves to',
	async () => {
		const flow = (en as { flow: { error: { server_draining: Record<string, string> } } }).flow
			.error;
		const flowZh = (zh as { flow: { error: { server_draining: Record<string, string> } } }).flow
			.error;
		assertEqual(
			flow.server_draining.apply_busy,
			'The server is too busy to save this change right now. It has been kept in the recovery draft. Try again in a moment.',
			'en text'
		);
		assertEqual(
			flowZh.server_draining.apply_busy,
			'服务器繁忙，暂时无法保存此修改。修改已保留在恢复草稿中，请稍后再试。',
			'zh text'
		);
		const error: FlowError = {
			code: 'server_draining',
			recoverable: false,
			origin: 'server',
			details: { reason: 'apply_busy', final: true, retry_mode: 'resend' }
		};
		assertEqual(flowErrorI18nKey(error), BUSY_KEY, 'key');
	}
);

// extra: unknown or missing retry_mode fails closed
await suite.checkAsync(
	'apply_busy without a usable retry_mode is permanent and reported',
	async () => {
		for (const details of [
			{ reason: 'apply_busy', retry_after_ms: 0, cause: 'x' },
			{ reason: 'apply_busy', retry_after_ms: 0, retry_mode: 'bogus', cause: 'x' }
		]) {
			const server = new FakeCollabServer();
			const ctx = await open(server);
			try {
				server.policy = () => ({
					kind: 'reject',
					reject: { code: 'server_draining', recoverable: true, details }
				});
				edit(ctx.entry.doc, 'x');
				await waitFor(() => ctx.entry.offline.draftEntries.length === 1, 'drafted');
				await ctx.entry.offline.idle();
				assertEqual(server.updatesSeen.length, 1, 'one send, no retry');
				const shown = ctx.lastError();
				assert(shown !== null, 'reported');
				assertEqual(flowErrorI18nKey(shown), 'flow.error.unsupported_protocol', 'fails closed');
			} finally {
				await ctx.close();
			}
		}
	}
);

// extra: reload after a rebase
await suite.checkAsync(
	'reload after a rebase: a new repository sends only the rebased entry',
	async () => {
		const idb = await freshIndexedDb();
		const server = new FakeCollabServer();
		const first = await open(server, idb);
		let rebasedId = '';
		let oldId = '';
		try {
			server.policy = (_seen, i) => (i === 0 ? applyBusy('rebase') : { kind: 'silence' });
			edit(first.entry.doc, 'survives');
			await waitFor(() => server.updatesSeen.length === 2, 'rebased update sent');
			oldId = server.updatesSeen[0].updateId;
			rebasedId = server.updatesSeen[1].updateId;
		} finally {
			await first.close();
		}
		assertDeepEqual(
			(await rawOutbox(idb)).map((e) => e.updateId),
			[rebasedId],
			'only the rebased entry is stored'
		);
		const server2 = new FakeCollabServer();
		const second = await open(server2, idb);
		try {
			server2.policy = () => ACCEPT;
			await waitFor(() => server2.accepted.length === 1, 'replayed');
			await second.entry.offline.idle();
			assertDeepEqual(
				server2.updatesSeen.map((s) => s.updateId),
				[rebasedId],
				'exactly the rebased entry is sent'
			);
			assert(rebasedId !== oldId, 'under its new id');
			assertEqual(serverText(server2), 'survives', 'content committed once');
		} finally {
			await second.close();
		}
	}
);

// extra: edits typed while the bootstrap is in flight
await suite.checkAsync(
	'edits typed during the rebase bootstrap are in the rebased update exactly once',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = (seen, i) => {
				if (i === 0) peerCommit(server, 'PEER|');
				return server.isSequential(seen.bytes) ? ACCEPT : applyBusy('rebase');
			};
			const realFetch = globalThis.fetch;
			let typed = false;
			globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
				const url =
					typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
				const response = await realFetch(input, init);
				if (!typed && url.includes('/bootstrap') && server.updatesSeen.length > 0) {
					typed = true;
					edit(ctx.entry.doc, 'late|');
				}
				return response;
			}) as typeof globalThis.fetch;
			try {
				edit(ctx.entry.doc, 'early|');
				await waitFor(() => server.accepted.length === 2, 'rebased update accepted');
				await ctx.entry.offline.idle();
			} finally {
				globalThis.fetch = realFetch;
			}
			assert(typed, 'the late edit was typed during the bootstrap');
			assertEqual(occurrences(serverText(server), 'early|'), 1, 'early once');
			assertEqual(occurrences(serverText(server), 'late|'), 1, 'late once');
			assertEqual(stateOf(ctx.entry.doc), stateOf(server.doc), 'client doc == server doc');
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'nothing drafted');
		} finally {
			await ctx.close();
		}
	}
);

suite.check(
	'classifySubmitError: busy modes, bounded; unknown mode and other reasons unchanged',
	() => {
		const busy = (details: unknown): FlowError => ({
			code: 'server_draining',
			recoverable: true,
			origin: 'server',
			details
		});
		assertDeepEqual(
			classifySubmitError(
				busy({
					reason: 'apply_busy',
					retry_mode: 'resend',
					retry_after_ms: 250,
					cause: 'cpu_starved'
				})
			),
			{ kind: 'busy_resend', retryAfterMs: 250 },
			'resend'
		);
		assertDeepEqual(
			classifySubmitError(busy({ reason: 'apply_busy', retry_mode: 'rebase', retry_after_ms: 0 })),
			{ kind: 'busy_rebase', retryAfterMs: 0 },
			'rebase'
		);
		assertDeepEqual(
			classifySubmitError(busy({ reason: 'apply_busy', retry_after_ms: 0 })),
			{ kind: 'permanent' },
			'missing mode'
		);
		assertDeepEqual(
			classifySubmitError(busy({ reason: 'apply_busy', retry_mode: 'x' })),
			{ kind: 'permanent' },
			'unknown mode'
		);
		assertDeepEqual(
			classifySubmitError(busy({ reason: 'contention', retry_after_ms: 10 })),
			{ kind: 'retry', delayMs: 250 },
			'contention unchanged'
		);
		assertDeepEqual(
			classifySubmitError(busy({ reason: 'drain' })),
			{ kind: 'retry', delayMs: 250 },
			'drain unchanged'
		);
	}
);

export const result = suite.result();

if (import.meta.main) finish(result);
