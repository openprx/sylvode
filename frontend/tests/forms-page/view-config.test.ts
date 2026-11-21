/**
 * Readers of a view's stored configuration and the built-in view (`src/lib/forms/view-config.ts`).
 *
 * Run standalone: `bun tests/forms-page/view-config.test.ts`
 */
import { Suite, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormField, FormView, UniversalForm } from '../../src/lib/api/forms';

await installSvelteSsr();
const m = await import('../../src/lib/forms/view-config');

const suite = new Suite('forms-page/view-config');

function view(config: Record<string, unknown>, extra: Partial<FormView> = {}): FormView {
	return {
		id: 'view-1',
		workspace_id: 'ws',
		project_id: 'pr',
		form_id: 'form-1',
		key: 'v1',
		name: 'View',
		view_type: 'grid',
		config,
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z',
		...extra
	};
}

function field(key: string, extra: Partial<FormField> = {}): FormField {
	return { key, type: 'text', ...extra };
}

suite.check('viewColumns and viewConfigStringArray keep non-blank strings', () => {
	assertDeepEqual(m.viewColumns(null), [], 'null view');
	assertDeepEqual(m.viewColumns(view({})), [], 'no columns');
	assertDeepEqual(m.viewColumns(view({ columns: 'a' })), [], 'not an array');
	assertDeepEqual(
		m.viewColumns(view({ columns: ['a', ' ', 3, null, 'b', ''] })),
		['a', 'b'],
		'filtered'
	);
	assertDeepEqual(m.viewColumns(view({ columns: [' a '] })), [' a '], 'items are not trimmed');
	assertDeepEqual(
		m.viewConfigStringArray(view({ tags: ['x', 1, 'y'] }), 'tags'),
		['x', 'y'],
		'custom key'
	);
	assertDeepEqual(m.viewConfigStringArray(view({}), 'tags'), [], 'missing key');
	assertDeepEqual(m.viewConfigStringArray(null, 'tags'), [], 'null view');
	assertDeepEqual(
		m.viewCardFieldKeys(view({ card_fields: ['a', 'b'] })),
		['a', 'b'],
		'card fields'
	);
	assertDeepEqual(m.viewCardFieldKeys(view({ columns: ['a'] })), [], 'card fields absent');
});

suite.check('orderedFieldsByKeys puts keyed fields first and appends the rest', () => {
	const [a, b, c] = [field('a'), field('b'), field('c')];
	const base = [a, b, c];
	assertEqual(m.orderedFieldsByKeys(base, []), base, 'no keys returns the same array');
	assertDeepEqual(
		m.orderedFieldsByKeys(base, ['c', 'a']).map((f) => f.key),
		['c', 'a', 'b'],
		'reordered'
	);
	assertDeepEqual(
		m.orderedFieldsByKeys(base, ['zz', 'b']).map((f) => f.key),
		['b', 'a', 'c'],
		'unknown keys skipped'
	);
	assertDeepEqual(
		m.orderedFieldsByKeys(base, ['zz']).map((f) => f.key),
		['a', 'b', 'c'],
		'only unknown keys'
	);
	assertDeepEqual(m.orderedFieldsByKeys([], ['a']), [], 'no fields');
	// CURRENT BEHAVIOUR: a key listed twice yields the field twice in the result.
	assertDeepEqual(
		m.orderedFieldsByKeys(base, ['a', 'a']).map((f) => f.key),
		['a', 'a', 'b', 'c'],
		'duplicate key'
	);
});

suite.check('viewConfigString, viewConfigBoolean and viewConfigRecord read typed members', () => {
	const v = view({ s: '  hi ', n: 5, t: true, f: 'true', r: { a: 1 }, arr: [1] });
	assertEqual(m.viewConfigString(v, 's'), 'hi', 'string trimmed');
	assertEqual(m.viewConfigString(v, 'n'), '', 'number');
	assertEqual(m.viewConfigString(null, 's'), '', 'null view');
	assertEqual(m.viewConfigBoolean(v, 't'), true, 'true');
	assertEqual(m.viewConfigBoolean(v, 'f'), false, 'string true is not true');
	assertEqual(m.viewConfigBoolean(v, 'missing'), false, 'missing');
	assertEqual(m.viewConfigBoolean(null, 't'), false, 'null view');
	assertDeepEqual(m.viewConfigRecord(v, 'r'), { a: 1 }, 'record');
	assertEqual(m.viewConfigRecord(v, 'arr'), null, 'array');
	assertEqual(m.viewConfigRecord(v, 's'), null, 'string');
	assertEqual(m.viewConfigRecord(null, 'r'), null, 'null view');
});

suite.check('isIsoDate checks the YYYY-MM-DD shape and that the day exists', () => {
	assertEqual(m.isIsoDate('2026-01-31'), true, 'valid');
	assertEqual(m.isIsoDate('2028-02-29'), true, 'leap day');
	assertEqual(m.isIsoDate('2026-1-1'), false, 'unpadded');
	assertEqual(m.isIsoDate(' 2026-01-01'), false, 'padded');
	assertEqual(m.isIsoDate('2026-01-01T00:00'), false, 'time part');
	assertEqual(m.isIsoDate(''), false, 'empty');
	assertEqual(m.isIsoDate('2026-02-30'), false, 'impossible date');
	assertEqual(m.isIsoDate('2026-02-31'), false, 'impossible date');
	assertEqual(m.isIsoDate('2026-02-29'), false, 'leap day in a common year');
	assertEqual(m.isIsoDate('2026-04-31'), false, 'day 31 of a 30-day month');
	assertEqual(m.isIsoDate('2026-13-01'), false, 'impossible month');
	assertEqual(m.isIsoDate('2026-13-45'), false, 'impossible month and day');
	assertEqual(m.isIsoDate('2026-00-10'), false, 'month zero');
	assertEqual(m.isIsoDate('2026-01-00'), false, 'day zero');
	assertEqual(m.isIsoDate('2100-02-29'), false, 'century non-leap year');
	assertEqual(m.isIsoDate('2000-02-29'), true, 'century leap year');
});

suite.check('calendarExceptionDateMap normalises, de-duplicates and sorts dates', () => {
	assertDeepEqual(m.calendarExceptionDateMap(null), {}, 'null');
	assertDeepEqual(m.calendarExceptionDateMap(['2026-01-01']), {}, 'array');
	assertDeepEqual(
		m.calendarExceptionDateMap({
			r1: ['2026-02-03', ' 2026-01-01 ', '2026-02-03', 'bad', 5],
			' ': ['2026-01-01'],
			r2: 'x',
			r3: [],
			r4: ['bad']
		}),
		{ r1: ['2026-01-01', '2026-02-03'] },
		'cleaned map'
	);
});

suite.check('viewIsDefault and viewVisibility read flags', () => {
	assertEqual(m.viewIsDefault(view({ is_default: true })), true, 'default');
	assertEqual(m.viewIsDefault(view({ is_default: 'true' })), false, 'string flag');
	assertEqual(m.viewIsDefault(null), false, 'null view');
	assertEqual(m.viewVisibility(view({ visibility: 'private' })), 'private', 'private');
	assertEqual(m.viewVisibility(view({ visibility: 'shared' })), 'shared', 'shared');
	assertEqual(m.viewVisibility(view({ visibility: 'other' })), 'shared', 'unknown');
	assertEqual(m.viewVisibility(null), 'shared', 'null view');
});

suite.check('sortViewsByDefault puts defaults first then orders by creation time', () => {
	const a = view({}, { id: 'a', created_at: '2026-03-01T00:00:00Z' });
	const b = view({ is_default: true }, { id: 'b', created_at: '2026-04-01T00:00:00Z' });
	const c = view({}, { id: 'c', created_at: '2026-01-01T00:00:00Z' });
	const input = [a, b, c];
	assertDeepEqual(
		m.sortViewsByDefault(input).map((v) => v.id),
		['b', 'c', 'a'],
		'order'
	);
	assertDeepEqual(
		input.map((v) => v.id),
		['a', 'b', 'c'],
		'input is not mutated'
	);
	assertDeepEqual(m.sortViewsByDefault([]), [], 'empty');
});

suite.check('normalizeLocalDefaultViews clears every other default flag', () => {
	const a = view({ is_default: true, keep: 1 }, { id: 'a', created_at: '2026-01-01T00:00:00Z' });
	const b = view({}, { id: 'b', created_at: '2026-02-01T00:00:00Z' });
	const c = view({ is_default: true }, { id: 'c', created_at: '2026-03-01T00:00:00Z' });
	const result = m.normalizeLocalDefaultViews([a, b, c], 'c');
	assertDeepEqual(
		result.map((v) => v.id),
		['c', 'a', 'b'],
		'new default first'
	);
	assertDeepEqual(
		result.map((v) => v.config.is_default),
		[true, false, false],
		'flags'
	);
	assertEqual(result[1]?.config.keep, 1, 'other config members are kept');
	assertEqual(a.config.is_default, true, 'input view is not mutated');
	assertDeepEqual(
		m.normalizeLocalDefaultViews([a, b], 'missing').map((v) => v.config.is_default),
		[false, false],
		'unknown default id clears all'
	);
});

suite.check('enumerated readers map known values and default otherwise', () => {
	const tables: Array<[string, (value: unknown) => unknown, Array<[unknown, unknown]>]> = [
		[
			'viewFilterOperator',
			m.viewFilterOperator,
			[
				['equals', 'equals'],
				['not_equals', 'not_equals'],
				['not_empty', 'not_empty'],
				['contains', 'contains'],
				['bogus', 'contains'],
				[undefined, 'contains'],
				[5, 'contains']
			]
		],
		[
			'viewFilterLogic',
			m.viewFilterLogic,
			[
				['any', 'any'],
				['or', 'any'],
				['all', 'all'],
				['and', 'all'],
				[null, 'all']
			]
		],
		[
			'viewSortDirection',
			m.viewSortDirection,
			[
				['desc', 'desc'],
				['asc', 'asc'],
				['DESC', 'asc'],
				[undefined, 'asc']
			]
		],
		[
			'viewCalendarRange',
			m.viewCalendarRange,
			[
				['month', 'month'],
				['week', 'week'],
				['year', 'week']
			]
		],
		[
			'viewCalendarDayScope',
			m.viewCalendarDayScope,
			[
				['workweek', 'workweek'],
				['all', 'all'],
				['x', 'all']
			]
		],
		[
			'viewCalendarLayout',
			m.viewCalendarLayout,
			[
				['agenda', 'agenda'],
				['grid', 'grid'],
				['x', 'grid']
			]
		],
		[
			'viewGanttScale',
			m.viewGanttScale,
			[
				['week', 'week'],
				['month', 'month'],
				['day', 'day'],
				['year', 'day']
			]
		],
		[
			'viewPivotAggregate',
			m.viewPivotAggregate,
			[
				['count', 'count'],
				['avg', 'avg'],
				['sum', 'sum'],
				['max', 'sum']
			]
		],
		[
			'viewPivotTotalsMode',
			m.viewPivotTotalsMode,
			[
				['rows', 'rows'],
				['columns', 'columns'],
				['none', 'none'],
				['all', 'all'],
				['x', 'all']
			]
		],
		[
			'viewPivotValueDisplay',
			m.viewPivotValueDisplay,
			[
				['percent_row', 'percent_row'],
				['percent_column', 'percent_column'],
				['percent_total', 'percent_total'],
				['value', 'value'],
				['x', 'value']
			]
		],
		[
			'viewPivotSortMode',
			m.viewPivotSortMode,
			[
				['row_total_desc', 'row_total_desc'],
				['row_total_asc', 'row_total_asc'],
				['column_total_desc', 'column_total_desc'],
				['column_total_asc', 'column_total_asc'],
				['label', 'label'],
				['x', 'label']
			]
		],
		[
			'viewCalendarRecurrence',
			m.viewCalendarRecurrence,
			[
				['daily', 'daily'],
				['weekly', 'weekly'],
				['monthly', 'monthly'],
				['none', 'none'],
				['yearly', 'none']
			]
		],
		[
			'viewCardLayout',
			m.viewCardLayout,
			[
				['list', 'list'],
				['compact', 'compact'],
				['gallery', 'gallery'],
				['standard', 'standard'],
				['x', 'standard']
			]
		],
		[
			'viewCardCoverAspect',
			m.viewCardCoverAspect,
			[
				['wide', 'wide'],
				['square', 'square'],
				['portrait', 'portrait'],
				['auto', 'auto'],
				['x', 'auto']
			]
		],
		[
			'viewCardCoverFit',
			m.viewCardCoverFit,
			[
				['contain', 'contain'],
				['cover', 'cover'],
				['x', 'cover']
			]
		],
		[
			'viewCardCoverPosition',
			m.viewCardCoverPosition,
			[
				['left', 'left'],
				['hidden', 'hidden'],
				['top', 'top'],
				['x', 'top']
			]
		],
		[
			'viewTimelineSource',
			m.viewTimelineSource,
			[
				['events', 'events'],
				['records', 'records'],
				['x', 'records']
			]
		]
	];
	for (const [name, reader, rows] of tables) {
		for (const [input, expected] of rows) {
			assertEqual(reader(input), expected, `${name}(${JSON.stringify(input)})`);
		}
	}
});

suite.check('numeric limit readers accept the allowed numbers and numeric text', () => {
	for (const reader of [m.viewPivotRowLimit, m.viewPivotColumnLimit]) {
		for (const allowed of [2, 5, 10]) {
			assertEqual(reader(allowed), allowed, `number ${allowed}`);
			assertEqual(reader(String(allowed)), allowed, `text ${allowed}`);
		}
		for (const rejected of [0, 1, 3, 11, -5, '3', 'abc', '', null, undefined, Number.NaN, {}]) {
			assertEqual(reader(rejected), 0, `rejected ${JSON.stringify(rejected)}`);
		}
	}
	for (const allowed of [3, 9, 99]) {
		assertEqual(m.viewCardFieldLimit(allowed), allowed, `card limit ${allowed}`);
		assertEqual(m.viewCardFieldLimit(String(allowed)), allowed, `card limit text ${allowed}`);
	}
	for (const rejected of [6, 0, 4, 100, 'abc', null, undefined]) {
		assertEqual(m.viewCardFieldLimit(rejected), 6, `card default for ${JSON.stringify(rejected)}`);
	}
});

suite.check('card cover class helpers compose Tailwind classes', () => {
	assertEqual(m.cardCoverObjectClass('cover'), 'object-cover', 'cover');
	assertEqual(m.cardCoverObjectClass('contain'), 'bg-surface object-contain', 'contain');
	const table: Array<[Parameters<typeof m.cardCoverImageClass>, string]> = [
		[['gallery', 'auto', 'cover'], 'aspect-[4/3] w-full object-cover'],
		[['standard', 'auto', 'cover'], 'aspect-[16/9] w-full object-cover'],
		[['list', 'auto', 'contain'], 'aspect-[16/9] w-full bg-surface object-contain'],
		[['compact', 'auto', 'cover'], 'aspect-[16/9] w-full object-cover'],
		[['gallery', 'square', 'cover'], 'aspect-square w-full object-cover'],
		[['standard', 'portrait', 'contain'], 'aspect-[3/4] w-full bg-surface object-contain'],
		[['gallery', 'wide', 'cover'], 'aspect-[16/9] w-full object-cover']
	];
	for (const [args, expected] of table) {
		assertEqual(m.cardCoverImageClass(...args), expected, `classes for ${args.join('/')}`);
	}
});

suite.check('viewFilterNodeDisabled honours disabled and enabled flags', () => {
	assertEqual(m.viewFilterNodeDisabled(null), false, 'null');
	assertEqual(m.viewFilterNodeDisabled({}), false, 'empty');
	assertEqual(m.viewFilterNodeDisabled({ disabled: true }), true, 'disabled');
	assertEqual(m.viewFilterNodeDisabled({ enabled: false }), true, 'enabled false');
	assertEqual(m.viewFilterNodeDisabled({ enabled: true }), false, 'enabled true');
	assertEqual(m.viewFilterNodeDisabled({ disabled: 'true' }), false, 'string flag');
	assertEqual(m.viewFilterNodeDisabled({ enabled: 0 }), false, 'falsy but not false');
});

suite.check('kanbanMoveOptions lists selectable single select options', () => {
	assertDeepEqual(m.kanbanMoveOptions(null), [], 'null field');
	assertDeepEqual(m.kanbanMoveOptions(field('a', { options: ['x'] })), [], 'not a single select');
	assertDeepEqual(m.kanbanMoveOptions(field('a', { type: 'single_select' })), [], 'no options');
	assertDeepEqual(
		m.kanbanMoveOptions(
			field('a', {
				type: 'single_select',
				options: ['todo', ' ', 'doing', 'old'],
				option_archived: ['old']
			})
		),
		['todo', 'doing'],
		'blank and archived options removed'
	);
});

suite.check('builtInView lists the list-visible fields as a grid', () => {
	const form: UniversalForm = {
		id: 'form-9',
		workspace_id: 'ws-9',
		project_id: 'pr-9',
		key: 'orders',
		name: 'Orders',
		description: '',
		title_template: '{id}',
		schema: {
			fields: [
				{ key: 'a' },
				{ key: 'b', visibility: { list: false } },
				{ key: 'c', visibility: { list: true } },
				{ label: 'no key' }
			]
		},
		detail_layout: {},
		schema_version: 3,
		created_at: '2026-05-01T00:00:00Z',
		updated_at: '2026-05-02T00:00:00Z'
	};
	assertDeepEqual(
		m.builtInView(form),
		{
			id: 'all',
			workspace_id: 'ws-9',
			project_id: 'pr-9',
			form_id: 'form-9',
			key: 'all',
			name: 'forms.views.allRecords',
			view_type: 'grid',
			config: { columns: ['a', 'c'] },
			created_at: '2026-05-01T00:00:00Z',
			updated_at: '2026-05-02T00:00:00Z'
		},
		'built-in view'
	);
	assertDeepEqual(m.builtInView({ ...form, schema: {} }).config, { columns: [] }, 'no fields');
});

finish(suite.result());
