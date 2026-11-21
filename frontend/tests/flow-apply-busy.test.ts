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
	limitsPayload,
	objectView,
	textOf,
	tick,
	type PolicyVerdict
} from './support/collab-fakes';
import {
	IndexedDbOutboxStore,
	type OutboxEntry,
	type OutboxStore,
	type RecoveryDraftEntry
} from '../src/lib/flow/outbox';
import { negotiateFlowLimitsVersion } from '../src/lib/flow/limits';
import type { FlowObjectView } from '../src/lib/api/flow';
import type { FlowError } from '../src/lib/flow/types';
import {
	CONTENTION_SENDS_MAX,
	REBASE_MAX,
	classifySubmitError
} from '../src/lib/flow/offline-sync';
import { flowErrorDetailKey, flowErrorI18nKey } from '../src/lib/flow/errors';
import en from '../src/lib/i18n/en.json';
import zh from '../src/lib/i18n/zh.json';

const suite = new Suite('flow_apply_busy');

const ACCEPT: PolicyVerdict = { kind: 'accept' };
const BUSY_KEY = 'flow.error.server_draining.apply_busy';

async function open(
	server: FakeCollabServer,
	factory?: IDBFactory,
	wrap?: (store: OutboxStore) => OutboxStore
) {
	const idb = factory ?? (await freshIndexedDb());
	const restore = await installFakes(server);
	const opened = await IndexedDbOutboxStore.open(idb, OUTBOX_DB);
	assert(opened, 'fake IndexedDB opens');
	const store = wrap ? wrap(opened) : opened;
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
			// The first rebase uses the session head; the second one fetches the head over REST,
			// which is the window this test types into.
			server.policy = (seen, i) => {
				if (i === 0) peerCommit(server, 'PEER|');
				if (i === 1) peerCommit(server, 'PEER2|');
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
				await waitFor(
					() =>
						occurrences(serverText(server), 'early|') === 1 &&
						occurrences(serverText(server), 'late|') === 1,
					'rebased update accepted'
				);
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

// ---- round 8 client findings -------------------------------------------------------------------

const SMALL_UPDATE_LIMIT = 300;
const OVER_LIMIT_MARK = `XMARK${'-'.repeat(700)}`;

function limitUpdates(ctx: Ctx, server: FakeCollabServer): void {
	server.bootstrapLimits = limitsPayload({ update_bytes_max: SMALL_UPDATE_LIMIT });
	ctx.entry.session.adoptLimits(
		negotiateFlowLimitsVersion(limitsPayload({ update_bytes_max: SMALL_UPDATE_LIMIT }))
	);
}

/**
 * How often `marker` is held only by the recovery draft. A draft's bytes can depend on updates the
 * server refused (a rebase re-encodes those under a new peer), so the draft is merged on top of
 * the server state plus every refused update the server saw; what the drafts add beyond that base
 * is what only the draft holds.
 */
function draftOnlyCount(
	server: FakeCollabServer,
	drafts: readonly RecoveryDraftEntry[],
	marker: string
): number {
	const skip = new Set([...server.accepted, ...drafts].map((e) => e.updateId));
	const base = new LoroDoc();
	base.import(server.doc.export({ mode: 'snapshot' }));
	for (const seen of server.updatesSeen) {
		if (!skip.has(seen.updateId)) base.import(seen.bytes);
	}
	const without = occurrences(base.getText('body').toString(), marker);
	for (const draft of [...drafts].sort((a, b) => a.order - b.order)) base.import(draft.bytes);
	const withDrafts = occurrences(base.getText('body').toString(), marker);
	base.free();
	return withDrafts - without;
}

/** Every edit typed ends in exactly one place: the server state or the recovery draft, once. */
async function assertConserved(
	ctx: Ctx,
	server: FakeCollabServer,
	markers: string[],
	label: string
) {
	await ctx.entry.offline.idle();
	const drafts = ctx.entry.offline.draftEntries;
	for (const marker of markers) {
		assertEqual(
			occurrences(serverText(server), marker) + draftOnlyCount(server, drafts, marker),
			1,
			`${label}: ${marker.slice(0, 8)} is on the server or only in the draft, exactly once`
		);
	}
	assertEqual(ctx.entry.offline.pendingEntries.length, 0, `${label}: nothing left queued`);
	assertEqual((await rawOutbox(ctx.idb)).length, 0, `${label}: nothing left in the outbox store`);
	assertEqual(server.pendingAccepted, 0, `${label}: no pending operations on the server`);
}

const SEQUENTIAL_OR_REBASE = (server: FakeCollabServer, peer?: string) =>
	((seen, i) => {
		if (i === 0 && peer) peerCommit(server, peer);
		return server.isSequential(seen.bytes) ? ACCEPT : applyBusy('rebase');
	}) as FakeCollabServer['policy'];

// B-2
await suite.checkAsync(
	'B-2: a queued edit that depends on a drafted edit is drafted by the rebase, not deleted',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			limitUpdates(ctx, server);
			server.policy = SEQUENTIAL_OR_REBASE(server, 'PEER|');
			edit(ctx.entry.doc, 'A|');
			edit(ctx.entry.doc, OVER_LIMIT_MARK);
			edit(ctx.entry.doc, 'B|');
			await waitFor(
				() => server.accepted.some((a) => a.updateId !== 'x') && ctx.pending() === 0,
				'settled'
			);
			await ctx.entry.offline.idle();
			assertEqual(occurrences(serverText(server), 'A|'), 1, 'A reached the server once');
			assertEqual(occurrences(serverText(server), 'B|'), 0, 'B cannot be on the server');
			const drafted = ctx.entry.offline.draftEntries;
			assertEqual(drafted.length, 2, 'the over-limit edit and its dependent are both drafted');
			assert(
				drafted.some((d) => d.reason.source === 'dependency'),
				'the dependent carries the dependency reason'
			);
			await assertConserved(ctx, server, ['A|', 'B|', OVER_LIMIT_MARK], 'B-2');
		} finally {
			await ctx.close();
		}
	}
);

// B-2 invariant: conservation over queue shapes
const SHAPES: Array<{
	name: string;
	run: (ctx: Ctx, server: FakeCollabServer) => Promise<string[]>;
}> = [
	{
		name: 'several dependent entries, a peer commit, a folded rebase',
		run: async (ctx, server) => {
			ctx.connectivity.goOffline();
			await waitFor(() => ctx.states.at(-1) === 'offline', 'offline');
			for (const part of ['s1|', 's2|', 's3|']) edit(ctx.entry.doc, part);
			await ctx.entry.offline.idle();
			peerCommit(server, 'PEER|');
			server.policy = SEQUENTIAL_OR_REBASE(server);
			ctx.connectivity.goOnline();
			await waitFor(() => server.accepted.length >= 2, 'folded update accepted');
			return ['s1|', 's2|', 's3|'];
		}
	},
	{
		name: 'an over-limit edit while an update is in flight, nothing after it',
		run: async (ctx, server) => {
			limitUpdates(ctx, server);
			server.policy = SEQUENTIAL_OR_REBASE(server, 'PEER|');
			edit(ctx.entry.doc, 'f1|');
			edit(ctx.entry.doc, OVER_LIMIT_MARK);
			await waitFor(() => ctx.entry.offline.draftEntries.length >= 1, 'drafted');
			return ['f1|', OVER_LIMIT_MARK];
		}
	},
	{
		name: 'one refused permanently while another is in flight, with a chain behind it',
		run: async (ctx, server) => {
			limitUpdates(ctx, server);
			server.policy = SEQUENTIAL_OR_REBASE(server, 'PEER|');
			edit(ctx.entry.doc, 'g1|');
			edit(ctx.entry.doc, OVER_LIMIT_MARK);
			edit(ctx.entry.doc, 'g2|');
			edit(ctx.entry.doc, 'g3|');
			await waitFor(() => ctx.entry.offline.draftEntries.length >= 3, 'chain drafted');
			return ['g1|', 'g2|', 'g3|', OVER_LIMIT_MARK];
		}
	},
	{
		name: 'the rebased update is then refused permanently',
		run: async (ctx, server) => {
			ctx.connectivity.goOffline();
			await waitFor(() => ctx.states.at(-1) === 'offline', 'offline');
			for (const part of ['r1|', 'r2|']) edit(ctx.entry.doc, part);
			await ctx.entry.offline.idle();
			peerCommit(server, 'PEER|');
			server.policy = (seen, i) =>
				i === 0
					? applyBusy('rebase')
					: {
							kind: 'reject',
							reject: { code: 'invalid_update', details: { reason: 'invalid_field_value' } }
						};
			ctx.connectivity.goOnline();
			await waitFor(() => ctx.entry.offline.draftEntries.length >= 1, 'drafted');
			return ['r1|', 'r2|'];
		}
	}
];
for (const shape of SHAPES) {
	await suite.checkAsync(`conservation of edits: ${shape.name}`, async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			const markers = await shape.run(ctx, server);
			await assertConserved(ctx, server, markers, shape.name);
		} finally {
			await ctx.close();
		}
	});
}

// B-3
await suite.checkAsync(
	'B-3: an uncommitted editor transaction at rebase time is saved once, no banner, no draft',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = (seen, i) => {
				if (i === 0) {
					peerCommit(server, 'PEER|');
					const body = (ctx.entry.doc as unknown as LoroDoc).getText('body');
					body.insert(body.length, 'late|'); // no commit: the transaction stays open
				}
				return server.isSequential(seen.bytes) ? ACCEPT : applyBusy('rebase');
			};
			edit(ctx.entry.doc, 'early|');
			await waitFor(() => server.accepted.length >= 2, 'rebased update accepted');
			await ctx.entry.offline.idle();
			await tick(50);
			await ctx.entry.offline.idle();
			assertEqual(occurrences(serverText(server), 'early|'), 1, 'early once');
			assertEqual(occurrences(serverText(server), 'late|'), 1, 'late once');
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'no recovery draft');
			assert(
				ctx.errors.every((e) => e === null),
				'no banner'
			);
			assertEqual(server.causalRefusals, 0, 'the server refused nothing');
			assertEqual((await rawOutbox(ctx.idb)).length, 0, 'outbox store empty');
			assertEqual(stateOf(ctx.entry.doc), stateOf(server.doc), 'client doc == server doc');
		} finally {
			await ctx.close();
		}
	}
);

// B-4
class FailingReplaceStore implements OutboxStore {
	failures: number;
	onFail: () => void = () => undefined;
	constructor(
		private readonly inner: OutboxStore,
		failures: number
	) {
		this.failures = failures;
	}
	list = (objectId: string) => this.inner.list(objectId);
	put = (entry: OutboxEntry) => this.inner.put(entry);
	delete = (updateId: string) => this.inner.delete(updateId);
	replace: OutboxStore['replace'] = async (oldIds, entry, drafts) => {
		if (this.failures > 0) {
			this.failures -= 1;
			this.onFail();
			throw new Error('simulated IndexedDB failure');
		}
		return this.inner.replace(oldIds, entry, drafts);
	};
	listDrafts = (objectId: string) => this.inner.listDrafts(objectId);
	moveToDraft = (draft: RecoveryDraftEntry) => this.inner.moveToDraft(draft);
	deleteDrafts = (ids: readonly string[]) => this.inner.deleteDrafts(ids);
	nextOrder = () => this.inner.nextOrder();
}

await suite.checkAsync(
	'B-4: a failed IndexedDB replacement keeps the old entries; the rebase is retried, no duplicate after a reload',
	async () => {
		const idb = await freshIndexedDb();
		const server = new FakeCollabServer();
		let failing: FailingReplaceStore | null = null;
		const first = await open(server, idb, (inner) => (failing = new FailingReplaceStore(inner, 1)));
		let oldId = '';
		let atFailure: { memory: string[]; rows: string[] } | null = null;
		try {
			assert(failing, 'wrapped');
			(failing as FailingReplaceStore).onFail = () => {
				atFailure = {
					memory: first.entry.offline.pendingEntries.map((e) => e.updateId),
					rows: []
				};
				void rawOutbox(idb).then((rows) => {
					if (atFailure) atFailure.rows = rows.map((r) => r.updateId);
				});
			};
			server.policy = (seen, i) => {
				if (i === 0) {
					oldId = seen.updateId;
					peerCommit(server, 'PEER|');
				}
				return server.isSequential(seen.bytes) ? ACCEPT : applyBusy('rebase');
			};
			edit(first.entry.doc, 'mine');
			await waitFor(() => server.accepted.length >= 2, 'accepted after the retried rebase');
			await first.entry.offline.idle();
			assert(atFailure !== null, 'the replacement failed once');
			assertDeepEqual(
				(atFailure as { memory: string[] }).memory,
				[oldId],
				'memory kept the old entry when the replacement failed'
			);
			assertDeepEqual(
				(atFailure as { rows: string[] }).rows,
				[oldId],
				'storage kept the old entry when the replacement failed'
			);
			assertEqual(occurrences(serverText(server), 'mine'), 1, 'saved once');
			assertEqual((await rawOutbox(idb)).length, 0, 'store empty after the accept');
			assertEqual(first.entry.offline.draftEntries.length, 0, 'nothing drafted');
			assertEqual(stateOf(first.entry.doc), stateOf(server.doc), 'client doc == server doc');
		} finally {
			await first.close();
		}
		const second = await open(server, idb);
		try {
			server.policy = () => ACCEPT;
			await tick(100);
			await second.entry.offline.idle();
			assertEqual(occurrences(serverText(server), 'mine'), 1, 'still once after a reload');
		} finally {
			await second.close();
		}
	}
);

await suite.checkAsync(
	'B-4: a store that always fails ends in the recovery draft, never in both old and rebased entries',
	async () => {
		const idb = await freshIndexedDb();
		const server = new FakeCollabServer();
		const first = await open(server, idb, (inner) => new FailingReplaceStore(inner, 100));
		try {
			server.policy = SEQUENTIAL_OR_REBASE(server, 'PEER|');
			edit(first.entry.doc, 'mine');
			await waitFor(() => first.entry.offline.draftEntries.length === 1, 'drafted after the cap');
			await first.entry.offline.idle();
			assertEqual(occurrences(serverText(server), 'mine'), 0, 'the rebased copy was never sent');
			assert(server.updatesSeen.length <= REBASE_MAX + 2, 'bounded');
			assertEqual(first.entry.offline.pendingEntries.length, 0, 'nothing queued');
			assertEqual(
				(await rawOutbox(idb)).length,
				0,
				'the old entry left the store (into the draft)'
			);
		} finally {
			await first.close();
		}
		const second = await open(server, idb);
		try {
			await tick(100);
			await second.entry.offline.idle();
			assertEqual(occurrences(serverText(server), 'mine'), 0, 'a reload sends nothing');
			assertEqual(second.entry.offline.draftEntries.length, 1, 'the draft survived the reload');
		} finally {
			await second.close();
		}
	}
);

// B-5
await suite.checkAsync(
	'B-5: a rebase that loses text marks is caught, reported as its own error, edits drafted',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		const proto = LoroDoc.prototype as unknown as {
			applyDiff: (...args: unknown[]) => unknown;
		};
		const realApply = proto.applyDiff;
		try {
			proto.applyDiff = function (this: LoroDoc, ...args: unknown[]) {
				const result = realApply.apply(this, args);
				const body = this.getText('body');
				if (body.length > 0) body.unmark({ start: 0, end: body.length }, 'bold');
				return result;
			};
			server.policy = SEQUENTIAL_OR_REBASE(server, 'PEER|');
			const doc = ctx.entry.doc as unknown as LoroDoc;
			doc.configTextStyle({ bold: { expand: 'after' } });
			const body = doc.getText('body');
			body.insert(0, 'bold-me');
			body.mark({ start: 0, end: 4 }, 'bold', true);
			doc.commit();
			await waitFor(() => ctx.entry.offline.draftEntries.length === 1, 'drafted');
			await ctx.entry.offline.idle();
			const error = ctx.lastError();
			assert(error !== null, 'reported');
			assertEqual(
				flowErrorDetailKey(error),
				'flow.error.invalid_update_reason.rebase_state_mismatch',
				'its own text, not the server busy text'
			);
			assert(flowErrorI18nKey(error) !== BUSY_KEY, 'not the busy key');
			assertEqual(ctx.entry.offline.draftEntries[0].reason.code, 'invalid_update', 'draft reason');
			assertEqual(occurrences(serverText(server), 'bold-me'), 0, 'stripped marks were never sent');
		} finally {
			proto.applyDiff = realApply;
			await ctx.close();
		}
	}
);

// B-6
const CONTENTION: PolicyVerdict = {
	kind: 'reject',
	reject: {
		code: 'server_draining',
		recoverable: true,
		details: { reason: 'contention', retry_after_ms: 10 }
	}
};

await suite.checkAsync(
	'B-6: contention is retried at most CONTENTION_SENDS_MAX times, then sending pauses (nothing drafted)',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = () => CONTENTION;
			edit(ctx.entry.doc, 'parked');
			await waitFor(() => server.updatesSeen.length === CONTENTION_SENDS_MAX, 'the cap is reached');
			await ctx.entry.offline.idle();
			await tick(150);
			assertEqual(server.updatesSeen.length, CONTENTION_SENDS_MAX, 'no send beyond the cap');
			assert(
				server.updatesSeen.every((s) => s.updateId === server.updatesSeen[0].updateId),
				'same update id throughout'
			);
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'not drafted');
			assertEqual(ctx.entry.offline.pendingEntries.length, 1, 'kept in the queue');
			assertEqual((await rawOutbox(ctx.idb)).length, 1, 'and in the store');
			assert(ctx.states.at(-1) !== 'saving', 'the indicator left saving');
			const shown = ctx.lastError();
			assert(shown !== null, 'the existing retry text is shown');
			assertEqual(
				flowErrorI18nKey(shown),
				'flow.error.server_draining.contention',
				'contention text'
			);
			// A later successful sync resumes sending the same entry.
			server.policy = () => ACCEPT;
			ctx.connectivity.goOffline();
			await waitFor(() => ctx.states.at(-1) === 'offline', 'offline');
			ctx.connectivity.goOnline();
			await waitFor(() => server.accepted.length === 1, 'resumed after the reconnect');
			await ctx.entry.offline.idle();
			assertEqual(occurrences(serverText(server), 'parked'), 1, 'saved once');
			assertEqual(ctx.entry.offline.pendingEntries.length, 0, 'queue drained');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'B-6: a new local edit also resumes a paused queue, with a fresh budget',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = () => CONTENTION;
			edit(ctx.entry.doc, 'one|');
			await waitFor(() => server.updatesSeen.length === CONTENTION_SENDS_MAX, 'paused');
			await ctx.entry.offline.idle();
			server.policy = () => ACCEPT;
			edit(ctx.entry.doc, 'two|');
			await waitFor(() => server.accepted.length === 2, 'both accepted');
			await ctx.entry.offline.idle();
			assertEqual(serverText(server), 'one|two|', 'in order');
		} finally {
			await ctx.close();
		}
	}
);

// B-8
await suite.checkAsync(
	'B-8: the first rebase uses the session head (no REST bootstrap); a second one fetches it',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		const realFetch = globalThis.fetch;
		let bootstraps = 0;
		try {
			server.policy = SEQUENTIAL_OR_REBASE(server, 'PEER|');
			globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
				const url =
					typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
				if (url.includes('/bootstrap')) bootstraps += 1;
				return realFetch(input, init);
			}) as typeof globalThis.fetch;
			edit(ctx.entry.doc, 'mine');
			await waitFor(() => server.accepted.length === 2, 'rebased update accepted');
			await ctx.entry.offline.idle();
			assertEqual(bootstraps, 0, 'no REST bootstrap for the first rebase');
			assertEqual(occurrences(serverText(server), 'mine'), 1, 'saved once');
			assertEqual(stateOf(ctx.entry.doc), stateOf(server.doc), 'client doc == server doc');
		} finally {
			globalThis.fetch = realFetch;
			await ctx.close();
		}
		const other = new FakeCollabServer();
		const ctx2 = await open(other);
		const realFetch2 = globalThis.fetch;
		let bootstraps2 = 0;
		try {
			other.policy = (seen, i) => {
				if (i === 0) peerCommit(other, 'ONE|');
				if (i === 1) peerCommit(other, 'TWO|');
				return other.isSequential(seen.bytes) ? ACCEPT : applyBusy('rebase');
			};
			globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
				const url =
					typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
				if (url.includes('/bootstrap')) bootstraps2 += 1;
				return realFetch2(input, init);
			}) as typeof globalThis.fetch;
			edit(ctx2.entry.doc, 'mine');
			await waitFor(() => other.accepted.length === 3, 'accepted after the second rebase');
			await ctx2.entry.offline.idle();
			assertEqual(bootstraps2, 1, 'the second rebase fetches the head once');
			assertEqual(stateOf(ctx2.entry.doc), stateOf(other.doc), 'client doc == server doc');
		} finally {
			globalThis.fetch = realFetch2;
			await ctx2.close();
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
