/**
 * Flow local data is per user (`ui-surface-v1.md` "职责边界": the IndexedDB outbox, recovery
 * drafts and accepted snapshot cache belong to `ObjectRepository`).
 *
 * Checks, against the real repository + session over `fake-indexeddb` and the fake server, with
 * the default stores (no injected outbox), so the database the repository picks is under test:
 * - user A's offline edit, left behind when A's page closed without a logout, is neither replayed
 *   over user B's session nor shown in B's recovery draft, whether B may write or only read;
 * - A's own next page load still replays it (isolation does not lose A's intent);
 * - the snapshot cache A wrote is not what B's open reads;
 * - `authStore.logout()` deletes every Flow database: outbox, drafts and the snapshot cache.
 *
 * Run standalone: `bun tests/flow-local-data-isolation.test.ts`
 */

import { Suite, assert, assertDeepEqual, assertEqual, finish, waitFor } from './support/harness';
import {
	FakeCollabServer,
	FakeConnectivity,
	OBJECT_ID,
	WORKSPACE_ID,
	edit,
	freshIndexedDb,
	installFakes,
	objectView
} from './support/collab-fakes';
import type { FlowObjectView } from '../src/lib/api/flow';
import type { RecoveryDraftEntry } from '../src/lib/flow/outbox';
import { authStore } from '../src/lib/stores/auth';

const suite = new Suite('flow_local_data_isolation');

const USER_A = {
	id: '00000000-0000-4000-8000-00000000000a',
	email: 'a@example.test',
	name: 'A',
	created_at: ''
};
const USER_B = {
	id: '00000000-0000-4000-8000-00000000000b',
	email: 'b@example.test',
	name: 'B',
	created_at: ''
};

type Repository = InstanceType<
	(typeof import('../src/lib/flow/object-repository'))['FlowObjectRepository']
>;

async function newRepository(connectivity: FakeConnectivity): Promise<Repository> {
	const { FlowObjectRepository } = await import('../src/lib/flow/object-repository');
	return new FlowObjectRepository({
		session: { connectivity, answerTimeoutMs: 1500 },
		offlineSync: { sleep: (ms) => new Promise((r) => setTimeout(r, Math.min(ms, 20))) }
	});
}

async function open(
	server: FakeCollabServer,
	shared?: { repository: Repository; connectivity: FakeConnectivity }
) {
	const restore = await installFakes(server);
	const connectivity = shared?.connectivity ?? new FakeConnectivity();
	const repository = shared?.repository ?? (await newRepository(connectivity));
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
	let drafts: ReadonlyArray<RecoveryDraftEntry> = [];
	entry.recoveryDraft.subscribe((d) => (drafts = d));
	return {
		connectivity,
		entry,
		states,
		pending: () => pending,
		drafts: () => drafts,
		async close() {
			await repository.close(OBJECT_ID);
			restore();
		}
	};
}

/** A, offline, makes one edit and leaves the page without logging out. */
async function leaveAnOfflineEditAsA(server: FakeCollabServer): Promise<void> {
	authStore.setUser(USER_A);
	const a = await open(server);
	a.connectivity.goOffline();
	await waitFor(() => a.states.at(-1) === 'offline', 'A offline');
	edit(a.entry.doc, 'written by A');
	await waitFor(() => a.pending() === 1, 'A has one pending edit');
	await a.close();
}

function flowDatabases(idb: IDBFactory): Promise<string[]> {
	return idb.databases().then((list) =>
		list
			.map((db) => db.name ?? '')
			.filter((name) => name.startsWith('sylvode-flow-'))
			.sort()
	);
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 300));

for (const level of ['edit', 'view'] as const) {
	await suite.checkAsync(
		`A's pending edit is not replayed over B's session nor drafted for B (B may ${level})`,
		async () => {
			const idb = await freshIndexedDb();
			const server = new FakeCollabServer();
			await leaveAnOfflineEditAsA(server);
			assertEqual(server.accepted.length, 0, 'nothing reached the server while A was offline');

			authStore.setUser(USER_B);
			server.effectiveLevel = level;
			const hellos = server.hellos.length;
			const b = await open(server);
			try {
				await waitFor(() => server.hellos.length > hellos, 'B connected', 5000);
				await settle();
				assertEqual(server.accepted.length, 0, "A's update was not sent over B's session");
				assertEqual(b.pending(), 0, 'B has nothing pending');
				assertDeepEqual(b.drafts(), [], "A's intent is not in B's recovery draft");
			} finally {
				await b.close();
			}
			assertDeepEqual(
				await flowDatabases(idb),
				[
					`sylvode-flow-cache-v1:${USER_A.id}`,
					`sylvode-flow-cache-v1:${USER_B.id}`,
					`sylvode-flow-outbox-v1:${USER_A.id}`,
					`sylvode-flow-outbox-v1:${USER_B.id}`
				],
				'each user has its own outbox and snapshot cache; no shared database exists'
			);
		}
	);
}

await suite.checkAsync(
	"a repository that outlives a sign-in switch opens the new user's outbox",
	async () => {
		await freshIndexedDb();
		const server = new FakeCollabServer();
		const connectivity = new FakeConnectivity();
		const repository = await newRepository(connectivity);
		authStore.setUser(USER_A);
		const a = await open(server, { repository, connectivity });
		connectivity.goOffline();
		await waitFor(() => a.states.at(-1) === 'offline', 'A offline');
		edit(a.entry.doc, 'written by A');
		await waitFor(() => a.pending() === 1, 'A has one pending edit');
		await a.close();
		connectivity.goOnline();

		authStore.setUser(USER_B);
		const hellos = server.hellos.length;
		const b = await open(server, { repository, connectivity });
		try {
			await waitFor(() => server.hellos.length > hellos, 'B connected', 5000);
			await settle();
			assertEqual(server.accepted.length, 0, "A's update was not sent over B's session");
			assertEqual(b.pending(), 0, 'B has nothing pending');
		} finally {
			await b.close();
		}
	}
);

await suite.checkAsync("A's next page load still replays A's own edit", async () => {
	await freshIndexedDb();
	const server = new FakeCollabServer();
	await leaveAnOfflineEditAsA(server);
	authStore.setUser(USER_A);
	const again = await open(server);
	try {
		await waitFor(
			() => again.pending() === 0 && server.accepted.length === 1,
			'A replayed its own edit',
			5000
		);
	} finally {
		await again.close();
	}
});

await suite.checkAsync('logout deletes the outbox, the drafts and the snapshot cache', async () => {
	const idb = await freshIndexedDb();
	const server = new FakeCollabServer();
	await leaveAnOfflineEditAsA(server);
	const before = await flowDatabases(idb);
	assert(
		before.some((name) => name.startsWith('sylvode-flow-outbox-v1')),
		`the outbox database exists before logout: ${JSON.stringify(before)}`
	);
	await authStore.logout();
	assertDeepEqual(await flowDatabases(idb), [], 'no Flow database is left after logout');

	authStore.setUser(USER_A);
	const hellos = server.hellos.length;
	const back = await open(server);
	try {
		await waitFor(() => server.hellos.length > hellos, 'A connected again', 5000);
		await settle();
		assertEqual(back.pending(), 0, 'nothing pending survived the logout');
		assertEqual(server.accepted.length, 0, 'and nothing was replayed');
	} finally {
		await back.close();
	}
});

await suite.checkAsync(
	'logout is not held up by another tab whose connection ignores versionchange',
	async () => {
		const idb = await freshIndexedDb();
		const name = `sylvode-flow-outbox-v1:${USER_A.id}`;
		const stale = await new Promise<IDBDatabase>((resolve, reject) => {
			const request = idb.open(name, 1);
			request.onsuccess = () => resolve(request.result);
			request.onerror = () => reject(request.error);
		});
		const { clearFlowLocalData } = await import('../src/lib/flow/local-data');
		const started = Date.now();
		await clearFlowLocalData([USER_A.id], idb, 200);
		const waited = Date.now() - started;
		assert(waited < 2000, `logout went on after ${waited} ms`);
		stale.close();
		for (let i = 0; i < 30 && (await flowDatabases(idb)).includes(name); i += 1) {
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
		assertDeepEqual(await flowDatabases(idb), [], 'and the database is gone afterwards');
	}
);

finish(suite.result());
