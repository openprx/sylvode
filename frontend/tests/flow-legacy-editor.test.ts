import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	legacyProseBlocks,
	canEnableLegacyEditor,
	initializeEditorRequest,
	editorMigrationState
} from '../src/lib/flow/legacy-editor';
test('legacy preview preserves text and IDs without expanding private/deleted/nonprose nodes', () => {
	const node = (text: string, order: string, extra = {}) => ({
		kind: 'block',
		text,
		order_key: order,
		deleted: false,
		properties: {},
		...extra
	});
	const source = {
		nodes: {
			b: node('<script>literal</script>', '2'),
			a: node('Heading', '1', { properties: { block_type: 'heading' } }),
			gone: node('deleted', '0', { deleted: true }),
			ref: node('private', '0', { properties: { embed_object_id: 'target' } }),
			schema: node('secret field', '0', { kind: 'collection_field' })
		}
	};
	const before = JSON.stringify(source);
	assert.deepEqual(
		legacyProseBlocks(source).map((x) => [x.id, x.kind, x.text]),
		[
			['a', 'heading', 'Heading'],
			['b', 'paragraph', '<script>literal</script>']
		]
	);
	assert.equal(JSON.stringify(source), before);
});
test('malformed semantic models do not manufacture content', () => {
	for (const value of [null, {}, [], { nodes: [] }, { nodes: { bad: null } }])
		assert.deepEqual(legacyProseBlocks(value), []);
});

test('editing requires explicit server capability, edit action and active page; request has no client content', () => {
	const object = {
		object_type: 'page',
		editor_state: 'migration_required',
		editor_initialization_supported: true,
		permitted_actions: ['edit'],
		lifecycle_status: 'active'
	} as import('../src/lib/api/flow').FlowObjectView;
	assert.equal(canEnableLegacyEditor(object), true);
	for (const patch of [
		{ permitted_actions: ['view'] },
		{ editor_initialization_supported: false },
		{ editor_initialization_supported: undefined },
		{ editor_state: 'unsupported' },
		{ object_type: 'collection' },
		{ lifecycle_status: 'archived' }
	])
		assert.equal(canEnableLegacyEditor({ ...object, ...patch } as typeof object), false);
	assert.deepEqual(initializeEditorRequest('captured-frontier', 'same-intent'), {
		command: { type: 'initialize_editor_doc', payload: {} },
		expected_frontier: 'captured-frontier',
		idempotency_key: 'same-intent'
	});
});
test('admission fallback accepts only exact typed migration reasons', () => {
	assert.equal(
		editorMigrationState({
			code: 'unsupported_protocol',
			recoverable: false,
			details: { reason: 'editor_migration_required' }
		}),
		'migration_required'
	);
	assert.equal(
		editorMigrationState({
			code: 'unsupported_protocol',
			recoverable: false,
			details: { reason: 'editor_migration_unsupported' }
		}),
		'unsupported'
	);
	assert.equal(
		editorMigrationState({
			code: 'forbidden',
			recoverable: false,
			details: { reason: 'editor_migration_required' }
		}),
		null
	);
	assert.equal(editorMigrationState({ code: 'unsupported_protocol', recoverable: false }), null);
});
