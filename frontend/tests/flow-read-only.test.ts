/**
 * Permission read-only mode (`ADR-0012`: "`edit` 起可写、`view` 只读";
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
	tick,
	textOf,
	OUTBOX_DB
} from './support/collab-fakes';
import { IndexedDbOutboxStore } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';

const suite = new Suite('flow_read_only');

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
	let readOnly = false;
	entry.readOnly.subscribe((v) => (readOnly = v));
	let pending = 0;
	entry.pending.subscribe((n) => (pending = n));
	return {
		idb,
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

function notifications(server: FakeCollabServer) {
	server.ticketResponse = {
		ticket: 'authorization-ticket',
		expires_at: '',
		websocket_path: '/api/v2/flow/collab/ws',
		authorization_notifications: true,
		authz_revision: 7,
		scope_id: WORKSPACE_ID,
		object_id: OBJECT_ID
	};
}

await suite.checkAsync(
	'bound permission downgrade updates read-only without an edit or losing legitimate peer updates',
	async () => {
		const server = new FakeCollabServer();
		notifications(server);
		const a = await open(server);
		const socket = MockWebSocket.instances.at(-1)!;
		try {
			assert(
				server.ticketRequests[0].authorization_notifications === true,
				'ticket explicitly opts in'
			);
			server.notifyAuthorization(socket, ['view'], 2);
			await waitFor(() => a.readOnly(), 'notification makes editor read-only');
			assertEqual(server.updatesSeen.length, 0, 'no rejected first edit needed');
			assertEqual(socket.readyState, MockWebSocket.OPEN, 'legitimate view socket remains open');
			const b = await open(server);
			try {
				edit(b.entry.doc, 'peer remains visible');
				await waitFor(
					() => textOf(a.entry.doc) === 'peer remains visible',
					'read stream still applies peer content'
				);
				assertEqual(a.readOnly(), true, 'peer update does not unlock writes');
			} finally {
				await b.close();
			}
		} finally {
			await a.close();
		}
	}
);

await suite.checkAsync(
	'unchanged source epoch plus ordered audience notification drafts pending input and upgrade never replays it',
	async () => {
		const server = new FakeCollabServer();
		notifications(server);
		const a = await open(server);
		const socket = MockWebSocket.instances.at(-1)!;
		try {
			edit(a.entry.doc, 'recover me');
			server.notifyAuthorization(socket, ['view'], 2);
			await waitFor(
				() => a.readOnly() && a.entry.offline.draftEntries.length === 1,
				'persisted intent is drafted'
			);
			await a.entry.offline.idle();
			assertEqual(server.updatesSeen.length, 0, 'queued edit not sent after downgrade');
			assertEqual(textOf(a.entry.doc), '', 'live document rebuilt to accepted baseline');
			server.notifyAuthorization(socket, ['view', 'comment', 'edit'], 3);
			await waitFor(() => !a.readOnly(), 'higher sequence with same source epoch restores edit');
			assertEqual(a.entry.offline.draftEntries.length, 1, 'recovery draft remains');
			assertEqual(server.updatesSeen.length, 0, 'upgrade did not replay recovery draft');
			edit(a.entry.doc, 'fresh authorized edit');
			await waitFor(() => server.accepted.length === 1, 'new authorized edit accepted');
			assertEqual(
				textOf(server.doc),
				'fresh authorized edit',
				'fresh edit does not depend on rejected local operations'
			);
		} finally {
			await a.close();
		}
	}
);

await suite.checkAsync(
	'stale sequence, old server session and regressed source epoch cannot unlock an active reader',
	async () => {
		const server = new FakeCollabServer();
		notifications(server);
		const a = await open(server);
		const socket = MockWebSocket.instances.at(-1)!;
		try {
			server.notifyAuthorization(socket, ['view'], 2);
			await waitFor(() => a.readOnly(), 'reader');
			server.notifyAuthorization(socket, ['view', 'edit'], 1);
			server.notifyAuthorization(socket, ['view', 'edit'], 3, { session_id: 'old-session' });
			server.notifyAuthorization(socket, ['view', 'edit'], 3, { authz_revision: 6 });
			await tick(10);
			assertEqual(a.readOnly(), true, 'stale metadata ignored');
			assertEqual(socket.readyState, MockWebSocket.OPEN, 'no unnecessary reconnect');
			server.notifyAuthorization(socket, ['view', 'edit'], 3);
			await waitFor(() => !a.readOnly(), 'valid ordered update applies');
		} finally {
			await a.close();
		}
	}
);

await suite.checkAsync(
	'initial read-only authorization arrives before snapshot without deadlocking or replaying persisted pending input',
	async () => {
		const server = new FakeCollabServer();
		const first = await open(server);
		first.connectivity.goOffline();
		await waitFor(() => first.states.at(-1) === 'offline', 'offline');
		edit(first.entry.doc, 'persisted before reconnect');
		await waitFor(() => first.pending() === 1, 'persisted queue');
		const idb = first.idb;
		await first.close();
		server.online = true;
		server.effectiveLevel = 'view';
		notifications(server);
		const second = await open(server, idb);
		const socket = MockWebSocket.instances.at(-1)!;
		try {
			assertEqual(second.readOnly(), true, 'initial editable baseline is blocked');
			assertEqual(second.pending(), 0, 'pending input moved to draft');
			assertEqual(second.entry.offline.draftEntries.length, 1, 'input retained');
			assertEqual(
				textOf(second.entry.doc),
				'',
				'accepted snapshot rendered, not premature pending text'
			);
			assertEqual(server.updatesSeen.length, 0, 'no unauthorized replay');
			const kinds = socket.received.map((frame) => frame.type);
			assert(
				kinds.indexOf('resync') < kinds.indexOf('snapshot'),
				'authorization preceded snapshot'
			);
		} finally {
			await second.close();
		}
	}
);

await suite.checkAsync(
	'new server session resets ordering and queued callbacks from the old socket cannot change permissions',
	async () => {
		const server = new FakeCollabServer();
		notifications(server);
		const a = await open(server);
		const old = MockWebSocket.instances.at(-1)!;
		try {
			await a.entry.session.reconnect('manual');
			await waitFor(
				() => MockWebSocket.instances.at(-1) !== old && a.entry.session.hasBoundAuthorization,
				'new bound session'
			);
			const current = MockWebSocket.instances.at(-1)!;
			const sessionId = server.connections.get(current)!.sessionId;
			const frame = {
				type: 'resync',
				protocol_version: 1,
				document_id: a.entry.handle.documentId,
				reason: 'authorization_changed',
				authorization: {
					scope_id: WORKSPACE_ID,
					object_id: OBJECT_ID,
					authz_revision: 7,
					authority_sequence: 99,
					session_id: sessionId,
					permitted_actions: ['view']
				}
			};
			old.deliverQueued(frame);
			await tick(10);
			assertEqual(a.readOnly(), false, 'old socket cannot lock current writer');
			server.notifyAuthorization(current, ['view'], 2);
			await waitFor(() => a.readOnly(), 'new connection sequence is independent');
			old.deliverQueued({
				...frame,
				authorization: {
					...frame.authorization,
					authority_sequence: 100,
					permitted_actions: ['view', 'edit']
				}
			});
			await tick(10);
			assertEqual(a.readOnly(), true, 'old callback cannot unlock reader');
			server.notifyAuthorization(current, ['view', 'edit'], 3);
			await waitFor(() => !a.readOnly(), 'valid current sequence still applies');
		} finally {
			await a.close();
		}
	}
);

export const result = suite.result();

if (import.meta.main) finish(result);
