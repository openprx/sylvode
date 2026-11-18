import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LoroDoc, LoroMap, LoroList } from 'loro-crdt';
import * as pm from 'prosemirror-model';
import { EditorState } from 'prosemirror-state';
import { createPageSchema } from '../src/lib/flow/editor-schema';
import {
	updateLoroToPmState,
	createNodeFromLoroObj,
	type LoroDocType
} from '../src/lib/flow/vendor/loro-prosemirror';
import {
	attachStructureContext,
	structureEntries
} from '../src/lib/flow/vendor/loro-prosemirror/page-structure';
const schema = createPageSchema(pm);
function fixture() {
	const doc = new LoroDoc() as LoroDocType;
	doc.setPeerId('991001');
	const mapping = new Map();
	const node = schema.node(
		'doc',
		null,
		['one', 'two', 'three'].map((text) =>
			schema.node('paragraph', { indent: 0 }, schema.text(text))
		)
	);
	updateLoroToPmState(doc, mapping, EditorState.create({ schema, doc: node }));
	const tree = (doc as LoroDoc).getTree('flow_editor_structure');
	tree.enableFractionalIndex(0);
	for (const block of (doc.getMap('doc').get('children') as LoroList).toArray() as LoroMap[]) {
		const n = tree.createNode();
		n.data.set('block_cid', block.id);
	}
	const meta = (doc as LoroDoc).getMap('flow_editor_structure_meta');
	meta.set('initialized', true);
	meta.set('version', 1);
	meta.set('model', 'rich_page_v4');
	doc.commit();
	mapping.clear();
	attachStructureContext(mapping, doc);
	return { doc, mapping, tree };
}
function read(doc: LoroDocType, mapping: Map<any, any>) {
	mapping.clear();
	attachStructureContext(mapping, doc);
	return createNodeFromLoroObj(schema, doc.getMap('doc'), mapping) as pm.Node;
}
test('Tree move preserves physical CID and next logical edits target the moved content', () => {
	const { doc, mapping, tree } = fixture();
	const original = structureEntries(doc).map((e) => e.block.id);
	const nodes = tree.roots();
	tree.move(nodes[2].id, undefined, 0);
	doc.commit();
	let node = read(doc, mapping);
	assert.deepEqual(
		node.content.content.map((n) => n.textContent),
		['three', 'one', 'two']
	);
	let state = EditorState.create({ schema, doc: node });
	state = state.apply(state.tr.insertText('!', 2));
	updateLoroToPmState(doc, mapping, state);
	assert.deepEqual(
		structureEntries(doc).map((e) => e.block.id),
		[original[2], original[0], original[1]]
	);
	assert.equal(read(doc, mapping).firstChild?.textContent, 't!hree');
	assert.equal((doc.getMap('doc').get('children') as LoroList).length, 3);
	const restored = new LoroDoc() as LoroDocType;
	restored.import(doc.export({ mode: 'snapshot' }));
	assert.deepEqual(read(restored, new Map()).toJSON(), read(doc, mapping).toJSON());
});
test('nested subtree move, insert, duplicate and delete maintain physical references', () => {
	const { doc, mapping, tree } = fixture();
	const roots = tree.roots();
	tree.move(roots[1].id, roots[0].id, 0);
	doc.commit();
	let node = read(doc, mapping);
	assert.deepEqual(
		node.content.content.map((n) => n.attrs.indent),
		[0, 1, 0]
	);
	let state = EditorState.create({ schema, doc: node });
	state = state.apply(state.tr.insertText('X', 2));
	updateLoroToPmState(doc, mapping, state);
	assert.deepEqual(
		structureEntries(doc).map((e) => e.depth),
		[0, 1, 0]
	);
	const source = structureEntries(doc)[0].block.id;
	state = EditorState.create({ schema, doc: read(doc, mapping) });
	const clone = state.doc.lastChild!;
	state = state.apply(state.tr.insert(state.doc.content.size, clone));
	updateLoroToPmState(doc, mapping, state);
	assert.equal(structureEntries(doc).length, 4);
	assert.equal(structureEntries(doc)[0].block.id, source);
	state = EditorState.create({ schema, doc: read(doc, mapping) });
	const first = state.doc.firstChild!;
	const second = state.doc.child(1);
	state = state.apply(state.tr.delete(0, first.nodeSize + second.nodeSize));
	updateLoroToPmState(doc, mapping, state);
	assert.equal(structureEntries(doc).length, 2);
	assert.equal((doc.getMap('doc').get('children') as LoroList).length, 4);
});

test('rich Tree interop artifact preserves Unicode, overlapping marks and canonical identities', async () => {
	const { doc, mapping, tree } = fixture();
	let node = read(doc, mapping);
	let state = EditorState.create({ schema, doc: node });
	state = state.apply(
		state.tr
			.insertText('中文😀', 2)
			.addMark(1, 4, schema.marks.bold.create())
			.addMark(2, 6, schema.marks.italic.create())
	);
	updateLoroToPmState(doc, mapping, state);
	const roots = tree.roots();
	tree.move(roots[1].id, roots[0].id, 0);
	tree.move(roots[2].id, undefined, 0);
	doc.commit();
	node = read(doc, mapping);
	assert.deepEqual(
		node.content.content.map((n) => n.textContent),
		['three', 'o中文😀ne', 'two']
	);
	assert.deepEqual(
		structureEntries(doc).map((e) => e.depth),
		[0, 0, 1]
	);
	if (process.env.WRITE_PAGE_STRUCTURE_FIXTURE === '1') {
		const { writeFile } = await import('node:fs/promises');
		await writeFile('/tmp/sv-page-rich-v4-js.snapshot', doc.export({ mode: 'snapshot' }));
		await writeFile(
			'/tmp/sv-page-rich-v4-js.json',
			JSON.stringify(
				{
					pm: node.toJSON(),
					blocks: structureEntries(doc).map((e) => ({
						cid: e.block.id,
						tree_id: e.tree.id,
						depth: e.depth
					})),
					physical: (doc.getMap('doc').get('children') as LoroList).toJSON()
				},
				null,
				2
			)
		);
	}
});

test('real Rust CID edit and Tree move rehydrate through vendor and accept next browser edit', async () => {
	const { readFile } = await import('node:fs/promises');
	const bytes = await readFile(new URL('./fixtures/page-v4/rust-rich.snapshot', import.meta.url));
	const doc = new LoroDoc() as LoroDocType;
	doc.import(bytes);
	const mapping = new Map();
	let node = read(doc, mapping);
	assert.deepEqual(
		node.content.content.map((n) => n.textContent),
		['o中文😀ne Rust', 'two', 'three']
	);
	assert.deepEqual(
		structureEntries(doc).map((e) => e.depth),
		[0, 1, 0]
	);
	const identities = structureEntries(doc).map((e) => e.block.id);
	assert.deepEqual(
		node.firstChild!.child(1).marks.map((m) => m.type.name),
		['bold', 'italic']
	);
	let state = EditorState.create({ schema, doc: node });
	state = state.apply(state.tr.insertText(' JS', state.doc.firstChild!.nodeSize - 1));
	updateLoroToPmState(doc, mapping, state);
	node = read(doc, mapping);
	assert.equal(node.firstChild!.textContent, 'o中文😀ne Rust JS');
	assert.deepEqual(
		structureEntries(doc).map((e) => e.block.id),
		identities
	);
	assert.deepEqual(
		node.firstChild!.child(1).marks.map((m) => m.type.name),
		['bold', 'italic']
	);
});

test('after native Tree movement a rich source stays intact when a different target is split', async () => {
	const { TextSelection } = await import('prosemirror-state');
	const { splitBlock } = await import('prosemirror-commands');
	const { doc, mapping, tree } = fixture();
	let state = EditorState.create({ schema, doc: read(doc, mapping) });
	state = state.apply(
		state.tr
			.insertText('Omeg', 1, 4)
			.addMark(1, 5, schema.marks.bold.create())
			.addMark(1, 5, schema.marks.italic.create())
	);
	updateLoroToPmState(doc, mapping, state);
	let roots = tree.roots();
	tree.move(roots[1].id, roots[2].id, 0);
	doc.commit();
	roots = tree.roots();
	const child = roots[1].children()![0];
	child.moveBefore(roots[1]);
	doc.commit();
	const before = read(doc, mapping);
	const original = structureEntries(doc).map((e) => e.block.id);
	state = EditorState.create({ schema, doc: before });
	state = state.apply(
		state.tr.setSelection(TextSelection.create(state.doc, state.doc.content.size - 1))
	);
	assert.equal(
		splitBlock(state, (tr) => {
			state = state.apply(tr);
		}),
		true
	);
	updateLoroToPmState(doc, mapping, state);
	const after = read(doc, mapping);
	assert.deepEqual(
		after.content.content.map((n) => n.textContent),
		['Omeg', 'two', 'three', '']
	);
	assert.deepEqual(
		structureEntries(doc)
			.slice(0, 3)
			.map((e) => e.block.id),
		original
	);
	assert.deepEqual(
		after.firstChild!.firstChild!.marks.map((m) => m.type.name),
		['bold', 'italic']
	);
});

test('native Tree deletion undo restores the same body CID and remains editable with historic placements', async () => {
	const { UndoManager } = await import('loro-crdt');
	const { doc, mapping, tree } = fixture();
	const manager = new UndoManager(doc, { mergeInterval: 0 });
	const before = structureEntries(doc);
	const target = before[0];
	const origins: string[] = [];
	const stop = doc.subscribe((event) => {
		origins.push(event.origin ?? '');
	});
	let state = EditorState.create({ schema, doc: read(doc, mapping) });
	state = state.apply(state.tr.delete(0, state.doc.firstChild!.nodeSize));
	updateLoroToPmState(doc, mapping, state);
	assert.equal(
		structureEntries(doc).some((entry) => entry.block.id === target.block.id),
		false
	);
	manager.undo();
	let restored = structureEntries(doc);
	assert.equal(restored[0].block.id, target.block.id);
	assert.notEqual(restored[0].tree.id, target.tree.id);
	assert.equal(
		tree
			.getNodes({ withDeleted: true })
			.filter((node) => node.data.get('block_cid') === target.block.id).length,
		2
	);
	assert(origins.includes('undo'));
	state = EditorState.create({ schema, doc: read(doc, mapping) });
	state = state.apply(state.tr.insertText('!', 2));
	updateLoroToPmState(doc, mapping, state);
	restored = structureEntries(doc);
	assert.equal(restored[0].block.id, target.block.id);
	assert.equal(read(doc, mapping).firstChild!.textContent, 'o!ne');
	assert.equal(new Set(restored.map((entry) => entry.block.id)).size, restored.length);
	assert.equal((doc.getMap('doc').get('children') as LoroList).length, 3);
	stop();
	manager.free();
});

test('real Rust move/edit after native delete Undo reopens and accepts next browser edit', async () => {
	const { readFile } = await import('node:fs/promises');
	const doc = new LoroDoc() as LoroDocType;
	doc.import(
		await readFile(new URL('./fixtures/page-v4/rust-delete-undo.snapshot', import.meta.url))
	);
	const mapping = new Map();
	const node = read(doc, mapping);
	assert.deepEqual(
		node.content.content.map((child) => child.textContent),
		['o中文😀ne', 'two', 'Restored three', 'grandchild']
	);
	assert.deepEqual(
		structureEntries(doc).map((entry) => entry.depth),
		[0, 1, 1, 2]
	);
	const before = structureEntries(doc).map((entry) => entry.block.id);
	assert.equal(before[2], 'cid:21@991001:Map');
	let state = EditorState.create({ schema, doc: node });
	let end = 0;
	state.doc.forEach((child, pos) => {
		if (child.textContent === 'Restored three') end = pos + child.nodeSize - 1;
	});
	assert(end > 0);
	state = state.apply(state.tr.insertText(' JS', end));
	updateLoroToPmState(doc, mapping, state);
	assert.equal(read(doc, mapping).child(2).textContent, 'Restored three JS');
	assert.deepEqual(
		structureEntries(doc).map((entry) => entry.block.id),
		before
	);
	assert.deepEqual(
		structureEntries(doc).map((entry) => entry.depth),
		[0, 1, 1, 2]
	);
});

test('production storage batch splits once, emits one valid update and creation Undo/Redo retains its carrier', async () => {
	const { UndoManager } = await import('loro-crdt');
	const { TextSelection } = await import('prosemirror-state');
	const { splitBlock } = await import('prosemirror-commands');
	const { editorLocalUpdate, editorObservation } = await import('../src/lib/flow/editor-batch');
	const { doc, mapping } = fixture();
	const beforeSnapshot = doc.export({ mode: 'snapshot' });
	const manager = new UndoManager(doc, {
		mergeInterval: 0,
		excludeOriginPrefixes: ['sys:storage']
	});
	const packets: Uint8Array[] = [];
	const sink = (bytes: Uint8Array) => packets.push(bytes);
	const stop = doc.subscribeLocalUpdates((bytes) => editorLocalUpdate(doc, bytes, sink));
	const key = {};
	let reads = 0;
	const observe = doc.subscribe(() =>
		editorObservation(doc, key, () => {
			reads++;
			const physical = new Set(
				((doc.getMap('doc').get('children') as LoroList).toArray() as LoroMap[]).map(
					(block) => block.id
				)
			);
			for (const entry of structureEntries(doc)) assert(physical.has(entry.block.id));
		})
	);
	let state = EditorState.create({ schema, doc: read(doc, mapping) });
	state = state.apply(state.tr.setSelection(TextSelection.create(state.doc, 2)));
	splitBlock(state, (tr) => {
		state = state.apply(tr);
	});
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	assert.equal(packets.length, 1);
	assert.equal(reads, 1);
	assert.deepEqual(
		read(doc, mapping).content.content.map((n) => n.textContent),
		['o', 'ne', 'two', 'three']
	);
	const carrier = structureEntries(doc)[1].block.id;
	const splitSnapshot = doc.export({ mode: 'snapshot' });
	const physical = doc.getMap('doc').get('children') as LoroList;
	assert.equal(physical.length, 4);
	const replica = new LoroDoc() as LoroDocType;
	replica.import(beforeSnapshot);
	replica.import(packets[0]);
	assert.deepEqual(read(replica, new Map()).toJSON(), read(doc, mapping).toJSON());
	manager.undo();
	assert.deepEqual(
		read(doc, mapping).content.content.map((n) => n.textContent),
		['one', 'two', 'three']
	);
	assert.equal(physical.length, 4);
	assert.equal(manager.canUndo(), false);
	assert.equal(packets.length, 2);
	const undoSnapshot = doc.export({ mode: 'snapshot' });
	manager.redo();
	assert.equal(structureEntries(doc)[1].block.id, carrier);
	assert.deepEqual(
		read(doc, mapping).content.content.map((n) => n.textContent),
		['o', 'ne', 'two', 'three']
	);
	assert.equal(physical.length, 4);
	assert.equal(packets.length, 3);
	const redoSnapshot = doc.export({ mode: 'snapshot' });
	state = EditorState.create({ schema, doc: read(doc, mapping) });
	const secondEnd = state.doc.firstChild!.nodeSize + state.doc.child(1).nodeSize - 1;
	state = state.apply(state.tr.insertText('!', secondEnd));
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	assert.equal(read(doc, mapping).child(1).textContent, 'ne!');
	assert.equal(structureEntries(doc)[1].block.id, carrier);
	if (process.env.WRITE_PAGE_STORAGE_FIXTURE === '1') {
		const { writeFile } = await import('node:fs/promises');
		await writeFile('/tmp/sv-page-storage-actual.before.snapshot', beforeSnapshot);
		await writeFile('/tmp/sv-page-storage-actual.split.update', packets[0]);
		await writeFile('/tmp/sv-page-storage-actual.split.snapshot', splitSnapshot);
		await writeFile('/tmp/sv-page-storage-actual.undo.update', packets[1]);
		await writeFile('/tmp/sv-page-storage-actual.undo.snapshot', undoSnapshot);
		await writeFile('/tmp/sv-page-storage-actual.redo.update', packets[2]);
		await writeFile('/tmp/sv-page-storage-actual.redo-before-edit.snapshot', redoSnapshot);
		await writeFile('/tmp/sv-page-storage-actual.redo.snapshot', doc.export({ mode: 'snapshot' }));
		await writeFile(
			'/tmp/sv-page-storage-actual.manifest.json',
			JSON.stringify(
				{
					carrier,
					physical: physical.length,
					blocks: structureEntries(doc).map((e) => ({ cid: e.block.id, depth: e.depth })),
					pm: read(doc, mapping).toJSON()
				},
				null,
				2
			)
		);
	}
	stop();
	observe();
	manager.free();
	replica.free();
});

test('preflight NotApplied keeps native document version, undo history and outbound queue unchanged', async () => {
	const { UndoManager } = await import('loro-crdt');
	const { editorLocalUpdate } = await import('../src/lib/flow/editor-batch');
	const { doc, mapping } = fixture();
	const manager = new UndoManager(doc, {
		mergeInterval: 0,
		excludeOriginPrefixes: ['sys:storage']
	});
	let state = EditorState.create({ schema, doc: read(doc, mapping) });
	state = state.apply(state.tr.insertText(' earlier', 2));
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	const before = doc.toJSON();
	const version = doc.oplogVersion().toJSON();
	const packets: Uint8Array[] = [];
	const sink = (bytes: Uint8Array) => packets.push(bytes);
	const stop = doc.subscribeLocalUpdates((bytes) => editorLocalUpdate(doc, bytes, sink));
	for (const bad of [
		schema.node('doc', null, [schema.nodes.paragraph.create({ indent: 7 }, schema.text('bad'))]),
		schema.node('doc', null, [
			schema.nodes.paragraph.create(
				{ indent: 0 },
				schema.text('bad', [schema.marks.link.create({ href: 'javascript:alert(1)' })])
			)
		])
	])
		assert.throws(() =>
			updateLoroToPmState(
				doc,
				mapping,
				EditorState.create({ schema, doc: bad }),
				undefined,
				manager
			)
		);
	assert.deepEqual(doc.toJSON(), before);
	assert.deepEqual(doc.oplogVersion().toJSON(), version);
	assert.equal(packets.length, 0);
	manager.undo();
	assert.equal(read(doc, mapping).firstChild!.textContent, 'one');
	assert.equal(manager.canUndo(), false);
	stop();
	manager.free();
});

test('injected batch exception preserves prior unsent text, sends no failed bytes and permanently blocks the retired doc', async () => {
	const { atomicEditorWrite, editorLocalUpdate, editorObservation, registerEditorRecovery } =
		await import('../src/lib/flow/editor-batch');
	const { doc } = fixture();
	doc.getMap('data').set('prior_unsent', 'Earlier unsaved 中文');
	doc.commit();
	const before = doc.toJSON();
	const oldQueue = ['prior-valid-item'];
	const packets: Uint8Array[] = [];
	const sink = (bytes: Uint8Array) => packets.push(bytes);
	const stop = doc.subscribeLocalUpdates((bytes) => editorLocalUpdate(doc, bytes, sink));
	let observed = 0;
	const observer = {};
	const unsub = doc.subscribe(() => editorObservation(doc, observer, () => observed++));
	let recovered: LoroDoc | undefined;
	let paused = false;
	const release = registerEditorRecovery(doc, (preimage) => {
		recovered = preimage;
		paused = true;
	});
	assert.throws(() =>
		atomicEditorWrite(
			doc,
			() => {
				doc.getMap('data').set('failed_partial', 'never sent');
				doc.commit({ origin: 'sys:storage' });
				throw new Error('Injected write failure');
			},
			{ protectStorage: true }
		)
	);
	assert(paused);
	assert(recovered);
	assert.deepEqual(recovered!.toJSON(), before);
	assert.equal(recovered!.getMap('data').get('prior_unsent'), 'Earlier unsaved 中文');
	assert.equal(packets.length, 0);
	assert.equal(observed, 0);
	assert.deepEqual(oldQueue, ['prior-valid-item']);
	assert.throws(() => atomicEditorWrite(doc, () => doc.getMap('data').set('retry', 'blocked')));
	doc.getMap('data').set('late', 'also blocked');
	doc.commit();
	assert.equal(packets.length, 0);
	assert.equal(observed, 0);
	release();
	stop();
	unsub();
	recovered!.free();
});

test('repeated successful carrier creation frees every protective fork and keeps one application subscription', async () => {
	const { UndoManager } = await import('loro-crdt');
	const { editorLocalUpdate } = await import('../src/lib/flow/editor-batch');
	const { doc, mapping } = fixture();
	const originalDoc = doc;
	const manager = new UndoManager(doc, {
		mergeInterval: 0,
		excludeOriginPrefixes: ['sys:storage']
	});
	let forks = 0,
		liveForks = 0;
	const nativeFork = doc.fork.bind(doc);
	doc.fork = () => {
		const fork = nativeFork();
		forks++;
		liveForks++;
		const free = fork.free.bind(fork);
		fork.free = () => {
			liveForks--;
			free();
		};
		return fork;
	};
	let subscriptions = 0;
	const nativeSubscribe = doc.subscribeLocalUpdates.bind(doc);
	doc.subscribeLocalUpdates = (listener) => {
		subscriptions++;
		const release = nativeSubscribe(listener);
		return () => {
			subscriptions--;
			release();
		};
	};
	const packets: Uint8Array[] = [];
	const sink = (bytes: Uint8Array) => packets.push(bytes);
	const stop = doc.subscribeLocalUpdates((bytes) => editorLocalUpdate(doc, bytes, sink));
	for (let i = 0; i < 12; i++) {
		let state = EditorState.create({ schema, doc: read(doc, mapping) });
		state = state.apply(
			state.tr.insert(
				state.doc.content.size,
				schema.nodes.paragraph.create({ indent: 0 }, schema.text('new-' + i))
			)
		);
		updateLoroToPmState(doc, mapping, state, undefined, manager);
		assert.equal(liveForks, 0);
		assert.equal(subscriptions, 1);
		assert.equal(doc, originalDoc);
	}
	assert.equal(forks, 12);
	assert.equal(packets.length, 12);
	assert(manager.canUndo());
	stop();
	assert.equal(subscriptions, 0);
	manager.free();
});

test('single-commit failure forks at the prior frontier on demand without a normal protective fork', async () => {
	const { atomicEditorWrite, editorLocalUpdate, registerEditorRecovery } =
		await import('../src/lib/flow/editor-batch');
	const { doc } = fixture();
	doc.getMap('data').set('prior', 'Unsent earlier text');
	doc.commit();
	const before = doc.toJSON();
	doc.fork = () => {
		throw new Error('A normal text action must not allocate a protective fork');
	};
	const packets: Uint8Array[] = [];
	const sink = (bytes: Uint8Array) => packets.push(bytes);
	const stop = doc.subscribeLocalUpdates((bytes) => editorLocalUpdate(doc, bytes, sink));
	let recovered: LoroDoc | undefined;
	const release = registerEditorRecovery(doc, (preimage) => (recovered = preimage));
	assert.throws(() =>
		atomicEditorWrite(doc, () => {
			doc.getMap('data').set('pending_failure', 'must not escape');
			throw new Error('Injected before commit');
		})
	);
	assert(recovered);
	assert.deepEqual(recovered!.toJSON(), before);
	assert.equal(packets.length, 0);
	release();
	stop();
	recovered!.free();
});

test('command-level duplicate clones PM nodes so source and copied block map to distinct live CIDs', () => {
	const { doc, mapping } = fixture();
	let state = EditorState.create({ schema, doc: read(doc, mapping) });
	const original = state.doc.firstChild!;
	const copy = schema.nodeFromJSON(original.toJSON());
	state = state.apply(state.tr.insert(original.nodeSize, copy));
	assert.notEqual(state.doc.child(0), state.doc.child(1));
	updateLoroToPmState(doc, mapping, state);
	const entries = structureEntries(doc);
	assert.notEqual(entries[0].block.id, entries[1].block.id);
	assert.notEqual(mapping.get(entries[0].block.id), mapping.get(entries[1].block.id));
});

test('drag-copy inserts a fresh sibling subtree and persists the same-page media reference', async () => {
	const { FlowEditorAdapter } = await import('../src/lib/flow/editor-adapter');
	const { doc, mapping } = fixture();
	let state = EditorState.create({ schema, doc: read(doc, mapping) });
	const assetId = '5131244e-aed7-4a91-a561-9ae01a792049';
	state = state.apply(
		state.tr.insert(
			state.doc.firstChild!.nodeSize,
			schema.nodes.image.create({ indent: 1, asset_id: assetId, alt: 'Photo', caption: 'Caption' })
		)
	);
	updateLoroToPmState(doc, mapping, state);
	state = EditorState.create({ schema, doc: read(doc, mapping) });
	const original = structureEntries(doc).map((entry) => entry.block.id);
	const adapter = new FlowEditorAdapter(doc);
	Object.assign(adapter, {
		mods: {
			pm,
			loroPm: {
				hasPageStructure: () => true,
				loroSyncPluginKey: { getState: () => ({ mapping }) }
			}
		},
		view: {
			get state() {
				return state;
			},
			dispatch(tr: import('prosemirror-state').Transaction) {
				state = state.apply(tr);
				updateLoroToPmState(doc, mapping, state);
			}
		}
	});

	assert.equal(await adapter.copyBlockByIds(original[0], original[1], 'after'), false);
	assert.equal(await adapter.copyBlockByIds(original[0], original[2], 'before'), true);
	const entries = structureEntries(doc);
	assert.deepEqual(
		entries.map((entry) => entry.depth),
		[0, 1, 0, 1, 0, 0]
	);
	assert.deepEqual(
		entries.filter((_, index) => index !== 2 && index !== 3).map((entry) => entry.block.id),
		original
	);
	assert.equal(new Set(entries.map((entry) => entry.block.id)).size, entries.length);
	const restored = new LoroDoc() as LoroDocType;
	restored.import(doc.export({ mode: 'snapshot' }));
	const result = read(restored, new Map());
	assert.deepEqual(result.child(3).attrs, result.child(1).attrs);
	assert.equal(result.child(3).attrs.asset_id, assetId);
	assert.equal(result.child(2).textContent, 'one');
	assert.equal(result.child(4).textContent, 'two');
});

test('ordinary typing still merges, while split and its next typing batch have independent undo boundaries', async () => {
	const { pageUndoManager, releaseEditorSession } = await import('../src/lib/flow/editor-session');
	const { TextSelection } = await import('prosemirror-state');
	const { splitBlock } = await import('prosemirror-commands');
	const { doc, mapping } = fixture();
	const manager = await pageUndoManager(doc);
	manager.addExcludeOriginPrefix('sys:storage');
	let state = EditorState.create({ schema, doc: read(doc, mapping) });
	state = state.apply(state.tr.insertText('A', 2));
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	state = state.apply(state.tr.insertText('B', 3));
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	manager.undo();
	assert.equal(read(doc, mapping).firstChild!.textContent, 'one');
	manager.redo();
	assert.equal(read(doc, mapping).firstChild!.textContent, 'oABne');
	state = EditorState.create({ schema, doc: read(doc, mapping) });
	state = state.apply(state.tr.setSelection(TextSelection.create(state.doc, 3)));
	splitBlock(state, (tr) => (state = state.apply(tr)));
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	manager.undo();
	assert.equal(read(doc, mapping).firstChild!.textContent, 'oABne');
	manager.redo();
	assert.deepEqual(
		read(doc, mapping).content.content.map((node) => node.textContent),
		['oA', 'Bne', 'two', 'three']
	);
	state = EditorState.create({ schema, doc: read(doc, mapping) });
	const end = state.doc.child(0).nodeSize + state.doc.child(1).nodeSize - 1;
	state = state.apply(state.tr.insertText('X', end));
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	state = state.apply(state.tr.insertText('Y', end + 1));
	updateLoroToPmState(doc, mapping, state, undefined, manager);
	manager.undo();
	assert.deepEqual(
		read(doc, mapping).content.content.map((node) => node.textContent),
		['oA', 'Bne', 'two', 'three']
	);
	manager.undo();
	assert.equal(read(doc, mapping).firstChild!.textContent, 'oABne');
	releaseEditorSession(doc);
});
