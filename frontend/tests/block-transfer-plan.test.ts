import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	planBlockTransfer,
	selectedTransferBlocks,
	type TransferPageReadiness
} from '../src/lib/flow/block-transfer/plan';
import { transferUndoPolicy } from '../src/lib/flow/block-transfer/contracts';
const source = '11111111-1111-4111-8111-111111111111',
	target = '22222222-2222-4222-8222-222222222222';
const page = (
	id: string,
	actions: string[] = ['view', 'edit', 'manage']
): TransferPageReadiness => ({
	object: {
		id,
		title: 'Authorized fixture',
		object_type: 'page',
		lifecycle_status: 'active',
		permitted_actions: actions,
		editor_state: 'ready'
	},
	frontier: `original-server-token-${id}`,
	nativeVersionMatches: true,
	pending: 0,
	persistedPending: 0,
	readinessKnown: true,
	localReplica: 'open',
	bodyJobs: 0,
	enqueuePending: false,
	paused: false
});
const blocks = [
	{ cid: 'cid:1@991001:Map', depth: 0 },
	{ cid: 'cid:2@991001:Map', depth: 1 },
	{ cid: 'cid:3@991001:Map', depth: 2 },
	{ cid: 'cid:4@991001:Map', depth: 0 }
];
const input = () => ({
	kind: 'move' as const,
	source: page(source),
	target: page(target),
	rootCid: blocks[0].cid,
	selection: 'current_subtree' as const,
	sourceBlocks: blocks,
	targetParentCid: null,
	targetSiblingIndex: 2
});
test('cross-Page review captures a complete relative subtree and original server tokens without nominating logical identities', () => {
	const result = planBlockTransfer(input());
	assert.deepEqual(result.fixedPreview, blocks.slice(0, 3));
	assert.equal(result.request.expected_source_frontier, page(source).frontier);
	assert.equal(result.request.expected_target_frontier, page(target).frontier);
	assert.equal(result.request.target_index, 2);
	assert.equal('logical_uuid' in result.request, false);
	assert.equal('snapshot' in result.request, false);
});
test('a move never silently expands a block into its children; a copy can explicitly select only the root', () => {
	assert.throws(
		() => selectedTransferBlocks(blocks, blocks[0].cid, 'move', 'block'),
		/explicit_subtree_required/
	);
	assert.equal(selectedTransferBlocks(blocks, blocks[0].cid, 'copy', 'block').length, 1);
});
test('source View is sufficient only for copy, target Edit only for copy, and unsaved/unknown pages are not actuated', () => {
	const value = input();
	assert.doesNotThrow(() =>
		planBlockTransfer({
			...value,
			kind: 'copy',
			source: page(source, ['view']),
			target: page(target, ['view', 'edit'])
		})
	);
	assert.throws(
		() => planBlockTransfer({ ...value, source: page(source, ['view', 'edit']) }),
		/source_permission/
	);
	for (const patch of [
		{ pending: 1 },
		{ bodyJobs: 1 },
		{ enqueuePending: true },
		{ nativeVersionMatches: false },
		{ paused: true }
	])
		assert.throws(
			() => planBlockTransfer({ ...value, target: { ...value.target, ...patch } }),
			/unsaved_content/
		);
	assert.throws(() => planBlockTransfer({ ...value, target: page(source) }), /same_page/);
});
test('native Undo reset policy changes only accepted changed documents and never pretends physical CID equality across Pages', () => {
	assert.deepEqual(transferUndoPolicy('copy', source, target).changedPageIds, [target]);
	assert.deepEqual(transferUndoPolicy('move', source, target).changedPageIds, [source, target]);
	assert.equal(transferUndoPolicy('move', source, target).notApplied, 'preserve_history');
	assert.equal(
		transferUndoPolicy('move', source, target).unknown,
		'keep_locked_until_authoritative_receipt'
	);
});
