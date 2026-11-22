/**
 * Field condition parsing, text round trips and matching (`src/lib/forms/field-conditions.ts`).
 *
 * Run standalone: `bun tests/forms-page/field-conditions.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormField, FormRecord } from '../../src/lib/api/forms';

await installSvelteSsr();
const m = await import('../../src/lib/forms/field-conditions');

const suite = new Suite('forms-page/field-conditions');

function field(key: string, extra: Partial<FormField> = {}): FormField {
	return { key, type: 'text', ...extra };
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

const hiddenWhenOpen = field('note', {
	conditional: { hidden_when: { field: 'status', equals: 'open' } }
});
const readOnlyWhenDone = field('note', {
	conditional: { read_only_when: { field: 'status', operator: 'equals', value: 'done' } }
});

suite.check('normalizeFieldConditionOperator maps every alias', () => {
	const table: Array<[string, string | null]> = [
		['equals', 'equals'],
		['eq', 'equals'],
		['==', 'equals'],
		['=', 'equals'],
		['not_equals', 'not_equals'],
		['neq', 'not_equals'],
		['!=', 'not_equals'],
		['<>', 'not_equals'],
		['contains', 'contains'],
		['not_contains', 'not_contains'],
		['not-contains', 'not_contains'],
		['is_empty', 'is_empty'],
		['empty', 'is_empty'],
		['not_empty', 'not_empty'],
		['exists', 'not_empty'],
		['gt', 'gt'],
		['>', 'gt'],
		['gte', 'gte'],
		['>=', 'gte'],
		['lt', 'lt'],
		['<', 'lt'],
		['lte', 'lte'],
		['<=', 'lte'],
		['  GT  ', 'gt'],
		['Not_Contains', 'not_contains'],
		['', null],
		['like', null],
		['=>', null]
	];
	for (const [input, expected] of table) {
		assertEqual(
			m.normalizeFieldConditionOperator(input),
			expected,
			`operator ${JSON.stringify(input)}`
		);
	}
});

suite.check('fieldConditionOperatorNeedsValue is false only for the emptiness operators', () => {
	assertEqual(m.fieldConditionOperatorNeedsValue('is_empty'), false, 'is_empty');
	assertEqual(m.fieldConditionOperatorNeedsValue('not_empty'), false, 'not_empty');
	for (const op of [
		'equals',
		'not_equals',
		'contains',
		'not_contains',
		'gt',
		'gte',
		'lt',
		'lte'
	] as const) {
		assertEqual(m.fieldConditionOperatorNeedsValue(op), true, op);
	}
});

suite.check('parseFieldConditionLine parses the not-equals form', () => {
	assertDeepEqual(
		m.parseFieldConditionLine('status!=done'),
		{ field: 'status', operator: 'not_equals', value: 'done' },
		'compact'
	);
	assertDeepEqual(
		m.parseFieldConditionLine('  status  !=  in progress '),
		{ field: 'status', operator: 'not_equals', value: 'in progress' },
		'spaces'
	);
	assertDeepEqual(
		m.parseFieldConditionLine('a!=b=c'),
		{ field: 'a', operator: 'not_equals', value: 'b=c' },
		'value keeps a later equals sign'
	);
	assertEqual(m.parseFieldConditionLine('status!='), null, 'missing value');
	assertEqual(m.parseFieldConditionLine('!=done'), null, 'missing field');
});

suite.check('parseFieldConditionLine parses operator words and symbols', () => {
	assertDeepEqual(
		m.parseFieldConditionLine('qty gt 5'),
		{ field: 'qty', operator: 'gt', value: '5' },
		'gt'
	);
	assertDeepEqual(
		m.parseFieldConditionLine('qty >= 5'),
		{ field: 'qty', operator: 'gte', value: '5' },
		'>='
	);
	assertDeepEqual(
		m.parseFieldConditionLine('name contains hello   world'),
		{ field: 'name', operator: 'contains', value: 'hello world' },
		'multi word value is re-joined with single spaces'
	);
	assertDeepEqual(
		m.parseFieldConditionLine('status = open'),
		{ field: 'status', operator: 'equals', value: 'open' },
		'spaced equals'
	);
	assertDeepEqual(
		m.parseFieldConditionLine('a eq 1'),
		{ field: 'a', operator: 'equals', value: '1' },
		'eq'
	);
});

suite.check('parseFieldConditionLine omits the value for emptiness operators', () => {
	assertDeepEqual(
		m.parseFieldConditionLine('name empty'),
		{ field: 'name', operator: 'is_empty' },
		'empty'
	);
	assertDeepEqual(
		m.parseFieldConditionLine('name exists'),
		{ field: 'name', operator: 'not_empty' },
		'exists'
	);
	assertDeepEqual(
		m.parseFieldConditionLine('name is_empty ignored words'),
		{ field: 'name', operator: 'is_empty' },
		'extra words are dropped'
	);
});

suite.check('parseFieldConditionLine parses the compact equals form', () => {
	assertDeepEqual(
		m.parseFieldConditionLine('status=open'),
		{ field: 'status', equals: 'open' },
		'compact'
	);
	assertDeepEqual(
		m.parseFieldConditionLine(' status = open=1'),
		{ field: 'status', operator: 'equals', value: 'open=1' },
		'spaced form goes through the operator branch'
	);
	assertDeepEqual(
		m.parseFieldConditionLine('a=b=c'),
		{ field: 'a', equals: 'b=c' },
		'only the first equals sign splits'
	);
});

suite.check('parseFieldConditionLine rejects malformed lines', () => {
	for (const line of ['', '   ', 'status', 'qty gt', 'status=', '=open', 'name contains', ' = ']) {
		assertEqual(m.parseFieldConditionLine(line), null, `line ${JSON.stringify(line)}`);
	}
});

suite.check('parseFieldConditionText handles empty and prefix-only input', () => {
	assertEqual(m.parseFieldConditionText(''), null, 'empty');
	assertEqual(m.parseFieldConditionText('\n  \n'), null, 'blank lines');
	assertEqual(m.parseFieldConditionText('all:'), null, 'prefix only');
	assertEqual(m.parseFieldConditionText('any:\nbogus line'), null, 'only invalid lines');
});

suite.check('parseFieldConditionText returns a single condition without a prefix', () => {
	assertDeepEqual(
		m.parseFieldConditionText('status=open'),
		{ field: 'status', equals: 'open' },
		'single'
	);
});

suite.check('parseFieldConditionText builds groups', () => {
	assertDeepEqual(
		m.parseFieldConditionText('status=open\nqty gt 3'),
		{
			logic: 'all',
			conditions: [
				{ field: 'status', equals: 'open' },
				{ field: 'qty', operator: 'gt', value: '3' }
			]
		},
		'two lines default to all'
	);
	assertDeepEqual(
		m.parseFieldConditionText('ANY:\nstatus=open\n\n  qty gt 3  '),
		{
			logic: 'any',
			conditions: [
				{ field: 'status', equals: 'open' },
				{ field: 'qty', operator: 'gt', value: '3' }
			]
		},
		'prefix is case insensitive, blank lines and padding are ignored'
	);
	assertDeepEqual(
		m.parseFieldConditionText('all:\nstatus=open'),
		{ logic: 'all', conditions: [{ field: 'status', equals: 'open' }] },
		'a prefix keeps a single condition inside a group'
	);
	assertDeepEqual(
		m.parseFieldConditionText('any:\nbogus\nstatus=open'),
		{ logic: 'any', conditions: [{ field: 'status', equals: 'open' }] },
		'invalid lines are dropped'
	);
});

suite.check('fieldConditionToText returns empty text for non-objects', () => {
	for (const value of [null, undefined, 'status=open', 5, ['a'], true]) {
		assertEqual(m.fieldConditionToText(value), '', `value ${JSON.stringify(value)}`);
	}
});

suite.check('fieldConditionToText writes single conditions', () => {
	assertEqual(m.fieldConditionToText({ field: 'a', equals: 'x' }), 'a=x', 'equals');
	assertEqual(
		m.fieldConditionToText({ field: ' a ', value: ' x ' }),
		'a=x',
		'trimmed, default operator'
	);
	assertEqual(
		m.fieldConditionToText({ field: 'a', operator: '!=', value: 'b' }),
		'a!=b',
		'not equals alias'
	);
	assertEqual(
		m.fieldConditionToText({ field: 'a', operator: 'contains', value: 'x' }),
		'a contains x',
		'contains'
	);
	assertEqual(
		m.fieldConditionToText({ field: 'a', operator: '>', value: '3' }),
		'a gt 3',
		'gt alias'
	);
	assertEqual(m.fieldConditionToText({ field: 'a', operator: 'empty' }), 'a is_empty', 'emptiness');
	assertEqual(
		m.fieldConditionToText({ field: 'a', operator: 'not_empty', value: 'ignored' }),
		'a not_empty',
		'value dropped for emptiness operators'
	);
	assertEqual(
		m.fieldConditionToText({ field: 'a', operator: 'unknown', value: 'x' }),
		'a=x',
		'unknown operator falls back to equals'
	);
	assertEqual(m.fieldConditionToText({ field: 'a' }), '', 'missing value');
	assertEqual(m.fieldConditionToText({ equals: 'x' }), '', 'missing field');
	assertEqual(m.fieldConditionToText({ field: 'a', value: 5 }), '', 'non-string value is ignored');
	assertEqual(
		m.fieldConditionToText({ conditions: 'nope', field: 'a', equals: 'x' }),
		'a=x',
		'non-array conditions use the single form'
	);
});

suite.check('fieldConditionToText writes groups', () => {
	assertEqual(
		m.fieldConditionToText({
			logic: 'any',
			conditions: [
				{ field: 'a', equals: 'x' },
				{ field: 'b', operator: 'gt', value: '3' },
				{ field: 'c', operator: 'is_empty' },
				{ field: '', equals: 'dropped' },
				{ field: 'd' },
				'not an object'
			]
		}),
		'any:\na=x\nb gt 3\nc is_empty',
		'any group, invalid entries dropped'
	);
	assertEqual(
		m.fieldConditionToText({ logic: 'whatever', conditions: [{ field: 'a', equals: 'x' }] }),
		'all:\na=x',
		'unknown logic is all'
	);
	assertEqual(m.fieldConditionToText({ logic: 'all', conditions: [] }), '', 'empty group');
	assertEqual(
		m.fieldConditionToText({ logic: 'all', conditions: [{ field: 'a' }] }),
		'',
		'group of invalid entries'
	);
});

suite.check('parseFieldConditionText and fieldConditionToText round trip', () => {
	const texts = [
		'a=x',
		'a!=x',
		'all:\na=x\nb gt 3',
		'any:\nstatus=open\nname contains foo\nnote is_empty\nqty lte 9'
	];
	for (const text of texts) {
		const parsed = m.parseFieldConditionText(text);
		assertEqual(m.fieldConditionToText(parsed), text, `round trip of ${JSON.stringify(text)}`);
	}
	assertEqual(
		m.fieldConditionToText(m.parseFieldConditionText('status = open')),
		'status=open',
		'spaced equals normalises to the compact form'
	);
});

suite.check('draftConditionValue flattens draft values to text', () => {
	assertEqual(m.draftConditionValue({ a: 'x' }, 'a'), 'x', 'string');
	assertEqual(m.draftConditionValue({ a: 5 }, 'a'), '5', 'number');
	assertEqual(m.draftConditionValue({ a: true }, 'a'), 'true', 'boolean');
	assertEqual(m.draftConditionValue({ a: ['x', 1, 'y'] }, 'a'), 'x,y', 'array keeps strings only');
	assertEqual(
		m.draftConditionValue({ a: { record_id: 'r1', user_id: 'u1' } }, 'a'),
		'r1',
		'record_id wins'
	);
	assertEqual(m.draftConditionValue({ a: { user_id: 'u1' } }, 'a'), 'u1', 'user_id');
	assertEqual(m.draftConditionValue({ a: { value: 'v' } }, 'a'), 'v', 'string value');
	assertEqual(m.draftConditionValue({ a: { value: 12 } }, 'a'), '12', 'number value');
	assertEqual(m.draftConditionValue({ a: null }, 'a'), '', 'null');
	assertEqual(m.draftConditionValue({}, 'a'), '', 'missing');
	// CURRENT BEHAVIOUR: an object with none of the known keys is stringified with String(), which
	// yields "[object Object]" rather than an empty string.
	assertEqual(
		m.draftConditionValue({ a: { other: 1 } }, 'a'),
		'[object Object]',
		'unknown object shape'
	);
});

suite.check('fieldConditionMatches handles equality operators', () => {
	const draft = { status: 'open', qty: '5' };
	assertEqual(
		m.fieldConditionMatches({ field: 'status', equals: 'open' }, draft),
		true,
		'equals hit'
	);
	assertEqual(
		m.fieldConditionMatches({ field: 'status', equals: 'done' }, draft),
		false,
		'equals miss'
	);
	assertEqual(
		m.fieldConditionMatches({ field: 'status', value: 'open' }, draft),
		true,
		'value alias'
	);
	assertEqual(
		m.fieldConditionMatches({ field: 'status', operator: 'not_equals', value: 'done' }, draft),
		true,
		'not_equals hit'
	);
	assertEqual(
		m.fieldConditionMatches({ field: 'status', operator: '!=', value: 'open' }, draft),
		false,
		'not_equals miss'
	);
	assertEqual(
		m.fieldConditionMatches({ field: 'status', operator: 'bogus', equals: 'open' }, draft),
		true,
		'unknown operator behaves as equals'
	);
	assertEqual(
		m.fieldConditionMatches({ field: ' status ', equals: ' open ' }, draft),
		true,
		'trimmed'
	);
});

suite.check('fieldConditionMatches handles text operators', () => {
	const draft = { name: 'Beef Noodles', blank: '   ', tags: ['a', 'b'] };
	const run = (operator: string, key: string, value?: string): boolean =>
		m.fieldConditionMatches({ field: key, operator, value }, draft);
	assertEqual(run('contains', 'name', 'Noodle'), true, 'contains hit');
	assertEqual(run('contains', 'name', 'noodle'), false, 'contains is case sensitive');
	assertEqual(run('not_contains', 'name', 'Rice'), true, 'not_contains hit');
	assertEqual(run('not_contains', 'name', 'Beef'), false, 'not_contains miss');
	assertEqual(run('is_empty', 'blank'), true, 'whitespace counts as empty');
	assertEqual(run('is_empty', 'missing'), true, 'missing counts as empty');
	assertEqual(run('is_empty', 'name'), false, 'non-empty');
	assertEqual(run('not_empty', 'name'), true, 'not_empty hit');
	assertEqual(run('not_empty', 'blank'), false, 'not_empty miss');
	assertEqual(run('contains', 'tags', 'b'), true, 'array draft is joined with commas');
	assertEqual(run('equals', 'tags', 'a,b'), true, 'array equality');
});

suite.check('fieldConditionMatches handles numeric operators', () => {
	const draft = { qty: '5', amount: 5, text: 'abc', none: '' };
	const run = (operator: string, key: string, value: string): boolean =>
		m.fieldConditionMatches({ field: key, operator, value }, draft);
	assertEqual(run('gt', 'qty', '4'), true, 'gt hit');
	assertEqual(run('gt', 'qty', '5'), false, 'gt boundary');
	assertEqual(run('gte', 'qty', '5'), true, 'gte boundary');
	assertEqual(run('lt', 'amount', '6'), true, 'lt hit');
	assertEqual(run('lt', 'amount', '5'), false, 'lt boundary');
	assertEqual(run('lte', 'amount', '5'), true, 'lte boundary');
	assertEqual(run('gt', 'qty', '1e1'), false, 'exponent literal is numeric');
	assertEqual(run('gt', 'text', '1'), false, 'non numeric draft');
	assertEqual(run('gt', 'qty', 'abc'), false, 'non numeric expectation');
	assertEqual(run('gt', 'qty', 'Infinity'), false, 'non finite expectation');
});

suite.check('fieldConditionMatches rejects incomplete conditions', () => {
	const draft = { a: 'x' };
	assertEqual(m.fieldConditionMatches(null, draft), false, 'null');
	assertEqual(m.fieldConditionMatches('a=x', draft), false, 'string');
	assertEqual(m.fieldConditionMatches([{ field: 'a', equals: 'x' }], draft), false, 'array');
	assertEqual(m.fieldConditionMatches({ equals: 'x' }, draft), false, 'no field');
	assertEqual(m.fieldConditionMatches({ field: 'a' }, draft), false, 'no value');
	assertEqual(
		m.fieldConditionMatches({ field: 'a', operator: 'gt' }, draft),
		false,
		'numeric operator without value'
	);
});

suite.check('fieldConditionMatches combines groups with all and any', () => {
	const draft = { a: 'x', b: 'y' };
	const hit = { field: 'a', equals: 'x' };
	const miss = { field: 'b', equals: 'z' };
	assertEqual(
		m.fieldConditionMatches({ conditions: [hit, hit] }, draft),
		true,
		'default all, both hit'
	);
	assertEqual(
		m.fieldConditionMatches({ conditions: [hit, miss] }, draft),
		false,
		'default all, one miss'
	);
	assertEqual(
		m.fieldConditionMatches({ logic: 'any', conditions: [hit, miss] }, draft),
		true,
		'any'
	);
	assertEqual(
		m.fieldConditionMatches({ logic: 'any', conditions: [miss, miss] }, draft),
		false,
		'any, none'
	);
	assertEqual(
		m.fieldConditionMatches({ logic: 'all', conditions: [] }, draft),
		false,
		'empty group'
	);
	assertEqual(
		m.fieldConditionMatches({ logic: 'any', conditions: ['x', 3] }, draft),
		false,
		'no object entries'
	);
	assertEqual(
		m.fieldConditionMatches(
			{ logic: 'any', conditions: [miss, { logic: 'all', conditions: [hit, hit] }] },
			draft
		),
		true,
		'nested group'
	);
});

suite.check(
	'fieldHasConditionalHidden and fieldHasConditionalReadOnly check configuration only',
	() => {
		assertEqual(m.fieldHasConditionalHidden(hiddenWhenOpen), true, 'hidden configured');
		assertEqual(m.fieldHasConditionalHidden(field('x')), false, 'hidden absent');
		assertEqual(
			m.fieldHasConditionalHidden(field('x', { conditional: {} })),
			false,
			'empty conditional'
		);
		assertEqual(m.fieldHasConditionalReadOnly(readOnlyWhenDone), true, 'read only configured');
		assertEqual(m.fieldHasConditionalReadOnly(hiddenWhenOpen), false, 'read only absent');
	}
);

suite.check('fieldHiddenByCondition and fieldReadOnlyByCondition evaluate against a draft', () => {
	assertEqual(m.fieldHiddenByCondition(hiddenWhenOpen, { status: 'open' }), true, 'hidden');
	assertEqual(m.fieldHiddenByCondition(hiddenWhenOpen, { status: 'done' }), false, 'visible');
	assertEqual(m.fieldHiddenByCondition(field('x'), { status: 'open' }), false, 'no condition');
	assertEqual(
		m.fieldReadOnlyByCondition(readOnlyWhenDone, { status: 'done' }),
		true,
		'read only by condition'
	);
	assertEqual(m.fieldReadOnlyByCondition(readOnlyWhenDone, { status: 'open' }), false, 'editable');
	assertEqual(
		m.fieldReadOnlyByCondition(field('x', { policy: { read_only: true } }), {}),
		true,
		'policy read-only wins without any condition'
	);
	assertEqual(
		m.fieldReadOnlyByCondition(field('x', { policy: { read_only: false } }), {}),
		false,
		'policy false'
	);
});

suite.check('record level helpers read the record values', () => {
	const open = record({ status: 'open' });
	const done = record({ status: 'done' });
	assertEqual(m.recordFieldHiddenByCondition(hiddenWhenOpen, open), true, 'hidden for open record');
	assertEqual(
		m.recordFieldHiddenByCondition(hiddenWhenOpen, done),
		false,
		'visible for done record'
	);
	assertEqual(m.recordFieldHiddenByCondition(field('x'), open), false, 'unconfigured field');
	assertEqual(
		m.recordFieldReadOnlyByCondition(readOnlyWhenDone, done),
		true,
		'read only for done record'
	);
	assertEqual(
		m.recordFieldReadOnlyByCondition(readOnlyWhenDone, open),
		false,
		'editable for open record'
	);
	assertEqual(
		m.recordFieldReadOnlyByCondition(field('x', { policy: { read_only: true } }), open),
		false,
		'policy flag is not a record condition'
	);
});

suite.check('recordsField*ByCondition apply when any record matches', () => {
	const records = [record({ status: 'done' }), record({ status: 'open' })];
	assertEqual(m.recordsFieldHiddenByCondition(hiddenWhenOpen, records), true, 'hidden any');
	assertEqual(
		m.recordsFieldHiddenByCondition(hiddenWhenOpen, [records[0] as FormRecord]),
		false,
		'hidden none'
	);
	assertEqual(m.recordsFieldHiddenByCondition(hiddenWhenOpen, []), false, 'hidden no records');
	assertEqual(m.recordsFieldHiddenByCondition(null, records), false, 'hidden null field');
	assertEqual(m.recordsFieldReadOnlyByCondition(readOnlyWhenDone, records), true, 'read only any');
	assertEqual(
		m.recordsFieldReadOnlyByCondition(readOnlyWhenDone, [records[1] as FormRecord]),
		false,
		'read only none'
	);
	assertEqual(
		m.recordsFieldReadOnlyByCondition(readOnlyWhenDone, []),
		false,
		'read only no records'
	);
	assertEqual(m.recordsFieldReadOnlyByCondition(null, records), false, 'read only null field');
	assert(m.recordsFieldReadOnlyByCondition(field('x'), records) === false, 'unconfigured field');
});

finish(suite.result());
