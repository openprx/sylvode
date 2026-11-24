/**
 * The shell side of milestones: the route belongs to the Projects destination, its row sits under
 * the workspace's projects and is selected on its pages, the page titles exist, write controls
 * are for owners and admins, and both languages carry every text the screens use.
 *
 * Run standalone: `bun test tests/features/milestones-shell.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { resolveRoute } from '../../src/lib/shell/route-map';
import { pageTitleKey } from '../../src/lib/shell/page-titles';
import { milestonesRow } from '../../src/lib/shell/column/lists';
import { selectedRowId } from '../../src/lib/shell/column/rows';
import { GLYPHS } from '../../src/lib/shell/column/glyphs';

const ROOT = new URL('../..', import.meta.url).pathname;
const read = (path: string) => readFileSync(join(ROOT, path), 'utf8');
const PAGES = 'src/routes/(app)/workspace/[workspaceId]/milestones';

test('milestone routes are part of the Projects destination, in the workspace', () => {
	for (const path of ['/workspace/w1/milestones', '/workspace/w1/milestones/m1']) {
		const route = resolveRoute(new URL(path, 'http://x'), null);
		assert.equal(route.destination, 'projects', path);
		assert.equal(route.workspaceId, 'w1', path);
		assert.equal(route.projectId, null, path);
	}
});

test('the Milestones row links the workspace list and is selected on the list and a detail page', () => {
	const row = milestonesRow('w 1', (key) => key);
	assert.equal(row.href, '/workspace/w%201/milestones');
	assert.equal(row.label, 'milestones.title');
	assert.ok(GLYPHS[row.glyph], 'the glyph exists');
	for (const pathname of ['/workspace/w%201/milestones', '/workspace/w%201/milestones/m1'])
		assert.equal(selectedRowId([row], { pathname, search: '' }), 'milestones', pathname);
	assert.equal(selectedRowId([row], { pathname: '/workspace/w%201/projects', search: '' }), null);
});

test('the column renders the row under the projects, in the workspace scope only', () => {
	const source = read('src/lib/shell/column/ProjectsColumn.svelte');
	assert.match(source, /milestonesRow\(scope, tr\)/);
	assert.match(source, /personal \? null : milestonesRow/);
	assert.ok(
		source.indexOf('{#if milestonesNav}') > source.lastIndexOf('<div class="branches">'),
		'after the projects'
	);
});

test('the pages have document titles', () => {
	assert.equal(pageTitleKey('/workspace/w1/milestones').key, 'milestones.pageTitle.list');
	assert.equal(pageTitleKey('/workspace/w1/milestones/m1').key, 'milestones.pageTitle.detail');
});

test('write controls are shown to owners and admins only', () => {
	const list = read(`${PAGES}/+page.svelte`);
	const detail = read(`${PAGES}/[milestoneId]/+page.svelte`);
	for (const [name, source] of [
		['list', list],
		['detail', detail]
	] as const) {
		assert.match(source, /canManageMilestones\(role\)/, `${name} derives the gate from the role`);
		assert.match(source, /\{#if canManage\}/, `${name} hides the write controls`);
	}
	// The create button and the three row actions all sit inside such a block.
	const gated = list.split('{#if canManage}').length - 1;
	assert.ok(gated >= 2, 'the create button and the row actions are gated');
});

test('milestone events are subscribable in the webhook form, under sprints and milestones', () => {
	const source = read('src/lib/webhooks/events.ts');
	for (const event of ['milestone.created', 'milestone.updated', 'milestone.deleted'])
		assert.match(source, new RegExp(`'${event.replace('.', '\\.')}'`));
});

test('the item page and list use the milestone field and filter', () => {
	const item = read(
		'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte'
	);
	assert.match(item, /milestone_id: milestoneValue \|\| null/);
	assert.match(item, /case 'milestone_changed'/);
	const list = read(
		'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/+page.svelte'
	);
	assert.match(list, /milestone_id: milestoneFilter === 'all' \? undefined : milestoneFilter/);
	assert.match(read('src/routes/(app)/workspace/+page.svelte'), /case 'milestone_changed'/);
});

function keysUsed(source: string): string[] {
	return [...source.matchAll(/\$?t\(\s*'(milestones\.[A-Za-z.]+)'/g)].map((m) => m[1]);
}

function lookup(tree: unknown, key: string): unknown {
	return key.split('.').reduce<unknown>((node, part) => {
		return node && typeof node === 'object' ? (node as Record<string, unknown>)[part] : undefined;
	}, tree);
}

test('every milestone text the screens use exists in both languages', () => {
	const en = JSON.parse(read('src/lib/milestones/locale.en.json'));
	const zh = JSON.parse(read('src/lib/milestones/locale.zh.json'));
	const files = [
		`${PAGES}/+page.svelte`,
		`${PAGES}/[milestoneId]/+page.svelte`,
		'src/lib/milestones/MilestoneDialog.svelte',
		'src/lib/milestones/rules.ts',
		'src/lib/shell/column/lists.ts',
		'src/lib/shell/page-titles.ts',
		'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte',
		'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/+page.svelte'
	];
	const used = new Set(files.flatMap((file) => keysUsed(read(file))));
	for (const key of [
		'milestones.pageTitle.list',
		'milestones.pageTitle.detail',
		'milestones.activity.set',
		'milestones.activity.setUnnamed',
		'milestones.activity.removed',
		'milestones.error.nameTaken',
		'milestones.error.failed'
	])
		used.add(key);
	assert.ok(used.size > 20, 'the scan found the keys');
	for (const key of used) {
		for (const [name, tree] of [
			['en', en],
			['zh', zh]
		] as const)
			assert.equal(typeof lookup(tree, key), 'string', `${name} has ${key}`);
	}
	assert.equal(zh.milestones.title, '里程碑');
	assert.match(zh.milestones.emptyAdmin, /[一-龥]/);
	assert.equal(en.milestones.empty, 'No milestones yet');
	assert.equal(zh.milestones.empty, '还没有里程碑');
});
