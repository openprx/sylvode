import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { LoroDoc, LoroText } from 'loro-crdt';
import type { LoroDocType } from 'loro-prosemirror';
import { TaskWorkbench } from '../src/lib/workbench/tabs';
import { captureBlockShareReview, sourceVersion } from '../src/lib/flow/shared-blocks/review';
import { structureEntries } from '../src/lib/flow/vendor/loro-prosemirror/page-structure';
import { SourceBlockShares } from '../src/lib/flow/shared-blocks/source';
import { sharedBlockApi } from '../src/lib/flow/shared-blocks/client';
import type { ManagedBlockShare } from '../src/lib/flow/shared-blocks/contracts';
const workspace = '11111111-1111-4111-8111-111111111111',
	user = '22222222-2222-4222-8222-222222222222',
	shareId = '33333333-3333-4333-8333-333333333333';
async function fixture() {
	const doc = new LoroDoc() as LoroDocType;
	doc.import(await readFile(new URL('./fixtures/page-v4/rust-rich.snapshot', import.meta.url)));
	return doc;
}
test('source review pins the exact accepted native version and fixed selection; later private edits cannot silently join publication', async () => {
	const doc = await fixture(),
		root = structureEntries(doc)[0].block.id,
		frontier = sourceVersion(doc);
	const review = captureBlockShareReview(doc, root, 'current_subtree', frontier);
	assert.equal(captureBlockShareReview(doc, root, 'block', frontier).members.length, 1);
	assert(review.members.length > 1);
	const original = sharedBlockApi.create;
	let sent = 0;
	sharedBlockApi.create = async () => {
		sent++;
		throw new Error('must not publish');
	};
	const manager = new SourceBlockShares({
		sourceId: 'original-source',
		allowed: () => true,
		review: async (root, selection) => ({
			ok: true,
			review: captureBlockShareReview(doc, root, selection, sourceVersion(doc))
		}),
		current: async (review) => sourceVersion(doc) === review.expected_frontier
	});
	try {
		manager.edit({
			rootCid: root,
			selection: 'current_subtree',
			workspaceId: workspace,
			audience: 'users',
			recipients: [user]
		});
		assert(await manager.review());
		const text = (structureEntries(doc)[0].block.get('children') as import('loro-crdt').LoroList)
			.toArray()
			.find((value) => value instanceof LoroText) as LoroText;
		text.insert(text.length, ' private update after review');
		doc.commit();
		assert.equal(await manager.publish(), false);
		assert.equal(sent, 0);
		assert.equal(manager.current().error, 'source_changed');
		assert.equal(manager.current().review, null);
		assert.throws(() => captureBlockShareReview(doc, root, 'current_subtree', frontier));
	} finally {
		sharedBlockApi.create = original;
		manager.dispose();
		doc.free();
	}
});
test('uncertain publication retry keeps original Page, fixed review, body and key even after source changes', async () => {
	const doc = await fixture(),
		root = structureEntries(doc)[0].block.id,
		review = captureBlockShareReview(doc, root, 'block', sourceVersion(doc));
	const create = sharedBlockApi.create,
		manage = sharedBlockApi.manage;
	const calls: { source: string; body: string; key: string }[] = [];
	let current = true;
	const output: ManagedBlockShare = {
		share_id: shareId,
		root_block_cid: root,
		selection: 'block',
		state: 'active',
		revision: 1,
		member_count: 1,
		audiences: [
			{ workspace_id: workspace, audience: 'users', recipient_user_ids: [user], permission: 'view' }
		]
	};
	sharedBlockApi.create = async (source, body, key) => {
		calls.push({ source, body: JSON.stringify(body), key });
		return calls.length === 1
			? { code: 500, message: 'response lost', data: null }
			: { code: 0, message: 'ok', data: output };
	};
	sharedBlockApi.manage = async () => ({ code: 0, message: 'ok', data: [output] });
	const manager = new SourceBlockShares({
		sourceId: 'original-source',
		allowed: () => true,
		review: async () => ({ ok: true, review }),
		current: async () => current
	});
	try {
		manager.edit({ rootCid: root, workspaceId: workspace, recipients: [user] });
		assert(await manager.review());
		assert.equal(await manager.publish(), false);
		assert.equal(manager.current().status, 'uncertain');
		current = false;
		assert.equal(manager.edit({ workspaceId: 'another-target' }), false);
		assert(await manager.publish());
		assert.equal(calls.length, 2);
		assert.deepEqual(calls[0], calls[1]);
		assert.equal(calls[0].source, 'original-source');
		assert.equal(JSON.parse(calls[0].body).expected_frontier, review.expected_frontier);
		assert.equal(manager.current().dirty, false);
	} finally {
		sharedBlockApi.create = create;
		sharedBlockApi.manage = manage;
		manager.dispose();
		doc.free();
	}
});
test('TaskTab auxiliary source drafts stay dirty independently of native pending and release on close/account change', () => {
	const workbench = new TaskWorkbench();
	workbench.activateUser('actor-a');
	workbench.visit('/flow/original');
	workbench.visit('/flow/other');
	let dirty = true,
		releases = 0;
	assert(
		workbench.retainAuxiliary('flow:original', 'block-sharing', {
			dirty: () => dirty,
			dispose: () => releases++
		})
	);
	workbench.markDirty('flow:original', false);
	assert.equal(workbench.current.tabs.find((tab) => tab.key === 'flow:original')?.dirty, true);
	assert.equal(workbench.close('flow:original').kind, 'confirm');
	dirty = false;
	workbench.refreshAuxiliaryDirty('flow:original');
	assert.equal(workbench.current.tabs.find((tab) => tab.key === 'flow:original')?.dirty, false);
	workbench.close('flow:original');
	assert.equal(releases, 1);
	workbench.retainAuxiliary('flow:other', 'block-sharing', {
		dirty: () => true,
		dispose: () => releases++
	});
	workbench.activateUser('actor-b');
	assert.equal(releases, 2);
	assert.equal(workbench.hasDirty(), false);
	assert.equal(
		workbench.retainAuxiliary('flow:missing', 'block-sharing', {
			dirty: () => true,
			dispose: () => {}
		}),
		false
	);
});

test('orphaned replies do not become Page-level/new-CID anchors and bound replies retain the parent anchor', async () => {
	const { commentCreationAnchor } = await import('../src/lib/flow/social-actions');
	assert.equal(
		commentCreationAnchor('new-body-cid', 'old-thread', [
			{ id: 'old-thread', block_id: 'reused-cid', anchor_status: 'orphaned' }
		]),
		null
	);
	assert.equal(
		commentCreationAnchor('other-body', 'bound-thread', [
			{ id: 'bound-thread', block_id: 'parent-body', anchor_status: 'bound' }
		]),
		'parent-body'
	);
	assert.equal(commentCreationAnchor('other-body', 'missing-thread', []), null);
	assert.equal(commentCreationAnchor('selected-body', null, []), 'selected-body');
});
