import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	BlockTransferJournal,
	type TransferJournalEntry
} from '../src/lib/flow/block-transfer/journal';
const store = () => {
	const values = new Map<string, string>();
	return {
		getItem: (k: string) => values.get(k) ?? null,
		setItem: (k: string, v: string) => {
			values.set(k, v);
		},
		removeItem: (k: string) => {
			values.delete(k);
		}
	};
};
const entry = (): TransferJournalEntry => ({
	actorId: 'actor-a',
	sourcePageId: 'source',
	checkpoint: {
		action: 'commit',
		state: {
			phase: 'unknown',
			review: {
				id: 'operation',
				kind: 'move',
				sourcePageId: 'source',
				targetPageId: 'target',
				sourceFrontier: 'AA==',
				targetFrontier: 'AQ==',
				digest: 'digest',
				expiresAt: 1000,
				memberCount: 2,
				assetCount: 0,
				affectedShareCount: 0,
				assetsReady: true
			},
			receipt: null,
			detailsUnavailable: false,
			error: null,
			commitKey: 'same-commit',
			undoKey: 'same-undo'
		}
	}
});
test('unknown journal retains same-key metadata under its actor without Page content', () => {
	const storage = store(),
		journal = new BlockTransferJournal(storage, 'actor-a'),
		value = entry();
	journal.put(value);
	assert.deepEqual(journal.list(), [value]);
	assert.deepEqual(new BlockTransferJournal(storage, 'actor-b').list(), []);
	assert.throws(() => journal.put({ ...value, title: 'private' } as TransferJournalEntry));
	const malicious = entry();
	(malicious.checkpoint.state.review as unknown as Record<string, unknown>).sourceFrontier = {
		body: 'private'
	};
	assert.throws(() => journal.put(malicious));
	assert.deepEqual(journal.list(), [value]);
});
test('undo unknown preserves the inverse key and original Page roles without saving text', () => {
	const storage = store(),
		journal = new BlockTransferJournal(storage, 'actor-a'),
		value = entry();
	value.checkpoint.action = 'undo';
	value.checkpoint.state.receipt = {
		id: 'operation',
		kind: 'move',
		sourcePageId: 'source',
		targetPageId: 'target',
		sourcePostFrontier: 'Ag==',
		targetPostFrontier: 'Aw==',
		firstTargetBlockCid: 'cid:9@10:Map'
	};
	journal.put(value);
	assert.deepEqual(journal.list()[0], value);
	value.checkpoint.state.receipt.targetPageId = 'other';
	assert.throws(() => journal.put(value));
	journal.remove('operation');
	assert.deepEqual(journal.list(), []);
});
