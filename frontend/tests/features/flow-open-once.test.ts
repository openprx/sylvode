import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	FakeCollabServer,
	FakeConnectivity,
	OBJECT_ID,
	OUTBOX_DB,
	WORKSPACE_ID,
	freshIndexedDb,
	installFakes,
	objectView
} from '../support/collab-fakes';
import { IndexedDbOutboxStore } from '../../src/lib/flow/outbox';
import type { FlowObjectView } from '../../src/lib/api/flow';

// Two callers that open the same Flow object while the first is still connecting (the page and a
// component that shows the same object) share one session: one ticket, one WebSocket, one entry.

test('concurrent opens of one object share a single collaboration connection', async () => {
	const server = new FakeCollabServer();
	server.doc.setPeerId(1);
	server.doc.getText('body').insert(0, 'base');
	server.doc.commit();
	const factory = await freshIndexedDb();
	const restore = await installFakes(server);
	try {
		const store = await IndexedDbOutboxStore.open(factory, OUTBOX_DB);
		assert.ok(store, 'fake IndexedDB opens');
		const { FlowObjectRepository } = await import('../../src/lib/flow/object-repository');
		const repository = new FlowObjectRepository({
			outboxStore: store,
			session: { connectivity: new FakeConnectivity(), answerTimeoutMs: 1500 },
			offlineSync: { sleep: (ms) => new Promise((r) => setTimeout(r, Math.min(ms, 20))) }
		});
		const input = () => ({
			scopeId: WORKSPACE_ID,
			objectId: OBJECT_ID,
			signal: new AbortController().signal,
			object: objectView() as unknown as FlowObjectView
		});
		const [first, second] = await Promise.all([repository.open(input()), repository.open(input())]);
		assert.equal(first, second, 'both callers hold the same entry');
		assert.equal(server.ticketRequests.length, 1, 'one ticket was issued');
		assert.equal(server.connections.size, 1, 'one socket is open');
		await repository.close(OBJECT_ID);
		assert.equal(server.connections.size, 1, 'the first release keeps the connection');
		await repository.close(OBJECT_ID);
		assert.equal(server.connections.size, 0, 'the last release closes it');
	} finally {
		restore();
	}
});
