import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LoroDoc, LoroList, LoroMap } from 'loro-crdt';
import * as pm from 'prosemirror-model';
import { EditorState } from 'prosemirror-state';
import { createPageSchema } from '../src/lib/flow/editor-schema';
import {
	updateLoroToPmState,
	createNodeFromLoroObj,
	type LoroDocType
} from '../src/lib/flow/vendor/loro-prosemirror';
import {
	structureEntries,
	attachStructureContext
} from '../src/lib/flow/vendor/loro-prosemirror/page-structure';
import { prepareBlockCut, applyBlockCut, pasteBlockCut } from '../src/lib/flow/editor-block-cut';
import { pageUndoManager } from '../src/lib/flow/editor-session';
import { clearBodyClipboard } from '../src/lib/flow/body/clipboard';
const schema = createPageSchema(pm);
async function fixture() {
	const doc = new LoroDoc() as LoroDocType,
		mapping = new Map();
	const content = ['alpha', 'child', 'target'].map((text) =>
		schema.nodes.paragraph.create({ indent: 0 }, schema.text(text, [schema.marks.bold.create()]))
	);
	updateLoroToPmState(
		doc,
		mapping,
		EditorState.create({ schema, doc: schema.nodes.doc.create(null, content) })
	);
	const tree = (doc as LoroDoc).getTree('flow_editor_structure');
	tree.enableFractionalIndex(0);
	const list = doc.getMap('doc').get('children') as LoroList;
	const placements = (list.toArray() as unknown as LoroMap[]).map((block) => {
		const n = tree.createNode();
		n.data.set('block_cid', block.id);
		return n;
	});
	tree.move(placements[1].id, placements[0].id, 0);
	const meta = (doc as LoroDoc).getMap('flow_editor_structure_meta');
	meta.set('version', 1);
	meta.set('initialized', true);
	meta.set('model', 'rich_page_v4');
	doc.commit();
	const manager = await pageUndoManager(doc);
	const ids = structureEntries(doc).map((e) => e.block.id);
	return { doc, manager, ids };
}
function read(doc: LoroDocType) {
	const mapping = new Map();
	attachStructureContext(mapping, doc);
	return createNodeFromLoroObj(schema, doc.getMap('doc'), mapping) as pm.Node;
}
const fallback = schema.nodes.paragraph.create({ indent: 0 });
test('whole subtree cut and paste reuse body CIDs/marks, each Undo and Redo is an independent human intent', async () => {
	const { doc, manager, ids } = await fixture();
	const blocks = read(doc)
		.content.content.slice(0, 2)
		.map((n) => n.toJSON());
	const ticket = prepareBlockCut(doc, ids.slice(0, 2), blocks);
	applyBlockCut(doc, ticket, blocks, manager, fallback);
	assert.deepEqual(
		structureEntries(doc).map((e) => e.block.id),
		[ids[2]]
	);
	pasteBlockCut(doc, ticket, blocks, ids[2], manager);
	assert.deepEqual(
		structureEntries(doc).map((e) => [e.block.id, e.depth]),
		[
			[ids[2], 0],
			[ids[0], 0],
			[ids[1], 1]
		]
	);
	assert.equal(read(doc).child(2).firstChild?.marks[0].type.name, 'bold');
	assert.equal((doc.getMap('doc').get('children') as LoroList).length, 3);
	assert.throws(() => pasteBlockCut(doc, ticket, blocks, ids[2], manager));
	manager.undo();
	assert.deepEqual(
		structureEntries(doc).map((e) => e.block.id),
		[ids[2]]
	);
	manager.undo();
	assert.deepEqual(
		structureEntries(doc).map((e) => e.block.id),
		ids
	);
	manager.redo();
	manager.redo();
	assert.deepEqual(
		structureEntries(doc).map((e) => e.block.id),
		[ids[2], ids[0], ids[1]]
	);
	const restored = new LoroDoc() as LoroDocType;
	restored.import(doc.export({ mode: 'snapshot' }));
	assert.deepEqual(read(restored).toJSON(), read(doc).toJSON());
});
test('full Page cut creates an excluded placeholder carrier and undo/redo retain original body identity', async () => {
	const { doc, manager, ids } = await fixture();
	const blocks = read(doc).content.content.map((n) => n.toJSON());
	const ticket = prepareBlockCut(doc, ids, blocks);
	applyBlockCut(doc, ticket, blocks, manager, fallback);
	const target = structureEntries(doc)[0].block.id;
	assert.equal(read(doc).childCount, 1);
	assert.equal(read(doc).textContent, '');
	manager.undo();
	assert.deepEqual(
		structureEntries(doc).map((e) => e.block.id),
		ids
	);
	manager.redo();
	assert.equal(structureEntries(doc)[0].block.id, target);
	assert.throws(() => pasteBlockCut(doc, ticket, blocks, target, manager)); // Undo/Redo invalidates the previous cut lifecycle.
	manager.undo();
	const fresh = prepareBlockCut(doc, ids, blocks);
	applyBlockCut(doc, fresh, blocks, manager, fallback);
	pasteBlockCut(doc, fresh, blocks, structureEntries(doc)[0].block.id, manager);
	assert.deepEqual(
		structureEntries(doc)
			.slice(1)
			.map((entry) => entry.block.id),
		ids
	);
});
test('foreign/replayed/revoked cut or concurrently restored/edited source cannot mutate document or undo stack', async () => {
	const { doc, manager, ids } = await fixture(),
		other = await fixture();
	const blocks = read(doc)
		.content.content.slice(0, 2)
		.map((n) => n.toJSON());
	assert.throws(() => prepareBlockCut(doc, [ids[0]], blocks.slice(0, 1))); // hidden child cannot be stranded
	const ticket = prepareBlockCut(doc, ids.slice(0, 2), blocks);
	applyBlockCut(doc, ticket, blocks, manager, fallback);
	const vv = doc.oplogVersion().encode();
	assert.throws(() => pasteBlockCut(other.doc, ticket, blocks, other.ids[2], other.manager));
	assert.throws(() => pasteBlockCut(doc, ticket, [{}], ids[2], manager));
	assert.deepEqual(doc.oplogVersion().encode(), vv);
	manager.undo();
	const before = doc.oplogVersion().encode();
	assert.throws(() => pasteBlockCut(doc, ticket, blocks, ids[2], manager));
	assert.deepEqual(doc.oplogVersion().encode(), before);
	manager.redo();
	clearBodyClipboard(doc);
	assert.throws(() => pasteBlockCut(doc, ticket, blocks, ids[2], manager));
});

test('old canonical Page captures cut intent before explicit structure initialization and rejects intervening content edits', async () => {
	const { doc, manager, ids } = await fixture();
	// A separate old plain snapshot has the same physical blocks and no structure metadata.
	const plain = new LoroDoc() as LoroDocType,
		mapping = new Map();
	updateLoroToPmState(plain, mapping, EditorState.create({ schema, doc: read(doc) }));
	const physical = plain.getMap('doc').get('children') as LoroList;
	const ownIds = (physical.toArray() as unknown as { id: string }[]).map((block) => block.id);
	const blocks = read(doc)
		.content.content.slice(0, 2)
		.map((node) => node.toJSON());
	const ticket = prepareBlockCut(plain, ownIds.slice(0, 2), blocks);
	const tree = (plain as LoroDoc).getTree('flow_editor_structure');
	tree.enableFractionalIndex(0);
	const nodes = ownIds.map((cid) => {
		const node = tree.createNode();
		node.data.set('block_cid', cid);
		return node;
	});
	tree.move(nodes[1].id, nodes[0].id, 0);
	const meta = (plain as LoroDoc).getMap('flow_editor_structure_meta');
	meta.set('version', 1);
	meta.set('initialized', true);
	meta.set('model', 'rich_page_v4');
	plain.commit({ origin: 'remote:init' });
	const undo = await pageUndoManager(plain);
	applyBlockCut(plain, ticket, blocks, undo, fallback);
	pasteBlockCut(plain, ticket, blocks, ownIds[2], undo);
	assert.deepEqual(
		structureEntries(plain).map((entry) => entry.block.id),
		[ownIds[2], ownIds[0], ownIds[1]]
	);
	const payload = read(doc)
		.content.content.slice(0, 2)
		.map((node) => node.toJSON());
	const editedTicket = prepareBlockCut(doc, ids.slice(0, 2), payload);
	applyBlockCut(doc, editedTicket, payload, manager, fallback);
	const carrier = doc.getContainerById(ids[0]);
	assert(carrier instanceof LoroMap);
	const attrs = carrier.get('attributes');
	assert(attrs instanceof LoroMap);
	attrs.set('indent', 7);
	doc.commit({ origin: 'remote:edit' });
	const before = doc.oplogVersion().encode();
	assert.throws(() => pasteBlockCut(doc, editedTicket, payload, ids[2], manager));
	assert.deepEqual(doc.oplogVersion().encode(), before);
});

test('production cut and paste export exactly one final valid packet per intent for Rust interop', async () => {
	const { doc, manager, ids } = await fixture();
	const { editorLocalUpdate } = await import('../src/lib/flow/editor-batch');
	const packets: Uint8Array[] = [];
	const sink = (value: Uint8Array) => packets.push(value);
	doc.subscribeLocalUpdates((bytes) => editorLocalUpdate(doc, bytes, sink));
	const before = doc.export({ mode: 'snapshot' }),
		blocks = read(doc).content.content.map((node) => node.toJSON());
	const ticket = prepareBlockCut(doc, ids, blocks);
	applyBlockCut(doc, ticket, blocks, manager, fallback);
	assert.equal(packets.length, 1);
	const cut = doc.export({ mode: 'snapshot' }),
		cutUpdate = packets[0];
	const placeholder = structureEntries(doc)[0].block.id;
	pasteBlockCut(doc, ticket, blocks, placeholder, manager);
	assert.equal(packets.length, 2);
	const pasted = doc.export({ mode: 'snapshot' }),
		pasteUpdate = packets[1];
	manager.undo();
	const undoPaste = doc.export({ mode: 'snapshot' });
	manager.undo();
	const undoCut = doc.export({ mode: 'snapshot' });
	assert.deepEqual(
		structureEntries(doc).map((entry) => entry.block.id),
		ids
	);
	if (process.env.WRITE_PAGE_CUT_FIXTURE === '1') {
		const { writeFileSync } = await import('node:fs');
		for (const [name, value] of Object.entries({
			'before.snapshot': before,
			'cut.update': cutUpdate,
			'cut.snapshot': cut,
			'paste.update': pasteUpdate,
			'paste.snapshot': pasted,
			'undo-paste.snapshot': undoPaste,
			'undo-cut.snapshot': undoCut
		}))
			writeFileSync('/tmp/sv-page-cut.' + name, value);
		writeFileSync(
			'/tmp/sv-page-cut.manifest.json',
			JSON.stringify(
				{
					ids,
					placeholder,
					cutPackets: 1,
					pastePackets: 1,
					preorderAfterPaste: [placeholder, ...ids],
					depthsAfterPaste: [0, 0, 1, 0]
				},
				null,
				2
			)
		);
	}
});

test('copy before identical source inserts new identities at the requested position, never steals source mappings', async () => {
	const { doc, manager, ids } = await fixture();
	// Move target before source, then insert closed copied subtree immediately after target.
	const entries = structureEntries(doc);
	entries[2].tree.moveBefore(entries[0].tree);
	doc.commit({ origin: 'flow:structure' });
	const mapping = new Map();
	attachStructureContext(mapping, doc);
	const old = createNodeFromLoroObj(schema, doc.getMap('doc'), mapping) as pm.Node;
	const clones = old.content.content.slice(1).map((node) => schema.nodeFromJSON(node.toJSON()));
	let state = EditorState.create({ schema, doc: old });
	state = state.apply(state.tr.insert(old.firstChild!.nodeSize, pm.Fragment.fromArray(clones)));
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	const after = structureEntries(doc).map((entry) => entry.block.id);
	assert.equal(after[0], ids[2]);
	assert(!ids.includes(after[1]));
	assert(!ids.includes(after[2]));
	assert.deepEqual(after.slice(3), ids.slice(0, 2));
	assert.deepEqual(
		structureEntries(doc).map((entry) => entry.depth),
		[0, 0, 1, 0, 1]
	);
	manager.undo();
	assert.deepEqual(
		structureEntries(doc).map((entry) => entry.block.id),
		[ids[2], ids[0], ids[1]]
	);
	manager.redo();
	assert.deepEqual(
		structureEntries(doc).map((entry) => entry.block.id),
		after
	);
});

test('deleting an identical sibling cannot steal the remaining sibling body identity', async () => {
	const { doc, manager } = await fixture();
	const mapping = new Map();
	attachStructureContext(mapping, doc);
	let node = createNodeFromLoroObj(schema, doc.getMap('doc'), mapping) as pm.Node;
	let state = EditorState.create({ schema, doc: node });
	// Make the two root blocks identical, keeping the nested child distinct.
	state = state.apply(
		state.tr.insertText(
			'alpha',
			node.child(0).nodeSize + node.child(1).nodeSize + 1,
			node.content.size - 1
		)
	);
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	mapping.clear();
	attachStructureContext(mapping, doc);
	node = createNodeFromLoroObj(schema, doc.getMap('doc'), mapping) as pm.Node;
	const keep = structureEntries(doc)[2].block.id;
	state = EditorState.create({ schema, doc: node });
	state = state.apply(state.tr.delete(0, node.child(0).nodeSize + node.child(1).nodeSize));
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	assert.equal(structureEntries(doc)[0].block.id, keep);
});
