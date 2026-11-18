import { test } from 'node:test';
import assert from 'node:assert/strict';
import fixture from './fixtures/block-projection/serde-all-kinds.json';
import {
	parseSharedRead,
	sharedProjectionRows,
	sharedSpansValid,
	type SharedRead,
	type SharedCommand
} from '../src/lib/flow/shared-blocks/contracts';
import { SharedBlockDraft, type SharedDraftState } from '../src/lib/flow/shared-blocks/drafts';
import { sharedAssetProvider } from '../src/lib/flow/shared-blocks/providers';
import { sharedBlockApi } from '../src/lib/flow/shared-blocks/client';
const read = () => parseSharedRead(fixture);
const command = (value: SharedRead, text = 'My draft'): SharedCommand => ({
	type: 'replace_text',
	member_id: value.projection.members[0].member_id,
	text,
	spans: []
});
test('actual Rust serde wrapper covers all recipient kinds and UTF8 overlap without private identities', () => {
	const parsed = read();
	assert.equal(parsed.projection.members.length, 12);
	assert.equal(parsed.effective_permission, 'edit');
	assert.equal(parsed.projection.members[0].content.kind, 'paragraph');
	assert.equal(sharedProjectionRows(parsed).length, 12);
	for (const key of [
		'object_id',
		'document_id',
		'block_cid',
		'logical_id',
		'title',
		'frontier',
		'history'
	])
		assert(!JSON.stringify(parsed).includes('"' + key + '"'));
	const injected = read() as SharedRead & { object_id: string };
	injected.object_id = 'private';
	assert.throws(() => parseSharedRead(injected));
	const child = read();
	Object.assign(child.projection.members[11].content, { child_page_id: 'private' });
	assert.throws(() => parseSharedRead(child));
	assert(!sharedSpansValid('😀', [{ start: 1, end: 4, mark: 'bold' }]));
	assert(
		!sharedSpansValid('safe', [{ start: 0, end: 4, mark: 'link', href: 'javascript:alert(1)' }])
	);
	assert(!sharedSpansValid('safe', [null]));
});
test('projection rejects wrong share, missing/foreign parent, cycles and unknown permission instead of falling back to a source Page', () => {
	assert.throws(() => parseSharedRead(fixture, '00000000-0000-0000-0000-000000000001'));
	const foreign = read();
	foreign.projection.members[0].parent_member_id = '00000000-0000-0000-0000-000000000001';
	assert.throws(() => parseSharedRead(foreign));
	const cycle = read();
	cycle.projection.members[0].parent_member_id = cycle.projection.members[1].member_id;
	cycle.projection.members[1].parent_member_id = cycle.projection.members[0].member_id;
	assert.throws(() => parseSharedRead(cycle));
	assert.throws(() => parseSharedRead({ ...fixture, effective_permission: 'owner' }));
});
test('authorized revision conflict preserves local draft and base; retry requires explicit rebase', async () => {
	const initial = read(),
		latest = read();
	latest.projection.revision++;
	const calls: { expected: number; key: string }[] = [];
	let state!: SharedDraftState;
	const draft = new SharedBlockDraft(initial, {
		patch: async (_id, patch, key) => {
			calls.push({ expected: patch.expected_revision, key });
			return calls.length === 1
				? {
						code: 409,
						message: 'conflict',
						data: null,
						details: {
							reason: 'block_share_revision_conflict',
							revision: latest.projection.revision,
							current_projection: latest.projection,
							effective_permission: 'edit'
						}
					}
				: {
						code: 0,
						message: 'ok',
						data: {
							...latest,
							projection: { ...latest.projection, revision: latest.projection.revision + 1 }
						}
					};
		}
	});
	draft.subscribe((value) => (state = value));
	assert(draft.edit([command(initial)]));
	assert.equal(await draft.save(), false);
	assert.equal(state.phase, 'conflict');
	assert.equal(state.expectedRevision, initial.projection.revision);
	assert.deepEqual(state.commands, [command(initial)]);
	assert.equal(await draft.save(), false);
	assert.equal(calls.length, 1);
	assert(draft.rebaseDraft());
	assert(await draft.save());
	assert.equal(calls[1].expected, latest.projection.revision);
	assert.notEqual(calls[0].key, calls[1].key);
	assert.equal(state.commands.length, 0);
	draft.dispose();
});
test('unavailable or unauthorized projection hides canonical data but keeps only local draft; transient retry keeps operation identity', async () => {
	const initial = read();
	let state!: SharedDraftState;
	const keys: string[] = [];
	const draft = new SharedBlockDraft(initial, {
		patch: async (_id, _patch, key) => {
			keys.push(key);
			return keys.length === 1
				? { code: 500, message: 'offline', data: null }
				: {
						code: 403,
						message: 'unavailable',
						data: null,
						details: { reason: 'block_share_unavailable' }
					};
		}
	});
	draft.subscribe((value) => (state = value));
	draft.edit([command(initial)]);
	await draft.save();
	assert.equal(state.phase, 'failed');
	await draft.save();
	assert.equal(keys[0], keys[1]);
	assert.equal(state.remote, null);
	assert.deepEqual(state.commands, [command(initial)]);
	assert.equal(await draft.save(), false);
	draft.dispose();
});
test('share byte provider only requests captured opaque aliases, never Page asset or bootstrap endpoints', async () => {
	const initial = read();
	const provider = sharedAssetProvider(initial);
	const original = sharedBlockApi.asset;
	let called = 0;
	sharedBlockApi.asset = async (shareId, alias) => {
		called++;
		assert.equal(shareId, initial.projection.share_id);
		assert.equal(alias, '00000000-0000-0000-0000-000000002329');
		return { code: 404, message: 'fixture', data: null };
	};
	try {
		assert.equal((await provider.read('foreign-private-asset-id')).code, 403);
		assert.equal(called, 0);
		const image = initial.projection.members.find((member) => member.content.kind === 'image')!;
		if (image.content.kind === 'image') await provider.read(image.content.asset_alias);
		assert.equal(called, 1);
	} finally {
		sharedBlockApi.asset = original;
	}
});

test('single-member draft conversion roundtrips real native marks/Unicode without carrying source structure or identities', async () => {
	const pm = await import('prosemirror-model');
	const { EditorState } = await import('prosemirror-state');
	const { LoroDoc } = await import('loro-crdt');
	const { createPageSchema } = await import('../src/lib/flow/editor-schema');
	const { sharedEditorNode, sharedEditorValue } =
		await import('../src/lib/flow/shared-blocks/editor');
	const { pageInlineRuns } = await import('../src/lib/flow/inline-runs');
	const binding = await import('../src/lib/flow/vendor/loro-prosemirror');
	const schema = createPageSchema(pm);
	for (const member of read().projection.members) {
		if (!('text' in member.content)) continue;
		const node = sharedEditorNode(schema, member.content),
			doc = new LoroDoc() as import('loro-prosemirror').LoroDocType,
			mapping = new Map();
		binding.updateLoroToPmState(
			doc,
			mapping,
			EditorState.create({ schema, doc: schema.nodes.doc.create(null, node) })
		);
		const restored = new LoroDoc() as import('loro-prosemirror').LoroDocType;
		restored.import(doc.export({ mode: 'snapshot' }));
		const value = sharedEditorValue(
			(
				binding.createNodeFromLoroObj(
					schema,
					restored.getMap('doc'),
					new Map()
				) as import('prosemirror-model').Node
			).firstChild!
		);
		assert.equal(value.text, member.content.text);
		assert.deepEqual(
			pageInlineRuns(value.text, value.spans),
			pageInlineRuns(member.content.text, 'spans' in member.content ? member.content.spans : [])
		);
		assert(!JSON.stringify(value).includes('member_id'));
		assert(!JSON.stringify(value).includes('parent_member_id'));
		restored.free();
		doc.free();
	}
});

test('read-only grants and member-kind limits prevent unsupported commands before network writes', () => {
	const initial = read();
	const draft = new SharedBlockDraft(initial, {
		patch: async () => {
			throw new Error('must not send');
		}
	});
	const image = initial.projection.members.find((member) => member.content.kind === 'image')!;
	assert.equal(
		draft.edit([
			{ type: 'replace_text', member_id: image.member_id, text: 'forbidden', spans: [] }
		]),
		false
	);
	const todo = initial.projection.members.find((member) => member.content.kind === 'todo')!;
	assert(draft.edit([{ type: 'set_checked', member_id: todo.member_id, checked: true }]));
	const text = initial.projection.members[0];
	assert(
		draft.edit([
			{
				type: 'set_marks',
				member_id: text.member_id,
				edits: [{ start: 0, end: 6, mark: 'link', clear: true }]
			}
		])
	);
	const view = read();
	view.effective_permission = 'view';
	const viewer = new SharedBlockDraft(view, {
		patch: async () => {
			throw new Error('must not send');
		}
	});
	assert.equal(viewer.edit([command(view)]), false);
	draft.dispose();
	viewer.dispose();
});

test('actual DB wrapper exposes only one opaque target context and never source-management identities', async () => {
	const actual = (await import('./fixtures/block-projection/actual-db-wrapper.json')).default;
	const management = (await import('./fixtures/block-projection/source-management.json')).default;
	const value = parseSharedRead(actual);
	assert.equal(value.display_context?.placement_id, actual.display_context.placement_id);
	assert(!JSON.stringify(value).includes(management[0].root_block_cid));
	assert.throws(() => parseSharedRead(management));
});
test('changing authentication epoch immediately withdraws mounted canonical state while keeping the personal draft', async () => {
	const { apiClient } = await import('../src/lib/api/client');
	const { TaskWorkbench } = await import('../src/lib/workbench/tabs');
	const { SharedViewSession } = await import('../src/lib/flow/shared-blocks/session');
	const original = sharedBlockApi.read,
		initial = read(),
		workbench = new TaskWorkbench();
	workbench.activateUser('viewer-fixture');
	workbench.visit(`/shared-blocks/${initial.projection.share_id}`);
	sharedBlockApi.read = async () => ({ code: 0, message: 'ok', data: initial });
	const session = new SharedViewSession(initial.projection.share_id, 'viewer-fixture', workbench);
	let snapshot = session.current();
	session.subscribe((value) => (snapshot = value));
	try {
		await session.refresh();
		assert(snapshot.authorized);
		const todo = initial.projection.members.find((member) => member.content.kind === 'todo')!;
		assert(session.openMember(todo.member_id, 'Fixture draft'));
		if (todo.content.kind === 'todo') session.setChecked(!todo.content.checked);
		assert.equal(snapshot.draft?.commands.length, 1);
		apiClient.beginAuthSession();
		assert.equal(snapshot.authorized, false);
		assert.equal(snapshot.draft?.remote, null);
		assert.equal(snapshot.draft?.commands.length, 1);
		assert.equal(snapshot.loadError, 'unavailable');
	} finally {
		session.dispose();
		workbench.dispose();
		sharedBlockApi.read = original;
	}
});
test('shared bytes require matching opaque share/alias headers and late disposed responses cannot mount', async () => {
	const initial = read(),
		image = initial.projection.members.find((member) => member.content.kind === 'image')!;
	assert(image.content.kind === 'image');
	const alias = image.content.asset_alias,
		original = sharedBlockApi.asset;
	const blob = new Blob(['synthetic'], { type: 'image/png' }),
		headers = new Headers({
			'content-type': 'image/png',
			'x-flow-asset-sha256': 'a'.repeat(64),
			'x-content-type-options': 'nosniff',
			'x-flow-share-id': initial.projection.share_id,
			'x-flow-asset-alias': alias
		});
	let resolve!: (
		value: import('../src/lib/api/client').ApiResult<{ blob: Blob; headers: Headers }>
	) => void;
	let signal: AbortSignal | undefined;
	try {
		sharedBlockApi.asset = async () => ({
			code: 0,
			message: 'ok',
			data: {
				blob,
				headers: new Headers({
					'x-flow-share-id': initial.projection.share_id,
					'x-flow-asset-alias': 'other-alias'
				})
			}
		});
		const wrong = sharedAssetProvider(initial);
		assert.equal((await wrong.read(alias)).code, 502);
		wrong.dispose();
		sharedBlockApi.asset = async (_share, _alias, abort) => {
			signal = abort;
			return new Promise((done) => (resolve = done));
		};
		const disposed = sharedAssetProvider(initial),
			pending = disposed.read(alias);
		disposed.dispose();
		assert(signal?.aborted);
		resolve({ code: 0, message: 'ok', data: { blob, headers } });
		assert.equal((await pending).code, 403);
	} finally {
		sharedBlockApi.asset = original;
	}
});

test('strict recipient DTO parsing accepts a reactive-proxy boundary without relaxing private-field rejection', () => {
	const value = read(),
		proxy = new Proxy(value, {});
	const parsed = parseSharedRead(proxy);
	assert.deepEqual(parsed, value);
	assert.notEqual(parsed, value);
	const injected = new Proxy({ ...value, source_object_id: 'private-source' }, {});
	assert.throws(() => parseSharedRead(injected));
});

test('a fresh local draft configures native styles before triple overlaps and separate code/link ranges', async () => {
	const pm = await import('prosemirror-model');
	const { EditorState } = await import('prosemirror-state');
	const { LoroDoc } = await import('loro-crdt');
	const { createPageSchema } = await import('../src/lib/flow/editor-schema');
	const { sharedEditorNode, sharedEditorValue } =
		await import('../src/lib/flow/shared-blocks/editor');
	const { configLoroTextStyle } =
		await import('../src/lib/flow/vendor/loro-prosemirror/text-style');
	const binding = await import('../src/lib/flow/vendor/loro-prosemirror');
	const schema = createPageSchema(pm),
		doc = new LoroDoc() as import('loro-prosemirror').LoroDocType,
		text = '中文😀abc';
	const spans = (
		[
			{ mark: 'bold' },
			{ mark: 'italic' },
			{ mark: 'strike' },
			{ mark: 'code' },
			{ mark: 'link', href: 'https://example.invalid/' }
		] as const
	).map((mark) => ({
		start: mark.mark === 'code' ? 10 : mark.mark === 'link' ? 11 : 0,
		end: mark.mark === 'code' ? 11 : mark.mark === 'link' ? 13 : 10,
		...mark
	}));
	configLoroTextStyle(doc, schema);
	const node = sharedEditorNode(schema, { kind: 'paragraph', text, spans });
	binding.updateLoroToPmState(
		doc,
		new Map(),
		EditorState.create({ schema, doc: schema.nodes.doc.create(null, node) })
	);
	const restored = binding.createNodeFromLoroObj(
		schema,
		doc.getMap('doc'),
		new Map()
	) as import('prosemirror-model').Node;
	const value = sharedEditorValue(restored.firstChild!);
	assert.equal(value.text, text);
	assert.deepEqual(
		new Set(value.spans.map((span) => span.mark)),
		new Set(['bold', 'italic', 'strike', 'code', 'link'])
	);
	doc.free();
});
