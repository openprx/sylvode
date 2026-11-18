import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { LoroDoc, LoroList } from 'loro-crdt';
import type { LoroDocType } from 'loro-prosemirror';
import { applyBodyIntent } from '../src/lib/flow/body/doc-intents';
import { pageUndoManager, releaseEditorSession } from '../src/lib/flow/editor-session';
import { structureEntries } from '../src/lib/flow/vendor/loro-prosemirror/page-structure';
import { editorLocalUpdate } from '../src/lib/flow/editor-batch';
const asset = '11111111-1111-4111-8111-111111111111';
async function fixture() {
	const doc = new LoroDoc() as LoroDocType;
	doc.import(await readFile(new URL('./fixtures/page-v4/rust-rich.snapshot', import.meta.url)));
	return doc;
}
test('view-independent local media intent captures original CID, preserves source and sticky model5 across Undo/Redo', async () => {
	const doc = await fixture();
	const entries = structureEntries(doc);
	const before = entries.map((entry) => entry.block.id);
	const packets: Uint8Array[] = [];
	const sink = (bytes: Uint8Array) => packets.push(bytes);
	const unsub = doc.subscribeLocalUpdates((bytes) => editorLocalUpdate(doc, bytes, sink));
	const inserted = await applyBodyIntent(
		doc,
		{ blockId: before[0], side: 'after' },
		{ kind: 'image', attrs: { asset_id: asset, alt: 'A generated fixture', caption: 'Caption' } },
		() => true
	);
	assert(inserted);
	assert.equal(packets.length, 1);
	const after = structureEntries(doc);
	assert.deepEqual(
		after.map((e) => e.depth),
		[0, 1, 0, 0]
	);
	assert.equal(after[2].block.id, inserted!.blockId);
	assert.equal(after[2].block.get('nodeName'), 'image');
	assert.equal((doc as LoroDoc).getMap('flow_editor_structure_meta').get('model'), 'rich_page_v5');
	const manager = await pageUndoManager(doc);
	manager.undo();
	assert.deepEqual(
		structureEntries(doc).map((e) => e.block.id),
		before
	);
	assert.equal((doc as LoroDoc).getMap('flow_editor_structure_meta').get('model'), 'rich_page_v5');
	manager.redo();
	assert.equal(structureEntries(doc)[2].block.id, inserted!.blockId);
	assert.equal((doc.getMap('doc').get('children') as LoroList).length, 4);
	unsub();
	releaseEditorSession(doc);
	doc.free();
});
test('missing original bookmark or lost document write authority is NotApplied, never redirected to another block', async () => {
	const doc = await fixture();
	const before = doc.toJSON();
	assert.equal(
		await applyBodyIntent(
			doc,
			{ blockId: 'cid:missing', side: 'after' },
			{ kind: 'childPage', attrs: { child_page_id: asset } },
			() => true
		),
		null
	);
	assert.equal(
		await applyBodyIntent(
			doc,
			{ blockId: structureEntries(doc)[0].block.id, side: 'after' },
			{ kind: 'childPage', attrs: { child_page_id: asset } },
			() => false
		),
		null
	);
	assert.deepEqual(doc.toJSON(), before);
	releaseEditorSession(doc);
	doc.free();
});

test('media Tree moves retain body identity and owned reference; replacement has its own reversible user intent', async () => {
	const doc = await fixture();
	const target = structureEntries(doc)[0].block.id;
	const inserted = await applyBodyIntent(
		doc,
		{ blockId: target, side: 'after' },
		{ kind: 'image', attrs: { asset_id: asset, alt: 'Before', caption: '' } },
		() => true
	);
	assert(inserted);
	const first = structureEntries(doc)[0];
	let media = structureEntries(doc).find((entry) => entry.block.id === inserted!.blockId)!;
	media.tree.moveBefore(first.tree);
	doc.commit({ origin: 'flow:structure' });
	media = structureEntries(doc)[0];
	assert.equal(media.block.id, inserted!.blockId);
	assert.equal(
		(media.block.get('attributes') as import('loro-crdt').LoroMap).get('asset_id'),
		asset
	);
	const replacement = '22222222-2222-4222-8222-222222222222';
	await applyBodyIntent(
		doc,
		{ blockId: inserted!.blockId, side: 'after' },
		{
			kind: 'image',
			attrs: { asset_id: replacement, alt: 'After', caption: 'New' },
			replace: true
		},
		() => true
	);
	const manager = await pageUndoManager(doc);
	manager.undo();
	media = structureEntries(doc).find((entry) => entry.block.id === inserted!.blockId)!;
	assert.equal(
		(media.block.get('attributes') as import('loro-crdt').LoroMap).get('asset_id'),
		asset
	);
	manager.redo();
	media = structureEntries(doc).find((entry) => entry.block.id === inserted!.blockId)!;
	assert.equal(
		(media.block.get('attributes') as import('loro-crdt').LoroMap).get('asset_id'),
		replacement
	);
	releaseEditorSession(doc);
	doc.free();
});

test('real model5 local intent packets retain the sticky floor and typed UUID references for backend validation', async () => {
	if (process.env.WRITE_PAGE_V5_FIXTURE !== '1') return;
	const { writeFile } = await import('node:fs/promises');
	for (const kind of ['image', 'attachment', 'childPage'] as const) {
		const doc = await fixture();
		const before = doc.export({ mode: 'snapshot' });
		const target = structureEntries(doc)[0].block.id;
		const packets: Uint8Array[] = [];
		const sink = (bytes: Uint8Array) => packets.push(bytes);
		const stop = doc.subscribeLocalUpdates((bytes) => editorLocalUpdate(doc, bytes, sink));
		const attrs: Record<string, string> =
			kind === 'image'
				? { asset_id: asset, alt: 'Synthetic 中文😀', caption: 'Caption' }
				: kind === 'attachment'
					? { asset_id: asset, name: '说明.json', mime: 'application/json', caption: 'Attachment' }
					: { child_page_id: '33333333-3333-4333-8333-333333333333' };
		const result = await applyBodyIntent(
			doc,
			{ blockId: target, side: 'after' },
			{ kind, attrs },
			() => true
		);
		assert(result);
		assert.equal(packets.length, 1);
		await writeFile(`/tmp/sv-page-v5-${kind}.before.snapshot`, before);
		await writeFile(`/tmp/sv-page-v5-${kind}.update`, packets[0]);
		await writeFile(`/tmp/sv-page-v5-${kind}.snapshot`, doc.export({ mode: 'snapshot' }));
		const manager = await pageUndoManager(doc);
		manager.undo();
		assert.equal(
			(doc as LoroDoc).getMap('flow_editor_structure_meta').get('model'),
			'rich_page_v5'
		);
		await writeFile(`/tmp/sv-page-v5-${kind}.undo.snapshot`, doc.export({ mode: 'snapshot' }));
		manager.redo();
		assert.equal(structureEntries(doc)[2].block.id, result!.blockId);
		await writeFile(`/tmp/sv-page-v5-${kind}.redo.snapshot`, doc.export({ mode: 'snapshot' }));
		await writeFile(
			`/tmp/sv-page-v5-${kind}.manifest.json`,
			JSON.stringify(
				{
					kind,
					reference: asset,
					child_page_id: '33333333-3333-4333-8333-333333333333',
					block_id: result!.blockId,
					packets: 1,
					floor: 'rich_page_v5',
					depths: structureEntries(doc).map((entry) => entry.depth)
				},
				null,
				2
			)
		);
		stop();
		releaseEditorSession(doc);
		doc.free();
	}
});

test('an empty Page bookmark inserts the first body block and refuses a stale empty target', async () => {
	const doc = await fixture();
	const tree = (doc as LoroDoc).getTree('flow_editor_structure');
	for (const root of tree.roots()) tree.delete(root.id);
	doc.commit({ origin: 'fixture:empty' });
	const bookmark = { blockId: null, side: 'before' } as const;
	const intent = { kind: 'childPage', attrs: { child_page_id: asset } } as const;
	const first = await applyBodyIntent(doc, bookmark, intent, () => true);
	assert(first);
	assert.deepEqual(
		structureEntries(doc).map((entry) => [entry.depth, entry.block.id]),
		[[0, first.blockId]]
	);
	assert.equal(await applyBodyIntent(doc, bookmark, intent, () => true), null);
	releaseEditorSession(doc);
	doc.free();
});
