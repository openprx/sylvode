import { createRawSnippet } from 'svelte';
import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import {
	ariaSort,
	columnWidth,
	frozenOffsets,
	headerState,
	isTwoLine,
	nextRow,
	nextSort,
	setPage,
	setRange,
	setRow,
	tableWidth,
	twoLineColumns,
	type Column
} from './data-table';

await installSvelteSsr();
const { default: DataTable } = await import('./DataTable.svelte');

const suite = new Suite('ui-data-table');
const cell = createRawSnippet<[unknown]>(() => ({ render: () => '<span>x</span>' }));
const col = (id: string, extra: Partial<Column<unknown>> = {}): Column<unknown> => ({
	id,
	header: id,
	cell,
	...extra
});
const sorted = (set: Set<string>) => [...set].sort();

suite.check('sort cycles ascending, descending, none; another column starts ascending', () => {
	const asc = nextSort(null, 'a');
	assertDeepEqual(asc, { column: 'a', direction: 'asc' }, 'first click');
	const desc = nextSort(asc, 'a');
	assertDeepEqual(desc, { column: 'a', direction: 'desc' }, 'second click');
	assertEqual(nextSort(desc, 'a'), null, 'third click clears');
	assertDeepEqual(nextSort(desc, 'b'), { column: 'b', direction: 'asc' }, 'other column');
});
suite.check('aria-sort only on sortable columns', () => {
	assertEqual(ariaSort(null, 'a', false), undefined, 'not sortable');
	assertEqual(ariaSort(null, 'a', true), 'none', 'unsorted');
	assertEqual(ariaSort({ column: 'a', direction: 'asc' }, 'a', true), 'ascending', 'asc');
	assertEqual(ariaSort({ column: 'a', direction: 'desc' }, 'a', true), 'descending', 'desc');
	assertEqual(ariaSort({ column: 'a', direction: 'desc' }, 'b', true), 'none', 'other column');
});
suite.check('header checkbox state: none, some (indeterminate), all', () => {
	const ids = ['a', 'b', 'c'];
	assertEqual(headerState(ids, new Set()), 'none', 'none');
	assertEqual(headerState(ids, new Set(['b'])), 'some', 'some');
	assertEqual(headerState(ids, new Set(ids)), 'all', 'all');
	assertEqual(headerState([], new Set()), 'none', 'empty page');
	assertEqual(headerState(ids, new Set(['z'])), 'none', 'ids from another page do not count');
});
suite.check('selecting a page keeps other selections; clearing removes only this page', () => {
	const on = setPage(['a', 'b'], new Set(['z']), true);
	assertDeepEqual(sorted(on), ['a', 'b', 'z'], 'select page');
	assertDeepEqual(sorted(setPage(['a', 'b'], on, false)), ['z'], 'clear page');
});
suite.check('toggling a row returns a new set', () => {
	const base = new Set(['a']);
	const next = setRow(base, 'b', true);
	assertDeepEqual(sorted(next), ['a', 'b'], 'added');
	assertDeepEqual(sorted(base), ['a'], 'input untouched');
	assertDeepEqual(sorted(setRow(next, 'a', false)), ['b'], 'removed');
});
suite.check(
	'range selection covers both directions and falls back to one row without an anchor',
	() => {
		const ids = ['a', 'b', 'c', 'd', 'e'];
		assertDeepEqual(sorted(setRange(ids, new Set(), 'b', 'd', true)), ['b', 'c', 'd'], 'down');
		assertDeepEqual(sorted(setRange(ids, new Set(), 'd', 'b', true)), ['b', 'c', 'd'], 'up');
		assertDeepEqual(sorted(setRange(ids, new Set(), null, 'c', true)), ['c'], 'no anchor');
		assertDeepEqual(
			sorted(setRange(ids, new Set(ids), 'a', 'c', false)),
			['d', 'e'],
			'unselect range'
		);
		assertDeepEqual(
			sorted(setRange(ids, new Set(['q']), 'a', 'zz', true)),
			['q'],
			'unknown target'
		);
	}
);
suite.check('two-line rows at 480 and below, a table above', () => {
	assertEqual(isTwoLine(340), true, '340');
	assertEqual(isTwoLine(480), true, '480');
	assertEqual(isTwoLine(481), false, '481');
	assertEqual(isTwoLine(0), false, 'unmeasured');
});
suite.check('row focus: arrows stop at the ends; Home and End need Ctrl or Cmd', () => {
	assertEqual(nextRow('ArrowDown', false, 0, 3), 1, 'down');
	assertEqual(nextRow('ArrowDown', false, 2, 3), 2, 'no wrap at the end');
	assertEqual(nextRow('ArrowUp', false, 0, 3), 0, 'no wrap at the start');
	assertEqual(nextRow('Home', false, 2, 3), null, "plain Home is the control's");
	assertEqual(nextRow('Home', true, 2, 3), 0, 'ctrl Home');
	assertEqual(nextRow('End', true, 0, 3), 2, 'ctrl End');
	assertEqual(nextRow('ArrowDown', false, 0, 0), null, 'no rows');
});
suite.check('column widths are clamped; frozen offsets follow the selection column', () => {
	const columns = [
		col('a', { frozen: true, width: 200 }),
		col('b', { frozen: true, width: 50 }),
		col('c', { width: 900 }),
		col('d')
	];
	assertEqual(columnWidth(columns[1], undefined), 80, 'minimum');
	assertEqual(columnWidth(columns[2], undefined), 400, 'maximum');
	assertEqual(columnWidth(columns[3], undefined), 220, 'default');
	assertEqual(columnWidth(columns[3], { d: 300 }), 300, 'override');
	assertDeepEqual(frozenOffsets(columns, undefined, 40), { a: 40, b: 240 }, 'offsets');
	assertEqual(tableWidth(columns, undefined, true, true), 40 + 200 + 80 + 400 + 220 + 96, 'total');
});
suite.check('two-line columns: first primary, rest secondary, hidden skipped', () => {
	const split = twoLineColumns([
		col('a'),
		col('b'),
		col('c', { twoLine: 'hidden' }),
		col('d', { twoLine: 'primary' })
	]);
	assertDeepEqual(
		split.primary.map((c) => c.id),
		['a', 'd'],
		'primary'
	);
	assertDeepEqual(
		split.secondary.map((c) => c.id),
		['b'],
		'secondary'
	);
});

const props = {
	rows: [{ id: 'r1' }, { id: 'r2' }],
	columns: [col('name', { sortable: true }), col('owner')],
	rowId: (row: { id: string }) => row.id,
	label: 'Rows'
};

await suite.checkAsync(
	'renders a native table with scoped headers and aria-sort on sortable ones',
	async () => {
		const html = await renderToHtml(DataTable, {
			...props,
			sort: { column: 'name', direction: 'asc' }
		});
		assert(html.includes('<table') && html.includes('aria-label="Rows"'), 'table with a name');
		assertEqual((html.match(/<th scope="col"/g) ?? []).length, 2, 'two headers');
		assert(html.includes('aria-sort="ascending"'), 'sorted column');
		assertEqual((html.match(/aria-sort/g) ?? []).length, 1, 'not on the plain column');
	}
);
await suite.checkAsync('selectable adds the header box (named) and one box per row', async () => {
	const html = await renderToHtml(DataTable, {
		...props,
		selectable: true,
		selected: new Set(['r1'])
	});
	assert(html.includes('aria-label="ui.dataTable.selectAll"'), 'header box');
	assertEqual((html.match(/ui\.dataTable\.selectRow/g) ?? []).length, 2, 'row boxes');
	assert(html.includes('aria-checked="mixed"'), 'indeterminate when some are selected');
	assertEqual((html.match(/data-selected/g) ?? []).length, 1, 'one selected row');
});
await suite.checkAsync('rowAttributes and the current row reach the row element', async () => {
	const html = await renderToHtml(DataTable, {
		...props,
		currentId: 'r2',
		rowAttributes: (row: { id: string }) => ({ 'data-record-row': row.id })
	});
	assert(
		html.includes('data-record-row="r1"') && html.includes('data-record-row="r2"'),
		'attributes'
	);
	assert(
		/<tr[^>]*aria-current="true"[^>]*data-record-row="r2"|<tr[^>]*data-record-row="r2"[^>]*aria-current="true"/.test(
			html
		),
		'current row'
	);
});
await suite.checkAsync('the title is a link when a row href is given', async () => {
	const html = await renderToHtml(DataTable, {
		...props,
		rowHref: (row: { id: string }) => `/r/${row.id}`
	});
	assert(html.includes('href="/r/r1"') && html.includes('data-record-title-link'), 'title link');
});
await suite.checkAsync(
	'loading sets aria-busy and draws skeleton rows instead of data',
	async () => {
		const html = await renderToHtml(DataTable, { ...props, state: 'loading' });
		assert(html.includes('aria-busy="true"'), 'busy');
		assert(!html.includes('data-row') && html.includes('skeleton-row'), 'skeletons');
		assert(html.includes('<thead'), 'header stays');
	}
);
await suite.checkAsync(
	'empty, failed and offline show the state block under the header',
	async () => {
		const empty = await renderToHtml(DataTable, { ...props, rows: [], state: 'empty' });
		assert(empty.includes('data-table-state="empty"') && empty.includes('<thead'), 'empty');
		const failed = await renderToHtml(DataTable, {
			...props,
			state: 'failed',
			stateAction: () => {}
		});
		assert(
			failed.includes('role="alert"') && failed.includes('ui.dataTable.state.retry'),
			'failed with retry'
		);
		assert(!failed.includes('data-row'), 'no rows');
	}
);
await suite.checkAsync('a narrow container renders two-line rows without a table', async () => {
	const html = await renderToHtml(DataTable, { ...props, containerWidth: 340, selectable: true });
	assert(!html.includes('<table') && html.includes('<ul'), 'list');
	assert(html.includes('data-two-line'), 'marker');
	assertEqual((html.match(/data-row/g) ?? []).length, 2, 'rows');
});
await suite.checkAsync('compact density is a data attribute the tokens react to', async () => {
	const html = await renderToHtml(DataTable, { ...props, density: 'compact' });
	assert(html.includes('data-density="compact"'), 'compact');
});

finish(suite.result());
