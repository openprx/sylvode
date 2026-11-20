/**
 * Default draft values for record editors (`src/lib/forms/record-draft-values.ts`).
 *
 * Run standalone: `bun tests/forms-page/record-draft-values.test.ts`
 */
import { Suite, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormField, FormRecord } from '../../src/lib/api/forms';

await installSvelteSsr();
const m = await import('../../src/lib/forms/record-draft-values');

const suite = new Suite('forms-page/record-draft-values');

function field(key: string, type: FormField['type'], extra: Partial<FormField> = {}): FormField {
	return { key, type, ...extra };
}

function record(values: Record<string, unknown>): FormRecord {
	return {
		id: 'rec-1',
		workspace_id: 'ws',
		project_id: 'pr',
		form_id: 'form',
		title: 'Record',
		values,
		source: {},
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-02T00:00:00Z'
	};
}

suite.check('defaultDraftValueForField uses the empty draft of each type without a default', () => {
	assertEqual(m.defaultDraftValueForField(field('a', 'text')), '', 'text');
	assertEqual(m.defaultDraftValueForField({ key: 'a' }), '', 'type absent');
	assertEqual(m.defaultDraftValueForField(field('a', 'number')), '', 'number');
	assertEqual(m.defaultDraftValueForField(field('a', 'boolean')), false, 'boolean');
	assertEqual(
		m.defaultDraftValueForField(field('a', 'single_select', { options: ['x'] })),
		'',
		'single select'
	);
	assertDeepEqual(
		m.defaultDraftValueForField(field('a', 'multi_select', { options: ['x', 'y'] })),
		[],
		'multi select with options'
	);
	assertEqual(
		m.defaultDraftValueForField(field('a', 'multi_select')),
		'',
		'multi select without options'
	);
	assertEqual(m.defaultDraftValueForField(field('a', 'relation')), '', 'relation');
});

suite.check('defaultDraftValueForField converts a configured default value', () => {
	assertEqual(
		m.defaultDraftValueForField(field('a', 'text', { default_value: 'hello' })),
		'hello',
		'text'
	);
	assertEqual(
		m.defaultDraftValueForField(field('a', 'number', { default_value: 5 })),
		'5',
		'number becomes text'
	);
	assertEqual(
		m.defaultDraftValueForField(field('a', 'number', { default_value: 0 })),
		'0',
		'zero default'
	);
	assertEqual(
		m.defaultDraftValueForField(field('a', 'text', { default_value: '' })),
		'',
		'empty string default'
	);
	assertEqual(
		m.defaultDraftValueForField(field('a', 'boolean', { default_value: true })),
		true,
		'true'
	);
	assertEqual(
		m.defaultDraftValueForField(field('a', 'boolean', { default_value: false })),
		false,
		'false'
	);
	assertEqual(
		m.defaultDraftValueForField(
			field('a', 'relation', { default_value: { record_id: 'r1', title: 'T' } })
		),
		'r1',
		'relation'
	);
	assertDeepEqual(
		m.defaultDraftValueForField(
			field('a', 'multi_select', { options: ['x', 'y'], default_value: ['x', 'y'] })
		),
		['x', 'y'],
		'multi select with options'
	);
	assertEqual(
		m.defaultDraftValueForField(field('a', 'multi_select', { default_value: ['x', 'y'] })),
		'x, y',
		'multi select without options'
	);
	assertEqual(
		m.defaultDraftValueForField(field('a', 'text', { default_value: null })),
		'',
		'null default is ignored'
	);
	assertEqual(
		m.defaultDraftValueForField(field('a', 'boolean', { default_value: null })),
		false,
		'null boolean default'
	);
});

suite.check('defaultDraftForFields builds a draft keyed by field key', () => {
	assertDeepEqual(m.defaultDraftForFields([]), {}, 'no fields');
	assertDeepEqual(
		m.defaultDraftForFields([
			field('name', 'text', { default_value: 'N/A' }),
			field('qty', 'integer'),
			field('done', 'boolean'),
			field('tags', 'multi_select', { options: ['x'] })
		]),
		{ name: 'N/A', qty: '', done: false, tags: [] },
		'mixed fields'
	);
});

suite.check('draftValueForField reads the draft form of a record value', () => {
	assertEqual(m.draftValueForField(field('a', 'text'), record({ a: 'hi' })), 'hi', 'text');
	assertEqual(m.draftValueForField(field('a', 'text'), record({})), '', 'missing value');
	assertEqual(m.draftValueForField(field('a', 'text'), record({ a: null })), '', 'null value');
	assertEqual(
		m.draftValueForField(field('a', 'amount'), record({ a: { decimal: '1.50', currency: 'USD' } })),
		'1.50',
		'amount decimal'
	);
	assertEqual(m.draftValueForField(field('a', 'number'), record({ a: 3 })), '3', 'number');
	assertEqual(m.draftValueForField(field('a', 'boolean'), record({ a: true })), true, 'boolean');
	assertEqual(m.draftValueForField(field('a', 'boolean'), record({})), false, 'missing boolean');
	assertEqual(
		m.draftValueForField(field('a', 'member'), record({ a: { user_id: 'u1', name: 'Ann' } })),
		'u1',
		'member'
	);
	assertDeepEqual(
		m.draftValueForField(
			field('a', 'multi_select', { options: ['x', 'y'] }),
			record({ a: [' x ', 3, 'y'] })
		),
		['x', 'y'],
		'multi select'
	);
	assertEqual(
		m.draftValueForField(
			field('a', 'location'),
			record({ a: { lat: 1.5, lng: 2, label: 'Home' } })
		),
		'1.5,2,Home',
		'location'
	);
	assertEqual(
		m.draftValueForField(field('a', 'text'), record({ b: 'other' })),
		'',
		'other keys are ignored'
	);
});

suite.check('draftArrayFrom returns the string items of an array draft', () => {
	assertDeepEqual(m.draftArrayFrom({ a: ['x', 1, 'y', null] }, 'a'), ['x', 'y'], 'array');
	assertDeepEqual(m.draftArrayFrom({ a: 'x' }, 'a'), [], 'string');
	assertDeepEqual(m.draftArrayFrom({}, 'a'), [], 'missing');
	assertDeepEqual(m.draftArrayFrom({ a: [] }, 'a'), [], 'empty');
});

suite.check('draftStringFrom returns a string draft or empty text', () => {
	assertEqual(m.draftStringFrom({ a: 'x' }, 'a'), 'x', 'string');
	assertEqual(m.draftStringFrom({ a: ' ' }, 'a'), ' ', 'whitespace is kept');
	assertEqual(m.draftStringFrom({ a: 5 }, 'a'), '', 'number');
	assertEqual(m.draftStringFrom({ a: ['x'] }, 'a'), '', 'array');
	assertEqual(m.draftStringFrom({}, 'a'), '', 'missing');
});

suite.check('childRelationKeyForField prefers the relation key', () => {
	assertEqual(
		m.childRelationKeyForField(
			field('lines', 'child_table', { relation: { relation_key: 'order_lines' } })
		),
		'order_lines',
		'relation key'
	);
	assertEqual(m.childRelationKeyForField(field('lines', 'child_table')), 'lines', 'no relation');
	assertEqual(
		m.childRelationKeyForField(field('lines', 'child_table', { relation: { relation_key: '' } })),
		'lines',
		'empty relation key'
	);
	assertEqual(
		m.childRelationKeyForField(field('lines', 'child_table', { relation: { form_key: 'x' } })),
		'lines',
		'relation without key'
	);
});

finish(suite.result());
