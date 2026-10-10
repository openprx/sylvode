/**
 * Permission read-only mode (FP2-M6 item 0; `ADR-0012`: "`edit` 起可写、`view` 只读";
 * `error-mapping-v1.md` `forbidden`: "转只读", `policy_rejected`: "保留 intent，不自动重放").
 *
 * - opening an object whose effective level is `view` or `comment` puts the session in
 *   `read_only` and no local update is sent or queued; `edit`/`full_access` stay writable;
 * - a 4403 close on a live connection switches to read-only, keeps pending edits in the recovery
 *   draft, and does not reconnect;
 * - a `policy_rejected` answer switches to read-only and keeps the intent in the draft;
 * - `policy_rejected` is final ("保留 intent，不自动重放"): the server re-sources a moved
 *   authorization epoch in place (`collab-protocol-v1.md`), so the client neither reconnects nor
 *   replays to second-guess it -- not even when its own grants read still says `edit`;
 * - a grant change elsewhere in the workspace does not disturb a writer whose level did not
 *   change: its next write on the same connection is simply accepted.
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

await suite.checkAsync('policy_rejected: read-only and drafted, nothing replayed', async () => {
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
});

await suite.checkAsync(
	'policy_rejected is final even when the grants read still says edit: no reconnect, no replay',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			// The level the grants endpoint reports stays `edit`; only the server's answer counts.
			server.policy = () => ({ kind: 'reject', reject: { code: 'policy_rejected' } });
			const sockets = MockWebSocket.instances.length;
			edit(ctx.entry.doc, 'refused');
			edit(ctx.entry.doc, ' and its follower');
			await waitFor(() => ctx.entry.offline.draftEntries.length === 2, 'both drafted', 5000);
			await ctx.entry.offline.idle();
			await tick(300);
			assertEqual(MockWebSocket.instances.length, sockets, 'no reconnect');
			assertEqual(server.updatesSeen.length, 1, 'sent once, never replayed');
			assertEqual(ctx.readOnly(), true, 'read-only');
			assertEqual(
				ctx.entry.offline.draftEntries.map((d) => d.reason.code).join(','),
				'policy_rejected,policy_rejected',
				'the intent and its follower are kept with the policy reason'
			);
			assertEqual(ctx.pending(), 0, 'nothing left to send');
		} finally {
			await ctx.close();
		}
	}
);

await suite.checkAsync(
	'a grant change elsewhere does not disturb an unchanged writer on its open connection',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await open(server);
		try {
			const sockets = MockWebSocket.instances.length;
			edit(ctx.entry.doc, 'before.');
			await waitFor(() => server.accepted.length === 1 && ctx.pending() === 0, 'first accepted');
			// Someone else's grant changed: the server re-sources this connection's epoch in place
			// and, the owner's level being unchanged, accepts. The client has nothing to do.
			edit(ctx.entry.doc, 'after.');
			await waitFor(() => server.accepted.length === 2 && ctx.pending() === 0, 'second accepted');
			assertEqual(MockWebSocket.instances.length, sockets, 'same connection throughout');
			assertEqual(server.updatesSeen.length, 2, 'each edit sent once');
			assertEqual(ctx.readOnly(), false, 'still writable');
		} finally {
			await ctx.close();
		}
	}
);

export const result = suite.result();

if (import.meta.main) finish(result);
