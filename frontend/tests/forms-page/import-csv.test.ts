/**
 * CSV parsing, cell conversion and mapping transforms of the forms import wizard.
 *
 * Run standalone: `bun tests/forms-page/import-csv.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormField } from '../../src/lib/api/forms';
import type { ImportMappingTransform } from '../../src/lib/forms/page-types';

await installSvelteSsr();
const csv = await import('../../src/lib/forms/import-csv');

const suite = new Suite('forms-page/import-csv');

function field(type: FormField['type'] | undefined, extra: Partial<FormField> = {}): FormField {
	return { key: 'some_key', type, ...extra };
}

suite.check('csvCell wraps in quotes and doubles embedded quotes', () => {
	assertEqual(csv.csvCell('abc'), '"abc"', 'plain');
	assertEqual(csv.csvCell(''), '""', 'empty');
	assertEqual(csv.csvCell('say "hi"'), '"say ""hi"""', 'quotes doubled');
	assertEqual(csv.csvCell('a,b'), '"a,b"', 'comma stays inside the quotes');
	assertEqual(csv.csvCell('a\nb'), '"a\nb"', 'line break stays inside the quotes');
	assertEqual(csv.csvCell('"'), '""""', 'single quote');
});

suite.check('csvCell leaves values starting with = + - @ unchanged', () => {
	// CURRENT BEHAVIOUR: no spreadsheet-formula neutralisation. An exported cell such as
	// =HYPERLINK(...) is written as ="..." text and is evaluated as a formula when the CSV is
	// opened in a spreadsheet application (CSV injection). Prefixing a single quote would avoid it.
	assertEqual(csv.csvCell('=1+1'), '"=1+1"', 'equals');
	assertEqual(csv.csvCell('+1'), '"+1"', 'plus');
	assertEqual(csv.csvCell('-1'), '"-1"', 'minus');
	assertEqual(csv.csvCell('@SUM(A1)'), '"@SUM(A1)"', 'at');
});

suite.check('parseCsvLines splits simple rows and cells', () => {
	assertDeepEqual(
		csv.parseCsvLines('a,b,c\n1,2,3'),
		[
			['a', 'b', 'c'],
			['1', '2', '3']
		],
		'two rows'
	);
	assertDeepEqual(csv.parseCsvLines('a'), [['a']], 'one cell');
	assertDeepEqual(csv.parseCsvLines(''), [['']], 'empty text is one row with one empty cell');
	assertDeepEqual(csv.parseCsvLines(',,'), [['', '', '']], 'only separators');
	assertDeepEqual(csv.parseCsvLines(' a , b '), [[' a ', ' b ']], 'spaces are kept');
});

suite.check('parseCsvLines handles quoted fields, embedded commas, quotes and line breaks', () => {
	assertDeepEqual(csv.parseCsvLines('"a,b",c'), [['a,b', 'c']], 'embedded comma');
	assertDeepEqual(csv.parseCsvLines('"say ""hi""",x'), [['say "hi"', 'x']], 'escaped quotes');
	assertDeepEqual(
		csv.parseCsvLines('"line1\nline2",x\ny,z'),
		[
			['line1\nline2', 'x'],
			['y', 'z']
		],
		'embedded LF'
	);
	assertDeepEqual(
		csv.parseCsvLines('"a\r\nb",x'),
		[['a\r\nb', 'x']],
		'embedded CRLF keeps its CR inside quotes'
	);
	assertDeepEqual(csv.parseCsvLines('""'), [['']], 'empty quoted field');
	assertDeepEqual(csv.parseCsvLines('"",""'), [['', '']], 'two empty quoted fields');
	assertDeepEqual(csv.parseCsvLines('""""'), [['"']], 'a quoted single quote');
	assertDeepEqual(csv.parseCsvLines('"a"b'), [['ab']], 'text after the closing quote is appended');
	assertDeepEqual(
		csv.parseCsvLines('a"b"c,d'),
		[['abc', 'd']],
		'a quote in the middle starts a quoted run'
	);
	assertDeepEqual(csv.parseCsvLines('"中文,字段",é'), [['中文,字段', 'é']], 'non-ASCII');
});

suite.check('parseCsvLines accepts CRLF line endings and drops carriage returns', () => {
	assertDeepEqual(
		csv.parseCsvLines('a,b\r\n1,2\r\n'),
		[['a', 'b'], ['1', '2'], ['']],
		'CRLF with trailing break'
	);
	assertDeepEqual(
		csv.parseCsvLines('a,b\r\n1,2'),
		[
			['a', 'b'],
			['1', '2']
		],
		'CRLF without trailing break'
	);
	assertDeepEqual(csv.parseCsvLines('a\rb'), [['ab']], 'a lone CR is dropped, not a line break');
});

suite.check(
	'parseCsvLines returns a final empty row for a trailing newline and keeps empty lines',
	() => {
		// CURRENT BEHAVIOUR: trailing and blank lines are not removed; the caller has to filter them.
		assertDeepEqual(
			csv.parseCsvLines('a,b\n1,2\n'),
			[['a', 'b'], ['1', '2'], ['']],
			'one trailing newline'
		);
		assertDeepEqual(csv.parseCsvLines('a\n\n'), [['a'], [''], ['']], 'two trailing newlines');
		assertDeepEqual(csv.parseCsvLines('a\n\nb'), [['a'], [''], ['b']], 'blank line in the middle');
		assertDeepEqual(csv.parseCsvLines('\n'), [[''], ['']], 'only a newline');
	}
);

suite.check('parseCsvLines keeps rows of different length as they are', () => {
	assertDeepEqual(
		csv.parseCsvLines('a,b,c\n1,2\n1,2,3,4'),
		[
			['a', 'b', 'c'],
			['1', '2'],
			['1', '2', '3', '4']
		],
		'ragged rows are not padded or cut'
	);
});

suite.check('parseCsvLines keeps a byte-order mark in the first cell', () => {
	// CURRENT BEHAVIOUR: a UTF-8 BOM (U+FEFF) at the start of the text is not stripped, so the
	// first header cell of a CSV saved by Excel is "<BOM>name" and will not match a field label.
	const rows = csv.parseCsvLines('\uFEFFname,age\nAda,36');
	assertEqual(rows[0][0], '\uFEFFname', 'BOM stays in the first header cell');
	assertEqual(rows[0][0].length, 5, 'one extra character');
	assertDeepEqual(rows[1], ['Ada', '36'], 'later rows are unaffected');
	assertDeepEqual(
		csv.parseCsvLines('\uFEFF"name",age')[0],
		['\uFEFFname', 'age'],
		'BOM before a quoted cell'
	);
});

suite.check('parseCsvLines does not give = + - @ values any special treatment', () => {
	// CURRENT BEHAVIOUR: formula-looking cells are read as plain text, unchanged.
	assertDeepEqual(
		csv.parseCsvLines('=1+1,+2,-3,@x\n"=SUM(A1,A2)",-,+,@'),
		[
			['=1+1', '+2', '-3', '@x'],
			['=SUM(A1,A2)', '-', '+', '@']
		],
		'formula-like cells'
	);
});

suite.check('parseCsvLines tolerates an unterminated quote by taking the rest as one cell', () => {
	assertDeepEqual(csv.parseCsvLines('a,"b,c\nd'), [['a', 'b,c\nd']], 'unterminated quote');
	assertDeepEqual(csv.parseCsvLines('"abc'), [['abc']], 'quote never closed');
});

suite.check('parseCsvLines round-trips cells written with csvCell', () => {
	const cells = ['plain', 'a,b', 'say "hi"', 'two\nlines', '', ' padded ', '中文'];
	const line = cells.map(csv.csvCell).join(',');
	assertDeepEqual(csv.parseCsvLines(line), [cells], 'one row');
	const text = [cells, cells.slice().reverse()]
		.map((row) => row.map(csv.csvCell).join(','))
		.join('\n');
	assertDeepEqual(csv.parseCsvLines(text), [cells, cells.slice().reverse()], 'two rows');
});

suite.check('sampleImportValue gives a type-appropriate example value', () => {
	assertEqual(csv.sampleImportValue(field('boolean')), 'true', 'boolean');
	assertEqual(csv.sampleImportValue(field('integer')), '1', 'integer');
	assertEqual(csv.sampleImportValue(field('number')), '1.00', 'number');
	assertEqual(csv.sampleImportValue(field('amount')), '1.00', 'amount');
	assertEqual(
		csv.sampleImportValue(field('single_select', { options: ['Open', 'Done'] })),
		'Open',
		'first select option'
	);
	assertEqual(csv.sampleImportValue(field('single_select')), '', 'select without options');
	assertEqual(
		csv.sampleImportValue(field('multi_select', { options: ['a,b', 'c'] })),
		'a b',
		'commas replaced by spaces'
	);
	assertEqual(
		csv.sampleImportValue(field('multi_select', { options: [] })),
		'',
		'multi select without options'
	);
	assertEqual(
		csv.sampleImportValue(field('text', { label: ' Name ' })),
		'Name',
		'text uses the label'
	);
	assertEqual(
		csv.sampleImportValue(field(undefined, { key: 'due_date' })),
		'due date',
		'no label uses the key'
	);
});

suite.check('isValidImportMappingTransform accepts exactly the nine transform names', () => {
	const valid = [
		'none',
		'trim',
		'uppercase',
		'lowercase',
		'capitalize',
		'titlecase',
		'normalize_spaces',
		'strip_non_digits',
		'slugify'
	];
	for (const name of valid) assert(csv.isValidImportMappingTransform(name), `${name} is valid`);
	for (const name of ['', 'NONE', ' trim', 'reverse', 'title_case', 'constructor']) {
		assert(!csv.isValidImportMappingTransform(name), `${JSON.stringify(name)} is not valid`);
	}
});

suite.check('applyImportMappingTransform: none returns the value untouched', () => {
	assertEqual(csv.applyImportMappingTransform('  Ab  ', 'none'), '  Ab  ', 'no trimming');
	assertEqual(csv.applyImportMappingTransform('', 'none'), '', 'empty');
});

suite.check('applyImportMappingTransform: trim, case and capitalise transforms', () => {
	assertEqual(csv.applyImportMappingTransform('  Ab c  ', 'trim'), 'Ab c', 'trim');
	assertEqual(csv.applyImportMappingTransform('  Ab c  ', 'uppercase'), 'AB C', 'uppercase');
	assertEqual(csv.applyImportMappingTransform('  Ab C  ', 'lowercase'), 'ab c', 'lowercase');
	assertEqual(
		csv.applyImportMappingTransform(' hELLO wORLD ', 'capitalize'),
		'Hello world',
		'capitalize'
	);
	assertEqual(
		csv.applyImportMappingTransform(' hELLO   wORLD ', 'titlecase'),
		'Hello World',
		'titlecase'
	);
	assertEqual(csv.applyImportMappingTransform('   ', 'titlecase'), '', 'titlecase of blanks');
	assertEqual(csv.applyImportMappingTransform('', 'capitalize'), '', 'capitalize of empty');
});

suite.check('applyImportMappingTransform: spaces, digits and slug transforms', () => {
	assertEqual(
		csv.applyImportMappingTransform('  a   b \t c  ', 'normalize_spaces'),
		'a b c',
		'normalize spaces'
	);
	assertEqual(
		csv.applyImportMappingTransform(' +86 (138) 0013-8000 ', 'strip_non_digits'),
		'8613800138000',
		'strip non digits'
	);
	assertEqual(csv.applyImportMappingTransform('abc', 'strip_non_digits'), '', 'no digits');
	assertEqual(
		csv.applyImportMappingTransform('  Hello, World! 2026  ', 'slugify'),
		'hello-world-2026',
		'slugify'
	);
	assertEqual(
		csv.applyImportMappingTransform('中文 Title', 'slugify'),
		'title',
		'slugify drops non-ASCII'
	);
	assertEqual(csv.applyImportMappingTransform('!!!', 'slugify'), '', 'slugify of punctuation');
});

suite.check('applyImportMappingTransform trims an unknown transform name', () => {
	// The cast passes an unsupported name on purpose: the function falls through to a trim.
	const unknown = 'bogus' as ImportMappingTransform;
	assertEqual(csv.applyImportMappingTransform('  x  ', unknown), 'x', 'unknown transform');
	// 'trim' itself reaches the same fall-through.
	assertEqual(csv.applyImportMappingTransform('  x  ', 'trim'), 'x', 'trim');
});

suite.check(
	'capitalizeImportValue upper-cases the first character and lower-cases the rest',
	() => {
		assertEqual(csv.capitalizeImportValue('hello'), 'Hello', 'lower');
		assertEqual(csv.capitalizeImportValue('HELLO'), 'Hello', 'upper');
		assertEqual(csv.capitalizeImportValue('a'), 'A', 'one letter');
		assertEqual(csv.capitalizeImportValue(''), '', 'empty');
		assertEqual(csv.capitalizeImportValue('1abc'), '1abc', 'digit first');
		assertEqual(csv.capitalizeImportValue(' abc'), ' abc', 'leading space is not trimmed');
		assertEqual(csv.capitalizeImportValue('élan'), 'Élan', 'non-ASCII letter');
	}
);

suite.check('slugifyImportValue lower-cases and joins ASCII words with single dashes', () => {
	assertEqual(csv.slugifyImportValue('Hello World'), 'hello-world', 'words');
	assertEqual(csv.slugifyImportValue('--A__B--'), 'a-b', 'dashes and underscores');
	assertEqual(csv.slugifyImportValue('a1 b2'), 'a1-b2', 'digits kept');
	assertEqual(csv.slugifyImportValue(''), '', 'empty');
	assertEqual(csv.slugifyImportValue('  '), '', 'blank');
	assertEqual(csv.slugifyImportValue('Café'), 'caf', 'accented letter is dropped');
});

suite.check('normalizeImportItem reads values and title from a row object', () => {
	assertDeepEqual(
		csv.normalizeImportItem({ title: 'T', values: { a: 1 } }, 3),
		{ row_number: 3, title: 'T', values: { a: 1 }, source: { type: 'web', origin: 'import' } },
		'values object'
	);
	assertDeepEqual(
		csv.normalizeImportItem({ a: 1, b: 'x' }, 1),
		{
			row_number: 1,
			title: undefined,
			values: { a: 1, b: 'x' },
			source: { type: 'web', origin: 'import' }
		},
		'flat object is its own values'
	);
	const flat = csv.normalizeImportItem({ title: 'T', a: 1 }, 2);
	assertEqual(flat.title, 'T', 'flat title');
	assertDeepEqual(flat.values, { title: 'T', a: 1 }, 'flat object keeps title inside values');
	assertEqual(
		csv.normalizeImportItem({ title: 5, values: {} }, 4).title,
		undefined,
		'non-string title'
	);
	assertDeepEqual(
		csv.normalizeImportItem({ values: [1] }, 5).values,
		{ values: [1] },
		'array values fall back to the item'
	);
	assertDeepEqual(
		csv.normalizeImportItem({ values: null }, 6).values,
		{ values: null },
		'null values fall back to the item'
	);
});

suite.check('normalizeImportItem rejects anything that is not an object', () => {
	for (const item of [null, undefined, 'text', 42, true, [1, 2]]) {
		let message = '';
		try {
			csv.normalizeImportItem(item, 7);
		} catch (error) {
			message = error instanceof Error ? error.message : String(error);
		}
		assertEqual(
			message,
			'forms.importInvalidRow',
			`${JSON.stringify(item)} throws the invalid-row message`
		);
	}
});

suite.check('importCellValue: blank cells are undefined for every type', () => {
	for (const type of ['text', 'boolean', 'multi_select', 'attachment', 'number'] as const) {
		assertEqual(csv.importCellValue(field(type), ''), undefined, `${type} empty`);
		assertEqual(csv.importCellValue(field(type), '  \t '), undefined, `${type} blank`);
	}
});

suite.check('importCellValue: booleans accept true, 1, yes, y and 是 in any case', () => {
	for (const raw of ['true', 'TRUE', ' Yes ', '1', 'y', 'Y', '是']) {
		assertEqual(csv.importCellValue(field('boolean'), raw), true, raw);
	}
	for (const raw of ['false', '0', 'no', 'n', '否', 'maybe', 'yes please']) {
		assertEqual(csv.importCellValue(field('boolean'), raw), false, raw);
	}
});

suite.check('importCellValue: multi select splits on semicolon and pipe', () => {
	assertDeepEqual(
		csv.importCellValue(field('multi_select'), 'a;b|c'),
		['a', 'b', 'c'],
		'both separators'
	);
	assertDeepEqual(
		csv.importCellValue(field('multi_select'), ' a ; ; b '),
		['a', 'b'],
		'trim and drop empties'
	);
	assertDeepEqual(
		csv.importCellValue(field('multi_select'), 'a,b'),
		['a,b'],
		'a comma is not a separator'
	);
	assertDeepEqual(csv.importCellValue(field('multi_select'), ';'), [], 'only a separator');
});

suite.check('importCellValue: JSON-like types parse objects and arrays and keep other text', () => {
	assertDeepEqual(
		csv.importCellValue(field('attachment'), '{"url":"https://a/x"}'),
		{ url: 'https://a/x' },
		'attachment object'
	);
	assertDeepEqual(csv.importCellValue(field('image'), '["u1","u2"]'), ['u1', 'u2'], 'image array');
	assertDeepEqual(
		csv.importCellValue(field('relation'), '[{"record_id":"r1"}]'),
		[{ record_id: 'r1' }],
		'relation'
	);
	assertDeepEqual(csv.importCellValue(field('child_table'), ' [] '), [], 'child table, trimmed');
	assertDeepEqual(csv.importCellValue(field('formula'), '{"a":1}'), { a: 1 }, 'formula');
	assertEqual(
		csv.importCellValue(field('attachment'), 'https://a/x.png'),
		'https://a/x.png',
		'plain URL stays a string'
	);
	assertEqual(
		csv.importCellValue(field('attachment'), '{broken'),
		'{broken',
		'malformed JSON stays text'
	);
	assertEqual(
		csv.importCellValue(field('image'), '["unterminated'),
		'["unterminated',
		'malformed array stays text'
	);
});

suite.check('importCellValue: other types return the trimmed text, even for = + - @ values', () => {
	assertEqual(csv.importCellValue(field('text'), '  Ada  '), 'Ada', 'text');
	assertEqual(csv.importCellValue(field(undefined), ' x '), 'x', 'no type is text');
	assertEqual(csv.importCellValue(field('number'), ' 1.50 '), '1.50', 'number stays a string');
	assertEqual(csv.importCellValue(field('date'), '2026-11-20'), '2026-11-20', 'date');
	assertEqual(
		csv.importCellValue(field('text'), '{"a":1}'),
		'{"a":1}',
		'JSON text in a text field'
	);
	// CURRENT BEHAVIOUR: values starting with = + - @ are stored unchanged (no escaping).
	assertEqual(csv.importCellValue(field('text'), '=1+1'), '=1+1', 'equals');
	assertEqual(csv.importCellValue(field('text'), '+1'), '+1', 'plus');
	assertEqual(csv.importCellValue(field('text'), '-1'), '-1', 'minus');
	assertEqual(csv.importCellValue(field('text'), '@a'), '@a', 'at');
});

finish(suite.result());
