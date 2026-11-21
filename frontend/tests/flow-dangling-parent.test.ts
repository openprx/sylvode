/**
 * A writer can send one Loro update that continues the document's head and creates a tree node under
 * a parent that does not exist. The server refuses it (`tree_refs` in `collab-core`); this suite is
 * the client's defence for the case where one is stored or relayed anyway: importing it into a doc
 * that has subscribed to its events panics the WASM engine (`tree_state.rs`) and leaves the doc dead.
 *
 * The real `FlowObjectRepository` and `LoroObjectSession` run against the fake collaboration server
 * (`tests/support/collab-fakes.ts`) and a real `loro-crdt` doc, so the panic is the real one:
 *
 *  A. a relayed hostile update: the doc is discarded, a fresh one is built from the bootstrap and
 *     rebound, the applied sequence did not advance past it, and the next ordinary update applies;
 *  B. a stored hostile node arriving in the snapshot of a reconnect onto a subscribed doc: the same
 *     recovery;
 *  C. repeated failures are bounded: after two rebuilds within a minute the editor's load error is
 *     reported and nothing loops;
 *  D. a fresh doc that imports the hostile history before subscribing is fine (the order every
 *     bootstrap uses).
 *
 * Run standalone: `bun tests/flow-dangling-parent.test.ts`
 */

import { LoroDoc } from 'loro-crdt';
import { Suite, assert, assertEqual, finish, waitFor } from './support/harness';
import {
	FakeCollabServer,
	FakeConnectivity,
	OBJECT_ID,
	OUTBOX_DB,
	WORKSPACE_ID,
	freshIndexedDb,
	installFakes,
	objectView
} from './support/collab-fakes';
import { IndexedDbOutboxStore } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';
import type { FlowError } from '../src/lib/flow/types';

const suite = new Suite('flow_dangling_parent_client_recovery');
const GHOST_PEER = '4242';

function seedServer(server: FakeCollabServer): void {
	server.doc.setPeerId(1);
	const root = server.doc.getTree('tree').createNode();
	root.data.set('k', 'root');
	server.doc.getText('body').insert(0, 'base');
	server.doc.commit();
}

/** One update by the peer of the server doc's head, depending on the head, that creates a node
 * under a parent that exists nowhere (the hand-encoded shape the library's API refuses to author). */
function hostileUpdate(server: FakeCollabServer): Uint8Array {
	const head = server.doc.frontiers()[0];
	const base = server.doc.export({ mode: 'snapshot' });
	const from = server.doc.version();
	const attacker = new LoroDoc();
	attacker.import(base);
	attacker.setPeerId(head.peer);
	const tree = attacker.getTree('tree');
	tree.createNode(tree.roots()[0].id);
	attacker.commit();
	const json: any = attacker.exportJsonUpdates(from);
	const ghost = json.peers.push(GHOST_PEER) - 1;
	let rewritten = 0;
	for (const change of json.changes) {
		for (const op of change.ops as Array<{ content: { type: string; parent?: string } }>) {
			if (op.content.type === 'create' && op.content.parent !== undefined) {
				op.content.parent = `999999@${ghost}`;
				rewritten += 1;
			}
		}
	}
	assert(rewritten === 1, 'one create rewritten');
	const forge = new LoroDoc();
	forge.import(base);
	forge.setPeerId(head.peer);
	forge.importJsonUpdates(json);
	return forge.export({ mode: 'update', from });
}

/** An ordinary text edit by another peer on top of the server's head. */
function ordinaryUpdate(server: FakeCollabServer, text: string): Uint8Array {
	const peer = new LoroDoc();
	peer.import(server.doc.export({ mode: 'snapshot' }));
	peer.setPeerId(77);
	const from = peer.version();
	const body = peer.getText('body');
	body.insert(body.length, text);
	peer.commit();
	return peer.export({ mode: 'update', from });
}

async function open() {
	const server = new FakeCollabServer();
	seedServer(server);
	const factory = await freshIndexedDb();
	const restore = await installFakes(server);
	const store = await IndexedDbOutboxStore.open(factory, OUTBOX_DB);
	assert(store, 'fake IndexedDB opens');
	const { FlowObjectRepository } = await import('../src/lib/flow/object-repository');
	const repository = new FlowObjectRepository({
		outboxStore: store,
		session: { connectivity: new FakeConnectivity(), answerTimeoutMs: 1500 },
		offlineSync: { sleep: (ms) => new Promise((r) => setTimeout(r, Math.min(ms, 20))) }
	});
	const entry = await repository.open({
		scopeId: WORKSPACE_ID,
		objectId: OBJECT_ID,
		signal: new AbortController().signal,
		object: objectView() as unknown as FlowObjectView
	});
	// The editor subscribes to the doc's events and rebinds on every generation.
	entry.docGeneration.subscribe(() => {
		entry.doc.subscribe(() => {});
	});
	let flowError: FlowError | null = null;
	entry.flowError.subscribe((value) => (flowError = value));
	const generation = () => {
		let value = 0;
		entry.docGeneration.subscribe((v) => (value = v))();
		return value;
	};
	const close = async () => {
		await repository.close(OBJECT_ID);
		restore();
	};
	return { server, entry, generation, error: () => flowError, close };
}

function usable(doc: LoroDoc): boolean {
	try {
		JSON.stringify(doc.toJSON());
		doc.getTree('tree').nodes();
		return true;
	} catch {
		return false;
	}
}

async function scenarioA(): Promise<void> {
	const { server, entry, generation, error, close } = await open();
	try {
		const dead = entry.doc as unknown as LoroDoc;
		assertEqual(generation(), 0, 'generation before');
		server.commitFromPeer(hostileUpdate(server));
		await waitFor(() => generation() >= 1, 'the doc is rebuilt');
		const fresh = entry.doc as unknown as LoroDoc;
		assert(fresh !== dead, 'a new doc replaced the dead one');
		assert(!usable(dead), 'the old doc is dead (the panic is real)');
		assert(usable(fresh), 'the new doc is usable');
		assertEqual(error(), null, 'no error is shown for a recovered doc');
		// The session is alive and applies the next ordinary update to the new, subscribed doc.
		await waitFor(() => server.connections.size === 1, 'the session reconnected');
		await new Promise((r) => setTimeout(r, 50));
		server.commitFromPeer(ordinaryUpdate(server, '+more'));
		await waitFor(
			() => fresh.getText('body').toString() === 'base+more',
			'the next update applies to the new doc'
		);
	} finally {
		await close();
	}
}

async function scenarioB(): Promise<void> {
	const { server, entry, generation, error, close } = await open();
	try {
		const dead = entry.doc as unknown as LoroDoc;
		// The hostile update is stored without being relayed; the next snapshot carries it.
		server.doc.import(hostileUpdate(server));
		server.headSeq += 1;
		await entry.session.reconnect('stale_frontier');
		await waitFor(() => generation() >= 1, 'the doc is rebuilt after the snapshot');
		assert(entry.doc !== (dead as unknown), 'a new doc replaced the dead one');
		assert(usable(entry.doc as unknown as LoroDoc), 'the new doc is usable');
		assertEqual(error(), null, 'no error is shown for a recovered doc');
	} finally {
		await close();
	}
}

async function scenarioC(): Promise<void> {
	const { server, entry, generation, error, close } = await open();
	try {
		for (let attempt = 1; attempt <= 2; attempt += 1) {
			server.commitFromPeer(hostileUpdate(server));
			await waitFor(() => generation() >= attempt, `rebuild ${attempt}`);
			await waitFor(() => server.connections.size === 1, `reconnect ${attempt}`);
			await new Promise((r) => setTimeout(r, 50));
		}
		assertEqual(error(), null, 'two rebuilds are not an error');
		server.commitFromPeer(hostileUpdate(server));
		await waitFor(() => error() !== null, 'the load error after the third failure');
		assertEqual(error()?.code, 'invalid_update', 'the error code');
		assertEqual(
			(error()?.details as { reason?: string } | undefined)?.reason,
			'remote_update_unreadable',
			'the error reason'
		);
		await new Promise((r) => setTimeout(r, 100));
		assertEqual(generation(), 2, 'no third rebuild');
	} finally {
		await close();
	}
}

async function scenarioE(): Promise<void> {
	const { server, entry, close } = await open();
	try {
		const session = entry.session as unknown as {
			lastAppliedSeq: number;
			hooks: {
				onAccepted: (notice: unknown) => void;
				onSnapshot: (payload: unknown) => void;
				onImportFailure: (error: unknown, frame: string) => void;
			};
		};
		const failures: string[] = [];
		session.hooks.onAccepted = () => {
			throw new Error('import threw');
		};
		session.hooks.onImportFailure = (_error, frame) => failures.push(frame);
		const applied = session.lastAppliedSeq;
		server.commitFromPeer(ordinaryUpdate(server, '+x'));
		await waitFor(() => failures.length === 1, 'the failure is reported');
		assertEqual(failures[0], 'accepted', 'the failing frame');
		assertEqual(session.lastAppliedSeq, applied, 'the applied sequence did not advance');
		// A snapshot whose hook throws does not move the baseline either.
		session.hooks.onSnapshot = () => {
			throw new Error('import threw');
		};
		await entry.session.reconnect('stale_frontier');
		await waitFor(() => failures.length === 2, 'the snapshot failure is reported');
		assertEqual(failures[1], 'snapshot', 'the failing frame');
		assertEqual(session.lastAppliedSeq, applied, 'the baseline did not move');
	} finally {
		await close();
	}
}

function scenarioD(): void {
	const server = new FakeCollabServer();
	seedServer(server);
	server.doc.import(hostileUpdate(server));
	const doc = new LoroDoc();
	doc.setPeerId(7);
	doc.import(server.doc.export({ mode: 'snapshot' }));
	doc.subscribe(() => {});
	assert(usable(doc), 'import first, subscribe afterwards');
	doc.getTree('tree').roots()[0].createNode();
	doc.commit();
	assert(usable(doc), 'a local edit after it');
}

console.log(`\n${suite.name}`);
await suite.checkAsync(
	'A. a relayed hostile update: the doc is rebuilt and the session goes on',
	scenarioA
);
await suite.checkAsync(
	'B. a stored hostile node in a reconnect snapshot: the doc is rebuilt',
	scenarioB
);
await suite.checkAsync('C. repeated failures are bounded and end in the load error', scenarioC);
await suite.checkAsync('E. a throwing import does not advance the applied sequence', scenarioE);
suite.check('D. importing before subscribing is safe', scenarioD);
finish(suite.result());
