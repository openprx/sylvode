/**
 * Permission read-only mode (FP2-M6 item 0; `ADR-0012`: "`edit` 起可写、`view` 只读";
 * `error-mapping-v1.md` `forbidden`: "转只读", `policy_rejected`: "保留 intent，不自动重放").
 *
 * - opening an object whose effective level is `view` or `comment` puts the session in
 *   `read_only` and no local update is sent or queued; `edit`/`full_access` stay writable;
 * - a 4403 close on a live connection switches to read-only, keeps pending edits in the recovery
 *   draft, and does not reconnect;
 * - a `policy_rejected` whose re-check says "no longer writable" switches to read-only and drafts;
 * - a `policy_rejected` whose re-check still says "writable" (the server fences writes on the
 *   connection's authorization epoch, so every older connection is refused after ANY grant change)
 *   reconnects once and replays under the same key.
 *
 * The editor's own `editable` flag needs a mounted ProseMirror view; the browser E2E
 * (`tests/e2e/web/flow-multi-user.spec.ts`) asserts the read-only canvas there.
 *
 * Run standalone: `bun tests/flow-read-only.test.ts`
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
	tick
} from './support/collab-fakes';
import { IndexedDbOutboxStore } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';

const suite = new Suite('flow_read_only');

async function open(server: FakeCollabServer) {
	const idb = await freshIndexedDb();
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
	let readOnly = false;
	entry.readOnly.subscribe((v) => (readOnly = v));
	let pending = 0;
	entry.pending.subscribe((n) => (pending = n));
	return {
		store,
		connectivity,
		repository,
		entry,
		states,
		readOnly: () => readOnly,
		pending: () => pending,
		async close() {
			await repository.close(OBJECT_ID);
			restore();
		}
	};
}

for (const level of ['view', 'comment'] as const) {
	await suite.checkAsync(
		`effective level ${level}: read_only, nothing is sent or queued`,
		async () => {
			const server = new FakeCollabServer();
			server.effectiveLevel = level;
			const ctx = await open(server);
			try {
				assertEqual(ctx.states.at(-1), 'read_only', 'indicator read_only');
				assertEqual(ctx.readOnly(), true, 'repository read-only');
				assertEqual(ctx.entry.session.isReadOnly, true, 'session read-only');
				edit(ctx.entry.doc, 'should not leave');
				await tick(50);
				assertEqual(server.updatesSeen.length, 0, 'no update sent');
				assertEqual((await ctx.store.list(OBJECT_ID)).length, 0, 'no outbox entry');
				ctx.repository.setTitle(OBJECT_ID, 'renamed');
				await tick(20);
				assertEqual(server.updatesSeen.length, 0, 'a title edit is refused too');
			} finally {
				await ctx.close();
			}
		}
	);
}

for (const level of ['edit', 'full_access'] as const) {
	await suite.checkAsync(`effective level ${level}: writable`, async () => {
		const server = new FakeCollabServer();
		server.effectiveLevel = level;
		const ctx = await open(server);
		try {
			assertEqual(ctx.readOnly(), false, 'not read-only');
			edit(ctx.entry.doc, 'ok');
			await waitFor(() => server.accepted.length === 1, 'accepted');
		} finally {
			await ctx.close();
		}
	});
}

await suite.checkAsync(
	'4403 on a live connection: read-only, pending edits drafted, no reconnect',
	async () => {
		const server = new FakeCollabServer();
		server.policy = () => ({ kind: 'silence' });
		const ctx = await open(server);
		try {
			edit(ctx.entry.doc, 'in flight');
			await waitFor(() => server.updatesSeen.length === 1 && ctx.pending() === 1, 'one in flight');
			const sockets = MockWebSocket.instances.length;
			MockWebSocket.instances[sockets - 1].serverClose(4403, 'authorization revoked');
			await waitFor(() => ctx.entry.offline.draftEntries.length === 1, 'drafted');
			assertEqual(ctx.states.at(-1), 'read_only', 'read_only, not error');
			assertEqual(ctx.readOnly(), true, 'repository read-only');
			assertEqual(ctx.entry.offline.draftEntries[0].reason.code, 'forbidden', 'draft reason');
			await tick(600);
			assertEqual(MockWebSocket.instances.length, sockets, 'no reconnect');
			ctx.connectivity.goOnline();
			await tick(50);
			assertEqual(MockWebSocket.instances.length, sockets, 'not even on an online event');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'policy_rejected and the re-check says no: read-only and drafted',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			server.effectiveLevel = 'view';
			server.policy = () => ({ kind: 'reject', reject: { code: 'policy_rejected' } });
			edit(ctx.entry.doc, 'lowered');
			await waitFor(() => ctx.entry.offline.draftEntries.length === 1, 'drafted');
			await ctx.entry.offline.idle();
			assertEqual(ctx.readOnly(), true, 'read-only');
			assertEqual(ctx.states.at(-1), 'read_only', 'indicator');
			assertEqual(server.updatesSeen.length, 1, 'not replayed');
			assertEqual(ctx.entry.doc.getText('body').toString(), '', 'rebuilt from accepted');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'policy_rejected but still writable (stale connection epoch): one reconnect, same key, accepted',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			let refused = false;
			server.policy = (seen) => {
				if (refused) return { kind: 'accept' };
				refused = true;
				void seen;
				return { kind: 'reject', reject: { code: 'policy_rejected' } };
			};
			const sockets = MockWebSocket.instances.length;
			edit(ctx.entry.doc, 'epoch moved');
			await waitFor(
				() => server.accepted.length === 1 && ctx.pending() === 0,
				'accepted on the new connection',
				5000
			);
			assertEqual(MockWebSocket.instances.length, sockets + 1, 'exactly one reconnect');
			assertEqual(server.updatesSeen.length, 2, 'sent twice');
			assertEqual(server.updatesSeen[0].updateId, server.updatesSeen[1].updateId, 'same key');
			assertEqual(ctx.readOnly(), false, 'still writable');
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'nothing drafted');
		} finally {
			await ctx.close();
		}
	}
);

export const result = suite.result();

if (import.meta.main) finish(result);
