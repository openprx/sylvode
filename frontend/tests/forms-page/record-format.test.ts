/**
 * Display, comparison, sort values and filter matching for records (`src/lib/forms/record-format.ts`).
 *
 * Run standalone: `bun tests/forms-page/record-format.test.ts`
 */
import { Suite, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { BusinessEvent, FormField, FormRecord } from '../../src/lib/api/forms';
import type { ParsedViewFilterExpression } from '../../src/lib/forms/page-types';

await installSvelteSsr();

// The shared SSR stub fixes the locale at "zh". `formatDate` branches on the locale, so this suite
// replaces the `svelte-i18n` stub with one whose locale store can be set, before the module under
// test is imported. Translations still return their key, as in the shared stub.
interface PluginBuild {
	module(specifier: string, load: () => { contents: string; loader: 'js' }): void;
}
const bun = (
	globalThis as unknown as {
		Bun: { plugin(o: { name: string; setup(b: PluginBuild): void }): void };
	}
).Bun;
bun.plugin({
	name: 'forms-page-settable-locale',
	setup(build) {
		build.module('svelte-i18n', () => ({
			contents:
				'import { writable } from "svelte/store"; export const t = writable((key) => key); export const locale = writable("zh"); export function init() {}',
			loader: 'js'
		}));
	}
});
const { locale } = await import('svelte-i18n');
const m = await import('../../src/lib/forms/record-format');

const suite = new Suite('forms-page/record-format');

function field(
	key: string,
	type: FormField['type'] = 'text',
	extra: Partial<FormField> = {}
): FormField {
	return { key, type, ...extra };
}

function record(values: Record<string, unknown>, extra: Partial<FormRecord> = {}): FormRecord {
	return {
		id: 'rec-1',
		workspace_id: 'ws',
		project_id: 'pr',
		form_id: 'form',
		title: 'Record',
		values,
		source: {},
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-02T00:00:00Z',
		...extra
	};
}

function event(
	parts: Partial<Pick<BusinessEvent, 'payload' | 'metadata' | 'source'>>
): BusinessEvent {
	return {
		id: 'ev-1',
		workspace_id: 'ws',
		event_type: 'form.record.created',
		aggregate_type: 'form_record',
		aggregate_id: 'rec-1',
		source: {},
		payload: {},
		metadata: {},
		created_at: '2026-01-01T00:00:00Z',
		...parts
	};
}

suite.check('formsReviewText pretty-prints objects and stringifies scalars', () => {
	assertEqual(
		m.formsReviewText({ a: 1, b: [2] }),
		'{\n  "a": 1,\n  "b": [\n    2\n  ]\n}',
		'object'
	);
	assertEqual(m.formsReviewText([1, 2]), '[\n  1,\n  2\n]', 'array');
	assertEqual(m.formsReviewText('text'), 'text', 'string');
	assertEqual(m.formsReviewText(5), '5', 'number');
	assertEqual(m.formsReviewText(false), 'false', 'boolean');
	assertEqual(m.formsReviewText(undefined), '', 'undefined');
	// CURRENT BEHAVIOUR: null has typeof "object", so it is rendered as the text "null" instead of
	// the empty text that undefined produces.
	assertEqual(m.formsReviewText(null), 'null', 'null');
});

suite.check('isRecord accepts plain objects only', () => {
	assertEqual(m.isRecord({}), true, 'object');
	assertEqual(m.isRecord({ a: 1 }), true, 'object with members');
	assertEqual(m.isRecord(null), false, 'null');
	assertEqual(m.isRecord([]), false, 'array');
	assertEqual(m.isRecord('x'), false, 'string');
	assertEqual(m.isRecord(1), false, 'number');
	assertEqual(m.isRecord(undefined), false, 'undefined');
});

suite.check('displayValue formats scalars', () => {
	assertEqual(m.displayValue(null), '-', 'null');
	assertEqual(m.displayValue(undefined), '-', 'undefined');
	assertEqual(m.displayValue(''), '-', 'empty string');
	assertEqual(m.displayValue('hi'), 'hi', 'string');
	assertEqual(m.displayValue(' '), ' ', 'blank string is kept');
	assertEqual(m.displayValue(0), '0', 'zero');
	assertEqual(m.displayValue(12.5), '12.5', 'number');
	assertEqual(m.displayValue(true), 'forms.yes', 'true');
	assertEqual(m.displayValue(false), 'forms.no', 'false');
});

suite.check('displayValue formats arrays and objects', () => {
	assertEqual(m.displayValue([1, null, 'x', true]), '1, -, x, forms.yes', 'array');
	assertEqual(m.displayValue([]), '', 'empty array');
	assertEqual(
		m.displayValue([{ name: 'Ann' }, { email: 'b@x.io' }]),
		'Ann, b@x.io',
		'array of objects'
	);
	assertEqual(
		m.displayValue({ display: 'D', name: 'N', email: 'E', title: 'T' }),
		'D',
		'display first'
	);
	assertEqual(m.displayValue({ name: 'N', email: 'E', title: 'T' }), 'N', 'name second');
	assertEqual(m.displayValue({ email: 'E', title: 'T' }), 'E', 'email third');
	assertEqual(m.displayValue({ title: 'T', decimal: '1' }), 'T', 'title fourth');
	assertEqual(m.displayValue({ decimal: '12.50' }), '12.50', 'decimal');
	assertEqual(
		m.displayValue({ decimal: '12.50', currency: 'USD' }),
		'12.50 USD',
		'decimal with currency'
	);
	assertEqual(m.displayValue({ decimal: '12.50', currency: 5 }), '12.50', 'non-string currency');
	assertEqual(m.displayValue({ value: 3 }), '3', 'numeric value');
	assertEqual(m.displayValue({ value: '3' }), '{"value":"3"}', 'string value is not special');
	assertEqual(
		m.displayValue({ name: 5, other: 1 }),
		'{"name":5,"other":1}',
		'non-string name falls through'
	);
	assertEqual(m.displayValue({}), '{}', 'empty object');
	assertEqual(m.displayValue(10n), '10', 'bigint');
});

suite.check('recordDisplay and recordComparableText read one field of a record', () => {
	const r = record({ a: 'Hello', b: '', c: true });
	assertEqual(m.recordDisplay(r, field('a')), 'Hello', 'value');
	assertEqual(m.recordDisplay(r, field('b')), '-', 'empty');
	assertEqual(m.recordDisplay(r, field('missing')), '-', 'missing');
	assertEqual(m.recordDisplay(r, null), '-', 'no field');
	assertEqual(m.recordComparableText(r, field('a')), 'Hello', 'comparable value');
	assertEqual(m.recordComparableText(r, field('b')), '', 'empty becomes empty text');
	assertEqual(m.recordComparableText(r, field('missing')), '', 'missing becomes empty text');
	assertEqual(m.recordComparableText(r, field('c')), 'forms.yes', 'boolean');
});

suite.check('cardFieldHasDisplayValue ignores empty and dash values', () => {
	const r = record({ a: 'x', b: '   ', c: '', d: '-', e: 0, f: false });
	assertEqual(m.cardFieldHasDisplayValue(r, field('a')), true, 'text');
	assertEqual(m.cardFieldHasDisplayValue(r, field('b')), false, 'blank text');
	assertEqual(m.cardFieldHasDisplayValue(r, field('c')), false, 'empty text');
	assertEqual(m.cardFieldHasDisplayValue(r, field('d')), false, 'literal dash');
	assertEqual(m.cardFieldHasDisplayValue(r, field('missing')), false, 'missing');
	assertEqual(m.cardFieldHasDisplayValue(r, field('e')), true, 'zero');
	assertEqual(m.cardFieldHasDisplayValue(r, field('f')), true, 'false has a label');
});

suite.check('recordMatchesViewFilter applies each operator case-insensitively', () => {
	const r = record({ name: 'Beef Noodles', empty: '' });
	const name = field('name');
	const empty = field('empty');
	const run = (
		operator: 'equals' | 'not_equals' | 'contains' | 'not_empty',
		value: string,
		f = name
	): boolean => m.recordMatchesViewFilter(r, { field: f, operator, value });
	assertEqual(run('equals', 'beef noodles'), true, 'equals hit');
	assertEqual(run('equals', ' BEEF NOODLES '), true, 'equals trimmed');
	assertEqual(run('equals', 'beef'), false, 'equals miss');
	assertEqual(run('not_equals', 'beef'), true, 'not_equals hit');
	assertEqual(run('not_equals', 'beef noodles'), false, 'not_equals miss');
	assertEqual(run('contains', 'NOOD'), true, 'contains hit');
	assertEqual(run('contains', 'rice'), false, 'contains miss');
	assertEqual(run('not_empty', ''), true, 'not_empty on a value');
	assertEqual(run('not_empty', 'ignored', empty), false, 'not_empty on an empty value');
	assertEqual(run('not_empty', 'ignored', field('missing')), false, 'not_empty on a missing value');
	for (const operator of ['equals', 'not_equals', 'contains'] as const) {
		assertEqual(
			run(operator, '   '),
			true,
			`${operator} with a blank expectation matches everything`
		);
	}
	assertEqual(run('equals', 'x', empty), false, 'equals against an empty value');
	assertEqual(run('contains', 'x', empty), false, 'contains against an empty value');
});

suite.check('recordMatchesViewFilterExpression evaluates conditions and groups', () => {
	const r = record({ a: 'x', b: 'y' });
	const hit: ParsedViewFilterExpression = {
		kind: 'condition',
		filter: { field: field('a'), operator: 'equals', value: 'x' }
	};
	const miss: ParsedViewFilterExpression = {
		kind: 'condition',
		filter: { field: field('b'), operator: 'equals', value: 'z' }
	};
	const off: ParsedViewFilterExpression = {
		kind: 'condition',
		disabled: true,
		filter: { field: field('b'), operator: 'equals', value: 'z' }
	};
	const group = (
		logic: 'all' | 'any',
		children: ParsedViewFilterExpression[],
		disabled?: boolean
	): ParsedViewFilterExpression => ({
		kind: 'group',
		logic,
		children,
		disabled
	});
	assertEqual(m.recordMatchesViewFilterExpression(r, hit), true, 'condition hit');
	assertEqual(m.recordMatchesViewFilterExpression(r, miss), false, 'condition miss');
	assertEqual(m.recordMatchesViewFilterExpression(r, off), null, 'disabled condition');
	assertEqual(m.recordMatchesViewFilterExpression(r, group('all', [hit, hit])), true, 'all hit');
	assertEqual(m.recordMatchesViewFilterExpression(r, group('all', [hit, miss])), false, 'all miss');
	assertEqual(m.recordMatchesViewFilterExpression(r, group('any', [hit, miss])), true, 'any hit');
	assertEqual(
		m.recordMatchesViewFilterExpression(r, group('any', [miss, miss])),
		false,
		'any miss'
	);
	assertEqual(
		m.recordMatchesViewFilterExpression(r, group('all', [hit, off])),
		true,
		'disabled children are ignored'
	);
	assertEqual(
		m.recordMatchesViewFilterExpression(r, group('any', [miss, off])),
		false,
		'disabled child does not rescue an any group'
	);
	assertEqual(
		m.recordMatchesViewFilterExpression(r, group('all', [off, off])),
		null,
		'only disabled children'
	);
	assertEqual(m.recordMatchesViewFilterExpression(r, group('all', [])), null, 'empty group');
	assertEqual(
		m.recordMatchesViewFilterExpression(r, group('all', [hit], true)),
		null,
		'disabled group'
	);
	assertEqual(
		m.recordMatchesViewFilterExpression(r, group('all', [group('any', [miss, hit]), hit])),
		true,
		'nested'
	);
	assertEqual(
		m.recordMatchesViewFilterExpression(r, group('all', [group('any', [miss, miss]), hit])),
		false,
		'nested miss'
	);
	assertEqual(
		m.recordMatchesViewFilterExpression(r, group('all', [group('all', []), hit])),
		true,
		'null nested group is ignored'
	);
});

suite.check('recordSortValue returns numbers for numeric fields', () => {
	for (const type of ['amount', 'number', 'integer'] as const) {
		const f = field('n', type);
		assertEqual(m.recordSortValue(record({ n: 12 }), f), 12, `${type} number`);
		assertEqual(m.recordSortValue(record({ n: '12.5' }), f), 12.5, `${type} numeric text`);
		assertEqual(m.recordSortValue(record({ n: 'abc' }), f), 'abc', `${type} non numeric text`);
		assertEqual(
			m.recordSortValue(record({ n: { decimal: '7.25', currency: 'USD' } }), f),
			7.25,
			`${type} decimal object`
		);
		assertEqual(
			m.recordSortValue(record({ n: { decimal: 'x' } }), f),
			'x',
			`${type} non numeric decimal`
		);
		assertEqual(m.recordSortValue(record({ n: { value: 3 } }), f), 3, `${type} value number`);
		assertEqual(m.recordSortValue(record({ n: { value: '4' } }), f), 4, `${type} value text`);
	}
	const f = field('n', 'number');
	assertEqual(
		m.recordSortValue(record({ n: { decimal: '1', value: 9 } }), f),
		1,
		'decimal wins over value'
	);
	assertEqual(m.recordSortValue(record({}), f), '', 'missing value sorts as empty text');
	assertEqual(
		m.recordSortValue(record({ n: { other: 1 } }), f),
		'{"other":1}',
		'unknown object sorts by its JSON text'
	);
	// CURRENT BEHAVIOUR: an empty string is a number to Number(), so it sorts as 0 while a missing
	// value sorts as the empty text.
	assertEqual(m.recordSortValue(record({ n: '' }), f), 0, 'empty text sorts as zero');
});

suite.check('recordSortValue returns timestamps for date fields', () => {
	const date = field('d', 'date');
	const datetime = field('d', 'datetime');
	assertEqual(
		m.recordSortValue(record({ d: '2026-01-15T10:00:00Z' }), datetime),
		Date.UTC(2026, 0, 15, 10),
		'datetime'
	);
	assertEqual(
		m.recordSortValue(record({ d: '2026-01-15' }), date),
		Date.UTC(2026, 0, 15),
		'date-only text is UTC'
	);
	assertEqual(
		m.recordSortValue(record({ d: { value: '2026-02-01T00:00:00Z' } }), date),
		Date.UTC(2026, 1, 1),
		'object value'
	);
	assertEqual(m.recordSortValue(record({ d: 'garbage' }), date), 0, 'unparseable date');
	assertEqual(
		m.recordSortValue(record({}, { updated_at: '2026-03-01T00:00:00Z' }), date),
		Date.UTC(2026, 2, 1),
		'missing value uses updated_at'
	);
});

suite.check('recordSortValue returns lower-case text for other types', () => {
	assertEqual(m.recordSortValue(record({ t: 'Hello World' }), field('t')), 'hello world', 'text');
	assertEqual(m.recordSortValue(record({ t: 'Hello' }), { key: 't' }), 'hello', 'type absent');
	assertEqual(
		m.recordSortValue(record({ t: 'High' }), field('t', 'single_select')),
		'high',
		'single select'
	);
	assertEqual(
		m.recordSortValue(record({ t: ['B', 'A'] }), field('t', 'multi_select')),
		'b, a',
		'multi select'
	);
	assertEqual(
		m.recordSortValue(record({ t: 5 }), field('t', 'rating')),
		'5',
		'rating is compared as text'
	);
	assertEqual(m.recordSortValue(record({}), field('t')), '', 'missing');
});

suite.check('compareRecordField orders numbers numerically and text by locale', () => {
	const num = field('n', 'number');
	const text = field('t');
	const two = record({ n: 2, t: 'apple' });
	const ten = record({ n: 10, t: 'banana' });
	assertEqual(m.compareRecordField(two, ten, num, 'asc'), -8, 'numeric ascending');
	assertEqual(m.compareRecordField(two, ten, num, 'desc'), 8, 'numeric descending');
	assertEqual(m.compareRecordField(two, two, num, 'asc'), 0, 'equal numbers');
	assertEqual(m.compareRecordField(two, ten, text, 'asc') < 0, true, 'text ascending');
	assertEqual(m.compareRecordField(two, ten, text, 'desc') > 0, true, 'text descending');
	assertEqual(m.compareRecordField(two, two, text, 'asc'), 0, 'equal text');
	assertEqual(
		m.compareRecordField(record({ n: 'abc' }), record({ n: 3 }), num, 'asc') > 0,
		true,
		'text against number compares as text'
	);
	assertDeepEqual(
		[ten, two]
			.sort((left, right) => m.compareRecordField(left, right, num, 'asc'))
			.map((r) => r.values.n),
		[2, 10],
		'sorting 2 and 10 numerically, not as text'
	);
});

suite.check('recordNumericValue converts values to finite numbers', () => {
	const f = field('n', 'number');
	assertEqual(m.recordNumericValue(record({ n: 5 }), null), 1, 'no field counts each record once');
	assertEqual(m.recordNumericValue(record({ n: 5 }), f), 5, 'number');
	assertEqual(m.recordNumericValue(record({ n: -2.5 }), f), -2.5, 'negative');
	assertEqual(m.recordNumericValue(record({ n: Number.POSITIVE_INFINITY }), f), 0, 'infinity');
	assertEqual(m.recordNumericValue(record({ n: Number.NaN }), f), 0, 'NaN');
	assertEqual(m.recordNumericValue(record({ n: '3.5' }), f), 3.5, 'numeric text');
	assertEqual(m.recordNumericValue(record({ n: 'abc' }), f), 0, 'text');
	assertEqual(m.recordNumericValue(record({ n: '' }), f), 0, 'empty text');
	assertEqual(m.recordNumericValue(record({ n: 'Infinity' }), f), 0, 'infinite text');
	assertEqual(m.recordNumericValue(record({ n: { decimal: '7.5' } }), f), 7.5, 'decimal');
	assertEqual(
		m.recordNumericValue(record({ n: { decimal: 'x', value: 4 } }), f),
		0,
		'bad decimal does not fall back to value'
	);
	assertEqual(m.recordNumericValue(record({ n: { value: 4 } }), f), 4, 'value number');
	assertEqual(m.recordNumericValue(record({ n: { value: '4.5' } }), f), 4.5, 'value text');
	assertEqual(m.recordNumericValue(record({ n: { value: Number.NaN } }), f), 0, 'value NaN');
	assertEqual(m.recordNumericValue(record({ n: { other: 1 } }), f), 0, 'unknown object');
	assertEqual(m.recordNumericValue(record({ n: true }), f), 0, 'boolean');
	assertEqual(m.recordNumericValue(record({}), f), 0, 'missing');
});

suite.check('recordAmountCurrency reads the configured or stored currency', () => {
	const amount = field('p', 'amount', { amount: { currency: 'EUR' } });
	const plain = field('p', 'amount');
	assertEqual(
		m.recordAmountCurrency(record({ p: { currency: 'USD' } }), amount),
		'EUR',
		'configured wins'
	);
	assertEqual(
		m.recordAmountCurrency(record({ p: { decimal: '1', currency: 'USD' } }), plain),
		'USD',
		'stored currency'
	);
	assertEqual(
		m.recordAmountCurrency(record({ p: { currency: 5 } }), plain),
		'',
		'non-string currency'
	);
	assertEqual(m.recordAmountCurrency(record({ p: '12' }), plain), '', 'scalar value');
	assertEqual(m.recordAmountCurrency(record({}), plain), '', 'missing value');
	assertEqual(m.recordAmountCurrency(record({ p: { currency: 'USD' } }), null), '', 'no field');
	assertEqual(
		m.recordAmountCurrency(
			record({ p: { currency: 'USD' } }),
			field('p', 'number', { amount: { currency: 'EUR' } })
		),
		'',
		'not an amount field'
	);
	assertEqual(
		m.recordAmountCurrency(record({ p: { currency: 'USD' } }), { key: 'p' }),
		'',
		'type absent'
	);
});

suite.check('eventValue searches payload, metadata and source in that order', () => {
	assertEqual(m.eventValue(event({ payload: { a: '  one  ' } }), ['a']), 'one', 'trimmed string');
	assertEqual(m.eventValue(event({ payload: { a: 3 } }), ['a']), '3', 'number');
	assertEqual(m.eventValue(event({ payload: { a: 0 } }), ['a']), '0', 'zero');
	assertEqual(m.eventValue(event({ payload: { a: false } }), ['a']), 'false', 'boolean');
	assertEqual(
		m.eventValue(event({ payload: { a: '   ' }, metadata: { a: 'meta' } }), ['a']),
		'meta',
		'blank string is skipped'
	);
	assertEqual(
		m.eventValue(event({ payload: { b: 'p' }, metadata: { a: 'm' } }), ['a', 'b']),
		'p',
		'source order beats key order'
	);
	assertEqual(
		m.eventValue(event({ metadata: { a: 'm' }, source: { a: 's' } }), ['a']),
		'm',
		'metadata before source'
	);
	assertEqual(m.eventValue(event({ source: { b: 's' } }), ['a', 'b']), 's', 'source');
	assertEqual(
		m.eventValue(event({ payload: { a: 'x', b: 'y' } }), ['b', 'a']),
		'y',
		'key order within a source'
	);
	assertEqual(
		m.eventValue(event({ payload: { a: { nested: 1 }, b: null } }), ['a', 'b']),
		'',
		'objects and null are ignored'
	);
	assertEqual(m.eventValue(event({ payload: { a: 'x' } }), []), '', 'no keys');
	assertEqual(m.eventValue(event({}), ['a']), '', 'nothing found');
});

suite.check('formatDate formats the local time for the active locale', () => {
	// Inputs carry no zone designator, so they are parsed as local time and the expected text is the
	// same in every timezone.
	locale.set('zh');
	assertEqual(m.formatDate('2026-01-15T10:30:00'), '2026/01/15 10:30', 'zh format');
	assertEqual(m.formatDate('2026-12-05T09:05:00'), '2026/12/05 09:05', 'zh zero padding');
	assertEqual(m.formatDate('2026-01-15T23:59:00'), '2026/01/15 23:59', 'zh 24 hour clock');
	locale.set('en');
	assertEqual(m.formatDate('2026-01-15T10:30:00'), '01/15/2026, 10:30 AM', 'en format');
	assertEqual(m.formatDate('2026-12-05T21:05:00'), '12/05/2026, 09:05 PM', 'en afternoon');
	locale.set('fr');
	assertEqual(
		m.formatDate('2026-01-15T10:30:00'),
		'2026/01/15 10:30',
		'other locales use the zh format'
	);
	assertEqual(m.formatDate('not a date'), 'Invalid Date', 'invalid input');
	locale.set('zh');
});

suite.check('inputValue and selectValue read the control that raised the event', () => {
	class FakeInput {
		constructor(readonly value: string) {}
	}
	const domWindow = globalThis as unknown as Record<string, unknown>;
	const saved = {
		input: domWindow.HTMLInputElement,
		textarea: domWindow.HTMLTextAreaElement,
		select: domWindow.HTMLSelectElement
	};
	class FakeTextarea {
		constructor(readonly value: string) {}
	}
	class FakeSelect {
		constructor(readonly value: string) {}
	}
	domWindow.HTMLInputElement = FakeInput;
	domWindow.HTMLTextAreaElement = FakeTextarea;
	domWindow.HTMLSelectElement = FakeSelect;
	try {
		// Deliberately minimal event objects: only `currentTarget` is read.
		const eventFor = (target: unknown): Event => ({ currentTarget: target }) as unknown as Event;
		assertEqual(m.inputValue(eventFor(new FakeInput('typed'))), 'typed', 'input');
		assertEqual(m.inputValue(eventFor(new FakeTextarea('long text'))), 'long text', 'textarea');
		assertEqual(m.inputValue(eventFor(new FakeSelect('x'))), '', 'select is not an input');
		assertEqual(m.inputValue(eventFor(null)), '', 'no target');
		assertEqual(
			m.inputValue(eventFor({ value: 'duck' })),
			'',
			'object that only looks like an input'
		);
		assertEqual(m.selectValue(eventFor(new FakeSelect('option-2'))), 'option-2', 'select');
		assertEqual(m.selectValue(eventFor(new FakeInput('x'))), '', 'input is not a select');
		assertEqual(m.selectValue(eventFor(null)), '', 'no target');
	} finally {
		domWindow.HTMLInputElement = saved.input;
		domWindow.HTMLTextAreaElement = saved.textarea;
		domWindow.HTMLSelectElement = saved.select;
	}
});

finish(suite.result());
