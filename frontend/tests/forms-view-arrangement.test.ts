/**
 * The working arrangement of a view: comparison, server query choice, storage and naming.
 *
 * Run standalone: `bun tests/forms-view-arrangement.test.ts`
 */

import {
	ARRANGEMENT_CAP,
	CLIENT_FILTER_PER_PAGE,
	arrangementDiffers,
	clearArrangement,
	generateViewKey,
	normalizedViewConfig,
	readArrangement,
	serverQueryFor,
	suggestViewName,
	writeArrangement,
	type StoredArrangement
} from '../src/lib/forms/view-arrangement';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { MemoryStore } from './support/memory-store';

const suite = new Suite('forms-view-arrangement');
const sortable = (key: string) => ['due', 'title', 'status'].includes(key);

suite.check('normalization ignores key order and audience keys', () => {
	assertEqual(
		normalizedViewConfig({ b: 1, a: { y: 2, x: 1 }, visibility: 'private', is_default: true }),
		normalizedViewConfig({ a: { x: 1, y: 2 }, b: 1 }),
		'equal'
	);
});

suite.check('differs ignores visibility and is_default', () => {
	const base = {
		view_type: 'grid' as const,
		config: { columns: ['a'], visibility: 'shared', is_default: false }
	};
	assertEqual(
		arrangementDiffers(base, 'grid', { columns: ['a'], visibility: 'private', is_default: true }),
		false,
		'same'
	);
	assertEqual(arrangementDiffers(base, 'grid', { columns: ['a', 'b'] }), true, 'columns changed');
	assertEqual(arrangementDiffers(base, 'kanban', { columns: ['a'] }), true, 'type changed');
	assertEqual(arrangementDiffers(null, 'grid', {}), false, 'built-in table unchanged');
});

suite.check('an unchanged saved view queries by view_id only', () => {
	assertDeepEqual(
		serverQueryFor(
			{ id: 'v1', config: { sort: { field: 'due' } } },
			{ sort: { field: 'due' } },
			{ modified: false, isServerSortable: sortable }
		),
		{ view_id: 'v1', per_page: 100, clientFilter: false },
		'unchanged'
	);
});

suite.check('a sort-only change keeps view_id and adds sort', () => {
	const base = {
		id: 'v1',
		config: { filters: [{ field: 'status', operator: 'equals', value: 'open' }] }
	};
	const draft = { ...base.config, sort: { field: 'due', direction: 'desc' } };
	assertDeepEqual(
		serverQueryFor(base, draft, { modified: true, isServerSortable: sortable }),
		{ view_id: 'v1', sort: 'due:desc', per_page: 100, clientFilter: false },
		'sort only'
	);
});

suite.check('a filter change drops view_id and filters in the browser', () => {
	const base = { id: 'v1', config: {} };
	const draft = {
		filters: [{ field: 'status', operator: 'equals', value: 'open' }],
		sort: { field: 'due', direction: 'asc' }
	};
	assertDeepEqual(
		serverQueryFor(base, draft, { modified: true, isServerSortable: sortable }),
		{ sort: 'due:asc', per_page: CLIENT_FILTER_PER_PAGE, clientFilter: true },
		'filter change'
	);
});

suite.check('the built-in table never sends view_id', () => {
	assertDeepEqual(
		serverQueryFor(null, {}, { modified: false, isServerSortable: sortable }),
		{ per_page: 100, clientFilter: false },
		'plain'
	);
	assertDeepEqual(
		serverQueryFor(
			null,
			{ filter: { field: 'status' } },
			{ modified: true, isServerSortable: sortable }
		),
		{ per_page: CLIENT_FILTER_PER_PAGE, clientFilter: true },
		'filtered'
	);
});

suite.check('a sort the server cannot run stays in the browser', () => {
	const query = serverQueryFor(
		null,
		{ sort: { field: 'location', direction: 'asc' } },
		{ modified: true, isServerSortable: sortable }
	);
	assertEqual(query.sort, undefined, 'no server sort');
});

function arrangement(index: number): StoredArrangement {
	return {
		base_updated_at: null,
		view_type: 'grid',
		config: { n: index },
		at: new Date(index).toISOString()
	};
}

suite.check('arrangements round-trip and clear', () => {
	const store = new MemoryStore();
	writeArrangement('alice', 'f', 'v', arrangement(1), store);
	assertDeepEqual(readArrangement('alice', 'f', 'v', store), arrangement(1), 'read back');
	assertEqual(readArrangement('bob', 'f', 'v', store), null, 'per user');
	clearArrangement('alice', 'f', 'v', store);
	assertEqual(readArrangement('alice', 'f', 'v', store), null, 'cleared');
});

suite.check(
	`at most ${ARRANGEMENT_CAP} arrangements per user, least recently written evicted`,
	() => {
		const store = new MemoryStore();
		for (let index = 0; index <= ARRANGEMENT_CAP; index += 1)
			writeArrangement('alice', 'f', `v${index}`, arrangement(index), store);
		assertEqual(readArrangement('alice', 'f', 'v0', store), null, 'oldest evicted');
		assert(readArrangement('alice', 'f', `v${ARRANGEMENT_CAP}`, store) !== null, 'newest kept');
		// Rewriting moves an entry to the end of the queue.
		writeArrangement('alice', 'f', 'v1', arrangement(1), store);
		writeArrangement('alice', 'f', 'vx', arrangement(9), store);
		assert(readArrangement('alice', 'f', 'v1', store) !== null, 'refreshed entry kept');
		assertEqual(readArrangement('alice', 'f', 'v2', store), null, 'next oldest evicted');
	}
);

suite.check('corrupted storage returns null', () => {
	const store = new MemoryStore();
	store.setItem('sv-forms-arrangement:v1:alice:f:v', '{oops');
	assertEqual(readArrangement('alice', 'f', 'v', store), null, 'invalid JSON');
	store.setItem(
		'sv-forms-arrangement:v1:alice:f:v',
		JSON.stringify({ view_type: 'grid', config: [] })
	);
	assertEqual(readArrangement('alice', 'f', 'v', store), null, 'wrong shape');
	store.failWrites = true;
	writeArrangement('alice', 'f', 'v', arrangement(1), store);
});

suite.check('suggested names describe the sort', () => {
	const label = (key: string) => (key === 'due' ? 'Due date' : key);
	assertEqual(
		suggestViewName({ name: 'Open deliveries' }, {}, label),
		'Open deliveries',
		'unsorted'
	);
	assertEqual(
		suggestViewName(
			{ name: 'Open deliveries' },
			{ sort: { field: 'due', direction: 'desc' } },
			label
		),
		'Open deliveries · by Due date',
		'sorted'
	);
	assert(
		suggestViewName({ name: 'x'.repeat(90) }, { sort: { field: 'due' } }, label).length <= 80,
		'capped at 80'
	);
});

suite.check('generated keys satisfy the form_views key constraint', () => {
	for (const name of ['Open deliveries', '我的视图', '123 numbers first', '', '__']) {
		const key = generateViewKey(name, 1_700_000_000_000, 0.5);
		assert(/^[a-z][a-z0-9_]*$/.test(key), `${name} -> ${key}`);
	}
	assert(generateViewKey('A', 1, 0.1) !== generateViewKey('A', 2, 0.1), 'time makes keys unique');
});

finish(suite.result());
