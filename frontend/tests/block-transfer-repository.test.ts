import { test } from 'node:test';
import assert from 'node:assert/strict';
import { get } from 'svelte/store';
import { atomicEditorWrite, editorWritesAllowed } from '../src/lib/flow/editor-batch';
import { FlowObjectRepository } from '../src/lib/flow/object-repository';
import { pageUndoManager, clearEditorSessionHistory } from '../src/lib/flow/editor-session';
import { IndexedDbOutboxStore } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';
import {
	FakeCollabServer,
	FakeConnectivity,
	freshIndexedDb,
	installFakes,
	objectView,
	OBJECT_ID,
	WORKSPACE_ID
} from './support/collab-fakes';
test('a recovered transfer barrier applies before a previously closed Page is opened, blocks mutations without clearing Undo, and releases only its own hold', async () => {
	const factory = await freshIndexedDb(),
		server = new FakeCollabServer(),
		restore = await installFakes(server);
	const store = await IndexedDbOutboxStore.open(factory, 'sv-transfer-barrier-fixture');
	assert(store);
	const repo = new FlowObjectRepository({
		outboxStore: store,
		currentUserId: () => 'transfer-fixture',
		session: { connectivity: new FakeConnectivity(), answerTimeoutMs: 1000 }
	});
	const first = repo.holdTransferEditing([OBJECT_ID], 'pending-first');
	try {
		const entry = await repo.open({
			scopeId: WORKSPACE_ID,
			objectId: OBJECT_ID,
			signal: new AbortController().signal,
			object: objectView() as unknown as FlowObjectView
		});
		assert.equal(get(entry.readOnly), true);
		assert.equal(editorWritesAllowed(entry.doc), false);
		assert.throws(() =>
			atomicEditorWrite(entry.doc, () => entry.doc.getMap('data').set('blocked', true))
		);
		const second = repo.holdTransferEditing([OBJECT_ID], 'pending-second');
		first();
		assert.equal(get(entry.readOnly), true);
		second();
		assert.equal(get(entry.readOnly), false);
		assert.equal(editorWritesAllowed(entry.doc), true);
		const undo = await pageUndoManager(entry.doc);
		entry.doc.getMap('data').set('new_typing', 'kept');
		entry.doc.commit();
		assert.equal(undo.canUndo(), true);
		const transient = repo.holdTransferEditing([OBJECT_ID], 'unknown');
		assert.equal(undo.canUndo(), true);
		transient();
		assert.equal(undo.canUndo(), true);
		assert.equal(clearEditorSessionHistory(entry.doc), true);
		assert.equal(undo.canUndo(), false);
		entry.doc.getMap('data').set('later_typing', 'ordinary');
		entry.doc.commit();
		assert.equal(undo.canUndo(), true);
		undo.undo();
		assert.equal(entry.doc.getMap('data').get('new_typing'), 'kept');
		assert.equal(entry.doc.getMap('data').get('later_typing'), undefined);
	} finally {
		first();
		await repo.disposeAll();
		restore();
	}
});
