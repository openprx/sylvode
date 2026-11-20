/**
 * Saved view to draft filter groups, summaries and labels (`src/lib/forms/view-draft-parse.ts`).
 *
 * Run standalone: `bun tests/forms-page/view-draft-parse.test.ts`
 */
import { Suite, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormRecord, FormView } from '../../src/lib/api/forms';

await installSvelteSsr();
const m = await import('../../src/lib/forms/view-draft-parse');

const suite = new Suite('forms-page/view-draft-parse');

function view(config: Record<string, unknown>): FormView {
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
		updated_at: '2026-01-01T00:00:00Z'
	};
}

function record(id: string): FormRecord {
	return {
		id,
		workspace_id: 'ws',
		project_id: 'pr',
		form_id: 'form-1',
		title: id,
		values: {},
		source: {},
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z'
	};
}

suite.check('viewDraftFilterExpressionRecordIdsText lists record ids one per line', () => {
	assertEqual(m.viewDraftFilterExpressionRecordIdsText(null), '', 'null');
	assertEqual(m.viewDraftFilterExpressionRecordIdsText([]), '', 'empty');
	assertEqual(m.viewDraftFilterExpressionRecordIdsText([record('r1')]), 'r1', 'one');
	assertEqual(
		m.viewDraftFilterExpressionRecordIdsText([record('r1'), record('r2')]),
		'r1\nr2',
		'two'
	);
});

suite.check('filterOperatorSummary words every operator', () => {
	assertEqual(m.filterOperatorSummary('equals'), 'equals', 'equals');
	assertEqual(m.filterOperatorSummary('not_equals'), 'does not equal', 'not_equals');
	assertEqual(m.filterOperatorSummary('not_empty'), 'is not empty', 'not_empty');
	assertEqual(m.filterOperatorSummary('contains'), 'contains', 'contains');
});

suite.check('parseViewFilterDrafts converts valid raw filters', () => {
	assertDeepEqual(
		m.parseViewFilterDrafts([
			{ field: 'a', operator: 'equals', value: 'x' },
			{ field: 'b' },
			{ field: 'c', operator: 'bogus', value: 5 },
			{ field: 'd', operator: 'not_empty', disabled: true },
			{ field: 'e', enabled: false }
		]),
		[
			{ field: 'a', operator: 'equals', value: 'x' },
			{ field: 'b', operator: 'contains', value: '' },
			{ field: 'c', operator: 'contains', value: '' },
			{ field: 'd', operator: 'not_empty', value: '', disabled: true },
			{ field: 'e', operator: 'contains', value: '', disabled: true }
		],
		'defaults, coercions and disabled flags'
	);
});

suite.check('parseViewFilterDrafts drops malformed entries and caps the list at five', () => {
	assertDeepEqual(m.parseViewFilterDrafts([]), [], 'empty');
	assertDeepEqual(
		m.parseViewFilterDrafts([null, 'x', 3, ['a'], { value: 'v' }, { field: 5 }]),
		[],
		'malformed'
	);
	const many = Array.from({ length: 8 }, (_, index) => ({ field: `f${index}` }));
	assertDeepEqual(
		m.parseViewFilterDrafts(many).map((filter) => filter.field),
		['f0', 'f1', 'f2', 'f3', 'f4'],
		'first five'
	);
	const mixed = [null, ...many];
	assertEqual(
		m.parseViewFilterDrafts(mixed).length,
		5,
		'invalid entries do not count towards the cap'
	);
});

suite.check('viewDraftExpressionFromView returns the stored expression object', () => {
	assertDeepEqual(
		m.viewDraftExpressionFromView(view({ filter_expression: { logic: 'any' } })),
		{ logic: 'any' },
		'object'
	);
	assertEqual(m.viewDraftExpressionFromView(view({ filter_expression: [] })), null, 'array');
	assertEqual(m.viewDraftExpressionFromView(view({ filter_expression: 'x' })), null, 'string');
	assertEqual(m.viewDraftExpressionFromView(view({})), null, 'absent');
});

suite.check(
	'viewDraftFilterGroupsFromExpression reads leaf children as single-filter groups',
	() => {
		assertDeepEqual(
			m.viewDraftFilterGroupsFromExpression({
				children: [
					{ kind: 'condition', field: 'a', operator: 'equals', value: 'x' },
					{ filter: { field: 'b', value: 'y' }, disabled: true },
					{ filter: { field: 'c', operator: 'not_empty' }, enabled: false },
					{ field: 'd', disabled: true }
				]
			}),
			[
				{ logic: 'all', filters: [{ field: 'a', operator: 'equals', value: 'x' }] },
				{
					logic: 'all',
					filters: [{ field: 'b', operator: 'contains', value: 'y', disabled: true }],
					disabled: true
				},
				{
					logic: 'all',
					filters: [{ field: 'c', operator: 'not_empty', value: '', disabled: true }],
					disabled: true
				},
				{
					logic: 'all',
					filters: [{ field: 'd', operator: 'contains', value: '', disabled: true }],
					disabled: true
				}
			],
			'leaf children'
		);
	}
);

suite.check('viewDraftFilterGroupsFromExpression reads nested groups with their logic', () => {
	assertDeepEqual(
		m.viewDraftFilterGroupsFromExpression({
			children: [
				{
					logic: 'or',
					children: [
						{ field: 'a', value: '1' },
						{ filter: { field: 'b', operator: 'not_empty' }, enabled: false },
						{ filter: { field: 'c', value: '3', disabled: true }, disabled: true },
						'junk',
						null
					]
				},
				{
					logic: 'and',
					disabled: true,
					children: [{ field: 'd', operator: 'equals', value: '4' }]
				},
				{ logic: 'any', children: [{ value: 'no field' }] },
				{ logic: 'any' }
			]
		}),
		[
			{
				logic: 'any',
				filters: [
					{ field: 'a', operator: 'contains', value: '1' },
					{ field: 'b', operator: 'not_empty', value: '', disabled: true },
					{ field: 'c', operator: 'contains', value: '3', disabled: true }
				]
			},
			{ logic: 'all', filters: [{ field: 'd', operator: 'equals', value: '4' }], disabled: true }
		],
		'nested groups, groups without usable filters are dropped'
	);
});

suite.check(
	'viewDraftFilterGroupsFromExpression accepts the expressions alias and the five group cap',
	() => {
		assertDeepEqual(
			m.viewDraftFilterGroupsFromExpression({ expressions: [{ field: 'a', value: 'x' }] }),
			[{ logic: 'all', filters: [{ field: 'a', operator: 'contains', value: 'x' }] }],
			'expressions alias'
		);
		const children = Array.from({ length: 7 }, (_, index) => ({ field: `f${index}` }));
		assertEqual(
			m.viewDraftFilterGroupsFromExpression({ children }).length,
			5,
			'capped at five groups'
		);
	}
);

suite.check(
	'viewDraftFilterGroupsFromExpression falls back to the expression itself as a filter',
	() => {
		assertDeepEqual(
			m.viewDraftFilterGroupsFromExpression({ field: 'a', operator: 'equals', value: 'x' }),
			[{ logic: 'all', filters: [{ field: 'a', operator: 'equals', value: 'x' }] }],
			'expression is a filter'
		);
		assertDeepEqual(
			m.viewDraftFilterGroupsFromExpression({ children: [], filter: { field: 'a', value: 'x' } }),
			[{ logic: 'all', filters: [{ field: 'a', operator: 'contains', value: 'x' }] }],
			'nested filter member with empty children'
		);
		assertDeepEqual(m.viewDraftFilterGroupsFromExpression({}), [], 'empty expression');
		assertDeepEqual(
			m.viewDraftFilterGroupsFromExpression({ children: [{ filter: {} }] }),
			[],
			'leaf without a field'
		);
	}
);

suite.check('viewDraftFilterRootLogicFromView reads the expression logic', () => {
	assertEqual(
		m.viewDraftFilterRootLogicFromView(view({ filter_expression: { logic: 'or' } })),
		'any',
		'or'
	);
	assertEqual(
		m.viewDraftFilterRootLogicFromView(view({ filter_expression: { logic: 'any' } })),
		'any',
		'any'
	);
	assertEqual(
		m.viewDraftFilterRootLogicFromView(view({ filter_expression: { logic: 'all' } })),
		'all',
		'all'
	);
	assertEqual(
		m.viewDraftFilterRootLogicFromView(view({ filter_expression: {} })),
		'all',
		'no logic'
	);
	assertEqual(m.viewDraftFilterRootLogicFromView(view({})), 'all', 'no expression');
});

suite.check('viewDraftWipLimitsFromView keeps non-negative limits as whole-number text', () => {
	assertDeepEqual(
		m.viewDraftWipLimitsFromView(
			view({
				wip_limits: { todo: 3, doing: '2.7', done: -1, bad: 'abc', big: 1e3, nan: Number.NaN }
			})
		),
		{ todo: '3', doing: '2', big: '1000' },
		'valid limits'
	);
	assertDeepEqual(m.viewDraftWipLimitsFromView(view({})), {}, 'absent');
	assertDeepEqual(m.viewDraftWipLimitsFromView(view({ wip_limits: [1] })), {}, 'array');
	// CURRENT BEHAVIOUR: null and the empty string are coerced with Number(), so they become a
	// limit of "0" instead of being ignored.
	assertDeepEqual(
		m.viewDraftWipLimitsFromView(view({ wip_limits: { a: null, b: '' } })),
		{ a: '0', b: '0' },
		'null and empty text'
	);
});

suite.check('viewDraftCalendarExceptionDatesFromView cleans the stored map', () => {
	assertDeepEqual(
		m.viewDraftCalendarExceptionDatesFromView(
			view({ calendar_exception_dates: { r1: ['2026-02-02', '2026-01-01', 'bad'] } })
		),
		{ r1: ['2026-01-01', '2026-02-02'] },
		'sorted valid dates'
	);
	assertDeepEqual(m.viewDraftCalendarExceptionDatesFromView(view({})), {}, 'absent');
});

suite.check('viewDraftFilterGroupsFromView prefers the expression', () => {
	assertDeepEqual(
		m.viewDraftFilterGroupsFromView(
			view({
				filter_expression: { children: [{ field: 'e', value: '1' }] },
				filter_groups: [{ filters: [{ field: 'g' }] }],
				filters: [{ field: 'f' }],
				filter: { field: 'l' }
			})
		),
		[{ logic: 'all', filters: [{ field: 'e', operator: 'contains', value: '1' }] }],
		'expression first'
	);
});

suite.check(
	'viewDraftFilterGroupsFromView falls through when the expression yields nothing',
	() => {
		assertDeepEqual(
			m.viewDraftFilterGroupsFromView(
				view({
					filter_expression: { children: [] },
					filter_groups: [{ filters: [{ field: 'g' }] }]
				})
			),
			[{ logic: 'all', filters: [{ field: 'g', operator: 'contains', value: '' }] }],
			'filter_groups after an empty expression'
		);
	}
);

suite.check('viewDraftFilterGroupsFromView reads filter_groups', () => {
	const groups = Array.from({ length: 7 }, (_, index) => ({ filters: [{ field: `f${index}` }] }));
	assertEqual(
		m.viewDraftFilterGroupsFromView(view({ filter_groups: groups })).length,
		5,
		'capped at five'
	);
	assertDeepEqual(
		m.viewDraftFilterGroupsFromView(
			view({
				filter_groups: [
					{
						logic: 'or',
						filters: [{ field: 'a', operator: 'equals', value: 'x' }, { field: 'b' }],
						disabled: true
					},
					{ logic: 'any', filters: [] },
					{ logic: 'any' },
					'junk',
					{ filters: [{ value: 'no field' }] },
					{ enabled: false, filters: [{ field: 'c' }] }
				],
				filters: [{ field: 'ignored' }]
			})
		),
		[
			{
				logic: 'any',
				filters: [
					{ field: 'a', operator: 'equals', value: 'x' },
					{ field: 'b', operator: 'contains', value: '' }
				],
				disabled: true
			},
			{ logic: 'all', filters: [{ field: 'c', operator: 'contains', value: '' }], disabled: true }
		],
		'valid groups only'
	);
});

suite.check(
	'viewDraftFilterGroupsFromView reads the flat filter list and the legacy filter',
	() => {
		assertDeepEqual(
			m.viewDraftFilterGroupsFromView(
				view({ filters: [{ field: 'a', operator: 'not_equals', value: 'x' }, { field: 'b' }] })
			),
			[
				{
					logic: 'all',
					filters: [
						{ field: 'a', operator: 'not_equals', value: 'x' },
						{ field: 'b', operator: 'contains', value: '' }
					]
				}
			],
			'flat list'
		);
		assertDeepEqual(
			m.viewDraftFilterGroupsFromView(
				view({ filter: { field: 'a', operator: 'equals', value: 'x' } })
			),
			[{ logic: 'all', filters: [{ field: 'a', operator: 'equals', value: 'x' }] }],
			'legacy filter'
		);
		assertDeepEqual(
			m.viewDraftFilterGroupsFromView(view({ filter: { field: 'a', value: 7, disabled: true } })),
			[
				{ logic: 'all', filters: [{ field: 'a', operator: 'contains', value: '', disabled: true }] }
			],
			'legacy filter with disabled flag and non-string value'
		);
		assertDeepEqual(
			m.viewDraftFilterGroupsFromView(view({ filter: { value: 'x' } })),
			[],
			'legacy filter without a field'
		);
		assertDeepEqual(
			m.viewDraftFilterGroupsFromView(view({ filter: 'x' })),
			[],
			'legacy filter of the wrong type'
		);
		assertDeepEqual(m.viewDraftFilterGroupsFromView(view({})), [], 'no filters at all');
		assertDeepEqual(
			m
				.viewDraftFilterGroupsFromView(
					view({ filters: [{ value: 'no field' }], filter: { field: 'l' } })
				)
				.map((g) => g.filters[0]?.field),
			['l'],
			'legacy after an unusable flat list'
		);
	}
);

suite.check('syncLegacyViewFilterFields mirrors the first filter', () => {
	assertDeepEqual(
		m.syncLegacyViewFilterFields([]),
		{ filters: [], filterField: '', filterOperator: 'contains', filterValue: '' },
		'no groups'
	);
	assertDeepEqual(
		m.syncLegacyViewFilterFields([{ logic: 'all', filters: [] }]),
		{ filters: [], filterField: '', filterOperator: 'contains', filterValue: '' },
		'empty first group'
	);
	const first = [
		{ field: 'a', operator: 'equals' as const, value: 'x' },
		{ field: 'b', operator: 'not_empty' as const, value: '' }
	];
	assertDeepEqual(
		m.syncLegacyViewFilterFields([
			{ logic: 'all', filters: first },
			{ logic: 'any', filters: [{ field: 'z', operator: 'contains', value: 'q' }] }
		]),
		{ filters: first, filterField: 'a', filterOperator: 'equals', filterValue: 'x' },
		'first group, first filter'
	);
});

suite.check('filterLabelSuffix numbers filters within groups', () => {
	assertEqual(m.filterLabelSuffix(0, 0), '1', 'first group first filter');
	assertEqual(m.filterLabelSuffix(0, 2), '3', 'first group third filter');
	assertEqual(m.filterLabelSuffix(1, 0), '2.1', 'second group');
	assertEqual(m.filterLabelSuffix(2, 3), '3.4', 'third group fourth filter');
});

finish(suite.result());
