/**
 * Pivot table buckets, number and percent formatting, totals, heatmap and matrix export.
 *
 * Run standalone: `bun tests/forms-page/views/pivot-format.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../../support/harness';
import { installSvelteSsr } from '../../support/svelte-ssr';
import type { FormField, FormRecord } from '../../../src/lib/api/forms';
import type {
	PivotAggregate,
	PivotSortMode,
	PivotTable,
	PivotTotalsMode,
	PivotValueDisplay
} from '../../../src/lib/forms/page-types';

await installSvelteSsr();
const pivot = await import('../../../src/lib/forms/views/pivot-format');

const suite = new Suite('forms-page/views/pivot-format');

const REGION: FormField = { key: 'region', label: 'Region', type: 'single_select' };
const QUARTER: FormField = { key: 'quarter', label: 'Quarter', type: 'single_select' };
const AMOUNT: FormField = { key: 'amount', label: 'Amount', type: 'amount', amount: { scale: 2 } };

function record(values: Record<string, unknown>): FormRecord {
	return {
		id: 'r',
		workspace_id: 'w',
		project_id: 'p',
		form_id: 'f',
		title: 'R',
		values,
		source: {},
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z'
	};
}

/** East has q1 10 and q2 20.5, west only q1 5; column totals 15 and 20.5; grand total 35.5. */
function table(overrides: Partial<PivotTable> = {}): PivotTable {
	return {
		rowField: REGION,
		columnField: QUARTER,
		valueField: AMOUNT,
		valueCurrency: 'USD',
		aggregate: 'sum',
		totalsMode: 'all',
		valueDisplay: 'value',
		sortMode: 'label',
		emptyBuckets: false,
		hideZeroBuckets: false,
		showCounts: false,
		heatmap: false,
		rowLimit: 0,
		columnLimit: 0,
		maxCellValue: 20.5,
		columns: [
			{ key: 'q1', label: 'Q1', total: 15 },
			{ key: 'q2', label: 'Q2', total: 20.5 }
		],
		rows: [
			{
				key: 'east',
				label: 'East',
				total: 30.5,
				cells: { q1: 10, q2: 20.5 },
				cellCounts: { q1: 1, q2: 2 }
			},
			{ key: 'west', label: 'West', total: 5, cells: { q1: 5 }, cellCounts: { q1: 1 } }
		],
		grandTotal: 35.5,
		...overrides
	};
}

suite.check('pivotShowRowTotals and pivotShowColumnTotals follow the totals mode', () => {
	const expected: Array<[PivotTotalsMode, boolean, boolean]> = [
		['all', true, true],
		['rows', true, false],
		['columns', false, true],
		['none', false, false]
	];
	for (const [mode, rows, columns] of expected) {
		assertEqual(pivot.pivotShowRowTotals(mode), rows, `${mode}: row totals`);
		assertEqual(pivot.pivotShowColumnTotals(mode), columns, `${mode}: column totals`);
	}
});

suite.check('pivotCellIntensity is a rounded percentage of the largest cell, 0 to 100', () => {
	const t = table({ heatmap: true, maxCellValue: 200 });
	assertEqual(pivot.pivotCellIntensity(100, t), 50, 'half');
	assertEqual(pivot.pivotCellIntensity(200, t), 100, 'the maximum');
	assertEqual(pivot.pivotCellIntensity(0, t), 0, 'zero');
	assertEqual(pivot.pivotCellIntensity(400, t), 100, 'clamped above the maximum');
	assertEqual(pivot.pivotCellIntensity(-100, t), 50, 'negative values count by magnitude');
	assertEqual(pivot.pivotCellIntensity(1, t), 1, '0.5 rounds up to 1');
	assertEqual(pivot.pivotCellIntensity(0.99, t), 0, 'below half a percent rounds to 0');
	assertEqual(
		pivot.pivotCellIntensity(1, table({ heatmap: true, maxCellValue: 3 })),
		33,
		'one third'
	);
	assertEqual(
		pivot.pivotCellIntensity(100, table({ heatmap: false, maxCellValue: 200 })),
		0,
		'heatmap off'
	);
	assertEqual(
		pivot.pivotCellIntensity(100, table({ heatmap: true, maxCellValue: 0 })),
		0,
		'maximum zero'
	);
	assertEqual(
		pivot.pivotCellIntensity(100, table({ heatmap: true, maxCellValue: -5 })),
		0,
		'maximum negative'
	);
});

suite.check('pivotCellHeatmapStyle mixes 6% to 24% of the chart colour, or nothing', () => {
	const t = table({ heatmap: true, maxCellValue: 200 });
	const css = (share: string) =>
		`background-color: color-mix(in srgb, var(--rf-chart-1) ${share}%, transparent);`;
	assertEqual(pivot.pivotCellHeatmapStyle(200, t), css('24.0'), 'full intensity');
	assertEqual(pivot.pivotCellHeatmapStyle(100, t), css('15.0'), 'half intensity');
	assertEqual(pivot.pivotCellHeatmapStyle(2, t), css('6.2'), 'intensity 1');
	assertEqual(pivot.pivotCellHeatmapStyle(0, t), '', 'zero has no style');
	assertEqual(pivot.pivotCellHeatmapStyle(0.5, t), '', 'intensity rounds to zero');
	assertEqual(pivot.pivotCellHeatmapStyle(100, table({ heatmap: false })), '', 'heatmap off');
	assertEqual(
		pivot.pivotCellHeatmapStyle(1, table({ heatmap: true, maxCellValue: 3 })),
		css('11.9'),
		'intensity 33 gives 11.94, shown with one decimal'
	);
});

suite.check('pivotFieldLabel shows the displayed value or the unspecified label', () => {
	assertEqual(pivot.pivotFieldLabel(record({ region: 'East' }), REGION), 'East', 'string');
	assertEqual(pivot.pivotFieldLabel(record({ region: 7 }), REGION), '7', 'number');
	assertEqual(pivot.pivotFieldLabel(record({ region: true }), REGION), 'forms.yes', 'boolean true');
	assertEqual(
		pivot.pivotFieldLabel(record({ region: false }), REGION),
		'forms.no',
		'boolean false'
	);
	assertEqual(
		pivot.pivotFieldLabel(record({ region: { name: 'Ada' } }), REGION),
		'Ada',
		'object name'
	);
	assertEqual(pivot.pivotFieldLabel(record({ region: ['a', 'b'] }), REGION), 'a, b', 'array');
	assertEqual(
		pivot.pivotFieldLabel(record({ region: '' }), REGION),
		'forms.unspecified',
		'empty string'
	);
	assertEqual(pivot.pivotFieldLabel(record({ region: null }), REGION), 'forms.unspecified', 'null');
	assertEqual(pivot.pivotFieldLabel(record({}), REGION), 'forms.unspecified', 'missing');
	assertEqual(
		pivot.pivotFieldLabel(record({ region: '-' }), REGION),
		'forms.unspecified',
		'a literal dash'
	);
	assertEqual(
		pivot.pivotFieldLabel(record({ region: 'East' }), null),
		'forms.unspecified',
		'no field'
	);
});

suite.check('pivotFieldBucket builds a lower-case key and a display label per value kind', () => {
	assertDeepEqual(
		pivot.pivotFieldBucket(record({ region: 'East' }), REGION),
		{ key: 'east', label: 'East' },
		'string'
	);
	assertDeepEqual(
		pivot.pivotFieldBucket(record({ region: 'EAST' }), REGION),
		{ key: 'east', label: 'EAST' },
		'case-insensitive key'
	);
	assertDeepEqual(
		pivot.pivotFieldBucket(record({ region: 12 }), REGION),
		{ key: '12', label: '12' },
		'number'
	);
	assertDeepEqual(
		pivot.pivotFieldBucket(record({ region: true }), REGION),
		{ key: 'true', label: 'forms.yes' },
		'boolean'
	);
	assertDeepEqual(
		pivot.pivotFieldBucket(record({ region: { record_id: 'Rec-1', name: 'Ada' } }), REGION),
		{ key: 'Rec-1', label: 'Ada' },
		'relation uses the record id as is'
	);
	assertDeepEqual(
		pivot.pivotFieldBucket(record({ region: { value: 5 } }), REGION),
		{ key: '5', label: '5' },
		'object with a numeric value'
	);
	assertEqual(
		pivot.pivotFieldBucket(record({ region: { value: 'X' } }), REGION).key,
		'x',
		'object with a string value'
	);
	assertDeepEqual(
		pivot.pivotFieldBucket(record({ region: { name: 'Ada Lovelace' } }), REGION),
		{ key: 'ada lovelace', label: 'Ada Lovelace' },
		'other object: key from the label'
	);
	assertDeepEqual(
		pivot.pivotFieldBucket(record({ region: ['A', 'B'] }), REGION),
		{ key: 'a, b', label: 'A, B' },
		'array: key from the label'
	);
	assertDeepEqual(
		pivot.pivotFieldBucket(record({}), REGION),
		{ key: 'forms.unspecified', label: 'forms.unspecified' },
		'missing value'
	);
	assertDeepEqual(
		pivot.pivotFieldBucket(record({ region: 'East' }), null),
		{ key: '__unspecified__', label: 'forms.unspecified' },
		'no field'
	);
});

suite.check(
	'pivotFieldBucket puts an empty string and a missing value in different buckets',
	() => {
		// CURRENT BEHAVIOUR: '' keeps the empty key '' while a missing or null value uses the key
		// 'forms.unspecified'; both show the same label, so the table gets two "unspecified" lines.
		assertDeepEqual(
			pivot.pivotFieldBucket(record({ region: '' }), REGION),
			{ key: '', label: 'forms.unspecified' },
			'empty string'
		);
		assertDeepEqual(
			pivot.pivotFieldBucket(record({ region: null }), REGION),
			{ key: 'forms.unspecified', label: 'forms.unspecified' },
			'null'
		);
	}
);

suite.check('formatPivotValue formats amounts with their scale and currency', () => {
	assertEqual(pivot.formatPivotValue(12.5, AMOUNT), '12.50', 'default scale 2');
	assertEqual(pivot.formatPivotValue(12.5, AMOUNT, 'USD'), '12.50 USD', 'with currency');
	assertEqual(pivot.formatPivotValue(12.5, AMOUNT, ''), '12.50', 'empty currency');
	assertEqual(
		pivot.formatPivotValue(1234.567, { ...AMOUNT, amount: { scale: 1 } }),
		'1234.6',
		'scale 1'
	);
	assertEqual(
		pivot.formatPivotValue(1234.567, { ...AMOUNT, amount: { scale: 0 } }),
		'1235',
		'scale 0'
	);
	assertEqual(
		pivot.formatPivotValue(0.5, { ...AMOUNT, amount: { scale: 4 } }),
		'0.5000',
		'scale 4'
	);
	assertEqual(
		pivot.formatPivotValue(3, { ...AMOUNT, amount: {} }, 'EUR'),
		'3.00 EUR',
		'scale missing'
	);
	assertEqual(pivot.formatPivotValue(3, { key: 'a', type: 'amount' }), '3.00', 'no amount config');
	assertEqual(pivot.formatPivotValue(-2.5, AMOUNT, 'USD'), '-2.50 USD', 'negative');
	assertEqual(
		pivot.formatPivotValue(12.5, AMOUNT, 'USD', 'avg'),
		'12.50 USD',
		'avg keeps the amount format'
	);
	assertEqual(pivot.formatPivotValue(12.5, AMOUNT, 'USD', 'sum'), '12.50 USD', 'explicit sum');
});

suite.check('formatPivotValue formats counts and other fields as integers or two decimals', () => {
	assertEqual(pivot.formatPivotValue(3, AMOUNT, 'USD', 'count'), '3', 'count of an amount field');
	assertEqual(pivot.formatPivotValue(5, { key: 'n', type: 'number' }), '5', 'whole number');
	assertEqual(pivot.formatPivotValue(2.5, { key: 'n', type: 'number' }), '2.50', 'fraction');
	assertEqual(
		pivot.formatPivotValue(1 / 3, { key: 'n', type: 'integer' }),
		'0.33',
		'long fraction'
	);
	assertEqual(pivot.formatPivotValue(7, null), '7', 'no field');
	assertEqual(
		pivot.formatPivotValue(7.25, null, 'USD'),
		'7.25',
		'currency is ignored for non-amount fields'
	);
	assertEqual(pivot.formatPivotValue(0, { key: 'n' }), '0', 'zero');
	assertEqual(pivot.formatPivotValue(-4, { key: 'n' }), '-4', 'negative whole number');
});

suite.check('formatPivotPercent gives two decimals and guards a zero denominator', () => {
	assertEqual(pivot.formatPivotPercent(1, 4), '25.00%', 'a quarter');
	assertEqual(pivot.formatPivotPercent(1, 3), '33.33%', 'a third');
	assertEqual(pivot.formatPivotPercent(2, 3), '66.67%', 'two thirds');
	assertEqual(pivot.formatPivotPercent(3, 2), '150.00%', 'above 100');
	assertEqual(pivot.formatPivotPercent(0, 5), '0.00%', 'zero value');
	assertEqual(pivot.formatPivotPercent(-1, 4), '-25.00%', 'negative value');
	assertEqual(pivot.formatPivotPercent(1, -4), '-25.00%', 'negative denominator');
	assertEqual(pivot.formatPivotPercent(5, 0), '0.00%', 'zero denominator');
	assertEqual(pivot.formatPivotPercent(5, 1e-17), '0.00%', 'denominator below epsilon');
	assertEqual(pivot.formatPivotPercent(5, -1e-17), '0.00%', 'negative denominator below epsilon');
});

suite.check('formatPivotDisplayValue honours every value display mode', () => {
	const modes: Array<[PivotValueDisplay, string]> = [
		['value', '10.00 USD'],
		['percent_row', '25.00%'],
		['percent_column', '50.00%'],
		['percent_total', '10.00%']
	];
	// value 10, row total 40, column total 20, grand total 100
	for (const [mode, expected] of modes) {
		assertEqual(
			pivot.formatPivotDisplayValue(10, 40, 20, table({ valueDisplay: mode, grandTotal: 100 })),
			expected,
			mode
		);
	}
	assertEqual(
		pivot.formatPivotDisplayValue(
			10,
			40,
			20,
			table({ valueDisplay: 'value', aggregate: 'count', valueField: null })
		),
		'10',
		'value mode with a count and no value field'
	);
	assertEqual(
		pivot.formatPivotDisplayValue(10, undefined, undefined, table({ valueDisplay: 'percent_row' })),
		'0.00%',
		'default row total 0'
	);
	assertEqual(
		pivot.formatPivotDisplayValue(
			10,
			undefined,
			undefined,
			table({ valueDisplay: 'percent_column' })
		),
		'0.00%',
		'default column total 0'
	);
});

suite.check(
	'formatPivotTotalDisplayValue shows 100% on the totals line that matches the display mode',
	() => {
		const t = (valueDisplay: PivotValueDisplay) => table({ valueDisplay, grandTotal: 200 });
		assertEqual(
			pivot.formatPivotTotalDisplayValue(50, 'row', t('value')),
			'50.00 USD',
			'value, row'
		);
		assertEqual(
			pivot.formatPivotTotalDisplayValue(50, 'column', t('value')),
			'50.00 USD',
			'value, column'
		);
		assertEqual(
			pivot.formatPivotTotalDisplayValue(200, 'grand', t('value')),
			'200.00 USD',
			'value, grand'
		);
		assertEqual(
			pivot.formatPivotTotalDisplayValue(50, 'row', t('percent_row')),
			'100.00%',
			'percent_row, row total'
		);
		assertEqual(
			pivot.formatPivotTotalDisplayValue(50, 'column', t('percent_row')),
			'25.00%',
			'percent_row, column total'
		);
		assertEqual(
			pivot.formatPivotTotalDisplayValue(200, 'grand', t('percent_row')),
			'100.00%',
			'percent_row, grand total'
		);
		assertEqual(
			pivot.formatPivotTotalDisplayValue(50, 'column', t('percent_column')),
			'100.00%',
			'percent_column, column total'
		);
		assertEqual(
			pivot.formatPivotTotalDisplayValue(50, 'row', t('percent_column')),
			'25.00%',
			'percent_column, row total'
		);
		assertEqual(
			pivot.formatPivotTotalDisplayValue(200, 'grand', t('percent_column')),
			'100.00%',
			'percent_column, grand total'
		);
		for (const kind of ['row', 'column', 'grand'] as const) {
			assertEqual(
				pivot.formatPivotTotalDisplayValue(50, kind, t('percent_total')),
				'25.00%',
				`percent_total, ${kind}`
			);
		}
		assertEqual(
			pivot.formatPivotTotalDisplayValue(0, 'row', t('percent_row')),
			'0.00%',
			'a zero row total is guarded'
		);
	}
);

suite.check('pivotAggregateValue picks sum, count or average', () => {
	const cases: Array<[number, number, PivotAggregate, number]> = [
		[30, 4, 'sum', 30],
		[30, 4, 'count', 4],
		[30, 4, 'avg', 7.5],
		[30, 0, 'avg', 0],
		[0, 3, 'avg', 0],
		[-9, 3, 'avg', -3],
		[30, 0, 'count', 0],
		[0, 0, 'sum', 0]
	];
	for (const [sum, count, aggregate, expected] of cases) {
		assertEqual(
			pivot.pivotAggregateValue(sum, count, aggregate),
			expected,
			`${aggregate} of ${sum}/${count}`
		);
	}
});

suite.check('sortPivotBuckets sorts by label for the label mode and the other axis', () => {
	const items = [
		{ label: 'cherry', total: 5 },
		{ label: 'apple', total: 1 },
		{ label: 'banana', total: 9 }
	];
	const labels = (sorted: typeof items) => sorted.map((item) => item.label);
	const byLabel = ['apple', 'banana', 'cherry'];
	assertDeepEqual(labels(pivot.sortPivotBuckets(items, 'label', 'row')), byLabel, 'label, row');
	assertDeepEqual(
		labels(pivot.sortPivotBuckets(items, 'label', 'column')),
		byLabel,
		'label, column'
	);
	assertDeepEqual(
		labels(pivot.sortPivotBuckets(items, 'column_total_desc', 'row')),
		byLabel,
		'column mode on the row axis'
	);
	assertDeepEqual(
		labels(pivot.sortPivotBuckets(items, 'column_total_asc', 'row')),
		byLabel,
		'column asc on the row axis'
	);
	assertDeepEqual(
		labels(pivot.sortPivotBuckets(items, 'row_total_desc', 'column')),
		byLabel,
		'row mode on the column axis'
	);
	assertDeepEqual(
		labels(pivot.sortPivotBuckets(items, 'row_total_asc', 'column')),
		byLabel,
		'row asc on the column axis'
	);
});

suite.check('sortPivotBuckets sorts by total for the matching axis, ties by label', () => {
	const items = [
		{ label: 'cherry', total: 5 },
		{ label: 'apple', total: 1 },
		{ label: 'banana', total: 9 },
		{ label: 'date', total: 5 }
	];
	const labels = (mode: PivotSortMode, axis: 'row' | 'column') =>
		pivot.sortPivotBuckets(items, mode, axis).map((item) => item.label);
	assertDeepEqual(
		labels('row_total_desc', 'row'),
		['banana', 'cherry', 'date', 'apple'],
		'row desc'
	);
	assertDeepEqual(labels('row_total_asc', 'row'), ['apple', 'cherry', 'date', 'banana'], 'row asc');
	assertDeepEqual(
		labels('column_total_desc', 'column'),
		['banana', 'cherry', 'date', 'apple'],
		'column desc'
	);
	assertDeepEqual(
		labels('column_total_asc', 'column'),
		['apple', 'cherry', 'date', 'banana'],
		'column asc'
	);
});

suite.check('sortPivotBuckets copies its input and keeps extra properties', () => {
	const items = [
		{ label: 'b', total: 1, key: 'kb' },
		{ label: 'a', total: 2, key: 'ka' }
	];
	const sorted = pivot.sortPivotBuckets(items, 'label', 'row');
	assertDeepEqual(
		items.map((item) => item.key),
		['kb', 'ka'],
		'the input order is unchanged'
	);
	assert(sorted !== items, 'a new array');
	assertDeepEqual(
		sorted.map((item) => item.key),
		['ka', 'kb'],
		'extra properties travel with the items'
	);
	assertDeepEqual(pivot.sortPivotBuckets([], 'row_total_desc', 'row'), [], 'empty input');
	assertDeepEqual(
		pivot.sortPivotBuckets([{ label: 'x', total: 1 }], 'row_total_desc', 'row'),
		[{ label: 'x', total: 1 }],
		'one item'
	);
	assertDeepEqual(
		pivot
			.sortPivotBuckets(
				[
					{ label: 'n', total: -1 },
					{ label: 'm', total: -5 }
				],
				'row_total_desc',
				'row'
			)
			.map((i) => i.label),
		['n', 'm'],
		'negative totals sort numerically'
	);
});

suite.check(
	'pivotConfiguredBuckets lists the distinct non-blank options of a single select',
	() => {
		assertDeepEqual(
			pivot.pivotConfiguredBuckets({
				key: 's',
				type: 'single_select',
				options: ['Open', ' Done ', '', '  ', 'open', 'DONE']
			}),
			[
				{ key: 'open', label: 'Open' },
				{ key: 'done', label: 'Done' }
			],
			'trimmed, de-duplicated case-insensitively, first spelling kept'
		);
		assertDeepEqual(
			pivot.pivotConfiguredBuckets({ key: 's', type: 'single_select', options: [] }),
			[],
			'no options'
		);
		assertDeepEqual(
			pivot.pivotConfiguredBuckets({ key: 's', type: 'single_select' }),
			[],
			'options missing'
		);
		assertDeepEqual(
			pivot.pivotConfiguredBuckets({ key: 's', type: 'multi_select', options: ['a'] }),
			[],
			'multi select'
		);
		assertDeepEqual(pivot.pivotConfiguredBuckets({ key: 's', options: ['a'] }), [], 'no type');
		assertDeepEqual(pivot.pivotConfiguredBuckets(null), [], 'no field');
	}
);

suite.check(
	'pivotTableMatrixRows builds header, rows and both total lines for the all mode',
	() => {
		assertDeepEqual(
			pivot.pivotTableMatrixRows(table()),
			[
				['Region', 'Q1', 'Q2', 'forms.pivotTotal'],
				['East', '10.00 USD', '20.50 USD', '30.50 USD'],
				['West', '5.00 USD', '0.00 USD', '5.00 USD'],
				['forms.pivotTotal', '15.00 USD', '20.50 USD', '35.50 USD']
			],
			'all totals; the missing west/q2 cell is 0'
		);
	}
);

suite.check('pivotTableMatrixRows leaves out the total column or line by totals mode', () => {
	assertDeepEqual(
		pivot.pivotTableMatrixRows(table({ totalsMode: 'rows' })),
		[
			['Region', 'Q1', 'Q2', 'forms.pivotTotal'],
			['East', '10.00 USD', '20.50 USD', '30.50 USD'],
			['West', '5.00 USD', '0.00 USD', '5.00 USD']
		],
		'rows: a total column, no total line'
	);
	assertDeepEqual(
		pivot.pivotTableMatrixRows(table({ totalsMode: 'columns' })),
		[
			['Region', 'Q1', 'Q2'],
			['East', '10.00 USD', '20.50 USD'],
			['West', '5.00 USD', '0.00 USD'],
			['forms.pivotTotal', '15.00 USD', '20.50 USD']
		],
		'columns: a total line, no total column'
	);
	assertDeepEqual(
		pivot.pivotTableMatrixRows(table({ totalsMode: 'none' })),
		[
			['Region', 'Q1', 'Q2'],
			['East', '10.00 USD', '20.50 USD'],
			['West', '5.00 USD', '0.00 USD']
		],
		'none'
	);
});

suite.check('pivotTableMatrixRows applies the value display and works without data rows', () => {
	assertDeepEqual(
		pivot.pivotTableMatrixRows(table({ valueDisplay: 'percent_total', grandTotal: 40 })),
		[
			['Region', 'Q1', 'Q2', 'forms.pivotTotal'],
			['East', '25.00%', '51.25%', '76.25%'],
			['West', '12.50%', '0.00%', '12.50%'],
			['forms.pivotTotal', '37.50%', '51.25%', '100.00%']
		],
		'percent of the grand total (the grand cell is the grand total itself)'
	);
	assertDeepEqual(
		pivot.pivotTableMatrixRows(table({ rows: [], columns: [], totalsMode: 'none' })),
		[['Region']],
		'no rows or columns'
	);
});

suite.check('pivotTableMatrixRows is empty when a field is missing', () => {
	assertDeepEqual(pivot.pivotTableMatrixRows(table({ rowField: null })), [], 'no row field');
	assertDeepEqual(pivot.pivotTableMatrixRows(table({ columnField: null })), [], 'no column field');
	assertDeepEqual(pivot.pivotTableMatrixRows(table({ valueField: null })), [], 'no value field');
});

suite.check('pivotTableMatrixText joins cells with tabs and rows with line feeds', () => {
	assertEqual(
		pivot.pivotTableMatrixText(table({ totalsMode: 'none' })),
		'Region\tQ1\tQ2\nEast\t10.00 USD\t20.50 USD\nWest\t5.00 USD\t0.00 USD',
		'text'
	);
	assertEqual(pivot.pivotTableMatrixText(table({ rowField: null })), '', 'empty table');
});

suite.check('pivotTableMatrixCsv quotes every cell and neutralises formula-like labels', () => {
	assertEqual(
		pivot.pivotTableMatrixCsv(table({ totalsMode: 'none' })),
		'"Region","Q1","Q2"\n"East","10.00 USD","20.50 USD"\n"West","5.00 USD","0.00 USD"',
		'csv'
	);
	assertEqual(pivot.pivotTableMatrixCsv(table({ columnField: null })), '', 'empty table');
	assertEqual(
		pivot.pivotTableMatrixCsv(
			table({
				totalsMode: 'none',
				columns: [{ key: 'q1', label: 'Q "1", early', total: 15 }],
				rows: [
					{
						key: 'x',
						label: '=HYPERLINK("http://x")',
						total: 1,
						cells: { q1: 1 },
						cellCounts: { q1: 1 }
					}
				]
			})
		),
		'"Region","Q ""1"", early"\n"\'=HYPERLINK(""http://x"")","1.00 USD"',
		'quotes doubled; a label starting with = gets a leading quote'
	);
});

finish(suite.result());
