/**
 * Record grouping and date sources (`src/lib/forms/record-grouping.ts`).
 *
 * Run standalone: `bun tests/forms-page/record-grouping.test.ts`
 */
import { Suite, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormField, FormRecord } from '../../src/lib/api/forms';

await installSvelteSsr();
const m = await import('../../src/lib/forms/record-grouping');

const suite = new Suite('forms-page/record-grouping');

function field(key: string, extra: Partial<FormField> = {}): FormField {
	return { key, type: 'text', ...extra };
}

function record(
	id: string,
	values: Record<string, unknown>,
	extra: Partial<FormRecord> = {}
): FormRecord {
	return {
		id,
		workspace_id: 'ws',
		project_id: 'pr',
		form_id: 'form',
		title: id,
		values,
		source: {},
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-02T00:00:00Z',
		...extra
	};
}

suite.check('recordGroupLabel shows the value or the unspecified key', () => {
	const status = field('status');
	assertEqual(m.recordGroupLabel(record('r', { status: 'Open' }), status), 'Open', 'value');
	assertEqual(m.recordGroupLabel(record('r', { status: 3 }), status), '3', 'number');
	assertEqual(m.recordGroupLabel(record('r', { status: true }), status), 'forms.yes', 'boolean');
	assertEqual(m.recordGroupLabel(record('r', {}), status), 'forms.unspecified', 'missing');
	assertEqual(
		m.recordGroupLabel(record('r', { status: '' }), status),
		'forms.unspecified',
		'empty string'
	);
	assertEqual(
		m.recordGroupLabel(record('r', { status: null }), status),
		'forms.unspecified',
		'null'
	);
	assertEqual(
		m.recordGroupLabel(record('r', { status: 'Open' }), null),
		'forms.unspecified',
		'no field'
	);
});

suite.check('recordDateSource picks the first usable date for the field', () => {
	const due = field('due', { type: 'date' });
	const base = { updated_at: '2026-02-02T00:00:00Z', created_at: '2026-01-01T00:00:00Z' };
	assertEqual(
		m.recordDateSource(record('r', { due: '2026-03-04' }, base), due),
		'2026-03-04',
		'string'
	);
	assertEqual(
		m.recordDateSource(record('r', { due: { value: '2026-03-05' } }, base), due),
		'2026-03-05',
		'value member'
	);
	assertEqual(
		m.recordDateSource(record('r', { due: { date: '2026-03-06' } }, base), due),
		'2026-03-06',
		'date member'
	);
	assertEqual(
		m.recordDateSource(record('r', { due: { datetime: '2026-03-07T10:00:00' } }, base), due),
		'2026-03-07T10:00:00',
		'datetime member'
	);
	assertEqual(
		m.recordDateSource(
			record('r', { due: { value: ' ', date: '2026-03-08', datetime: 'x' } }, base),
			due
		),
		'2026-03-08',
		'blank value is skipped, order is value, date, datetime'
	);
	assertEqual(
		m.recordDateSource(record('r', { due: '   ' }, base), due),
		'2026-02-02T00:00:00Z',
		'blank string falls back to updated_at'
	);
	assertEqual(
		m.recordDateSource(record('r', {}, base), due),
		'2026-02-02T00:00:00Z',
		'missing value'
	);
	assertEqual(
		m.recordDateSource(record('r', { due: 5 }, base), due),
		'2026-02-02T00:00:00Z',
		'number'
	);
	assertEqual(
		m.recordDateSource(record('r', { due: { other: 'x' } }, base), due),
		'2026-02-02T00:00:00Z',
		'unknown object'
	);
	assertEqual(
		m.recordDateSource(record('r', { due: '2026-03-04' }, base), null),
		'2026-02-02T00:00:00Z',
		'no field'
	);
	assertEqual(
		m.recordDateSource(record('r', {}, { ...base, updated_at: '' }), due),
		'2026-01-01T00:00:00Z',
		'created_at when updated_at is empty'
	);
});

suite.check('groupRecordsByField groups by display value and sorts by label', () => {
	const status = field('status');
	const groups = m.groupRecordsByField(
		[
			record('r1', { status: 'open' }),
			record('r2', { status: 'Done' }),
			record('r3', { status: 'OPEN' }),
			record('r4', {}),
			record('r5', { status: 'Blocked' })
		],
		status
	);
	assertDeepEqual(
		groups.map((g) => [g.key, g.label, g.records.map((r) => r.id)]),
		[
			['blocked', 'Blocked', ['r5']],
			['done', 'Done', ['r2']],
			['forms.unspecified', 'forms.unspecified', ['r4']],
			['open', 'open', ['r1', 'r3']]
		],
		'case-insensitive keys, first label wins, record order kept'
	);
});

suite.check('groupRecordsByField seeds one group per single select option', () => {
	const status = field('status', { type: 'single_select', options: ['Todo', ' Done ', '', '  '] });
	const groups = m.groupRecordsByField(
		[record('r1', { status: 'todo' }), record('r2', { status: 'Other' }), record('r3', {})],
		status
	);
	assertDeepEqual(
		groups.map((g) => [g.key, g.label, g.records.map((r) => r.id)]),
		[
			['done', 'Done', []],
			['forms.unspecified', 'forms.unspecified', ['r3']],
			['other', 'Other', ['r2']],
			['todo', 'Todo', ['r1']]
		],
		'empty option groups are kept, option labels are trimmed'
	);
	assertDeepEqual(
		m.groupRecordsByField([], status).map((g) => g.label),
		['Done', 'Todo'],
		'options without records'
	);
});

suite.check(
	'groupRecordsByField does not seed groups for other field types or a missing field',
	() => {
		assertDeepEqual(
			m.groupRecordsByField([], field('tags', { type: 'multi_select', options: ['a'] })),
			[],
			'multi select is not seeded'
		);
		assertDeepEqual(m.groupRecordsByField([], null), [], 'no records');
		assertDeepEqual(
			m
				.groupRecordsByField([record('r1', { a: 1 }), record('r2', { a: 2 })], null)
				.map((g) => [g.label, g.records.length]),
			[['forms.unspecified', 2]],
			'no field puts everything in one group'
		);
		assertDeepEqual(
			m
				.groupRecordsByField(
					[record('r1', { flag: true }), record('r2', { flag: false })],
					field('flag', { type: 'boolean' })
				)
				.map((g) => g.label),
			['forms.no', 'forms.yes'],
			'boolean labels'
		);
	}
);

suite.check('recordsByUpdatedAt orders newest first without mutating the input', () => {
	const a = record('a', {}, { updated_at: '2026-01-01T00:00:00Z' });
	const b = record('b', {}, { updated_at: '2026-03-01T00:00:00Z' });
	const c = record('c', {}, { updated_at: '2026-02-01T00:00:00Z' });
	const input = [a, b, c];
	assertDeepEqual(
		m.recordsByUpdatedAt(input).map((r) => r.id),
		['b', 'c', 'a'],
		'order'
	);
	assertDeepEqual(
		input.map((r) => r.id),
		['a', 'b', 'c'],
		'input unchanged'
	);
	assertDeepEqual(m.recordsByUpdatedAt([]), [], 'empty');
	assertDeepEqual(
		m
			.recordsByUpdatedAt([
				record('x', {}, { updated_at: '2026-01-01T00:00:00Z' }),
				record('y', {}, { updated_at: '2026-01-01T00:00:00Z' })
			])
			.map((r) => r.id),
		['x', 'y'],
		'equal timestamps keep their order'
	);
});

finish(suite.result());
