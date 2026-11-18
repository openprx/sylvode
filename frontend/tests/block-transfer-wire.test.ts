import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parsePreparedTransfer, transferDecision } from '../src/lib/flow/block-transfer/wire';
import type { ReviewedBlockTransfer } from '../src/lib/flow/block-transfer/controller';
const source = '11111111-1111-4111-8111-111111111111',
	target = '22222222-2222-4222-8222-222222222222';
const review: ReviewedBlockTransfer = {
	id: '33333333-3333-4333-8333-333333333333',
	kind: 'move',
	sourcePageId: source,
	targetPageId: target,
	sourceFrontier: 'AA==',
	targetFrontier: 'AQ==',
	digest: 'digest',
	expiresAt: 1000,
	memberCount: 1,
	assetCount: 0,
	affectedShareCount: 0,
	assetsReady: true
};
const inverse = () => ({
	transfer_id: review.id,
	kind: 'move',
	state: 'undone',
	receipt: {
		changed_page_ids: [source, target],
		source: { object_id: source, accepted_seq: 3, frontier: 'Ag==' },
		target: { object_id: target, accepted_seq: 4, frontier: 'Aw==' },
		mappings: [
			{ source_block_cid: 'cid:7@10:Map', target_block_cid: 'cid:9@20:Map', identity: 'preserved' }
		],
		source_placeholder_cid: null,
		affected_share_count: 0,
		undo: { state: 'unavailable' }
	}
});
test('inverse receipt keeps original Page roles, takes final heads from those roles, and rejects an invented server Redo', () => {
	const result = transferDecision(inverse(), review);
	assert.equal(result.state, 'undone');
	if (result.state !== 'undone') throw new Error('expected inverse');
	assert.equal(result.receipt?.sourcePageId, source);
	assert.equal(result.receipt?.sourcePostFrontier, 'Ag==');
	const bad = inverse();
	Object.assign(bad.receipt.undo, { expected_source_frontier: 'old' });
	assert.throws(() => transferDecision(bad, review));
	const swapped = inverse();
	swapped.receipt.source.object_id = target;
	assert.throws(() => transferDecision(swapped, review));
});
test('metadata-only Applied remains an acknowledgement without enabling a private detail fallback', () => {
	assert.deepEqual(
		transferDecision(
			{
				transfer_id: review.id,
				kind: 'move',
				state: 'accepted',
				receipt: null,
				expires_at: '2026-11-17T00:00:00Z',
				failure_reason: null
			},
			review
		),
		{ state: 'applied', id: review.id, kind: 'move', receipt: null }
	);
});

function backendFixture(name: string) {
	return JSON.parse(
		readFileSync(
			new URL(`./fixtures/block-transfer/transfer-${name}.json`, import.meta.url),
			'utf8'
		)
	);
}

test('decodes the retained Rust serde prepare, move, copy, inverse and redacted receipts', () => {
	const prepared = backendFixture('prepare');
	const dto = prepared.review;
	const accepted = backendFixture('accepted');
	const { review: fixed } = parsePreparedTransfer(prepared, dto.source_object_id, {
		kind: 'move',
		target_object_id: dto.target_object_id,
		root_block_cid: accepted.receipt.mappings[0].source_block_cid,
		selection: 'current_subtree',
		target_parent_block_cid: null,
		target_index: 0,
		expected_source_frontier: dto.source_frontier,
		expected_target_frontier: dto.target_frontier
	});
	assert.equal(fixed.memberCount, 2);
	for (const [name, expected] of [
		['accepted', 'applied'],
		['undone', 'undone']
	] as const) {
		const result = transferDecision(backendFixture(name), fixed);
		assert.equal(result.state, expected);
		assert(result.state === 'applied' || result.state === 'undone');
		assert.equal(result.receipt?.targetPageId, dto.target_object_id);
	}
	for (const [name, expected] of [
		['copy-accepted', 'applied'],
		['copy-undone', 'undone']
	] as const) {
		const wire = backendFixture(name);
		const result = transferDecision(wire, { ...fixed, kind: 'copy', id: wire.transfer_id });
		assert.equal(result.state, expected);
		assert(result.state === 'applied' || result.state === 'undone');
		assert.equal(result.receipt?.sourcePageId, dto.source_object_id);
	}
	assert.deepEqual(transferDecision(backendFixture('metadata-only'), fixed), {
		state: 'undone',
		id: fixed.id,
		kind: 'move',
		receipt: null
	});
});
