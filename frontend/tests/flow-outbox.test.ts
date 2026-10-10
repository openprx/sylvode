/**
 * The IndexedDB outbox (`ui-surface-v1.md` "ObjectRepository：...IndexedDB accepted cache/本地
 * outbox"; `versions/v0.5-collaboration.md`: "IndexedDB outbox 同时保存 update bytes、base
 * frontier、idempotency key 和可重放 semantic intent").
 *
 * Checks, each against the real repository + session over `fake-indexeddb` and the fake server:
 * - an entry persists all four fields (read back raw from IndexedDB, not from memory);
 * - offline edits accumulate, the indicator shows `offline` with the pending count, nothing is sent;
 * - reconnect replays in enqueue order under the SAME update_id / idempotency key;
 * - a refused intent lands in the recovery draft (IndexedDB too), never silently dropped;
 * - an update over `update_bytes_max` is not enqueued and is reported as `limit_exceeded`;
 * - entries persisted by an earlier page load are replayed by the next one;
 * - a rate refusal (answered by its `update_id`) is retried under the same key;
 * - a `rejected` without `update_id` answers no update and is never attributed to the one in flight;
 * - a server `causal_dependency_missing` refusal keeps the intent in the draft with its reason.
 *
 * Run standalone: `bun tests/flow-outbox.test.ts`
 */

import { Suite, assert, assertDeepEqual, assertEqual, finish, waitFor } from './support/harness';
import {
	FakeCollabServer,
	FakeConnectivity,
	OBJECT_ID,
	OUTBOX_DB,
	WORKSPACE_ID,
	edit,
	freshIndexedDb,
	installFakes,
	objectView,
	textOf
} from './support/collab-fakes';
import { IndexedDbOutboxStore, type OutboxEntry } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';
import type { FlowError } from '../src/lib/flow/types';

const suite = new Suite('flow_outbox');

async function open(server: FakeCollabServer, factory?: IDBFactory) {
	const idb = factory ?? (await freshIndexedDb());
	const restore = await installFakes(server);
	const store = await IndexedDbOutboxStore.open(idb);
	assert(store, 'fake IndexedDB opens');
	const connectivity = new FakeConnectivity();
	const { FlowObjectRepository } = await import('../src/lib/flow/object-repository');
	const repository = new FlowObjectRepository({
		outboxStore: store,
		session: { connectivity, answerTimeoutMs: 1500 },
		offlineSync: { sleep: (ms) => new Promise((r) => setTimeout(r, Math.min(ms, 20))) }
	});
	const entry = await repository.open({
		workspaceId: WORKSPACE_ID,
		objectId: OBJECT_ID,
		signal: new AbortController().signal,
		object: objectView() as unknown as FlowObjectView
	});
	const states: string[] = [];
	entry.session.state.subscribe((s) => states.push(s));
	let pending = 0;
	entry.pending.subscribe((n) => (pending = n));
	let lastError: FlowError | null = null;
	entry.flowError.subscribe((e) => (lastError = e));
	return {
		idb,
		store,
		connectivity,
		repository,
		entry,
		states,
		pending: () => pending,
		lastError: () => lastError,
		async close() {
			await repository.close(OBJECT_ID);
			restore();
		}
	};
}

/** Reads the outbox object store directly, bypassing the store class under test. */
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

function rawDrafts(
	idb: IDBFactory
): Promise<Array<{ updateId: string; reason: { code: string } }>> {
	return new Promise((resolve, reject) => {
		const request = idb.open(OUTBOX_DB, 1);
		request.onsuccess = () => {
			const tx = request.result.transaction('drafts', 'readonly');
			const all = tx.objectStore('drafts').getAll();
			all.onsuccess = () => {
				request.result.close();
				resolve(all.result as Array<{ updateId: string; reason: { code: string } }>);
			};
			all.onerror = () => reject(all.error);
		};
		request.onerror = () => reject(request.error);
	});
}

await suite.checkAsync(
	'an entry persists bytes, base frontier, idempotency key and intent',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			ctx.connectivity.goOffline();
			await waitFor(() => ctx.states.at(-1) === 'offline', 'offline');
			edit(ctx.entry.doc, 'persist me');
			await waitFor(() => ctx.pending() === 1, 'one pending');
			const rows = await rawOutbox(ctx.idb);
			assertEqual(rows.length, 1, 'one row in IndexedDB');
			const row = rows[0];
			assert(row.bytes instanceof Uint8Array && row.bytes.length > 0, 'update bytes stored');
			assertEqual(
				row.baseFrontier,
				'f0',
				'base frontier = the accepted head when the edit was made'
			);
			assert(typeof row.updateId === 'string' && row.updateId.length > 0, 'update id stored');
			assertEqual(row.idempotencyKey, row.updateId, 'idempotency key stored (= update id)');
			assertDeepEqual(row.intent, { kind: 'content_edit' }, 'replayable semantic intent stored');
			assertEqual(row.objectId, OBJECT_ID, 'owner object');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'offline edits accumulate (indicator offline + count), replay in order with the same keys',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			ctx.connectivity.goOffline();
			await waitFor(() => ctx.states.at(-1) === 'offline', 'offline');
			for (const text of ['a', 'b', 'c', 'd']) edit(ctx.entry.doc, text);
			await waitFor(() => ctx.pending() === 4, 'four pending');
			assertEqual(ctx.states.at(-1), 'offline', 'still offline');
			assertEqual(server.updatesSeen.length, 0, 'nothing sent offline');
			const stored = (await ctx.store.list(OBJECT_ID)).map((e) => e.updateId);
			ctx.connectivity.goOnline();
			await waitFor(() => ctx.pending() === 0, 'drained', 5000);
			assertDeepEqual(
				server.updatesSeen.map((s) => s.updateId),
				stored,
				'the server saw exactly the stored ids, in enqueue order'
			);
			assertDeepEqual(
				server.updatesSeen.map((s) => s.idempotencyKey),
				stored,
				'and the stored idempotency keys'
			);
			assertEqual((await rawOutbox(ctx.idb)).length, 0, 'accepted entries left IndexedDB');
			assertEqual(textOf(ctx.entry.doc), 'abcd', 'content intact');
			assertEqual(server.doc.getText('body').toString(), 'abcd', 'server has all four');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'a refused intent goes to the recovery draft (IndexedDB), never dropped',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.policy = () => ({
				kind: 'reject',
				reject: { code: 'server_rejected', details: { reason: 'deterministic_database_refusal' } }
			});
			edit(ctx.entry.doc, 'refused');
			await waitFor(() => ctx.entry.offline.draftEntries.length === 1, 'drafted');
			await ctx.entry.offline.idle();
			const drafts = await rawDrafts(ctx.idb);
			assertEqual(drafts.length, 1, 'the draft is persisted');
			assertEqual(drafts[0].reason.code, 'server_rejected', 'with the server reason');
			assertEqual((await rawOutbox(ctx.idb)).length, 0, 'and left the outbox');
			assertEqual(ctx.pending(), 0, 'nothing pending');
			assertEqual(textOf(ctx.entry.doc), '', 'the canvas was rebuilt from the accepted state');
			const blob = await ctx.repository.exportRecoveryDraft(OBJECT_ID);
			const json = JSON.parse(await blob.text()) as {
				entries: Array<{ update_id: string; bytes_base64: string; reason: { code: string } }>;
			};
			assertEqual(json.entries.length, 1, 'downloadable');
			assertEqual(json.entries[0].update_id, drafts[0].updateId, 'same intent');
			assert(json.entries[0].bytes_base64.length > 0, 'with its raw bytes');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'an over-limit update is not enqueued and reports limit_exceeded',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			const { negotiateFlowLimitsVersion } = await import('../src/lib/flow/limits');
			const { limitsPayload } = await import('./support/collab-fakes');
			ctx.entry.session.adoptLimits(
				negotiateFlowLimitsVersion(limitsPayload({ update_bytes_max: 8 }))
			);
			edit(ctx.entry.doc, 'this update is far larger than eight bytes');
			await waitFor(() => ctx.lastError() !== null, 'an error was reported');
			await ctx.entry.offline.idle();
			const error = ctx.lastError() as FlowError | null;
			assertEqual(error?.code, 'limit_exceeded', 'limit_exceeded at the edit');
			assertDeepEqual(
				(error?.details as { limit_kind: string }).limit_kind,
				'update_bytes',
				'the limit kind'
			);
			assertEqual((await rawOutbox(ctx.idb)).length, 0, 'never entered the outbox');
			assertEqual(server.updatesSeen.length, 0, 'never sent');
			assertEqual(
				ctx.entry.offline.draftEntries[0]?.reason.source,
				'precheck',
				'kept in the draft'
			);
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'entries left by an earlier page load are replayed by the next one',
	async () => {
		const server = new FakeCollabServer();
		const first = await open(server);
		const idb = first.idb;
		first.connectivity.goOffline();
		await waitFor(() => first.states.at(-1) === 'offline', 'offline');
		edit(first.entry.doc, 'survives a reload');
		await waitFor(() => first.pending() === 1, 'pending');
		const [stored] = await first.store.list(OBJECT_ID);
		await first.close();
		const second = await open(server, idb);
		try {
			await waitFor(() => second.pending() === 0 && server.accepted.length === 1, 'replayed', 5000);
			assertEqual(server.accepted[0].updateId, stored.updateId, 'under the original id');
			assertEqual(textOf(second.entry.doc), 'survives a reload', 'and visible in the new doc');
		} finally {
			await second.close();
		}
	}
);

await suite.checkAsync(
	'a rate refusal answered by its update_id is retried under the same key',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			let refusals = 0;
			server.policy = () => {
				if (refusals > 0) return { kind: 'accept' };
				refusals += 1;
				return {
					kind: 'reject',
					reject: {
						code: 'limit_exceeded',
						recoverable: true,
						details: { limit_kind: 'update_rate', limit: 10, retry_after_ms: 100 }
					}
				};
			};
			edit(ctx.entry.doc, 'rate');
			await waitFor(
				() => server.accepted.length === 1 && ctx.pending() === 0,
				'accepted after retry',
				5000
			);
			assertEqual(server.updatesSeen.length, 2, 'sent twice');
			assertEqual(server.updatesSeen[0].updateId, server.updatesSeen[1].updateId, 'same update id');
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'not drafted');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'a rejected frame without update_id is not attributed to the update in flight',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			let refusals = 0;
			server.policy = () => {
				if (refusals > 0) return { kind: 'accept' };
				refusals += 1;
				return {
					kind: 'reject',
					reject: {
						code: 'limit_exceeded',
						recoverable: true,
						withoutUpdateId: true,
						details: { limit_kind: 'update_rate', limit: 10, retry_after_ms: 50 }
					}
				};
			};
			edit(ctx.entry.doc, 'unattributed');
			await waitFor(() => server.updatesSeen.length === 1, 'sent once');
			await new Promise((r) => setTimeout(r, 400));
			assertEqual(
				server.updatesSeen.length,
				1,
				'not resent on the strength of a refusal that named no update'
			);
			assertEqual(ctx.pending(), 1, 'still waiting for its own answer');
			// The answer never comes on this connection; the liveness timeout replays the same id
			// on the next one, where the server answers it.
			await waitFor(
				() => server.accepted.length === 1 && ctx.pending() === 0,
				'accepted after the liveness replay',
				6000
			);
			assertEqual(server.updatesSeen[0].updateId, server.updatesSeen[1].updateId, 'same update id');
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'not drafted');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'a causal_dependency_missing refusal keeps the intent in the draft with its reason',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			// Operations the server never received enter the client doc (an import is not a local
			// update, so the outbox does not send them); the next local edit depends on them.
			const { LoroDoc } = await import('loro-crdt');
			const stranger = new LoroDoc();
			stranger.getText('body').insert(0, 'unseen.');
			stranger.commit();
			(ctx.entry.doc as unknown as InstanceType<typeof LoroDoc>).import(
				stranger.export({ mode: 'update' })
			);
			edit(ctx.entry.doc, 'depends.');
			await waitFor(() => ctx.entry.offline.draftEntries.length === 1, 'drafted');
			await ctx.entry.offline.idle();
			assertEqual(server.causalRefusals, 1, 'the server refused it as causally incomplete');
			assertEqual(server.accepted.length, 0, 'nothing got a seq');
			assertEqual(server.pendingAccepted, 0, 'nothing pending was stored');
			const [draft] = ctx.entry.offline.draftEntries;
			assertEqual(draft.reason.code, 'invalid_update', 'draft code');
			assertEqual(draft.reason.source, 'server', 'server refusal');
			assertDeepEqual(
				draft.reason.details,
				{ reason: 'causal_dependency_missing' },
				'the named reason is kept'
			);
			const { flowErrorDetailKey } = await import('../src/lib/flow/errors');
			const error = ctx.lastError() as FlowError | null;
			assert(error, 'reported');
			assertEqual(
				flowErrorDetailKey(error as FlowError),
				'flow.error.invalid_update_reason.causal_dependency_missing',
				'reported with the causal reason key'
			);
			assertEqual(server.updatesSeen.length, 1, 'not retried');
		} finally {
			await ctx.close();
		}
	}
);

export const result = suite.result();

if (import.meta.main) finish(result);
