/**
 * `server_rejected` "连续 3 次" (`collab-protocol-v1.md` 2026-09-01 frozen counting rule,
 * `error-mapping-v1.md` `server_rejected` row: "保持连接；连续 3 次关闭 4500"):
 *
 *   "计数只由 `update` 帧驱动——每一个被答复的 `update` 帧，答成 `server_rejected` 则 +1，答成任何其它
 *    结果（accepted / `invalid_update` / 其它 code）则归零；`presence` / `ack` / `ping` / `open` 等非
 *    update 帧完全不参与"
 *
 * The real session against the fake server: three consecutive refusals close the connection
 * with 4500 and park the session in `error` with no automatic reconnect; an accepted or any other
 * answer in between resets the run; a ping, a presence frame or a `rejected` that answers no
 * update do not. A server-side 4500 close is honoured the same way. The intents refused on the way
 * are in the recovery draft.
 *
 * Run standalone: `bun tests/flow-server-rejected-streak.test.ts`
 */

import { Suite, assert, assertEqual, finish, waitFor } from './support/harness';
import {
	FakeCollabServer,
	FakeConnectivity,
	MockWebSocket,
	OBJECT_ID,
	WORKSPACE_ID,
	edit,
	freshIndexedDb,
	installFakes,
	objectView,
	tick,
	OUTBOX_DB
} from './support/collab-fakes';
import { IndexedDbOutboxStore } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';

const suite = new Suite('flow_server_rejected_streak');

const SERVER_REJECTED = {
	kind: 'reject' as const,
	reject: { code: 'server_rejected', details: { reason: 'deterministic_database_refusal' } }
};

async function open(server: FakeCollabServer) {
	const idb = await freshIndexedDb();
	const restore = await installFakes(server);
	const store = await IndexedDbOutboxStore.open(idb, OUTBOX_DB);
	assert(store, 'fake IndexedDB opens');
	const { FlowObjectRepository } = await import('../src/lib/flow/object-repository');
	const repository = new FlowObjectRepository({
		outboxStore: store,
		session: { connectivity: new FakeConnectivity(), answerTimeoutMs: 1500 },
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
	return {
		entry,
		states,
		socketsOpened: () => MockWebSocket.instances.length,
		currentSocket: () => MockWebSocket.instances[MockWebSocket.instances.length - 1],
		async close() {
			await repository.close(OBJECT_ID);
			restore();
		}
	};
}

/** One edit, waiting until the server answered it. */
async function editAndAwait(
	ctx: Awaited<ReturnType<typeof open>>,
	server: FakeCollabServer,
	text: string
): Promise<void> {
	const before = server.updatesSeen.length;
	edit(ctx.entry.doc, text);
	await waitFor(() => server.updatesSeen.length > before, `update for ${text} reached the server`);
	await tick(10);
	await ctx.entry.offline.idle();
}

await suite.checkAsync(
	'three consecutive server_rejected: close 4500, error, no reconnect',
	async () => {
		const server = new FakeCollabServer();
		server.policy = () => SERVER_REJECTED;
		const ctx = await open(server);
		try {
			const socket = ctx.currentSocket();
			await editAndAwait(ctx, server, 'a');
			assertEqual(ctx.entry.session.consecutiveServerRejections, 1, 'first refusal counted');
			assertEqual(socket.closedByClient, null, 'one refusal keeps the connection');
			await editAndAwait(ctx, server, 'b');
			assertEqual(ctx.entry.session.consecutiveServerRejections, 2, 'second refusal counted');
			assertEqual(socket.closedByClient, null, 'two refusals keep the connection');
			await editAndAwait(ctx, server, 'c');
			await waitFor(() => socket.closedByClient !== null, 'the client closed the socket');
			assertEqual(socket.closedByClient?.code, 4500, 'closed with 4500');
			assertEqual(ctx.states.at(-1), 'error', 'error state');
			assertEqual(ctx.entry.session.isTerminal, true, 'terminal');
			const opened = ctx.socketsOpened();
			await tick(700);
			assertEqual(ctx.socketsOpened(), opened, 'no automatic reconnect');
			await ctx.entry.session.reconnect('network');
			assertEqual(ctx.socketsOpened(), opened, 'not even on request');
			assertEqual(
				ctx.entry.offline.draftEntries.length,
				3,
				'the three refused intents are in the draft'
			);
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync('an accepted answer in between resets the run', async () => {
	const server = new FakeCollabServer();
	const verdicts = [
		SERVER_REJECTED,
		SERVER_REJECTED,
		{ kind: 'accept' as const },
		SERVER_REJECTED,
		SERVER_REJECTED
	];
	server.policy = (_seen, index) => verdicts[index] ?? { kind: 'accept' };
	const ctx = await open(server);
	try {
		const socket = ctx.currentSocket();
		// The first two refusals draft their intents and rebuild the doc; each edit after that is
		// a fresh, independent update.
		for (const text of ['a', 'b', 'c', 'd', 'e']) await editAndAwait(ctx, server, text);
		assertEqual(server.updatesSeen.length, 5, 'five updates answered');
		assertEqual(ctx.entry.session.consecutiveServerRejections, 2, 'run restarted after the accept');
		assertEqual(socket.closedByClient, null, 'R R A R R does not close');
		assertEqual(ctx.entry.session.isTerminal, false, 'not terminal');
	} finally {
		await ctx.close();
	}
});

await suite.checkAsync('another rejection code in between resets the run', async () => {
	const server = new FakeCollabServer();
	const verdicts = [
		SERVER_REJECTED,
		SERVER_REJECTED,
		{ kind: 'reject' as const, reject: { code: 'invalid_update' } },
		SERVER_REJECTED,
		SERVER_REJECTED
	];
	server.policy = (_seen, index) => verdicts[index] ?? { kind: 'accept' };
	const ctx = await open(server);
	try {
		const socket = ctx.currentSocket();
		for (const text of ['a', 'b', 'c', 'd', 'e']) await editAndAwait(ctx, server, text);
		assertEqual(ctx.entry.session.consecutiveServerRejections, 2, 'invalid_update reset the run');
		assertEqual(socket.closedByClient, null, 'R R I R R does not close');
	} finally {
		await ctx.close();
	}
});

await suite.checkAsync(
	'ping, presence and an update-less rejected do not touch the run',
	async () => {
		const server = new FakeCollabServer();
		server.policy = () => SERVER_REJECTED;
		const ctx = await open(server);
		try {
			const socket = ctx.currentSocket();
			await editAndAwait(ctx, server, 'a');
			await editAndAwait(ctx, server, 'b');
			assertEqual(ctx.entry.session.consecutiveServerRejections, 2, 'two refusals');
			socket.deliver({ type: 'ping', protocol_version: 1, nonce: 'n1' });
			socket.deliver({
				type: 'presence',
				protocol_version: 1,
				document_id: 'd',
				session_id: 'peer',
				payload: { v: 1, user: { userId: 'u', name: 'U', color: '#000' }, cursor: null },
				ttl_seconds: 30
			});
			socket.deliver({
				type: 'rejected',
				protocol_version: 1,
				document_id: 'd',
				update_id: null,
				code: 'invalid_update',
				recoverable: false,
				write_state: 'not_applied'
			});
			await tick(10);
			assertEqual(
				ctx.entry.session.consecutiveServerRejections,
				2,
				'non-update frames left the run alone'
			);
			await editAndAwait(ctx, server, 'c');
			await waitFor(() => socket.closedByClient !== null, 'closed on the third');
			assertEqual(socket.closedByClient?.code, 4500, '4500');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync('a server-side 4500 close is terminal too', async () => {
	const server = new FakeCollabServer();
	const ctx = await open(server);
	try {
		const opened = ctx.socketsOpened();
		ctx.currentSocket().serverClose(4500, 'three consecutive updates were permanently refused');
		await waitFor(() => ctx.states.at(-1) === 'error', 'error state');
		await tick(700);
		assertEqual(ctx.socketsOpened(), opened, 'no reconnect after the server gave up');
		assertEqual(ctx.entry.session.isTerminal, true, 'terminal');
	} finally {
		await ctx.close();
	}
});

export const result = suite.result();

if (import.meta.main) finish(result);
