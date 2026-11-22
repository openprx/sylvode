// Developer characterization using installed real libraries, with no DOM/network/runtime.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { LoroDoc, LoroMap, LoroList, LoroText, UndoManager } from 'loro-crdt';
import * as pm from 'prosemirror-model';
import { EditorState } from 'prosemirror-state';
import { updateLoroToPmState, createNodeFromLoroObj, type LoroDocType } from 'loro-prosemirror';
import { buildSchema, resolveEditorBlockId } from '../src/lib/flow/editor-adapter';

const schema = buildSchema(pm);
function fixture() {
	const doc = new LoroDoc() as LoroDocType;
	doc.setPeerId('71001');
	const node = schema.nodeFromJSON({
		type: 'doc',
		content: [
			{
				type: 'paragraph',
				attrs: { indent: 0 },
				content: [{ type: 'text', text: 'Browser 😀 prose' }]
			},
			{
				type: 'heading',
				attrs: { indent: 1, level: 2 },
				content: [{ type: 'text', text: 'Heading' }]
			},
			{ type: 'bulletItem', attrs: { indent: 2 }, content: [{ type: 'text', text: 'Bullet' }] },
			{ type: 'codeBlock', attrs: { indent: 0 }, content: [{ type: 'text', text: 'let x = 1;\n' }] }
		]
	});
	updateLoroToPmState(doc, new Map(), EditorState.create({ schema, doc: node }));
	return { doc, node };
}
function blocks(doc: LoroDocType) {
	return doc.getMap('doc').get('children') as LoroList;
}
function read(doc: LoroDocType) {
	return createNodeFromLoroObj(schema, doc.getMap('doc'), new Map())!;
}

test('real binding snapshot preserves schema, block IDs and Unicode round trip', async () => {
	const { doc, node } = fixture();
	const list = blocks(doc),
		first = list.get(0) as LoroMap;
	assert(first instanceof LoroMap);
	assert.match(first.id, /^cid:\d+@71001:Map$/);
	assert(first.get('children') instanceof LoroList);
	assert((first.get('children') as LoroList).get(0) instanceof LoroText);
	const snapshot = doc.export({ mode: 'snapshot' });
	const restored = new LoroDoc() as LoroDocType;
	restored.import(snapshot);
	assert.deepEqual(read(restored).toJSON(), node.toJSON());
	assert.equal((blocks(restored).get(0) as LoroMap).id, first.id);
	if (process.env.WRITE_EDITOR_INTEROP_FIXTURE === '1') {
		await writeFile(
			new URL('./fixtures/editor-doc-v1/browser.snapshot', import.meta.url),
			snapshot
		);
		await writeFile(
			new URL('./fixtures/editor-doc-v1/contract.json', import.meta.url),
			JSON.stringify(
				{
					library: 'loro-prosemirror@0.4.3',
					peer_id: '71001',
					pm: node.toJSON(),
					deep: doc.toJSON(),
					blocks: list.toArray().map((b) => {
						const m = b as LoroMap;
						return {
							id: m.id,
							type: m.get('nodeName'),
							attributes_id: (m.get('attributes') as LoroMap).id,
							children_id: (m.get('children') as LoroList).id,
							text_id: ((m.get('children') as LoroList).get(0) as LoroText).id
						};
					})
				},
				null,
				2
			) + '\n'
		);
	}
});

test('in-place server-like LoroText edit is visible to binding without replacing comment block identity', () => {
	const { doc } = fixture();
	const first = blocks(doc).get(0) as LoroMap;
	const text = (first.get('children') as LoroList).get(0) as LoroText;
	const blockId = first.id,
		textId = text.id;
	text.insert(text.length, ' + agent');
	doc.commit({ origin: 'server-command' });
	assert.equal(read(doc).child(0).textContent, 'Browser 😀 prose + agent');
	assert.equal((blocks(doc).get(0) as LoroMap).id, blockId);
	assert.equal(((first.get('children') as LoroList).get(0) as LoroText).id, textId);
	const mapping = new Map();
	const state = EditorState.create({ schema, doc: read(doc) });
	updateLoroToPmState(
		doc,
		mapping,
		state.apply(state.tr.insertText(' again', state.doc.content.size - 1))
	);
	assert.equal(read(doc).lastChild?.textContent, 'let x = 1;\n again');
	assert.equal((blocks(doc).get(0) as LoroMap).id, blockId);
});

test('rich schema keeps native mark names and refuses unsupported embeds', () => {
	assert.deepEqual(Object.keys(schema.marks), ['bold', 'italic', 'strike', 'code', 'link']);
	assert.equal(schema.nodes.embed, undefined);
	for (const kind of ['image', 'attachment', 'childPage']) assert(schema.nodes[kind]?.spec.atom);
	assert.equal(
		schema.mark('link', { href: 'https://example.test' }).attrs.href,
		'https://example.test'
	);
});

test('native rich marks and everyday blocks survive snapshot with stable containers', () => {
	const { doc } = fixture();
	const mapping = new Map();
	const initial = read(doc);
	let state = EditorState.create({ schema, doc: initial });
	const id = (blocks(doc).get(0) as LoroMap).id;
	const tr = state.tr
		.addMark(1, 8, schema.marks.bold.create())
		.addMark(9, 11, schema.marks.link.create({ href: 'https://example.test' }));
	state = state.apply(tr);
	updateLoroToPmState(doc, mapping, state);
	const rich = read(doc);
	assert.equal((blocks(doc).get(0) as LoroMap).id, id);
	const text = ((blocks(doc).get(0) as LoroMap).get('children') as LoroList).get(0) as LoroText;
	assert(text.toDelta().some((run) => run.attributes?.bold));
	assert(
		text
			.toDelta()
			.some((run) => (run.attributes?.link as { href?: string })?.href === 'https://example.test')
	);
	const restored = new LoroDoc() as LoroDocType;
	restored.import(doc.export({ mode: 'snapshot' }));
	assert.deepEqual(read(restored).toJSON(), rich.toJSON());
	assert.equal((blocks(restored).get(0) as LoroMap).id, id);
	for (const kind of ['orderedItem', 'todoItem', 'blockquote', 'toggle', 'divider'])
		assert(schema.nodes[kind]);
});

test('committed real-library fixture is consumable and Unicode offsets are UTF-16 in JS', async () => {
	const bytes = await readFile(
		new URL('./fixtures/editor-doc-v1/browser.snapshot', import.meta.url)
	);
	const contract = JSON.parse(
		await readFile(new URL('./fixtures/editor-doc-v1/contract.json', import.meta.url), 'utf8')
	);
	const doc = new LoroDoc() as LoroDocType;
	doc.import(bytes);
	assert.deepEqual(JSON.parse(JSON.stringify(read(doc).toJSON())), contract.pm);
	const block = blocks(doc).get(0) as LoroMap;
	assert.equal(block.id, contract.blocks[0].id);
	const text = (block.get('children') as LoroList).get(0) as LoroText;
	assert.equal(text.length, 'Browser 😀 prose'.length);
	assert.notEqual(text.length, new TextEncoder().encode('Browser 😀 prose').length);
	text.insert('Browser 😀'.length, ' agent');
	doc.commit();
	assert.equal(read(doc).child(0).textContent, 'Browser 😀 agent prose');
});

test('actual Rust bridge update merges into browser doc and remains editable with stable anchors', async () => {
	const doc = new LoroDoc() as LoroDocType;
	doc.import(await readFile(new URL('./fixtures/editor-doc-v1/browser.snapshot', import.meta.url)));
	const before = blocks(doc)
		.toArray()
		.map((b) => (b as LoroMap).id);
	const textId = (((blocks(doc).get(0) as LoroMap).get('children') as LoroList).get(0) as LoroText)
		.id;
	doc.import(
		await readFile(new URL('./fixtures/editor-doc-v1/rust-agent.update', import.meta.url))
	);
	assert.equal(read(doc).child(0).textContent, 'Browser 😀 + Rust agent prose');
	assert.deepEqual(
		blocks(doc)
			.toArray()
			.map((b) => (b as LoroMap).id),
		before
	);
	assert.equal(
		(((blocks(doc).get(0) as LoroMap).get('children') as LoroList).get(0) as LoroText).id,
		textId
	);
	const snapshot = new LoroDoc() as LoroDocType;
	snapshot.import(
		await readFile(new URL('./fixtures/editor-doc-v1/rust-agent.snapshot', import.meta.url))
	);
	assert.deepEqual(read(snapshot).toJSON(), read(doc).toJSON());
	const state = EditorState.create({ schema, doc: read(doc) });
	updateLoroToPmState(doc, new Map(), state.apply(state.tr.insertText('JS ', 1)));
	assert.equal(read(doc).child(0).textContent, 'JS Browser 😀 + Rust agent prose');
	assert.deepEqual(
		blocks(doc)
			.toArray()
			.map((b) => (b as LoroMap).id),
		before
	);
});

test('actual backend Engine snapshot reconstructs agent prose and accepts next PM edit', async () => {
	const doc = new LoroDoc() as LoroDocType;
	doc.import(
		await readFile(new URL('./fixtures/editor-doc-v1/engine-agent.snapshot', import.meta.url))
	);
	assert.equal(read(doc).child(0).textContent, 'Browser 😀 agent prose');
	const ids = blocks(doc)
		.toArray()
		.map((b) => (b as LoroMap).id);
	const state = EditorState.create({ schema, doc: read(doc) });
	updateLoroToPmState(doc, new Map(), state.apply(state.tr.insertText('JS ', 1)));
	assert.equal(read(doc).child(0).textContent, 'JS Browser 😀 agent prose');
	assert.deepEqual(
		blocks(doc)
			.toArray()
			.map((b) => (b as LoroMap).id),
		ids
	);
});

test('Rust-created alias block renders and logical anchors survive a browser text edit', async () => {
	const doc = new LoroDoc() as LoroDocType;
	doc.import(
		await readFile(new URL('./fixtures/editor-doc-v1/rust-created.snapshot', import.meta.url))
	);
	const cid = resolveEditorBlockId(doc, 'agent-block');
	assert.match(cid, /^cid:/);
	const map = doc.getContainerById(cid as import('loro-crdt').ContainerID) as LoroMap;
	assert.equal(map.id, cid);
	assert.equal(read(doc).child(1).textContent, 'Agent created 😀');
	const state = EditorState.create({ schema, doc: read(doc) });
	const offset = state.doc.child(0).nodeSize + 1;
	updateLoroToPmState(doc, new Map(), state.apply(state.tr.insertText('JS ', offset)));
	assert.equal(read(doc).child(1).textContent, 'JS Agent created 😀');
	assert.equal(resolveEditorBlockId(doc, 'agent-block'), cid);
	assert.equal((blocks(doc).get(1) as LoroMap).id, cid);
});

test('browser local undo leaves imported Rust agent prose intact', async () => {
	const doc = new LoroDoc() as LoroDocType;
	doc.import(await readFile(new URL('./fixtures/editor-doc-v1/browser.snapshot', import.meta.url)));
	doc.setPeerId('73001');
	const undo = new UndoManager(doc, { mergeInterval: 0 });
	doc.import(
		await readFile(new URL('./fixtures/editor-doc-v1/rust-agent.update', import.meta.url))
	);
	assert.equal(undo.canUndo(), false);
	const state = EditorState.create({ schema, doc: read(doc) });
	updateLoroToPmState(doc, new Map(), state.apply(state.tr.insertText('JS ', 1)));
	assert.equal(undo.canUndo(), true);
	undo.undo();
	assert.equal(read(doc).child(0).textContent, 'Browser 😀 + Rust agent prose');
	undo.free();
});

test('Rust initialization renders legacy aliases and empty pages through actual shipped schema', async () => {
	for (const [name, expected] of [
		['rust-initialized.snapshot', 'legacy + agent'],
		['rust-empty.snapshot', '']
	] as const) {
		const doc = new LoroDoc() as LoroDocType;
		doc.import(await readFile(new URL(`./fixtures/editor-doc-v1/${name}`, import.meta.url)));
		assert.equal(read(doc).childCount, 1);
		assert.equal(read(doc).child(0).textContent, expected);
		const id = (blocks(doc).get(0) as LoroMap).id;
		if (name === 'rust-initialized.snapshot')
			assert.equal(resolveEditorBlockId(doc, 'legacy-id'), id);
		const state = EditorState.create({ schema, doc: read(doc) });
		updateLoroToPmState(doc, new Map(), state.apply(state.tr.insertText('JS ', 1)));
		assert.equal(read(doc).child(0).textContent, `JS ${expected}`);
		assert.equal((blocks(doc).get(0) as LoroMap).id, id);
	}
});

test('actual API command planner structure output renders updated heading and retains aliases', async () => {
	const doc = new LoroDoc() as LoroDocType;
	doc.import(
		await readFile(new URL('./fixtures/editor-doc-v1/api-structure.snapshot', import.meta.url))
	);
	const rendered = read(doc);
	let heading: pm.Node | null = null;
	rendered.forEach((node) => {
		if (node.textContent === 'Updated 😀 by agent') heading = node;
	});
	assert(heading);
	assert.equal((heading as pm.Node).type.name, 'heading');
	assert.equal((heading as pm.Node).attrs.level, 2);
	assert.equal(rendered.child(0).textContent, 'Browser 😀 prose');
	const aliases = (doc as LoroDoc).getMap('flow_editor_aliases').toJSON();
	const entries = Object.entries(aliases);
	assert(entries.length > 0);
	for (const [logical, cid] of entries) assert.equal(resolveEditorBlockId(doc, logical), cid);
	const ids = blocks(doc)
		.toArray()
		.map((b) => (b as LoroMap).id);
	const state = EditorState.create({ schema, doc: rendered });
	updateLoroToPmState(doc, new Map(), state.apply(state.tr.insertText('JS ', 1)));
	assert.equal(read(doc).child(0).textContent, 'JS Browser 😀 prose');
	assert(read(doc).textContent.includes('Updated 😀 by agent'));
	assert.deepEqual(
		blocks(doc)
			.toArray()
			.map((b) => (b as LoroMap).id),
		ids
	);
});

test('duplicating one block creates a distinct CID without replacing the source', () => {
	const { doc } = fixture();
	const mapping = new Map();
	let node = read(doc);
	let state = EditorState.create({ schema, doc: node });
	updateLoroToPmState(doc, mapping, state);
	const original = (blocks(doc).get(0) as LoroMap).id;
	const first = state.doc.firstChild!;
	state = state.apply(state.tr.insert(first.nodeSize, first));
	updateLoroToPmState(doc, mapping, state);
	assert.equal((blocks(doc).get(0) as LoroMap).id, original);
	assert.notEqual((blocks(doc).get(1) as LoroMap).id, original);
	assert.equal(read(doc).child(1).textContent, first.textContent);
});
