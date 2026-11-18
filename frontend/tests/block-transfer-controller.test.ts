import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	BlockTransferOperation,
	type TransferDecision,
	type ReviewedBlockTransfer,
	type BlockTransferReceipt,
	type TransferOperationTransport,
	type TransferOperationHost
} from '../src/lib/flow/block-transfer/controller';
import type { PrepareBlockTransfer } from '../src/lib/flow/block-transfer/contracts';
const source = '11111111-1111-4111-8111-111111111111',
	target = '22222222-2222-4222-8222-222222222222',
	id = '33333333-3333-4333-8333-333333333333';
const request = (kind: 'move' | 'copy' = 'move'): PrepareBlockTransfer => ({
	kind,
	target_object_id: target,
	root_block_cid: 'cid:1@991001:Map',
	selection: 'current_subtree',
	target_parent_block_cid: null,
	target_index: 0,
	expected_source_frontier: 'source-token',
	expected_target_frontier: 'target-token'
});
const review = (kind: 'move' | 'copy' = 'move'): ReviewedBlockTransfer => ({
	id,
	kind,
	sourcePageId: source,
	targetPageId: target,
	sourceFrontier: 'source-token',
	targetFrontier: 'target-token',
	digest: 'review-digest',
	expiresAt: Date.now() + 60000,
	memberCount: 2,
	assetCount: 0,
	affectedShareCount: 1,
	assetsReady: true
});
const receipt = (kind: 'move' | 'copy' = 'move'): BlockTransferReceipt => ({
	id,
	kind,
	sourcePageId: source,
	targetPageId: target,
	sourcePostFrontier: 'source-post',
	targetPostFrontier: 'target-post',
	firstTargetBlockCid: 'cid:9@881001:Map'
});
function fixture(kind: 'move' | 'copy' = 'move') {
	const events: {
		held: string[][];
		clears: boolean[];
		releases: number;
		prepareKeys: string[];
		commitKeys: string[];
		undoKeys: string[];
	} = { held: [], clears: [], releases: 0, prepareKeys: [], commitKeys: [], undoKeys: [] };
	let commit: TransferDecision = { state: 'applied', id, kind, receipt: receipt(kind) },
		status: TransferDecision = { state: 'pending' },
		undo: TransferDecision = { state: 'undone', id, kind, receipt: receipt(kind) },
		current = true,
		inverse = true,
		applyFailure = false;
	const host: TransferOperationHost = {
		allowed: () => true,
		hold: async (pages) => {
			events.held.push([...pages]);
			return {
				applied: async (_receipt, clear) => {
					if (applyFailure) throw new Error('head_unavailable');
					events.clears.push(clear);
				},
				release: () => events.releases++
			};
		},
		reviewStillCurrent: async () => current,
		inverseStillCurrent: async () => inverse
	};
	const transport: TransferOperationTransport = {
		prepare: async (_source, _input, key) => {
			events.prepareKeys.push(key);
			return review(kind);
		},
		commit: async (_review, key) => {
			events.commitKeys.push(key);
			return commit;
		},
		receipt: async () => status,
		undo: async (_receipt, key) => {
			events.undoKeys.push(key);
			return undo;
		},
		cancel: async () => ({ state: 'not_applied', reason: 'cancelled' })
	};
	return {
		op: new BlockTransferOperation(source, host, transport),
		events,
		setCommit: (value: TransferDecision) => (commit = value),
		setStatus: (value: TransferDecision) => (status = value),
		setUndo: (value: TransferDecision) => (undo = value),
		setCurrent: (value: boolean) => (current = value),
		setInverse: (value: boolean) => (inverse = value),
		setApplyFailure: (value: boolean) => (applyFailure = value)
	};
}
test('prepare has no editor barrier and commit requires reviewed-current consent; copy clears destination history only after Applied', async () => {
	const f = fixture('copy');
	assert(await f.op.prepare(request('copy')));
	assert.equal(f.events.held.length, 0);
	assert.equal(await f.op.commit(false), false);
	assert.equal(f.events.commitKeys.length, 0);
	f.setCurrent(false);
	assert.equal(await f.op.commit(true), false);
	assert.equal(f.events.held.length, 0);
	f.setCurrent(true);
	assert(await f.op.commit(true));
	assert.deepEqual(f.events.held, [[source, target]]);
	assert.deepEqual(f.events.clears, [true]);
	assert.equal(f.events.releases, 1);
	f.op.dispose();
});
test('unknown commit preserves the barrier and same-key explicit retry, never falling through to earlier native typing', async () => {
	const f = fixture();
	f.setCommit({ state: 'unknown' });
	await f.op.prepare(request());
	assert.equal(await f.op.commit(true), false);
	assert.equal(f.op.current().phase, 'unknown');
	assert.deepEqual(f.events.held, [[source, target]]);
	assert.equal(f.events.releases, 0);
	assert.equal(f.events.clears.length, 0);
	await f.op.resolve();
	assert.equal(f.op.current().phase, 'unknown');
	assert.equal(f.events.releases, 0);
	f.setCommit({ state: 'applied', id, kind: 'move', receipt: receipt() });
	assert(await f.op.retryUnknown());
	assert.equal(f.events.commitKeys[0], f.events.commitKeys[1]);
	assert.deepEqual(f.events.clears, [true]);
	assert.equal(f.events.releases, 1);
	f.op.dispose();
});
test('definitive NotApplied releases without clearing the pre-existing native Undo history', async () => {
	const f = fixture();
	f.setCommit({ state: 'not_applied', reason: 'stale_heads' });
	await f.op.prepare(request());
	assert.equal(await f.op.commit(true), false);
	assert.equal(f.op.current().phase, 'not_applied');
	assert.equal(f.events.clears.length, 0);
	assert.equal(f.events.releases, 1);
	f.op.dispose();
});
test('a stale publication review releases the edit hold and allows a fresh review with the same Page frontiers', async () => {
	const f = fixture();
	f.setCommit({ state: 'not_applied', reason: 'transfer_review_stale' });
	assert(await f.op.prepare(request()));
	assert.equal(await f.op.commit(true), false);
	assert.equal(f.op.current().error, 'review_changed');
	assert.equal(f.events.releases, 1);
	assert.equal(f.events.clears.length, 0);
	assert(await f.op.prepare(request()));
	assert.notEqual(f.events.prepareKeys[0], f.events.prepareKeys[1]);
	f.op.dispose();
});

test('ack-only Applied can settle this session without private payload while recovered older operations preserve new history', async () => {
	const own = fixture();
	own.setCommit({ state: 'applied', id, kind: 'move', receipt: null });
	await own.op.prepare(request());
	assert(await own.op.commit(true));
	assert.equal(own.op.current().detailsUnavailable, true);
	assert.equal(own.op.current().receipt, null);
	assert.deepEqual(own.events.clears, [true]);
	assert.equal(await own.op.undo(), false);
	const restored = fixture();
	restored.op.recover(review(), 'original-commit-key');
	restored.setStatus({ state: 'applied', id, kind: 'move', receipt: null });
	assert(await restored.op.resolve());
	assert.deepEqual(restored.events.clears, [false]);
	own.op.dispose();
	restored.op.dispose();
});
test('a changed destination blocks explicit inverse; failed inverse leaves original operation Applied and does not erase local history', async () => {
	const f = fixture();
	await f.op.prepare(request());
	await f.op.commit(true);
	f.setInverse(false);
	assert.equal(await f.op.undo(), false);
	assert.equal(f.events.undoKeys.length, 0);
	f.setInverse(true);
	f.setUndo({ state: 'not_applied', reason: 'intervening_edit' });
	assert.equal(await f.op.undo(), false);
	assert.equal(f.op.current().phase, 'applied');
	assert.equal(f.op.current().error, 'undo_conflict');
	assert.deepEqual(f.events.clears, [true]);
	f.op.dispose();
});
test('GET accepted after unknown Undo does not falsely resolve the inverse; retry uses the same inverse identity', async () => {
	const f = fixture();
	await f.op.prepare(request());
	await f.op.commit(true);
	f.setUndo({ state: 'unknown' });
	await f.op.undo();
	f.setStatus({ state: 'applied', id, kind: 'move', receipt: receipt() });
	assert.equal(await f.op.resolve(), false);
	assert.equal(f.op.current().phase, 'unknown');
	assert.equal(f.events.releases, 1);
	f.setUndo({ state: 'undone', id, kind: 'move', receipt: receipt() });
	assert(await f.op.retryUnknown());
	assert.equal(f.events.undoKeys[0], f.events.undoKeys[1]);
	assert.equal(f.op.current().phase, 'undone');
	assert.deepEqual(f.events.clears, [true, true]);
	f.op.dispose();
});
test('receipt scope mismatch cannot release a barrier or clear unrelated Page histories', async () => {
	const f = fixture();
	f.setCommit({
		state: 'applied',
		id,
		kind: 'move',
		receipt: { ...receipt(), targetPageId: 'other-page' }
	});
	await f.op.prepare(request());
	assert.equal(await f.op.commit(true), false);
	assert.equal(f.op.current().phase, 'unknown');
	assert.equal(f.events.clears.length, 0);
	assert.equal(f.events.releases, 0);
	f.op.dispose();
});
test('a full-reload checkpoint retries the original commit but never clears the new session history', async () => {
	const old = fixture();
	old.setCommit({ state: 'unknown' });
	await old.op.prepare(request());
	await old.op.commit(true);
	const checkpoint = old.op.checkpoint(),
		fresh = fixture();
	fresh.op.restore(checkpoint);
	assert.equal(fresh.op.current().phase, 'unknown');
	await fresh.op.retryUnknown();
	assert.deepEqual(fresh.events.commitKeys, [checkpoint.state.commitKey]);
	assert.deepEqual(fresh.events.clears, [false]);
});
test('an unknown inverse survives reload without turning GET accepted into a failed inverse or a fresh commit', async () => {
	const old = fixture();
	await old.op.prepare(request());
	await old.op.commit(true);
	old.setUndo({ state: 'unknown' });
	await old.op.undo();
	const checkpoint = old.op.checkpoint(),
		fresh = fixture();
	fresh.op.restore(checkpoint);
	fresh.setStatus({ state: 'applied', id, kind: 'move', receipt: receipt() });
	await fresh.op.resolve();
	assert.equal(fresh.op.current().phase, 'unknown');
	await fresh.op.retryUnknown();
	assert.deepEqual(fresh.events.undoKeys, [checkpoint.state.undoKey]);
	assert.deepEqual(fresh.events.commitKeys, []);
	assert.deepEqual(fresh.events.clears, [false]);
});

test('a confirmed receipt with an unavailable accepted head keeps the barrier and old history until resolution can finish', async () => {
	const f = fixture();
	await f.op.prepare(request());
	f.setApplyFailure(true);
	assert.equal(await f.op.commit(true), false);
	assert.equal(f.op.current().phase, 'unknown');
	assert.deepEqual(f.events.clears, []);
	assert.equal(f.events.releases, 0);
	f.setApplyFailure(false);
	f.setStatus({ state: 'applied', id, kind: 'move', receipt: receipt() });
	assert.equal(await f.op.resolve(), true);
	assert.deepEqual(f.events.clears, [true]);
	assert.equal(f.events.releases, 1);
});
