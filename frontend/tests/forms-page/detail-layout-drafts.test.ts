/**
 * Detail page widget and section helpers of the forms page designer.
 *
 * Run standalone: `bun tests/forms-page/detail-layout-drafts.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormField, FormRecord } from '../../src/lib/api/forms';
import type {
	DetailLayoutSection,
	DetailPageWidgetEntry,
	DetailPageWidgetKey
} from '../../src/lib/forms/page-types';

await installSvelteSsr();
const drafts = await import('../../src/lib/forms/detail-layout-drafts');

const suite = new Suite('forms-page/detail-layout-drafts');

const DEFAULT_ORDER = ['children', 'links', 'comments', 'attachments', 'events'];

function defaults(key: DetailPageWidgetKey): DetailPageWidgetEntry {
	return {
		key,
		enabled: true,
		title: '',
		description: '',
		emptyText: '',
		showCount: false,
		hideEmpty: false,
		collapsible: false,
		defaultCollapsed: false,
		density: 'comfortable',
		limit: ''
	};
}

function formField(key: string, type: FormField['type']): FormField {
	return { key, label: key, type };
}

function section(overrides: Partial<DetailLayoutSection> = {}): DetailLayoutSection {
	return {
		key: 's',
		title: 'S',
		description: '',
		columns: 2,
		density: 'comfortable',
		hideEmptyFields: false,
		collapsible: false,
		defaultCollapsed: false,
		fields: [],
		...overrides
	};
}

function recordWith(values: Record<string, unknown>): FormRecord {
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

suite.check('detailPageWidgetKey accepts the five known widget keys only', () => {
	for (const key of DEFAULT_ORDER) assertEqual(drafts.detailPageWidgetKey(key), key, key);
	for (const value of ['', 'Children', 'unknown', ' links', null, undefined, 3, {}, ['links']]) {
		assertEqual(drafts.detailPageWidgetKey(value), null, `${JSON.stringify(value)} is rejected`);
	}
});

suite.check(
	'detailPageWidgetEntriesFromLayout returns the five defaults for an empty layout',
	() => {
		for (const layout of [null, undefined, {}, { widgets: 'children' }, { widgets: [] }]) {
			const entries = drafts.detailPageWidgetEntriesFromLayout(layout);
			assertDeepEqual(
				entries.map((entry) => entry.key),
				DEFAULT_ORDER,
				`order for ${JSON.stringify(layout)}`
			);
			assertDeepEqual(entries[0], defaults('children'), 'default entry values');
		}
	}
);

suite.check(
	'detailPageWidgetEntriesFromLayout reads configured widgets first, then the rest',
	() => {
		const entries = drafts.detailPageWidgetEntriesFromLayout({
			widgets: [
				'comments',
				{
					key: 'links',
					enabled: false,
					title: 'Related',
					description: 'Linked items',
					empty_text: 'Nothing linked',
					show_count: true,
					hide_empty: true,
					collapsible: true,
					default_collapsed: true,
					density: 'compact',
					limit: 12
				}
			]
		});
		assertDeepEqual(
			entries.map((entry) => entry.key),
			['comments', 'links', 'children', 'attachments', 'events'],
			'configured first, remaining in option order'
		);
		assertDeepEqual(entries[0], defaults('comments'), 'a bare string key gets the defaults');
		assertDeepEqual(
			entries[1],
			{
				key: 'links',
				enabled: false,
				title: 'Related',
				description: 'Linked items',
				emptyText: 'Nothing linked',
				showCount: true,
				hideEmpty: true,
				collapsible: true,
				defaultCollapsed: true,
				density: 'compact',
				limit: '12'
			},
			'a full object is read field by field'
		);
	}
);

suite.check('detailPageWidgetEntriesFromLayout skips duplicates and unknown keys', () => {
	const entries = drafts.detailPageWidgetEntriesFromLayout({
		widgets: [
			{ key: 'events', title: 'first' },
			{ key: 'events', title: 'second' },
			'events',
			{ key: 'bogus' },
			'bogus',
			42,
			null,
			{ title: 'no key' }
		]
	});
	assertEqual(entries.length, 5, 'still one entry per known key');
	assertEqual(entries[0].key, 'events', 'events first');
	assertEqual(entries[0].title, 'first', 'the first occurrence wins');
});

suite.check(
	'detailPageWidgetEntriesFromLayout applies the flag and type rules per property',
	() => {
		const [entry] = drafts.detailPageWidgetEntriesFromLayout({
			widgets: [
				{
					key: 'children',
					enabled: 'no',
					title: 5,
					description: null,
					empty_text: false,
					show_count: 'true',
					hide_empty: 1,
					collapsible: false,
					default_collapsed: true,
					density: 'dense',
					limit: 'many'
				}
			]
		});
		assertEqual(entry.enabled, true, 'enabled is true unless it is exactly false');
		assertEqual(entry.title, '', 'non-string title');
		assertEqual(entry.description, '', 'non-string description');
		assertEqual(entry.emptyText, '', 'non-string empty text');
		assertEqual(entry.showCount, false, 'show_count must be exactly true');
		assertEqual(entry.hideEmpty, false, 'hide_empty must be exactly true');
		assertEqual(entry.collapsible, false, 'collapsible false');
		assertEqual(entry.defaultCollapsed, false, 'default_collapsed ignored when not collapsible');
		assertEqual(entry.density, 'comfortable', 'unknown density');
		assertEqual(entry.limit, '', 'non-numeric limit');
		assertEqual(
			drafts.detailPageWidgetEntriesFromLayout({
				widgets: [{ key: 'children', enabled: false }]
			})[0].enabled,
			false,
			'enabled false is kept'
		);
		assertEqual(
			drafts.detailPageWidgetEntriesFromLayout({
				widgets: [{ key: 'children', collapsible: true }]
			})[0].defaultCollapsed,
			false,
			'collapsible without default_collapsed'
		);
	}
);

suite.check('detailPageWidgetOption finds an option and falls back to the first one', () => {
	assertEqual(
		drafts.detailPageWidgetOption('comments').labelKey,
		'forms.detailPageWidgetComments',
		'comments'
	);
	assertEqual(
		drafts.detailPageWidgetOption('events').descriptionKey,
		'forms.detailPageWidgetEventsDescription',
		'events description'
	);
	// The cast passes a key that is not in the union on purpose.
	assertEqual(
		drafts.detailPageWidgetOption('nope' as DetailPageWidgetKey).key,
		'children',
		'unknown key'
	);
});

suite.check('detailPageWidgetPayload writes only the non-default properties', () => {
	assertDeepEqual(
		drafts.detailPageWidgetPayload([
			defaults('children'),
			{ ...defaults('links'), enabled: false }
		]),
		[
			{ key: 'children', enabled: true },
			{ key: 'links', enabled: false }
		],
		'defaults produce a minimal payload'
	);
	assertDeepEqual(
		drafts.detailPageWidgetPayload([
			{
				key: 'comments',
				enabled: true,
				title: '  Notes  ',
				description: ' About ',
				emptyText: ' None ',
				showCount: true,
				hideEmpty: true,
				collapsible: true,
				defaultCollapsed: true,
				density: 'compact',
				limit: ' 25 '
			}
		]),
		[
			{
				key: 'comments',
				enabled: true,
				title: 'Notes',
				description: 'About',
				empty_text: 'None',
				show_count: true,
				hide_empty: true,
				collapsible: true,
				default_collapsed: true,
				density: 'compact',
				limit: 25
			}
		],
		'full entry, text trimmed, limit as a number'
	);
	assertDeepEqual(
		drafts.detailPageWidgetPayload([
			{ ...defaults('events'), title: '   ', limit: '0', defaultCollapsed: true }
		]),
		[{ key: 'events', enabled: true }],
		'blank text, out-of-range limit, and default_collapsed without collapsible are dropped'
	);
	assertDeepEqual(drafts.detailPageWidgetPayload([]), [], 'no entries');
});

suite.check('detailPageWidgetPayload and detailPageWidgetEntriesFromLayout round-trip', () => {
	const entries: DetailPageWidgetEntry[] = [
		{
			...defaults('attachments'),
			title: 'Files',
			collapsible: true,
			defaultCollapsed: true,
			limit: '8'
		},
		{ ...defaults('children'), enabled: false, density: 'compact', showCount: true },
		defaults('comments'),
		defaults('events'),
		defaults('links')
	];
	const back = drafts.detailPageWidgetEntriesFromLayout({
		widgets: drafts.detailPageWidgetPayload(entries)
	});
	assertDeepEqual(back, entries, 'entries survive a payload round trip');
});

suite.check('detailPageWidgetLimitNumber accepts 1 to 50 and truncates fractions', () => {
	assertEqual(drafts.detailPageWidgetLimitNumber(1), 1, 'lower bound');
	assertEqual(drafts.detailPageWidgetLimitNumber(50), 50, 'upper bound');
	assertEqual(drafts.detailPageWidgetLimitNumber(0), null, 'below the lower bound');
	assertEqual(drafts.detailPageWidgetLimitNumber(51), null, 'above the upper bound');
	assertEqual(drafts.detailPageWidgetLimitNumber(-3), null, 'negative');
	assertEqual(drafts.detailPageWidgetLimitNumber(7.9), 7, 'fraction truncated');
	assertEqual(drafts.detailPageWidgetLimitNumber(0.9), null, 'fraction truncated to zero');
	assertEqual(
		drafts.detailPageWidgetLimitNumber(50.9),
		50,
		'fraction above 50 truncates into range'
	);
	assertEqual(drafts.detailPageWidgetLimitNumber('7'), 7, 'numeric string');
	assertEqual(drafts.detailPageWidgetLimitNumber(' 12 '), 12, 'padded numeric string');
	assertEqual(drafts.detailPageWidgetLimitNumber(''), null, 'empty string');
	assertEqual(drafts.detailPageWidgetLimitNumber('abc'), null, 'text');
	assertEqual(drafts.detailPageWidgetLimitNumber(NaN), null, 'NaN');
	assertEqual(drafts.detailPageWidgetLimitNumber(Infinity), null, 'Infinity');
	assertEqual(drafts.detailPageWidgetLimitNumber(null), null, 'null');
	assertEqual(drafts.detailPageWidgetLimitNumber(undefined), null, 'undefined');
	assertEqual(drafts.detailPageWidgetLimitNumber({}), null, 'object');
	assertEqual(drafts.detailPageWidgetLimitNumber(true), null, 'boolean');
});

suite.check('detailPageWidgetLimitText is the limit as text or an empty string', () => {
	assertEqual(drafts.detailPageWidgetLimitText(12), '12', 'number');
	assertEqual(drafts.detailPageWidgetLimitText('30'), '30', 'string');
	assertEqual(drafts.detailPageWidgetLimitText(0), '', 'out of range');
	assertEqual(drafts.detailPageWidgetLimitText(99), '', 'out of range high');
	assertEqual(drafts.detailPageWidgetLimitText(undefined), '', 'undefined');
});

suite.check('detailPageWidgetRegion is below or sidebar', () => {
	assertEqual(drafts.detailPageWidgetRegion('below'), 'below', 'below');
	assertEqual(drafts.detailPageWidgetRegion('sidebar'), 'sidebar', 'sidebar');
	assertEqual(drafts.detailPageWidgetRegion('Below'), 'sidebar', 'case matters');
	assertEqual(drafts.detailPageWidgetRegion(undefined), 'sidebar', 'undefined');
	assertEqual(drafts.detailPageWidgetRegion(null), 'sidebar', 'null');
});

suite.check('detailPageFieldColumns is 1 only for the number or string 1', () => {
	assertEqual(drafts.detailPageFieldColumns(1), 1, 'number 1');
	assertEqual(drafts.detailPageFieldColumns('1'), 1, 'string 1');
	assertEqual(drafts.detailPageFieldColumns(2), 2, 'number 2');
	assertEqual(drafts.detailPageFieldColumns('2'), 2, 'string 2');
	assertEqual(drafts.detailPageFieldColumns(3), 2, 'three is not offered');
	assertEqual(drafts.detailPageFieldColumns(0), 2, 'zero');
	assertEqual(drafts.detailPageFieldColumns(' 1 '), 1, 'padded string');
	assertEqual(drafts.detailPageFieldColumns(null), 2, 'null');
	assertEqual(drafts.detailPageFieldColumns(undefined), 2, 'undefined');
	assertEqual(drafts.detailPageFieldColumns('x'), 2, 'text');
});

suite.check('detailLayoutSectionDescription trims strings and drops other types', () => {
	assertEqual(drafts.detailLayoutSectionDescription('  About  '), 'About', 'string');
	assertEqual(drafts.detailLayoutSectionDescription('   '), '', 'blank');
	assertEqual(drafts.detailLayoutSectionDescription(5), '', 'number');
	assertEqual(drafts.detailLayoutSectionDescription(null), '', 'null');
	assertEqual(drafts.detailLayoutSectionDescription(undefined), '', 'undefined');
});

suite.check('keepDetailSectionOpen re-opens a closed, non-collapsible details element', () => {
	// The test runs without a DOM: a stand-in class is installed as HTMLDetailsElement for the
	// duration of the checks, and events are dispatched on a real EventTarget so that
	// `currentTarget` is set the way a browser sets it.
	class FakeDetails extends EventTarget {
		open = false;
	}
	const globalWithDom = globalThis as unknown as { HTMLDetailsElement?: unknown };
	const original = globalWithDom.HTMLDetailsElement;
	globalWithDom.HTMLDetailsElement = FakeDetails;
	try {
		function run(details: EventTarget, collapsible: boolean): void {
			details.addEventListener(
				'toggle',
				(event) => drafts.keepDetailSectionOpen(event, collapsible),
				{
					once: true
				}
			);
			details.dispatchEvent(new Event('toggle'));
		}

		const closed = new FakeDetails();
		run(closed, false);
		assertEqual(closed.open, true, 'closed and not collapsible: forced open');

		const collapsed = new FakeDetails();
		run(collapsed, true);
		assertEqual(collapsed.open, false, 'collapsible: left as it is');

		const open = new FakeDetails();
		open.open = true;
		run(open, false);
		assertEqual(open.open, true, 'already open: stays open');

		const other = new EventTarget();
		run(other, false);
		assert(!('open' in other), 'an element that is not a details element is not touched');
	} finally {
		globalWithDom.HTMLDetailsElement = original;
	}
});

suite.check(
	'detailLayoutSectionGridClass builds the grid classes per column count and density',
	() => {
		assertEqual(drafts.detailLayoutSectionGridClass(1), 'grid gap-3', 'one column');
		assertEqual(drafts.detailLayoutSectionGridClass(2), 'grid gap-3 sm:grid-cols-2', 'two columns');
		assertEqual(
			drafts.detailLayoutSectionGridClass(3),
			'grid gap-3 sm:grid-cols-2 xl:grid-cols-3',
			'three columns'
		);
		assertEqual(
			drafts.detailLayoutSectionGridClass(1, 'compact'),
			'grid gap-2',
			'compact one column'
		);
		assertEqual(
			drafts.detailLayoutSectionGridClass(2, 'compact'),
			'grid gap-2 sm:grid-cols-2',
			'compact two'
		);
		assertEqual(
			drafts.detailLayoutSectionGridClass(3, 'compact'),
			'grid gap-2 sm:grid-cols-2 xl:grid-cols-3',
			'compact three'
		);
		assertEqual(
			drafts.detailLayoutSectionGridClass(2, 'comfortable'),
			'grid gap-3 sm:grid-cols-2',
			'explicit comfortable'
		);
	}
);

suite.check('detailLayoutSectionFieldClass spans child tables across the columns', () => {
	assertEqual(
		drafts.detailLayoutSectionFieldClass(formField('a', 'text'), 2),
		'min-w-0',
		'text, two columns'
	);
	assertEqual(
		drafts.detailLayoutSectionFieldClass(formField('a', undefined), 3),
		'min-w-0',
		'untyped, three'
	);
	assertEqual(
		drafts.detailLayoutSectionFieldClass(formField('a', 'child_table'), 1),
		'min-w-0',
		'child table, one column'
	);
	assertEqual(
		drafts.detailLayoutSectionFieldClass(formField('a', 'child_table'), 2),
		'min-w-0 sm:col-span-2',
		'child table, two columns'
	);
	assertEqual(
		drafts.detailLayoutSectionFieldClass(formField('a', 'child_table'), 3),
		'min-w-0 sm:col-span-2 xl:col-span-3',
		'child table, three columns'
	);
});

suite.check(
	'detailLayoutSectionFieldsForRecord hides empty fields only when the section asks to',
	() => {
		const fields = [
			formField('a', 'text'),
			formField('b', 'text'),
			formField('c', 'text'),
			formField('d', 'text')
		];
		const record = recordWith({ a: 'x', b: '   ', c: null, d: { name: 'Ada' } });

		assertDeepEqual(
			drafts.detailLayoutSectionFieldsForRecord(section({ fields }), record).map((f) => f.key),
			['a', 'b', 'c', 'd'],
			'all fields kept when hideEmptyFields is false'
		);
		assertDeepEqual(
			drafts
				.detailLayoutSectionFieldsForRecord(section({ fields, hideEmptyFields: true }), record)
				.map((f) => f.key),
			['a', 'd'],
			'blank and null values are hidden'
		);
		assertDeepEqual(
			drafts
				.detailLayoutSectionFieldsForRecord(
					section({ fields, hideEmptyFields: true }),
					recordWith({})
				)
				.map((f) => f.key),
			[],
			'a record without values hides every field'
		);
		assertEqual(
			drafts.detailLayoutSectionFieldsForRecord(
				section({ fields: [], hideEmptyFields: true }),
				record
			).length,
			0,
			'no fields'
		);
		const same = section({ fields });
		assertEqual(
			drafts.detailLayoutSectionFieldsForRecord(same, record),
			same.fields,
			'returns the section list itself when nothing is hidden'
		);
	}
);

finish(suite.result());
