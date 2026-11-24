import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Issue } from '../../src/lib/api/issues';
import {
	SPRINT_ISSUES_PER_PAGE,
	emptyIssueList,
	hasMoreIssues,
	nextIssuePage,
	sprintProgressOf,
	withIssuePage
} from '../../src/lib/sprints/issue-pages';
import { rolloverMessage } from '../../src/lib/sprints/rules';

const issue = (id: string) => ({ id }) as Issue;

test('progress comes from the server count of the sprint', () => {
	assert.deepEqual(sprintProgressOf({ progress: { total: 8, done: 2 } }), {
		total: 8,
		done: 2,
		percent: 25
	});
	assert.deepEqual(sprintProgressOf({}), { total: 0, done: 0, percent: 0 });
	assert.equal(sprintProgressOf({ progress: { total: 3, done: 9 } }).done, 3);
});

test('items are read 50 at a time and "Load more" ends with the last page', () => {
	let list = emptyIssueList();
	assert.deepEqual(nextIssuePage('s1', list), { sprint_id: 's1', page: 1, per_page: 50 });
	assert.equal(SPRINT_ISSUES_PER_PAGE, 50);
	assert.equal(nextIssuePage('s1', { ...list, loading: true }), null, 'one read at a time');
	list = withIssuePage(list, { items: [issue('a'), issue('b')], page: 1, total_pages: 2 });
	assert.equal(hasMoreIssues(list), true);
	assert.deepEqual(nextIssuePage('s1', list), { sprint_id: 's1', page: 2, per_page: 50 });
	list = withIssuePage(list, { items: [issue('b'), issue('c')], page: 2, total_pages: 2 });
	assert.deepEqual(
		list.items.map((item) => item.id),
		['a', 'b', 'c'],
		'an item that moved between pages is shown once'
	);
	assert.equal(hasMoreIssues(list), false);
	assert.equal(nextIssuePage('s1', list), null);
});

test('an empty sprint has no further page', () => {
	const list = withIssuePage(emptyIssueList(), { items: [], page: 1, total_pages: 0 });
	assert.equal(hasMoreIssues(list), false);
});

test('the Cycles page no longer asks for a thousand items', () => {
	const page = readFileSync(
		join(
			new URL('../..', import.meta.url).pathname,
			'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/cycles/+page.svelte'
		),
		'utf8'
	);
	assert.doesNotMatch(page, /per_page:\s*1000/);
});

test('the rollover text uses the singular form for one item', () => {
	const translate = (key: string, options?: { values?: Record<string, unknown> }) =>
		`${key}${JSON.stringify(options?.values ?? {})}`;
	const one = rolloverMessage(
		translate,
		'completed',
		{ moved_count: 1, target_sprint_id: null },
		[],
		'en'
	);
	assert.match(one, /cycles\.rolloverMoved\.one/);
	const many = rolloverMessage(
		translate,
		'completed',
		{ moved_count: 2, target_sprint_id: null },
		[],
		'en'
	);
	assert.match(many, /cycles\.rolloverMoved\.other/);
	const zh = rolloverMessage(
		translate,
		'completed',
		{ moved_count: 1, target_sprint_id: null },
		[],
		'zh'
	);
	assert.match(zh, /cycles\.rolloverMoved\.other/);
});

test('both plural forms exist in English and the Chinese single form under both keys', () => {
	const root = new URL('../..', import.meta.url).pathname;
	const en = JSON.parse(readFileSync(join(root, 'src/lib/i18n/en.json'), 'utf8')).cycles;
	const zh = JSON.parse(readFileSync(join(root, 'src/lib/i18n/zh.json'), 'utf8')).cycles;
	for (const key of ['rolloverMoved', 'rolloverMovedElsewhere']) {
		assert.ok(en[key].one && en[key].other, `${key} en one/other`);
		assert.ok(zh[key].one && zh[key].other, `${key} zh one/other`);
	}
});
