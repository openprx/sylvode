/**
 * The lists on the work item page: paging helpers (`lib/project/issue-detail/paging.ts`), the
 * wording of the item-field activity entries and the rendered activity panel with fixtures.
 *
 * Run standalone: `bun tests/issue-detail.test.ts`
 */
import { readFileSync } from 'node:fs';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import {
	ACTIVITY_PAGE_SIZE,
	COMMENTS_PAGE_SIZE,
	appendPage,
	nextPageToLoad,
	refreshNewest,
	remainingCount
} from '../src/lib/project/issue-detail/paging';
import { itemFieldActivityText } from '../src/lib/project/issue-detail/activity-text';
import type { Activity } from '../src/lib/api/activity';

await installSvelteSsr();

const suite = new Suite('issue-detail');

const at = (minute: number) => new Date(Date.UTC(2026, 10, 27, 10, minute)).toISOString();
const entry = (id: string, minute: number) => ({ id, created_at: at(minute) });
const ids = (list: readonly { id: string }[]) => list.map((item) => item.id);

suite.check('page sizes follow the owner request', () => {
	assertEqual(COMMENTS_PAGE_SIZE, 20, 'comments');
	assertEqual(ACTIVITY_PAGE_SIZE, 30, 'activity');
});

suite.check('a later page is appended below and never repeats an entry', () => {
	const loaded = [entry('c', 3), entry('b', 2)];
	assertDeepEqual(
		ids(appendPage(loaded, [entry('b', 2), entry('a', 1)])),
		['c', 'b', 'a'],
		'merge'
	);
});

suite.check('a newly posted comment goes to the top', () => {
	const merged = appendPage([entry('new', 9)], [entry('c', 3), entry('b', 2)]);
	assertDeepEqual(ids(merged), ['new', 'c', 'b'], 'new first');
});

suite.check('reading the newest page again keeps the older loaded entries below it', () => {
	const loaded = [entry('d', 4), entry('c', 3), entry('b', 2), entry('a', 1)];
	const fresh = [entry('e', 5), entry('d', 4), entry('c', 3)];
	assertDeepEqual(ids(refreshNewest(loaded, fresh)), ['e', 'd', 'c', 'b', 'a'], 'refresh');
	assertDeepEqual(ids(refreshNewest([], fresh)), ['e', 'd', 'c'], 'nothing loaded yet');
});

suite.check('load more never skips an entry after a delete or a post', () => {
	assertEqual(nextPageToLoad(20, 20), 2, 'a full first page');
	// One of 20 loaded comments was deleted: entry 19 of the server list is still unseen, and page 2
	// (offsets 20..39) would skip it, so the page that starts at offset 0 is read instead.
	assertEqual(nextPageToLoad(19, 20), 1, 'after a delete');
	assertEqual(nextPageToLoad(21, 20), 2, 'after a post');
	const server = ['n', 'm', 'l']; // the server list after a delete, page size 2
	const loaded = ['n']; // 'o' was loaded and deleted: one entry loaded, two per page
	const page = nextPageToLoad(loaded.length, 2);
	const fetched = server.slice((page - 1) * 2, page * 2);
	assertDeepEqual(
		appendPage(
			loaded.map((id) => ({ id, created_at: at(0) })),
			fetched.map((id) => ({ id, created_at: at(0) }))
		).map((item) => item.id),
		['n', 'm'],
		'the entry after the loaded one is read'
	);
});

suite.check('the remaining count never goes below zero', () => {
	assertEqual(remainingCount(45, 30), 15, 'some left');
	assertEqual(remainingCount(3, 5), 0, 'more loaded than counted');
});

suite.check('the item-field entries have their own wording and others are left alone', () => {
	const t = (key: string, options?: { values?: Record<string, string> }) =>
		`${key}${options?.values ? JSON.stringify(options.values) : ''}`;
	assertEqual(itemFieldActivityText(t, 'created', {}), 'activity.created', 'created');
	assertEqual(
		itemFieldActivityText(t, 'title_changed', { from: 'A', to: 'B' }),
		'activity.titleChanged{"from":"A","to":"B"}',
		'title'
	);
	assertEqual(
		itemFieldActivityText(t, 'description_changed', {}),
		'activity.descriptionChanged',
		'description'
	);
	assertEqual(
		itemFieldActivityText(t, 'due_date_changed', { from: '2030-01-01T00:00:00Z', to: null }),
		'activity.dueDateCleared',
		'due date removed'
	);
	assertEqual(itemFieldActivityText(t, 'status_changed', {}), null, 'not an item-field entry');
});

const activity = (id: string, minute: number): Activity => ({
	id,
	issue_id: 'issue',
	user_id: 'user',
	action: 'status_changed',
	detail: {},
	created_at: at(minute)
});

await suite.checkAsync(
	'the activity panel shows every received entry in the given order',
	async () => {
		const { default: ActivityPanel } =
			await import('../src/lib/project/issue-detail/ActivityPanel.svelte');
		const html = await renderToHtml(ActivityPanel, {
			activities: [activity('n3', 3), activity('n2', 2), activity('n1', 1)],
			total: 3,
			onloadmore: () => {},
			text: (a: Activity) => `text-${a.id}`,
			author: () => 'Ann',
			date: (value: string) => `date-${value}`
		});
		assertEqual(html.split('data-testid="activity-entry"').length - 1, 3, 'all three rendered');
		assert(html.indexOf('text-n3') < html.indexOf('text-n2'), 'newest first');
		assert(html.indexOf('text-n2') < html.indexOf('text-n1'), 'oldest last');
		assert(!html.includes('issue.loadMoreActivity'), 'no load more when everything is loaded');
	}
);

await suite.checkAsync('the activity panel offers more while the server holds more', async () => {
	const { default: ActivityPanel } =
		await import('../src/lib/project/issue-detail/ActivityPanel.svelte');
	const html = await renderToHtml(ActivityPanel, {
		activities: [activity('n2', 2), activity('n1', 1)],
		total: 40,
		onloadmore: () => {},
		text: (a: Activity) => a.id,
		author: () => 'Ann',
		date: () => ''
	});
	assert(html.includes('issue.loadMoreActivity'), 'load more is offered');
});

suite.check(
	'the page keeps the agent tasks panel above the activity panel in the right column',
	() => {
		const source = readFileSync(
			new URL(
				'../src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte',
				import.meta.url
			),
			'utf8'
		);
		const right = source.slice(source.indexOf('lg:col-span-3'));
		const tasks = right.indexOf('<AgentTasksPanel');
		const activity = right.indexOf('<ActivityPanel');
		assert(tasks > 0 && activity > tasks, 'agent tasks directly above the activity');
		assert(
			!source.slice(0, source.indexOf('lg:col-span-3')).includes('<AgentTasksPanel'),
			'not left'
		);
	}
);

finish(suite.result());
