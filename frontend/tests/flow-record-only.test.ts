// Developer state/contract tests only: no database, live permission probe or browser.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { CollectionViewController } from '../src/lib/flow/collection-view';
import type { CollectionReads, CollectionWrites } from '../src/lib/flow/collection-view';
import type {
	FlowRecordContext,
	FlowObjectView,
	AcceptedChange,
	FlowCommandInput
} from '../src/lib/api/flow';
const ID = 'record-only',
	FIELD = 'visible-field';
function record(extra: Partial<FlowObjectView> = {}): FlowObjectView {
	return {
		id: ID,
		scope_id: 'canonical-source',
		project_id: null,
		object_type: 'record',
		title: 'Shared record',
		lifecycle_status: 'active',
		governance_metadata: {},
		document_id: 'record-document',
		document_seq: 1,
		frontier: 'record-frontier-1',
		projection_seq: 1,
		projection_lag: 0,
		created_at: 'now',
		updated_at: 'now',
		archived_at: null,
		record_context_supported: true,
		record_content_writable: true,
		record_archive_allowed: true,
		permitted_actions: ['view', 'edit'],
		semantic_content: { nodes: { 'record:body': { properties: { body: 'stored body' } } } },
		...extra
	};
}
function fixture() {
	let context: FlowRecordContext = {
		record: record(),
		fields: [
			{
				field_id: FIELD,
				label: 'Visible label',
				type: 'text',
				config: {},
				position: 0,
				archived: false
			}
		],
		values_by_field_id: { [FIELD]: 'stored' },
		field_secrecy_enabled: false,
		schema_revision: 'opaque-schema-1'
	};
	const readsCalled: string[] = [];
	const writesCalled: Array<{ id: string; command: FlowCommandInput; options: unknown }> = [];
	let reject: unknown = null;
	const forbidden = async () => {
		throw new Error('Parent Collection access must never occur');
	};
	const reads: CollectionReads = {
		getCollection: forbidden,
		getSchema: forbidden,
		listRecords: forbidden,
		openRecord: forbidden,
		closeRecord: () => {
			throw new Error('No whole record document opened');
		},
		getRecordContext: async (id) => {
			readsCalled.push(id);
			return { code: 0, message: 'ok', data: structuredClone(context) };
		}
	};
	const writes: CollectionWrites = {
		createRecord: forbidden,
		execute: async (id, command, options) => {
			writesCalled.push({ id, command, options });
			if (reject) throw reject;
			const view =
				command.type === 'record_archive'
					? record({
							...context.record,
							lifecycle_status: 'archived',
							record_content_writable: false,
							record_archive_allowed: false
						})
					: record({ frontier: 'record-frontier-2', document_seq: 2 });
			return {
				object: view,
				accepted_seq: 2,
				head_frontier: view.frontier,
				projection_seq: 2,
				semantic_diff: {},
				affected_object_ids: [ID],
				event_id: 'event'
			} satisfies AcceptedChange;
		}
	};
	const controller = new CollectionViewController(null, reads, writes, { schemaFromRecord: ID });
	return {
		controller,
		readsCalled,
		writesCalled,
		get context() {
			return context;
		},
		set context(value: FlowRecordContext) {
			context = value;
		},
		set reject(value: unknown) {
			reject = value;
		}
	};
}
test('standalone record loads one context without parent identity/views/count/siblings and patches actual Record', async () => {
	const f = fixture();
	await f.controller.load();
	await f.controller.openPanel(ID);
	assert.deepEqual(f.readsCalled, [ID]);
	assert.equal(f.controller.collectionId, null);
	const state = f.controller.snapshot();
	assert.equal(state.rows.length, 1);
	assert.equal(state.collection?.collectionId, null);
	assert.equal(state.collection?.schemaSeq, null);
	assert.equal(state.collection?.recordCount, null);
	assert.equal(state.collection?.title, null);
	assert.deepEqual(state.collection?.views, []);
	f.controller.setDraft(ID, FIELD, 'edited');
	assert.equal(await f.controller.commitCell(ID, FIELD), true);
	assert.deepEqual(f.writesCalled[0], {
		id: ID,
		command: {
			type: 'record_patch',
			payload: { properties: { [FIELD]: 'edited' }, schema_revision: 'opaque-schema-1' }
		},
		options: {
			idempotencyKey: (f.writesCalled[0].options as { idempotencyKey: string }).idempotencyKey,
			expectedFrontier: 'record-frontier-1'
		}
	});
	await f.controller.loadMore();
	assert.equal(await f.controller.findRecord('unshared-sibling'), null);
	assert.deepEqual(f.readsCalled, [ID]);
	assert.equal((await f.controller.createRecord()).ok, false);
	f.controller.dispose();
});
test('refresh keeps drafts/captured frontier and schema token until explicit local review; review sends no mutation', async () => {
	const f = fixture();
	await f.controller.load();
	await f.controller.openPanel(ID);
	f.controller.setDraft(ID, FIELD, 'draft');
	f.controller.setBodyDraft(ID, 'body draft');
	f.context = {
		...f.context,
		record: record({ frontier: 'record-frontier-remote' }),
		values_by_field_id: { [FIELD]: 'remote' },
		schema_revision: 'opaque-schema-remote'
	};
	await f.controller.refresh();
	assert.equal(f.controller.snapshot().drafts[ID]?.[FIELD], 'draft');
	assert.equal(f.controller.snapshot().bodyDrafts[ID], 'body draft');
	assert.equal(f.controller.snapshot().panel?.contextReviewRequired, true);
	assert.equal(await f.controller.commitCell(ID, FIELD), false);
	assert.equal(f.writesCalled.length, 0);
	f.controller.reviewRecordContext(ID);
	assert.equal(f.writesCalled.length, 0);
	assert.equal(await f.controller.commitCell(ID, FIELD), true);
	assert.equal(
		(f.writesCalled[0].command.payload as { schema_revision: string }).schema_revision,
		'opaque-schema-remote'
	);
	assert.equal(
		(f.writesCalled[0].options as { expectedFrontier: string }).expectedFrontier,
		'record-frontier-remote'
	);
	f.controller.dispose();
});
test('schema change error refreshes only bounded context and never auto-replays a draft', async () => {
	const f = fixture();
	await f.controller.load();
	await f.controller.openPanel(ID);
	f.controller.setDraft(ID, FIELD, 'draft');
	f.reject = {
		code: 'stale_frontier',
		recoverable: true,
		details: {
			reason: 'record_schema_changed',
			action: 'refresh_record_context',
			write_state: 'not_accepted'
		}
	};
	f.context = { ...f.context, schema_revision: 'opaque-schema-2' };
	assert.equal(await f.controller.commitCell(ID, FIELD), false);
	assert.equal(f.writesCalled.length, 1);
	assert.deepEqual(f.readsCalled, [ID, ID]);
	assert.equal(f.controller.snapshot().drafts[ID]?.[FIELD], 'draft');
	assert.equal(f.controller.snapshot().panel?.contextReviewRequired, true);
	assert.equal(await f.controller.commitCell(ID, FIELD), false);
	assert.equal(f.writesCalled.length, 1);
	f.controller.dispose();
});
test('record-only read access retains drafts but refuses saves/archive and withheld bodies are never overwritten', async () => {
	const f = fixture();
	await f.controller.load();
	await f.controller.openPanel(ID);
	f.controller.setDraft(ID, FIELD, 'draft');
	f.controller.setBodyDraft(ID, 'body draft');
	f.context = {
		...f.context,
		record: record({
			permitted_actions: ['view'],
			record_content_writable: false,
			record_archive_allowed: false,
			semantic_content: {}
		}),
		field_secrecy_enabled: true
	};
	await f.controller.refresh();
	assert.equal(f.controller.snapshot().panel?.canEdit, false);
	assert.equal(f.controller.snapshot().panel?.bodyWithheld, true);
	assert.equal(f.controller.snapshot().drafts[ID]?.[FIELD], 'draft');
	assert.equal(await f.controller.commitCell(ID, FIELD), false);
	assert.equal(await f.controller.commitBody(ID), false);
	assert.equal((await f.controller.archiveRecord(ID, true)).kind, 'failed');
	assert.equal(f.writesCalled.length, 0);
	f.controller.dispose();
});

test('inactive container is content-readonly without faking Record lifecycle, while confirmed cleanup remains available', async () => {
	const f = fixture();
	f.context = {
		...f.context,
		record: record({
			record_content_writable: false,
			record_archive_allowed: true,
			record_readonly_reason: 'container_inactive'
		})
	};
	await f.controller.load();
	await f.controller.openPanel(ID);
	assert.equal(f.controller.snapshot().recordObject?.lifecycle_status, 'active');
	assert.equal(f.controller.snapshot().panel?.canEdit, false);
	assert.equal(f.controller.snapshot().panel?.canArchive, true);
	f.controller.setDraft(ID, FIELD, 'kept draft');
	assert.equal(await f.controller.commitCell(ID, FIELD), false);
	assert.equal(f.writesCalled.length, 0);
	assert.equal((await f.controller.archiveRecord(ID, false)).kind, 'needs_confirmation');
	assert.equal(f.writesCalled.length, 0);
	assert.equal((await f.controller.archiveRecord(ID, true)).kind, 'archived');
	assert.equal(f.writesCalled[0].id, ID);
	assert.equal(f.controller.snapshot().recordObject?.lifecycle_status, 'archived');
	assert.equal(f.controller.snapshot().panel?.canArchive, false);
	f.controller.dispose();
});
