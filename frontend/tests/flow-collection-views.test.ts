/**
 * Collection renderers' state and service layer (`versions/v0.6-collections.md` "Web 交互";
 * `ui-surface-v1.md` "Renderer registry" / "职责边界"; `ADR-0018` NR-7).
 *
 * - schema parsing (positions, archived, unknown types, restricted, select options, the root
 *   order `field_reorder`/`view_reorder` index into);
 * - typed value encode/decode for every first-release type, including a number typed as a string
 *   and a calendar date;
 * - `record_patch`: the exact request through the real `CommandService` and a mocked `fetch`
 *   (one field, the record's frontier as `expected_frontier`), `stale_frontier` handling, key
 *   reuse;
 * - large collections: loading, paging and windowing open no record document; only the panel
 *   does, and closing or moving it closes the previous one;
 * - schema mode: outside it no `field_*` / `view_*` command is sent at all;
 * - board: a drag patches the grouping field with the target option;
 * - restricted / missing / malformed values are placeholders, never guessed.
 *
 * Run standalone: `bun tests/flow-collection-views.test.ts`
 */

import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	COLLECTION_FIELD_TYPES,
	CELL_PLACEHOLDER_KEYS,
	ENCODE_FAILURE_KEYS,
	collectionRootOrder,
	decodeValue,
	displayValue,
	editorValue,
	encodeFilterValue,
	encodeValue,
	parseCollection,
	parseField,
	reorderIndex
} from '../src/lib/flow/collection-values';
import {
	CollectionViewController,
	RECORD_STALE_KEY,
	SCHEMA_MODE_REQUIRED_KEY,
	boardColumns,
	boardGroupFieldIn,
	nextGridCell,
	virtualWindow,
	type CollectionReads,
	type CollectionWrites
} from '../src/lib/flow/collection-view';
import { FlowCommandService } from '../src/lib/flow/command-service';
import { resolveRenderer } from '../src/lib/flow/renderer-registry';
import type {
	AcceptedChange,
	FlowCollectionDescription,
	FlowCollectionFieldView,
	FlowCollectionRecordsQuery,
	FlowCollectionRecordsResponse,
	FlowCommandInput,
	FlowObjectView
} from '../src/lib/api/flow';
import type { ApiResult } from '../src/lib/api/client';

const ROOT = new URL('..', import.meta.url).pathname;
const COLLECTION = 'c0000000-0000-4000-8000-000000000001';
const F_NAME = 'f0000000-0000-4000-8000-00000000000a';
const F_COUNT = 'f0000000-0000-4000-8000-00000000000b';
const F_STATUS = 'f0000000-0000-4000-8000-00000000000c';
const F_SECRET = 'f0000000-0000-4000-8000-00000000000d';
const V_TABLE = 'v0000000-0000-4000-8000-000000000001';
const V_BOARD = 'v0000000-0000-4000-8000-000000000002';

const suite = new Suite('flow_collection_views');

function ok<T>(data: T): ApiResult<T> {
	return { code: 0, message: 'ok', data };
}

function objectView(id: string, extra: Partial<FlowObjectView> = {}): FlowObjectView {
	return {
		id,
		workspace_id: 'ws-1',
		project_id: null,
		object_type: 'record',
		lifecycle_status: 'active',
		governance_metadata: {},
		title: '',
		semantic_content: {},
		document_id: `doc-${id}`,
		document_seq: 1,
		frontier: `fr-${id}-1`,
		projection_seq: 1,
		projection_lag: 0,
		created_at: '2026-10-10T00:00:00Z',
		updated_at: '2026-10-10T00:00:00Z',
		archived_at: null,
		...extra
	};
}

function fieldView(
	field_id: string,
	label: string,
	type: string,
	position: number,
	config: Record<string, unknown> = {},
	archived = false
): FlowCollectionFieldView {
	return { field_id, label, type, config, position, archived, document_seq: 1 };
}

function description(extra: Partial<FlowCollectionDescription> = {}): FlowCollectionDescription {
	return {
		object: objectView(COLLECTION, {
			object_type: 'collection',
			title: 'Tasks',
			frontier: 'collection-fr-1',
			semantic_content: {
				nodes: {
					[V_TABLE]: {
						kind: 'collection_view',
						order_key: '00000000',
						parent: null,
						deleted: false
					},
					[F_NAME]: {
						kind: 'collection_field',
						order_key: '00000001',
						parent: null,
						deleted: false
					},
					[F_COUNT]: {
						kind: 'collection_field',
						order_key: '00000002',
						parent: null,
						deleted: false
					},
					[F_STATUS]: {
						kind: 'collection_field',
						order_key: '00000003',
						parent: null,
						deleted: false
					},
					[V_BOARD]: {
						kind: 'collection_view',
						order_key: '00000004',
						parent: null,
						deleted: false
					},
					gone: { kind: 'collection_field', order_key: '00000005', parent: null, deleted: true }
				}
			}
		}),
		fields: [
			fieldView(F_STATUS, 'Status', 'select', 2, { options: ['Todo', 'Done'] }),
			fieldView(F_NAME, 'Name', 'text', 0),
			fieldView(F_COUNT, 'Count', 'number', 1),
			fieldView('f-archived', 'Old', 'text', 3, {}, true)
		],
		views: [
			{ view_id: V_BOARD, name: 'Board', type: 'board', config: {}, position: 1, document_seq: 1 },
			{
				view_id: V_TABLE,
				name: 'Table',
				type: 'table',
				config: {
					sort: { field_id: F_COUNT, direction: 'desc' },
					column_widths: { [F_NAME]: 240 }
				},
				position: 0,
				document_seq: 1
			}
		],
		record_count: 0,
		schema_seq: 1,
		projection_seq: 1,
		client_crdt_enabled: false,
		field_secrecy_enabled: false,
		...extra
	};
}

function recordItem(id: string, values: Record<string, unknown>, frontier = `fr-${id}-1`) {
	return { record: objectView(id, { frontier }), values_by_field_id: values };
}

function accepted(id: string, headFrontier: string): AcceptedChange {
	return {
		object: objectView(id, { frontier: headFrontier }),
		accepted_seq: 2,
		head_frontier: headFrontier,
		projection_seq: 2,
		semantic_diff: {},
		affected_object_ids: [id],
		event_id: 'ev-1'
	};
}

interface FakeRepository extends CollectionReads {
	readonly opened: string[];
	readonly closed: string[];
	readonly listQueries: FlowCollectionRecordsQuery[];
	readonly schemaReads: string[];
	readonly open: Map<string, number>;
	pages: FlowCollectionRecordsResponse[];
	describe: FlowCollectionDescription;
}

function fakeRepository(pages: FlowCollectionRecordsResponse[]): FakeRepository {
	const repo: FakeRepository = {
		opened: [],
		closed: [],
		listQueries: [],
		schemaReads: [],
		open: new Map(),
		pages,
		describe: description(),
		async getCollection(collectionId: string) {
			repo.schemaReads.push(`collection:${collectionId}`);
			return ok(repo.describe);
		},
		async getSchema(objectId: string) {
			repo.schemaReads.push(`object:${objectId}`);
			return ok(repo.describe);
		},
		async listRecords(_id: string, query: FlowCollectionRecordsQuery) {
			repo.listQueries.push(query);
			const index = query.cursor ? Number(query.cursor.replace('cursor-', '')) : 0;
			const page = repo.pages[index] ?? { fields: [], items: [], projection_seq: 1, schema_seq: 1 };
			return ok(page);
		},
		async openRecord(recordId: string) {
			repo.opened.push(recordId);
			repo.open.set(recordId, (repo.open.get(recordId) ?? 0) + 1);
			return objectView(recordId, {
				frontier: `fr-${recordId}-panel`,
				semantic_content: {
					nodes: {
						'record:body': {
							kind: 'block',
							deleted: false,
							properties: { body: `body of ${recordId}` }
						},
						[F_SECRET]: {
							kind: 'record_property',
							deleted: false,
							properties: { value: '"leaked"' }
						}
					}
				}
			});
		},
		closeRecord(recordId: string) {
			repo.closed.push(recordId);
			const count = (repo.open.get(recordId) ?? 0) - 1;
			if (count <= 0) repo.open.delete(recordId);
			else repo.open.set(recordId, count);
		}
	};
	return repo;
}

interface FakeWrites extends CollectionWrites {
	readonly executed: Array<{
		objectId: string;
		command: FlowCommandInput;
		key?: string;
		frontier?: string;
	}>;
	readonly created: Array<{ values: Record<string, unknown>; key?: string }>;
	failNext: unknown[];
}

function fakeWrites(): FakeWrites {
	const writes: FakeWrites = {
		executed: [],
		created: [],
		failNext: [],
		async execute(objectId, command, options = {}) {
			writes.executed.push({
				objectId,
				command,
				key: options.idempotencyKey,
				frontier: options.expectedFrontier
			});
			const failure = writes.failNext.shift();
			if (failure) throw failure;
			const payload = (command.payload ?? {}) as { record_id?: string };
			return accepted(payload.record_id ?? objectId, `head-${writes.executed.length}`);
		},
		async createRecord(_collectionId, input, options = {}) {
			writes.created.push({ values: input.values_by_field_id, key: options.idempotencyKey });
			const failure = writes.failNext.shift();
			if (failure) throw failure;
			return accepted('new-record', 'head-new');
		}
	};
	return writes;
}

function page(
	ids: string[],
	next: string | undefined,
	values: (id: string) => Record<string, unknown> = () => ({})
): FlowCollectionRecordsResponse {
	return {
		fields: [],
		items: ids.map((id) => recordItem(id, values(id))),
		next_cursor: next,
		projection_seq: 1,
		schema_seq: 1
	};
}

const textField = parseField(fieldView(F_NAME, 'Name', 'text', 0));
const numberField = parseField(fieldView(F_COUNT, 'Count', 'number', 1));
const boolField = parseField(fieldView('fb', 'Done', 'boolean', 2));
const dateField = parseField(fieldView('fd', 'Due', 'date', 3));
const selectField = parseField(
	fieldView(F_STATUS, 'Status', 'select', 4, { options: ['Todo', 'Done'] })
);
const multiField = parseField(
	fieldView('fm', 'Tags', 'multi_select', 5, { options: ['a', 'b', 'c'] })
);
const relationField = parseField(fieldView('fr', 'Links', 'relation', 6));

// ---- 1. schema parsing ----------------------------------------------------------------------

suite.check('schema: fields by position, archived dropped, views by position, root order', () => {
	const parsed = parseCollection(description());
	assertDeepEqual(
		parsed.fields.map((field) => [field.label, field.type]),
		[
			['Name', 'text'],
			['Count', 'number'],
			['Status', 'select']
		],
		'live fields in position order'
	);
	assertDeepEqual(parsed.fields[2].options, ['Todo', 'Done'], 'select options from config.options');
	assertDeepEqual(
		parsed.views.map((view) => view.name),
		['Table', 'Board'],
		'views by position'
	);
	assertDeepEqual(
		parsed.views[0].query.sort,
		{ field_id: F_COUNT, direction: 'desc' },
		'saved sort'
	);
	assertEqual(parsed.views[0].query.columnWidths[F_NAME], 240, 'saved column width');
	assertDeepEqual(
		parsed.rootOrder,
		[V_TABLE, F_NAME, F_COUNT, F_STATUS, V_BOARD],
		'fields and views share one root order; deleted nodes excluded'
	);
	assertEqual(parsed.frontier, 'collection-fr-1', 'collection frontier for schema commands');
});

suite.check('schema: unknown type is read-only, restricted is anything but absent/false', () => {
	const unknown = parseField(fieldView('x', 'Formula', 'formula', 0));
	assertEqual(unknown.type, null, 'unknown type -> null');
	assert(
		parseField(fieldView('r', 'R', 'text', 0, { restricted: true })).restricted,
		'restricted true'
	);
	assert(
		parseField(fieldView('r', 'R', 'text', 0, { restricted: 'yes' })).restricted,
		'non-bool counts'
	);
	assert(
		!parseField(fieldView('r', 'R', 'text', 0, { restricted: false })).restricted,
		'false is not'
	);
	assertEqual(collectionRootOrder(null).length, 0, 'no semantic content -> empty order');
});

suite.check('reorder index is the neighbour position in the shared root order', () => {
	const order = [V_TABLE, F_NAME, F_COUNT, F_STATUS, V_BOARD];
	assertEqual(reorderIndex(order, F_NAME, F_COUNT), 2, 'move Name down past Count');
	assertEqual(reorderIndex(order, F_STATUS, F_COUNT), 2, 'move Status up past Count');
	assertEqual(reorderIndex(order, F_NAME, 'missing'), null, 'unknown neighbour');
});

// ---- 2. typed value encode/decode -----------------------------------------------------------

suite.check('number: the input string is sent as a JSON number, round-trips', () => {
	const cases: Array<[string, number]> = [
		['42', 42],
		[' 3.50 ', 3.5],
		['-1e3', -1000],
		['.5', 0.5]
	];
	for (const [input, expected] of cases) {
		const encoded = encodeValue(numberField, input);
		assert(encoded.ok, `${input} encodes`);
		if (encoded.ok) {
			assertEqual(typeof encoded.value, 'number', `${input} is a number on the wire, not a string`);
			assertEqual(encoded.value, expected, `${input} value`);
			const decoded = decodeValue(numberField, encoded.value);
			assertEqual(decoded, String(expected), 'decodes to the editor string');
			const again = encodeValue(numberField, decoded ?? '');
			assert(again.ok && again.value === expected, 'round trip is stable');
		}
	}
	for (const bad of ['abc', '1,5', '1e999', '0x10', 'NaN']) {
		const encoded = encodeValue(numberField, bad);
		assert(!encoded.ok && encoded.reason === 'number_invalid', `${bad} is refused locally`);
	}
	const empty = encodeValue(numberField, '  ');
	assert(empty.ok && empty.value === null, 'empty clears');
	assertEqual(decodeValue(numberField, '42'), null, 'a stored numeric STRING is not coerced');
});

suite.check('date: only a real calendar date YYYY-MM-DD is sent', () => {
	const good = encodeValue(dateField, '2026-10-10');
	assert(good.ok && good.value === '2026-10-10', 'date string');
	assertEqual(decodeValue(dateField, '2026-10-10'), '2026-10-10', 'round trip');
	for (const bad of ['2026-02-30', '2026-13-01', '10/10/2026', 'notadate', '2026-1-1']) {
		const encoded = encodeValue(dateField, bad);
		assert(!encoded.ok && encoded.reason === 'date_invalid', `${bad} refused`);
	}
	assert(encodeValue(dateField, '2024-02-29').ok, 'leap day');
	assertEqual(
		decodeValue(dateField, '2026-10-10T08:00:00Z'),
		null,
		'a date-time is not truncated into the editor'
	);
	assertDeepEqual(
		displayValue(dateField, { fd: '2026-10-10T08:00:00Z' }),
		{ kind: 'text', text: '2026-10-10T08:00:00Z' },
		'a date-time written elsewhere is displayed as stored'
	);
});

suite.check('text, boolean, select, multi_select round trips; relation read-only', () => {
	const text = encodeValue(textField, 'hello');
	assert(text.ok && text.value === 'hello', 'text');
	assertEqual(decodeValue(textField, 'hello'), 'hello', 'text decode');
	const cleared = encodeValue(textField, '');
	assert(cleared.ok && cleared.value === null, 'empty text clears');

	for (const value of [true, false]) {
		const encoded = encodeValue(boolField, value);
		assert(encoded.ok && encoded.value === value, `boolean ${value}`);
		assertEqual(decodeValue(boolField, value), value, 'boolean decode');
	}
	assert(!encodeValue(boolField, 'true').ok, 'a string is not a boolean');

	const select = encodeValue(selectField, 'Done');
	assert(select.ok && select.value === 'Done', 'select option');
	assert(!encodeValue(selectField, 'Doing').ok, 'unknown option refused');
	const none = encodeValue(selectField, '');
	assert(none.ok && none.value === null, 'no option clears');

	const multi = encodeValue(multiField, ['c', 'a']);
	assert(multi.ok, 'multi encodes');
	if (multi.ok) assertDeepEqual(multi.value, ['a', 'c'], 'option order, not click order');
	assertDeepEqual(decodeValue(multiField, ['a', 'c']), ['a', 'c'], 'multi decode');
	assert(!encodeValue(multiField, ['z']).ok, 'unknown option refused');

	const relation = encodeValue(relationField, '');
	assert(!relation.ok && relation.reason === 'read_only', 'relation is not edited in this release');
	assertDeepEqual(
		displayValue(relationField, { fr: ['a0000000-0000-4000-8000-000000000001'] }),
		{ kind: 'relation', ids: ['a0000000-0000-4000-8000-000000000001'] },
		'relation displayed as links'
	);
	assertEqual(
		COLLECTION_FIELD_TYPES.length,
		7,
		'the seven first-release types (collections.rs::validate_field_type)'
	);
});

suite.check('filter values use the field wire type; operators follow the server table', () => {
	const number = encodeFilterValue(numberField, 'gt', '10');
	assert(number.ok && number.value === 10, 'numeric filter value is a number');
	assert(!encodeFilterValue(numberField, 'contains', '10').ok, 'contains not valid for number');
	assert(!encodeFilterValue(textField, 'lt', 'a').ok, 'lt not valid for text');
	const multi = encodeFilterValue(multiField, 'contains', 'b');
	assert(multi.ok, 'contains on multi_select');
	if (multi.ok) assertDeepEqual(multi.value, ['b'], 'contains takes an array');
	assert(!encodeFilterValue(numberField, 'eq', '').ok, 'empty filter value refused');
});

// ---- 3. restricted / missing / malformed are never guessed ---------------------------------

suite.check('restricted, missing and malformed values are placeholders', () => {
	const restricted = parseField(fieldView(F_SECRET, 'Salary', 'number', 0, { restricted: true }));
	assertEqual(
		displayValue(restricted, { [F_SECRET]: 100 }).kind,
		'restricted',
		'restricted shown as such'
	);
	assertEqual(displayValue(numberField, {}).kind, 'empty', 'absent key -> empty');
	assertEqual(displayValue(numberField, { [F_COUNT]: null }).kind, 'empty', 'null -> empty');
	assertEqual(
		displayValue(numberField, { [F_COUNT]: '42' }).kind,
		'invalid',
		'string number -> invalid, not 42'
	);
	assertEqual(displayValue(boolField, { fb: 'true' }).kind, 'invalid', 'string boolean -> invalid');
	assertEqual(
		displayValue(selectField, { [F_STATUS]: ['Done'] }).kind,
		'invalid',
		'array for select -> invalid'
	);
	assertEqual(
		editorValue(numberField, { [F_COUNT]: '42' }, null),
		'',
		'editor starts empty, not from a guess'
	);
	assertEqual(
		editorValue(restricted, { [F_SECRET]: 100 }, null),
		'',
		'restricted editor never shows the value'
	);
	for (const key of Object.values(CELL_PLACEHOLDER_KEYS))
		assert(key.startsWith('flow.collection.'), key);
});

await suite.checkAsync(
	'a record row takes values only from values_by_field_id, never semantic_content',
	async () => {
		const repo = fakeRepository([page(['r1'], undefined, () => ({ [F_NAME]: 'shown' }))]);
		repo.describe = description({ field_secrecy_enabled: true });
		const writes = fakeWrites();
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		await controller.openPanel('r1');
		const state = controller.snapshot();
		assertDeepEqual(state.rows[0].values, { [F_NAME]: 'shown' }, 'only the list values');
		assertEqual(state.panel?.body, 'body of r1', 'the panel reads the record body block');
		assert(
			!JSON.stringify(state).includes('leaked'),
			'the record property in semantic_content never enters state'
		);
	}
);

// ---- 4. record_patch request shape -----------------------------------------------------------

await suite.checkAsync(
	'record_patch over the real CommandService: one field, expected_frontier, number',
	async () => {
		const bodies: Array<{ url: string; body: Record<string, unknown> }> = [];
		const original = globalThis.fetch;
		globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
			bodies.push({ url: String(input), body: JSON.parse(String(init?.body)) });
			return new Response(JSON.stringify(ok(accepted('r1', 'head-after'))), {
				headers: { 'Content-Type': 'application/json' }
			});
		}) as typeof fetch;
		const repo = fakeRepository([page(['r1', 'r2'], undefined)]);
		const controller = new CollectionViewController(COLLECTION, repo, new FlowCommandService());
		let accepted1 = false;
		let accepted2 = false;
		try {
			await controller.load();
			accepted1 = await controller.commitCell('r1', F_COUNT, '42');
			accepted2 = await controller.commitCell('r1', F_NAME, 'Alpha');
		} finally {
			globalThis.fetch = original;
		}
		assert(accepted1 && accepted2, 'both accepted');
		assertEqual(bodies.length, 2, 'one request per cell');
		assert(
			bodies[0].url.endsWith(`/api/v1/flow/objects/${COLLECTION}/commands`),
			'collection command endpoint'
		);
		assertDeepEqual(
			bodies[0].body.command,
			{ type: 'record_patch', payload: { record_id: 'r1', properties: { [F_COUNT]: 42 } } },
			'exactly one property, the number as a JSON number'
		);
		assertEqual(bodies[0].body.expected_frontier, 'fr-r1-1', 'the record frontier from the list');
		assert(typeof bodies[0].body.idempotency_key === 'string', 'key present');
		assertEqual(
			bodies[1].body.expected_frontier,
			'head-after',
			'the next patch uses the accepted head frontier'
		);
		assertNotSameKey(bodies[0].body.idempotency_key, bodies[1].body.idempotency_key);
		assertDeepEqual(
			controller.snapshot().rows[0].values,
			{ [F_COUNT]: 42, [F_NAME]: 'Alpha' },
			'row updated'
		);
		assertEqual(controller.snapshot().drafts.r1, undefined, 'drafts cleared after accept');
	}
);

function assertNotSameKey(a: unknown, b: unknown): void {
	assert(a !== b, 'two different intents use two keys');
}

await suite.checkAsync('an invalid value is refused locally with zero requests', async () => {
	const repo = fakeRepository([page(['r1'], undefined)]);
	const writes = fakeWrites();
	const controller = new CollectionViewController(COLLECTION, repo, writes);
	await controller.load();
	const result = await controller.commitCell('r1', F_COUNT, 'twelve');
	assert(!result, 'not accepted');
	assertEqual(writes.executed.length, 0, 'nothing sent');
	const status = controller.snapshot().cellStatus[`r1:${F_COUNT}`];
	assert(
		status?.state === 'error' && status.key === ENCODE_FAILURE_KEYS.number_invalid,
		'local error key'
	);
	assertEqual(controller.snapshot().drafts.r1?.[F_COUNT], 'twelve', 'draft kept');
});

await suite.checkAsync(
	'stale_frontier keeps the draft, re-reads the records, does not resend',
	async () => {
		const repo = fakeRepository([page(['r1'], undefined)]);
		const writes = fakeWrites();
		writes.failNext.push({
			code: 'stale_frontier',
			recoverable: true,
			origin: 'server',
			details: {}
		});
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		assertEqual(repo.listQueries.length, 1, 'one read');
		repo.pages = [
			{ ...page(['r1'], undefined), items: [recordItem('r1', { [F_NAME]: 'theirs' }, 'fr-r1-2')] }
		];
		const result = await controller.commitCell('r1', F_NAME, 'mine');
		assert(!result, 'not accepted');
		assertEqual(writes.executed.length, 1, 'no automatic resend');
		assertEqual(repo.listQueries.length, 2, 'records re-read');
		const state = controller.snapshot();
		assertEqual(state.drafts.r1?.[F_NAME], 'mine', 'draft kept');
		assertEqual(state.cellStatus[`r1:${F_NAME}`]?.state, 'conflict', 'conflict shown');
		assertEqual(state.notice?.key, RECORD_STALE_KEY, 'notice');
		assertDeepEqual(state.rows[0].values, { [F_NAME]: 'theirs' }, 'fresh values');
		const ok2 = await controller.commitCell('r1', F_NAME);
		assert(ok2, 'resubmit accepted');
		assertEqual(writes.executed[1].frontier, 'fr-r1-2', 'resubmit uses the re-read frontier');
		assert(writes.executed[1].key !== writes.executed[0].key, 'a new frontier is a new intent');
	}
);

await suite.checkAsync(
	'a transient failure retried as the same intent reuses its key',
	async () => {
		const repo = fakeRepository([page(['r1'], undefined)]);
		const writes = fakeWrites();
		writes.failNext.push({
			code: 'server_draining',
			recoverable: true,
			origin: 'server',
			details: { reason: 'contention', retry_after_ms: 0 }
		});
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		assert(!(await controller.commitCell('r1', F_NAME, 'x')), 'first attempt failed');
		const status = controller.snapshot().cellStatus[`r1:${F_NAME}`];
		assert(status?.state === 'error' && status.code === 'server_draining', 'typed error kept');
		assert(await controller.commitCell('r1', F_NAME), 'retry accepted');
		assertEqual(writes.executed[0].key, writes.executed[1].key, 'same key for the same intent');
	}
);

await suite.checkAsync(
	'CommandService.execute throws the envelope error_code, never the message',
	async () => {
		const original = globalThis.fetch;
		globalThis.fetch = (async () =>
			new Response(
				JSON.stringify({
					code: 409,
					message: 'policy_rejected: everything is fine',
					data: null,
					error_code: 'stale_frontier',
					details: { current_seq: 3 }
				}),
				{ headers: { 'Content-Type': 'application/json' } }
			)) as typeof fetch;
		let thrown: unknown;
		try {
			await new FlowCommandService().execute(COLLECTION, {
				type: 'record_patch',
				payload: { record_id: 'r1', properties: {} }
			});
		} catch (error) {
			thrown = error;
		} finally {
			globalThis.fetch = original;
		}
		assertEqual((thrown as { code?: string }).code, 'stale_frontier', 'typed code');
		assertDeepEqual((thrown as { details?: unknown }).details, { current_seq: 3 }, 'details kept');
	}
);

// ---- 5. large collections: documents open only for the panel ----------------------------------

await suite.checkAsync(
	'loading, paging, windowing and editing open no record document',
	async () => {
		const ids = (from: number) => Array.from({ length: 100 }, (_, i) => `r${from + i}`);
		const repo = fakeRepository([
			page(ids(0), 'cursor-1'),
			page(ids(100), 'cursor-2'),
			page(ids(200), undefined)
		]);
		const writes = fakeWrites();
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		await controller.loadMore();
		await controller.loadMore();
		assertEqual(controller.snapshot().rows.length, 300, '300 rows loaded');
		assertEqual(repo.listQueries[1].cursor, 'cursor-1', 'paging sends the cursor');
		assertEqual(repo.listQueries[1].view_id, V_TABLE, 'with the same query (the saved view)');
		for (let scrollTop = 0; scrollTop < 300 * 48; scrollTop += 900) {
			const window = virtualWindow({
				scrollTop,
				viewportHeight: 600,
				rowHeight: 48,
				rowCount: 300
			});
			assert(window.end - window.start <= 26, `window bounded at ${scrollTop}`);
		}
		await controller.commitCell('r150', F_NAME, 'edited');
		await controller.setActiveView(V_BOARD);
		await controller.moveCard('r10', 'Done');
		assertEqual(repo.opened.length, 0, 'no record document opened by list, scroll, edit or drag');

		await controller.openPanel('r5');
		assertDeepEqual(repo.opened, ['r5'], 'the panel opens exactly its record');
		await controller.openPanel('r7');
		assertDeepEqual(repo.opened, ['r5', 'r7'], 'moving the panel opens the next record');
		assertDeepEqual(repo.closed, ['r5'], '... and closes the previous one');
		controller.closePanel();
		assertDeepEqual(repo.closed, ['r5', 'r7'], 'closing the panel closes its record');
		assertEqual(repo.open.size, 0, 'no record document left open');
		await controller.openPanel('r9');
		controller.dispose();
		assertEqual(repo.open.size, 0, 'dispose closes the open record');
	}
);

suite.check('virtual window: only the visible slice plus overscan', () => {
	const top = virtualWindow({ scrollTop: 0, viewportHeight: 480, rowHeight: 48, rowCount: 10_000 });
	assertEqual(top.start, 0, 'start');
	assertEqual(top.end, 17, '11 visible + 6 overscan');
	assertEqual(top.padBottom, (10_000 - 17) * 48, 'bottom spacer');
	const mid = virtualWindow({
		scrollTop: 48 * 5000,
		viewportHeight: 480,
		rowHeight: 48,
		rowCount: 10_000
	});
	assertEqual(mid.start, 4994, 'mid start');
	assertEqual(mid.end, 5017, 'mid end');
	assertEqual(mid.padTop, 4994 * 48, 'top spacer');
	const small = virtualWindow({ scrollTop: 0, viewportHeight: 480, rowHeight: 48, rowCount: 3 });
	assertEqual(small.end, 3, 'clamped to the row count');
});

// ---- 6. schema mode gate ---------------------------------------------------------------------

await suite.checkAsync('outside schema mode no field_* / view_* command is sent', async () => {
	const repo = fakeRepository([page(['r1'], undefined)]);
	const writes = fakeWrites();
	const controller = new CollectionViewController(COLLECTION, repo, writes);
	await controller.load();
	assertEqual(controller.snapshot().mode, 'entry', 'entry mode by default');
	const outcomes = [
		await controller.createField('Notes', 'text'),
		await controller.renameField(F_NAME, 'Title'),
		await controller.setFieldOptions(F_STATUS, ['A']),
		await controller.changeFieldType(F_NAME, 'number'),
		await controller.archiveField(F_NAME),
		await controller.moveField(F_NAME, 1),
		await controller.createView('Kanban', 'board'),
		await controller.renameView(V_TABLE, 'Grid'),
		await controller.moveView(V_TABLE, 1),
		await controller.saveActiveView()
	];
	for (const outcome of outcomes) {
		assert(
			!outcome.ok && outcome.key === SCHEMA_MODE_REQUIRED_KEY,
			'refused with the schema-mode key'
		);
	}
	assertEqual(writes.executed.length, 0, 'zero schema requests');
	// Values remain editable in entry mode.
	assert(await controller.commitCell('r1', F_NAME, 'ok'), 'value edit allowed');
	assertDeepEqual(
		writes.executed.map((entry) => entry.command.type),
		['record_patch'],
		'only record_patch went out'
	);
});

await suite.checkAsync(
	'in schema mode the field/view commands carry the server payload shapes',
	async () => {
		const repo = fakeRepository([page([], undefined)]);
		const writes = fakeWrites();
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		controller.setMode('schema');
		assert(
			(await controller.createField(' Stage ', 'select', ['Todo', 'Done', 'Todo'])).ok,
			'create'
		);
		const create = writes.executed[0];
		assertEqual(create.command.type, 'field_create', 'type');
		const payload = create.command.payload as Record<string, unknown>;
		assertEqual(payload.label, 'Stage', 'trimmed label');
		assertEqual(payload.field_type, 'select', 'field_type');
		assertDeepEqual(payload.config, { options: ['Todo', 'Done', 'Todo'] }, 'options as typed');
		assertEqual(payload.index, 5, 'appended after the five live root nodes');
		assert(
			typeof payload.field_id === 'string' && (payload.field_id as string).length === 36,
			'client field id'
		);
		assertEqual(create.frontier, 'collection-fr-1', 'collection frontier as expected_frontier');
		assertEqual(create.objectId, COLLECTION, 'sent to the collection');

		assert((await controller.moveField(F_NAME, 1)).ok, 'move');
		assertDeepEqual(
			writes.executed[1].command,
			{ type: 'field_reorder', payload: { field_id: F_NAME, index: 2 } },
			'reorder index into the root order'
		);
		assert((await controller.setFieldOptions(F_STATUS, [' A ', 'B', 'A', ''])).ok, 'options');
		assertDeepEqual(
			writes.executed[2].command,
			{ type: 'field_update', payload: { field_id: F_STATUS, config: { options: ['A', 'B'] } } },
			'options cleaned, other config kept'
		);
		writes.failNext.push({ code: 'invalid_update', recoverable: false, origin: 'server' });
		const typeChange = await controller.changeFieldType(F_NAME, 'number');
		assert(
			!typeChange.ok && typeChange.code === 'invalid_update',
			'server refusal surfaced by code'
		);
		if (!typeChange.ok) assertEqual(typeChange.key, 'flow.error.invalid_update', 'error key');
		assertEqual(controller.snapshot().notice?.code, 'invalid_update', 'notice by code');
		assert(!(await controller.createField('', 'text')).ok, 'empty label refused locally');
		assertEqual(writes.executed.length, 4, 'the empty label sent nothing');
	}
);

await suite.checkAsync(
	'save view writes the draft filter/sort and widths with view_update',
	async () => {
		const repo = fakeRepository([page([], undefined)]);
		const writes = fakeWrites();
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		await controller.setDraftQuery({ filter: { field_id: F_COUNT, op: 'gt', value: 3 } });
		controller.setColumnWidth(F_COUNT, 300);
		assertDeepEqual(
			repo.listQueries[1].filter,
			{ field_id: F_COUNT, op: 'gt', value: 3 },
			'the draft filter is a query parameter'
		);
		assertEqual(writes.executed.length, 0, 'a draft filter writes nothing');
		controller.setMode('schema');
		assert((await controller.saveActiveView()).ok, 'saved');
		assertDeepEqual(
			writes.executed[0].command,
			{
				type: 'view_update',
				payload: {
					view_id: V_TABLE,
					config: {
						sort: { field_id: F_COUNT, direction: 'desc' },
						column_widths: { [F_NAME]: 240, [F_COUNT]: 300 },
						filter: { field_id: F_COUNT, op: 'gt', value: 3 }
					}
				}
			},
			'view_update with the merged config'
		);
		assertEqual(controller.snapshot().draftQuery, null, 'draft cleared after save');
	}
);

// ---- 7. board -------------------------------------------------------------------------------

await suite.checkAsync(
	'board: grouping by the select field; a drag patches that field',
	async () => {
		const repo = fakeRepository([
			page(['a', 'b', 'c'], undefined, (id) =>
				id === 'a' ? { [F_STATUS]: 'Todo' } : id === 'b' ? { [F_STATUS]: 'Legacy' } : {}
			)
		]);
		const writes = fakeWrites();
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load(V_BOARD);
		assertEqual(repo.listQueries[0].group, F_STATUS, 'board reads grouped by the select field');
		const field = boardGroupFieldIn(controller.snapshot());
		assert(field?.fieldId === F_STATUS, 'group field');
		if (!field) return;
		const columns = boardColumns(controller.snapshot().rows, field);
		assertDeepEqual(
			columns.map((column) => [column.value, column.rows.map((row) => row.recordId)]),
			[
				['Todo', ['a']],
				['Done', []],
				['Legacy', ['b']],
				[null, ['c']]
			],
			'options in order, then a non-option value, then no value'
		);
		assert(await controller.moveCard('c', 'Done'), 'drag accepted');
		assertDeepEqual(
			writes.executed[0].command,
			{ type: 'record_patch', payload: { record_id: 'c', properties: { [F_STATUS]: 'Done' } } },
			'the drag patches the grouping field with the target option'
		);
		assertEqual(writes.executed[0].frontier, 'fr-c-1', 'with the card record frontier');
		assert(await controller.moveCard('a', null), 'to no value');
		assertDeepEqual(
			writes.executed[1].command,
			{ type: 'record_patch', payload: { record_id: 'a', properties: { [F_STATUS]: null } } },
			'no-value column clears the field'
		);
		assert(await controller.moveCard('b', 'Legacy'), 'same column is a no-op');
		assertEqual(writes.executed.length, 2, 'no request for a drop on the same column');
	}
);

await suite.checkAsync('board without a select field is disabled', async () => {
	const repo = fakeRepository([page([], undefined)]);
	repo.describe = description({ fields: [fieldView(F_NAME, 'Name', 'text', 0)] });
	const controller = new CollectionViewController(COLLECTION, repo, fakeWrites());
	await controller.load(V_BOARD);
	assertEqual(boardGroupFieldIn(controller.snapshot()), null, 'no group field');
	assert(!(await controller.moveCard('x', 'Done')), 'move refused');
});

// ---- 8. drafts and panel survive view switches; record create ---------------------------------

await suite.checkAsync(
	'drafts and the panel survive a table/board switch and a panel reopen',
	async () => {
		const repo = fakeRepository([page(['r1'], undefined)]);
		const writes = fakeWrites();
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		await controller.openPanel('r1');
		controller.setDraft('r1', F_NAME, 'unsaved');
		controller.setBodyDraft('r1', 'draft body');
		await controller.setActiveView(V_BOARD);
		await controller.setActiveView(V_TABLE);
		let state = controller.snapshot();
		assertEqual(state.panel?.recordId, 'r1', 'panel still open');
		assertEqual(state.drafts.r1?.[F_NAME], 'unsaved', 'cell draft kept');
		assertEqual(state.bodyDrafts.r1, 'draft body', 'body draft kept');
		controller.closePanel();
		await controller.openPanel('r1');
		state = controller.snapshot();
		assertEqual(state.bodyDrafts.r1, 'draft body', 'body draft kept across close/reopen');
		assert(await controller.commitBody('r1'), 'body saved');
		assertDeepEqual(
			writes.executed[0].command,
			{ type: 'record_patch', payload: { record_id: 'r1', body: 'draft body' } },
			'body patch'
		);
		assertEqual(writes.executed[0].frontier, 'fr-r1-panel', 'frontier of the panel read');
		assertEqual(controller.snapshot().panel?.body, 'draft body', 'stored body updated');
	}
);

await suite.checkAsync(
	'record create: values_by_field_id encoded, one key per intent',
	async () => {
		const repo = fakeRepository([page([], undefined)]);
		const writes = fakeWrites();
		writes.failNext.push({
			code: 'server_draining',
			recoverable: true,
			origin: 'server',
			details: { reason: 'contention' }
		});
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		const first = await controller.createRecord({
			[F_COUNT]: '7',
			[F_STATUS]: 'Todo',
			[F_NAME]: ''
		});
		assert(!first.ok, 'first failed');
		const second = await controller.createRecord({
			[F_COUNT]: '7',
			[F_STATUS]: 'Todo',
			[F_NAME]: ''
		});
		assert(second.ok, 'retry created');
		assertDeepEqual(
			writes.created[0].values,
			{ [F_COUNT]: 7, [F_STATUS]: 'Todo' },
			'typed values, empty omitted'
		);
		assertEqual(writes.created[0].key, writes.created[1].key, 'retry reuses the key');
		await controller.createRecord();
		assert(writes.created[2].key !== writes.created[1].key, 'a new record is a new intent');
		assertEqual(repo.listQueries.length, 3, 'records re-read after each create');
		const bad = await controller.createRecord({ [F_COUNT]: 'x' });
		assert(
			!bad.ok && bad.key === ENCODE_FAILURE_KEYS.number_invalid,
			'invalid value refused locally'
		);
		assertEqual(writes.created.length, 3, 'nothing sent for it');
	}
);

// ---- 9. grid keyboard, registry, adapters, i18n ---------------------------------------------

suite.check('grid keyboard navigation is clamped', () => {
	assertDeepEqual(nextGridCell([0, 0], 'ArrowUp', 5, 3), [0, 0], 'top clamp');
	assertDeepEqual(nextGridCell([4, 2], 'ArrowDown', 5, 3), [4, 2], 'bottom clamp');
	assertDeepEqual(nextGridCell([1, 1], 'ArrowRight', 5, 3), [1, 2], 'right');
	assertDeepEqual(nextGridCell([1, 2], 'Home', 5, 3), [1, 0], 'home');
	assertEqual(nextGridCell([1, 1], 'Enter', 5, 3), null, 'Enter is not navigation');
});

suite.check('registry: collection:table, collection:board and record:panel load lazily', () => {
	for (const [type, view] of [
		['collection', 'table'],
		['collection', 'board'],
		['record', 'panel']
	] as const) {
		const registration = resolveRenderer(type, view);
		assert(registration, `${type}:${view} registered`);
		assertEqual(typeof registration?.load, 'function', `${type}:${view} has a dynamic loader`);
	}
	assertEqual(
		resolveRenderer('collection', 'canvas'),
		null,
		'a collection never reaches the canvas'
	);
	const registry = readFileSync(join(ROOT, 'src/lib/flow/renderer-registry.ts'), 'utf8');
	assert(
		!/^import\s+\w+\s+from\s+'\$lib\/components/m.test(registry),
		'no static component import'
	);
	assert(
		registry.includes("import('$lib/components/flow/collection/CollectionTable.svelte')"),
		'dynamic import'
	);
});

suite.check(
	'collection components use adapters only (no api/flow, engine or socket imports)',
	() => {
		const dir = join(ROOT, 'src/lib/components/flow/collection');
		for (const file of readdirSync(dir)) {
			const source = readFileSync(join(dir, file), 'utf8');
			for (const line of source.split('\n')) {
				if (!/^\s*import\s/.test(line)) continue;
				assert(
					!/\$lib\/api\/flow/.test(line) || /^\s*import\s+type\s/.test(line),
					`${file} imports lib/api/flow at runtime: ${line.trim()}`
				);
				assert(!/loro|WebSocket|indexedDB/i.test(line), `${file}: ${line.trim()}`);
			}
		}
		const route = readFileSync(
			join(ROOT, 'src/routes/(app)/workspace/[workspaceId]/flow/[objectId]/+page.svelte'),
			'utf8'
		);
		assert(
			route.includes('CollectionRenderer'),
			'object route dispatches collections to the renderer'
		);
		assert(
			/object_type === 'collection' \|\| object\.object_type === 'record'/.test(route),
			'collections and records are dispatched before repository.open (no engine doc, no ticket)'
		);
	}
);

suite.check('every static key the collection layer selects exists in zh and en', () => {
	const locales = ['zh', 'en'].map(
		(locale) =>
			JSON.parse(readFileSync(join(ROOT, `src/lib/i18n/${locale}.json`), 'utf8')) as Record<
				string,
				unknown
			>
	);
	const lookup = (catalogue: Record<string, unknown>, key: string): unknown =>
		key
			.split('.')
			.reduce<unknown>(
				(node, part) =>
					typeof node === 'object' && node !== null
						? (node as Record<string, unknown>)[part]
						: undefined,
				catalogue
			);
	const keys = new Set<string>([
		...Object.values(ENCODE_FAILURE_KEYS),
		...Object.values(CELL_PLACEHOLDER_KEYS),
		SCHEMA_MODE_REQUIRED_KEY,
		RECORD_STALE_KEY
	]);
	for (const dir of ['src/lib/components/flow/collection', 'src/lib/flow']) {
		for (const file of readdirSync(join(ROOT, dir))) {
			if (!/collection/i.test(file) && dir.endsWith('flow')) continue;
			if (!/\.(ts|svelte)$/.test(file)) continue;
			const source = readFileSync(join(ROOT, dir, file), 'utf8');
			for (const match of source.matchAll(/'(flow\.(?:collection|record)\.[A-Za-z0-9_.]+)'/g))
				keys.add(match[1]);
		}
	}
	assert(keys.size > 50, `collected ${keys.size} keys`);
	for (const key of keys) {
		for (const catalogue of locales) {
			const value = lookup(catalogue, key);
			assert(typeof value === 'string' && value.length > 0, `missing ${key}`);
		}
	}
});

// ---- FP2-M9: saved-view reads, field errors, withheld body, archive ---------------------------

await suite.checkAsync(
	'records are read by view_id; only a draft query is sent explicitly, without view_id',
	async () => {
		const repo = fakeRepository([page(['a'], undefined)]);
		const controller = new CollectionViewController(COLLECTION, repo, fakeWrites());
		await controller.load();
		assertDeepEqual(
			repo.listQueries[0],
			{ view_id: V_TABLE, cursor: undefined, limit: 50 },
			'the saved view is named, its filter/sort are not repeated'
		);
		await controller.setDraftQuery({ sort: { field_id: F_NAME, direction: 'asc' } });
		const drafted = repo.listQueries[1];
		assertEqual(drafted.view_id, undefined, 'a draft query carries no view_id');
		assertDeepEqual(drafted.sort, { field_id: F_NAME, direction: 'asc' }, 'the draft sort');
		await controller.setDraftQuery({ filter: null });
		assertEqual(repo.listQueries[2].filter, undefined, 'a cleared filter is not refilled');
		assertEqual(repo.listQueries[2].view_id, undefined, 'still no view_id');
		await controller.clearDraftQuery();
		assertEqual(repo.listQueries[3].view_id, V_TABLE, 'back on the saved view');
		await controller.setActiveView(V_BOARD);
		assertEqual(repo.listQueries[4].view_id, V_BOARD, 'board view by id');
		assertEqual(
			repo.listQueries[4].group,
			F_STATUS,
			'the board has no saved group: the fallback select field is sent explicitly'
		);
	}
);

await suite.checkAsync(
	'invalid_field_value is shown on the cell details.field_id names, with its own wording',
	async () => {
		const repo = fakeRepository([page(['a'], undefined)]);
		const writes = fakeWrites();
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		writes.failNext.push({
			code: 'invalid_update',
			recoverable: false,
			origin: 'server',
			details: { reason: 'invalid_field_value', field_id: F_STATUS }
		});
		controller.setDraft('a', F_NAME, 'x');
		assertEqual(await controller.commitCell('a', F_NAME), false, 'refused');
		const status = controller.snapshot().cellStatus;
		assertDeepEqual(
			status[`a:${F_STATUS}`],
			{
				state: 'error',
				key: 'flow.error.invalid_update_reason.invalid_field_value',
				code: 'invalid_field_value'
			},
			'the named cell shows the reason'
		);
		assertEqual(status[`a:${F_NAME}`], undefined, 'the committed cell is not left saving');
		writes.failNext.push({
			code: 'invalid_update',
			recoverable: false,
			origin: 'server',
			details: { reason: 'invalid_field_value', field_id: F_NAME }
		});
		controller.setDraft('a', F_NAME, 'y');
		await controller.commitCell('a', F_NAME);
		assertEqual(
			controller.snapshot().cellStatus[`a:${F_NAME}`]?.state === 'error' &&
				(controller.snapshot().cellStatus[`a:${F_NAME}`] as { key: string }).key,
			'flow.error.invalid_update_reason.invalid_field_value',
			'not the generic invalid_update wording'
		);
		assertEqual(controller.draftFor('a', F_NAME), 'y', 'the draft is kept for correction');
		writes.failNext.push({
			code: 'invalid_update',
			recoverable: false,
			origin: 'server',
			details: { reason: 'invalid_field_value', field_id: F_STATUS }
		});
		const created = await controller.createRecord({ [F_STATUS]: 'Todo' });
		assertDeepEqual(
			created,
			{
				ok: false,
				key: 'flow.error.invalid_update_reason.invalid_field_value',
				code: 'invalid_field_value',
				fieldId: F_STATUS
			},
			'a refused create names the field'
		);
		writes.failNext.push({ code: 'invalid_update', recoverable: false, origin: 'server' });
		controller.setDraft('a', F_NAME, 'z');
		await controller.commitCell('a', F_NAME);
		assertEqual(
			(controller.snapshot().cellStatus[`a:${F_NAME}`] as { key: string }).key,
			'flow.error.invalid_update',
			'no reason: the generic wording'
		);
	}
);

await suite.checkAsync(
	'field secrecy withholds the body: placeholder state, no editor write, controls stay readable',
	async () => {
		const withheldRead = (repo: FakeRepository, content: unknown) => {
			repo.openRecord = async (recordId: string) => {
				repo.opened.push(recordId);
				repo.open.set(recordId, (repo.open.get(recordId) ?? 0) + 1);
				return objectView(recordId, {
					frontier: `fr-${recordId}-panel`,
					semantic_content: content
				});
			};
		};
		const secret = fakeRepository([page(['a'], undefined)]);
		secret.describe = description({ field_secrecy_enabled: true });
		withheldRead(secret, {});
		const writes = fakeWrites();
		const controller = new CollectionViewController(COLLECTION, secret, writes);
		await controller.load();
		await controller.openPanel('a');
		const panel = controller.snapshot().panel;
		assertEqual(panel?.bodyWithheld, true, 'withheld under field secrecy');
		assertEqual(panel?.body, null, 'no body text is invented');
		controller.setBodyDraft('a', 'overwrite');
		assertEqual(await controller.commitBody('a'), false, 'a withheld body cannot be saved');
		assertEqual(writes.executed.length, 0, 'nothing sent');

		const restricted = fakeRepository([page(['a'], undefined)]);
		restricted.describe = description({
			fields: [
				...description().fields,
				fieldView(F_SECRET, 'Salary', 'number', 9, { restricted: true })
			]
		});
		withheldRead(restricted, {});
		const viaField = new CollectionViewController(COLLECTION, restricted, fakeWrites());
		await viaField.load();
		await viaField.openPanel('a');
		assertEqual(viaField.snapshot().panel?.bodyWithheld, true, 'a restricted field also counts');

		const open = fakeRepository([page(['a'], undefined)]);
		withheldRead(open, {});
		const noSecrecy = new CollectionViewController(COLLECTION, open, fakeWrites());
		await noSecrecy.load();
		await noSecrecy.openPanel('a');
		assertEqual(
			noSecrecy.snapshot().panel?.bodyWithheld,
			false,
			'without secrecy an empty content is just an empty body'
		);

		const readable = fakeRepository([page(['a'], undefined)]);
		readable.describe = description({ field_secrecy_enabled: true });
		const withNodes = new CollectionViewController(COLLECTION, readable, fakeWrites());
		await withNodes.load();
		await withNodes.openPanel('a');
		assertEqual(withNodes.snapshot().panel?.bodyWithheld, false, 'content present: readable');
		assertEqual(withNodes.snapshot().panel?.body, 'body of a', 'and shown');
	}
);

await suite.checkAsync(
	'record_archive needs confirmation; retry reuses the key; success drops the row and the panel',
	async () => {
		const repo = fakeRepository([page(['a', 'b'], undefined)]);
		const writes = fakeWrites();
		const controller = new CollectionViewController(COLLECTION, repo, writes);
		await controller.load();
		await controller.openPanel('a');
		assertDeepEqual(
			await controller.archiveRecord('a', false),
			{ kind: 'needs_confirmation' },
			'unconfirmed'
		);
		assertEqual(
			await controller
				.archiveRecord('a', undefined as unknown as boolean)
				.then((outcome) => outcome.kind),
			'needs_confirmation',
			'no default confirmation'
		);
		assertEqual(writes.executed.length, 0, 'nothing sent without confirmation');
		writes.failNext.push({
			code: 'server_draining',
			recoverable: true,
			origin: 'server',
			details: { reason: 'contention', retry_after_ms: 0 }
		});
		const failed = await controller.archiveRecord('a', true);
		assertEqual(failed.kind, 'failed', 'first attempt fails');
		assertEqual(controller.snapshot().panel?.recordId, 'a', 'panel stays on failure');
		repo.pages = [page(['b'], undefined)];
		const archived = await controller.archiveRecord('a', true);
		assertEqual(archived.kind, 'archived', 'archived');
		assertDeepEqual(
			writes.executed[0].command,
			{ type: 'record_archive', payload: { record_id: 'a' } },
			'record_archive on the collection'
		);
		assertEqual(writes.executed[0].objectId, COLLECTION, 'sent to the collection');
		assertEqual(writes.executed[1].key, writes.executed[0].key, 'the retry reuses the key');
		assertEqual(controller.snapshot().panel, null, 'panel closed');
		assertDeepEqual(
			controller.snapshot().rows.map((row) => row.recordId),
			['b'],
			'the row left the list'
		);
		assertEqual(repo.open.size, 0, 'the record document was released');
	}
);

await suite.checkAsync(
	'a record opened on its own route reads its schema through GET objects/{record}/schema',
	async () => {
		const repo = fakeRepository([page(['r1'], undefined)]);
		const controller = new CollectionViewController(COLLECTION, repo, fakeWrites(), {
			schemaFromRecord: 'r1'
		});
		await controller.load();
		assertEqual(controller.snapshot().status, 'ready', 'loaded');
		assertDeepEqual(repo.schemaReads, ['object:r1'], 'the record-scoped schema read, only');
		const plain = fakeRepository([page(['r1'], undefined)]);
		await new CollectionViewController(COLLECTION, plain, fakeWrites()).load();
		assertDeepEqual(plain.schemaReads, [`collection:${COLLECTION}`], 'a Collection reads its own');

		const seen: string[] = [];
		const original = globalThis.fetch;
		globalThis.fetch = (async (input: string | URL | Request) => {
			const raw = String(input);
			seen.push(raw.startsWith('http') ? new URL(raw).pathname : raw);
			return new Response(JSON.stringify({ code: 0, message: 'ok', data: description() }), {
				headers: { 'Content-Type': 'application/json' }
			});
		}) as typeof fetch;
		try {
			const { FlowObjectRepository } = await import('../src/lib/flow/object-repository');
			await new FlowObjectRepository().getSchema('rec-1');
		} finally {
			globalThis.fetch = original;
		}
		assertDeepEqual(seen, ['/api/v1/flow/objects/rec-1/schema'], 'the repository route');

		const renderer = readFileSync(
			join(ROOT, 'src/lib/components/flow/collection/CollectionRenderer.svelte'),
			'utf8'
		);
		assert(
			/schemaFromRecord:\s*recordId/.test(renderer),
			'the record route hands its record id to the controller for the schema read'
		);
	}
);

suite.check('ObjectRepository has no Collection adapters without a caller', () => {
	const repository = readFileSync(join(ROOT, 'src/lib/flow/object-repository.ts'), 'utf8');
	for (const name of ['queryRecords', 'openRecordIds']) {
		assert(!new RegExp(`\\b${name}\\(`).test(repository), `${name} is gone`);
	}
});

export const result = suite.result();

if (import.meta.main) finish(result);
