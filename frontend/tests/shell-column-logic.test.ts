/**
 * Secondary column logic: widths, route selection, tree rows, tree keys, scope rules and the list
 * derivations of each destination (`lib/shell/column/*.ts`).
 *
 * Run standalone: `bun tests/shell-column-logic.test.ts`
 */
import { readFileSync, readdirSync } from 'node:fs';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { resolveRoute } from '../src/lib/shell/route-map';
import type { FlowAvailability } from '../src/lib/shell/destinations';
import { projectTree } from '../src/lib/project/nav';
import {
	COLUMN_BREAKPOINT,
	columnWidth,
	isNarrowWindow,
	searchHint
} from '../src/lib/shell/column/layout';
import {
	emptyIdSet,
	flattenTree,
	selectedRowId,
	toggledIds,
	treeKeyAction,
	unionIds,
	type NavRow
} from '../src/lib/shell/column/rows';
import { scopeChangeTarget, scopeLabel, sortWorkspaces } from '../src/lib/shell/column/scope';
import {
	flowList,
	formsGroupToExpand,
	governanceRows,
	homeRows,
	projectToExpand,
	projectsOfWorkspace,
	recentRows,
	shouldRefreshProjects,
	workspaceChoiceRows
} from '../src/lib/shell/column/lists';
import { appsCountOf } from '../src/lib/shell/column/apps-count';

const suite = new Suite('shell-column-logic');
const W = 'w1';
const url = (path: string) => new URL(path, 'http://x');
const ctx = (path: string) => resolveRoute(url(path), null);
const tr = (key: string, values?: Record<string, string | number>) =>
	values ? `${key}|${JSON.stringify(values)}` : key;
const flowOff: FlowAvailability = { personal: false, workspaces: {} };
const flowOn: FlowAvailability = { personal: true, workspaces: { [W]: true } };

suite.check('column width: 264 from 1280, 232 below; narrow window at 800 or less', () => {
	assertEqual(COLUMN_BREAKPOINT, 1280, 'breakpoint');
	assertEqual(columnWidth(1440), 264, '1440');
	assertEqual(columnWidth(1280), 264, '1280');
	assertEqual(columnWidth(1279), 232, '1279');
	assertEqual(columnWidth(801), 232, '801');
	assertEqual(isNarrowWindow(800), true, '800');
	assertEqual(isNarrowWindow(801), false, '801');
	assertEqual(searchHint('MacIntel'), '⌘K', 'mac');
	assertEqual(searchHint('Win32'), 'Ctrl K', 'windows');
});

suite.check('selection: longest matching path wins, query-bearing row beats its parent', () => {
	const rows: NavRow[] = [
		{
			id: 'a',
			label: 'A',
			href: `/workspace/${W}/projects/p1`,
			glyph: 'folder',
			level: 0,
			parentId: null
		},
		{
			id: 'b',
			label: 'B',
			href: `/workspace/${W}/projects/p1/issues`,
			glyph: 'folder',
			level: 1,
			parentId: 'a'
		},
		{
			id: 'c',
			label: 'C',
			href: `/workspace/${W}/projects/p1/forms`,
			glyph: 'folder',
			level: 1,
			parentId: 'a'
		},
		{
			id: 'd',
			label: 'D',
			href: `/workspace/${W}/projects/p1/forms?form=f1`,
			glyph: 'folder',
			level: 2,
			parentId: 'c'
		},
		{
			id: 'e',
			label: 'E',
			href: `/workspace/${W}/projects/p1/forms`,
			glyph: 'folder',
			level: 2,
			parentId: 'c',
			selectable: false
		}
	];
	const sel = (path: string) => selectedRowId(rows, url(path));
	assertEqual(sel(`/workspace/${W}/projects/p1`), 'a', 'overview');
	assertEqual(sel(`/workspace/${W}/projects/p1/issues/i9`), 'b', 'child path');
	assertEqual(sel(`/workspace/${W}/projects/p1/forms`), 'c', 'forms without form');
	assertEqual(sel(`/workspace/${W}/projects/p1/forms?form=f1`), 'd', 'form row');
	assertEqual(sel(`/workspace/${W}/projects/p1/forms?form=other`), 'c', 'unknown form falls back');
	assertEqual(sel(`/workspace/${W}/projects/p1/forms/records/r1`), 'c', 'record page');
	assertEqual(sel('/inbox'), null, 'no match');
	assertEqual(sel(`/workspace/${W}/projects/p10`), null, 'prefix needs a segment boundary');
});

const project = { id: 'p1', name: 'Core', type_key: 'software' };
const formsProject = { id: 'p2', name: 'Orders', type_key: 'custom_form' };
const base = (id: string) => `/workspace/${W}/projects/${id}`;
const treeOf = (p: typeof project, extra: Partial<Parameters<typeof projectTree>[0]> = {}) =>
	projectTree({
		workspaceId: W,
		project: p,
		isWorkspaceAdmin: false,
		appsCount: null,
		forms: null,
		formsTotal: null,
		...extra
	});

suite.check('tree rows: collapsed project is one row; expansion lists children with levels', () => {
	const tree = treeOf(project);
	const closed = flattenTree(tree, emptyIdSet(), { workspaceId: W });
	assertEqual(closed.length, 1, 'only the project');
	assertEqual(closed[0].expandable, true, 'expandable');
	assertEqual(closed[0].expanded, false, 'collapsed');
	const open = flattenTree(tree, new Set(['p1']), { workspaceId: W });
	assertDeepEqual(
		open.map((r) => r.attrs?.['data-tree-kind']),
		['project', 'overview', 'issues', 'board', 'cycles', 'forms'],
		'kinds in order'
	);
	assert(
		open.slice(1).every((r) => r.level === 1 && r.parentId === 'p1'),
		'children indent one level'
	);
	assertEqual(open[0].attrs?.['data-project-id'], 'p1', 'project id attribute');
});

suite.check('tree rows: forms group lazy, loaded, empty, and over 100 forms', () => {
	const lazy = flattenTree(treeOf(formsProject), new Set(['p2']), { workspaceId: W });
	const formsRow = lazy.find((r) => r.attrs?.['data-tree-kind'] === 'forms');
	assertEqual(formsRow?.expandable, true, 'lazy forms row is expandable');
	assertEqual(formsRow?.expanded, false, 'closed until opened');

	const loaded = treeOf(formsProject, { forms: [{ id: 'f1', name: 'Orders' }], formsTotal: 1 });
	const open = flattenTree(loaded, new Set(['p2', 'forms:p2']), { workspaceId: W });
	const form = open.find((r) => r.attrs?.['data-tree-kind'] === 'form');
	assertEqual(form?.href, `${base('p2')}/forms?form=f1`, 'form href');
	assertEqual(form?.attrs?.['data-form-id'], 'f1', 'form id attribute');
	assertEqual(form?.level, 2, 'form level');
	assertEqual(form?.pin?.kind, 'form', 'a form can be pinned');

	const emptyForms = flattenTree(
		treeOf(formsProject, { forms: [], formsTotal: 0 }),
		new Set(['p2']),
		{
			workspaceId: W
		}
	);
	assertEqual(
		emptyForms.find((r) => r.attrs?.['data-tree-kind'] === 'forms')?.expandable,
		false,
		'no chevron when no forms'
	);

	const many = treeOf(formsProject, {
		forms: Array.from({ length: 100 }, (_, i) => ({ id: `f${i}`, name: `F${i}` })),
		formsTotal: 140
	});
	const rows = flattenTree(many, new Set(['p2', 'forms:p2']), { workspaceId: W });
	const all = rows.find((r) => r.attrs?.['data-tree-kind'] === 'all-forms');
	assert(all !== undefined, 'All forms row when more than 100');
	assertEqual(all?.selectable, false, 'All forms is not selected by route');
});

suite.check('tree rows: custom_form project selects Forms, not the project row, when open', () => {
	const tree = treeOf(formsProject, { forms: [{ id: 'f1', name: 'Orders' }], formsTotal: 1 });
	const open = flattenTree(tree, new Set(['p2']), { workspaceId: W });
	const sel = selectedRowId(open, url(`${base('p2')}/forms`));
	assertEqual(open.find((r) => r.id === sel)?.attrs?.['data-tree-kind'], 'forms', 'forms row');
	const closed = flattenTree(tree, emptyIdSet(), { workspaceId: W });
	assertEqual(
		selectedRowId(closed, url(`${base('p2')}/forms`)),
		'p2',
		'project row when collapsed'
	);
});

suite.check('tree rows: visibility of Apps and Settings follows the viewer', () => {
	const kinds = (admin: boolean, apps: number | null) =>
		flattenTree(treeOf(project, { isWorkspaceAdmin: admin, appsCount: apps }), new Set(['p1']), {
			workspaceId: W
		}).map((r) => r.attrs?.['data-tree-kind']);
	assert(!kinds(false, 0).includes('apps'), 'member, no apps');
	assert(kinds(false, 2).includes('apps'), 'member, apps exist');
	assert(!kinds(false, 2).includes('settings'), 'member never sees Settings');
	assert(kinds(true, 0).includes('apps') && kinds(true, 0).includes('settings'), 'admin sees both');
});

suite.check(
	'tree keys: right expands then enters, left collapses then goes to parent, P pins',
	() => {
		const ref = (over: Partial<Parameters<typeof treeKeyAction>[1]> = {}) => ({
			id: 'r',
			parentId: 'parent',
			expandable: true,
			expanded: false,
			firstChildId: 'kid',
			...over
		});
		assertDeepEqual(treeKeyAction('ArrowRight', ref()), { type: 'expand', id: 'r' }, 'expand');
		assertDeepEqual(
			treeKeyAction('ArrowRight', ref({ expanded: true })),
			{ type: 'focus', id: 'kid' },
			'first child'
		);
		assertEqual(
			treeKeyAction('ArrowRight', ref({ expanded: true, firstChildId: null })),
			null,
			'no child'
		);
		assertEqual(treeKeyAction('ArrowRight', ref({ expandable: false })), null, 'leaf');
		assertDeepEqual(
			treeKeyAction('ArrowLeft', ref({ expanded: true })),
			{ type: 'collapse', id: 'r' },
			'collapse'
		);
		assertDeepEqual(treeKeyAction('ArrowLeft', ref()), { type: 'focus', id: 'parent' }, 'parent');
		assertEqual(
			treeKeyAction('ArrowLeft', ref({ parentId: null, expandable: false })),
			null,
			'top leaf'
		);
		assertDeepEqual(treeKeyAction('P', ref()), { type: 'pin', id: 'r' }, 'pin');
		assertEqual(
			treeKeyAction('ArrowDown', ref()),
			null,
			'vertical keys belong to the roving group'
		);
	}
);

suite.check('id sets are copied, never mutated', () => {
	const a = emptyIdSet();
	const b = toggledIds(a, 'x', true);
	assertEqual(a.size, 0, 'original untouched');
	assert(b.has('x'), 'added');
	assertEqual(toggledIds(b, 'x', false).size, 0, 'removed');
	assertDeepEqual([...unionIds(['a'], new Set(['b', 'a']))].sort(), ['a', 'b'], 'union');
});

suite.check('scope: workspaces sorted by name, label with fallback', () => {
	const list = [
		{ id: '2', name: 'Zeta' },
		{ id: '1', name: 'alpha' },
		{ id: '3', name: 'Beta' }
	];
	assertDeepEqual(
		sortWorkspaces(list, 'en').map((w) => w.name),
		['alpha', 'Beta', 'Zeta'],
		'sorted'
	);
	assertEqual(list[0].name, 'Zeta', 'input unchanged');
	const labels = { personal: 'Personal', fallback: 'Workspace' };
	assertEqual(scopeLabel('personal', list, labels), 'Personal', 'personal');
	assertEqual(scopeLabel('3', list, labels), 'Beta', 'known');
	assertEqual(scopeLabel('9', list, labels), 'Workspace', 'unknown');
});

suite.check('scope change: where each destination goes', () => {
	const go = (
		path: string,
		scope: string,
		extra: Partial<Parameters<typeof scopeChangeTarget>[0]> = {}
	) => scopeChangeTarget({ ctx: ctx(path), scope, flow: flowOn, ...extra });
	assertEqual(
		go('/workspace/w0/projects', 'w1'),
		'/workspace/w1/projects',
		'projects to workspace'
	);
	assertEqual(go('/workspace/w0/projects', 'personal'), '/workspace', 'projects to personal');
	assertEqual(go('/flow', 'w1'), '/workspace/w1/flow', 'flow to workspace');
	assertEqual(go('/workspace/w1/flow', 'personal'), '/flow', 'flow to personal');
	assertEqual(go('/flow', 'w2'), null, 'flow off in the target: stay');
	assertEqual(go('/workspace/w0', 'w1'), '/workspace/w1', 'home with another workspace id');
	assertEqual(go('/workspace/w0', 'personal'), '/workspace', 'home to personal');
	assertEqual(go('/inbox', 'w1'), null, 'home without a workspace id stays');
	assertEqual(go('/governance', 'w1'), null, 'governance stays');
	assertEqual(
		go('/workspace/w0/members', 'w1', { adminInNewScope: true }),
		'/workspace/w1/members',
		'same section when admin'
	);
	assertEqual(
		go('/workspace/w0/members', 'w1', { adminInNewScope: false }),
		'/workspace/w1/settings',
		'general when not admin'
	);
	assertEqual(
		go('/workspace/w0/settings/flow', 'w1', { adminInNewScope: true }),
		'/workspace/w1/settings/flow',
		'flow settings'
	);
	assertEqual(go('/settings', 'w1'), null, 'account settings stay');
	assertEqual(
		go('/workspace/w0/members', 'personal'),
		'/settings',
		'workspace settings to personal'
	);
});

suite.check('home rows: three links, unread count only when positive, recent capped at 8', () => {
	const rows = homeRows({ unread: 3, tr });
	assertDeepEqual(
		rows.map((r) => r.href),
		['/workspace', '/inbox', '/my-work'],
		'links'
	);
	assertEqual(rows[1].count, 3, 'unread');
	assertEqual(homeRows({ unread: 0, tr })[1].count, undefined, 'no zero badge');
	const recent = recentRows(
		Array.from({ length: 12 }, (_, i) => ({ href: `/flow/${i}`, title: `T${i}` }))
	);
	assertEqual(recent.length, 8, 'cap');
	assertEqual(recent[0].label, 'T0', 'order kept');
});

suite.check(
	'projects: only the workspace, sorted; workspace choice rows; auto-expand; refresh gap',
	() => {
		const options = [
			{ id: 'b', name: 'Beta', workspace_id: W, type_key: 'x' },
			{ id: 'a', name: 'Alpha', workspace_id: W, type_key: 'x' },
			{ id: 'c', name: 'Other', workspace_id: 'w2', type_key: 'x' }
		];
		assertDeepEqual(
			projectsOfWorkspace(options, W, 'en').map((p) => p.id),
			['a', 'b'],
			'filtered and sorted'
		);
		assertEqual(
			workspaceChoiceRows([{ id: 'w1', name: 'N' }])[0].href,
			'/workspace/w1/projects',
			'choice row goes to the workspace projects'
		);
		const never = () => false;
		assertEqual(
			projectToExpand(ctx(`${base('p1')}/issues`), W, never),
			'p1',
			'route project opens'
		);
		assertEqual(
			projectToExpand(ctx(`${base('p1')}/issues`), W, () => true),
			null,
			'already open'
		);
		assertEqual(projectToExpand(ctx(`${base('p1')}/issues`), 'w2', never), null, 'other scope');
		assertEqual(
			projectToExpand(ctx(`${base('p1')}/issues`), 'personal', never),
			null,
			'personal scope'
		);
		assertEqual(
			projectToExpand(ctx('/workspace/w1/projects'), W, never),
			null,
			'no project in URL'
		);
		assertEqual(
			formsGroupToExpand(ctx(`${base('p1')}/forms?form=f1`)),
			'p1',
			'form open opens Forms'
		);
		assertEqual(formsGroupToExpand(ctx(`${base('p1')}/forms`)), null, 'no form, no auto-open');
		assertEqual(shouldRefreshProjects(1000, null), true, 'never refreshed');
		assertEqual(shouldRefreshProjects(1000 + 59_999, 1000), false, 'within a minute');
		assertEqual(shouldRefreshProjects(1000 + 60_000, 1000), true, 'after a minute');
	}
);

suite.check('flow list: private, workspace, off, unknown and personal-unavailable states', () => {
	const list = (scope: string, flow = flowOn, admin = false) =>
		flowList({ scope, workspaceName: 'Northwind', flow, workspaceAdmin: admin, tr });
	const ws = list(W);
	assertDeepEqual(
		ws.rows.map((r) => r.attrs?.['data-flow-section']),
		['private', 'workspace'],
		'both rows'
	);
	assertEqual(ws.rows[0].attrs?.['data-testid'], 'global-flow-link', 'test id kept on Private');
	assertEqual(ws.rows[1].label, 'Northwind', 'workspace name');
	assertEqual(ws.rows[1].href, '/workspace/w1/flow', 'workspace href');
	const off = list(W, { personal: true, workspaces: { [W]: false } }, true);
	assertDeepEqual(
		off.notice,
		{ kind: 'off', adminHref: '/workspace/w1/settings/flow' },
		'admin link'
	);
	assertEqual(off.rows.length, 1, 'only Private');
	assertDeepEqual(
		list(W, { personal: false, workspaces: { [W]: false } }).notice,
		{ kind: 'off', adminHref: null },
		'member: no link'
	);
	assertEqual(list(W, { personal: false, workspaces: {} }).loading, true, 'flag unknown');
	assertDeepEqual(
		list('personal', flowOff).notice,
		{ kind: 'unavailable-personal' },
		'personal unavailable'
	);
	assertEqual(
		list('personal', { personal: true, workspaces: {} }).rows.length,
		1,
		'personal available'
	);
});

suite.check('governance rows: admins get config and audit logs', () => {
	const member = governanceRows({ instanceAdmin: false, tr }).map((r) => r.href);
	assertEqual(member.length, 7, 'seven for everyone');
	assert(!member.includes('/governance/config'), 'no config for members');
	const admin = governanceRows({ instanceAdmin: true, tr }).map((r) => r.href);
	assert(
		admin.includes('/governance/config') && admin.includes('/governance/audit-logs'),
		'admin extras'
	);
	const rows = governanceRows({ instanceAdmin: true, tr });
	assertEqual(
		selectedRowId(rows, url('/governance/config')),
		'governance:config',
		'config beats centre'
	);
});

suite.check(
	'apps count: ready counts project.view items, off and denied are zero, loading is unknown',
	() => {
		assertEqual(appsCountOf({ status: 'loading', items: [] }), null, 'loading');
		assertEqual(appsCountOf({ status: 'error', items: [] }), null, 'error');
		assertEqual(appsCountOf({ status: 'off', items: [] }), 0, 'off');
		assertEqual(appsCountOf({ status: 'denied', items: [] }), 0, 'denied');
		assertEqual(appsCountOf({ status: 'ready', items: [] }), 0, 'ready and empty');
	}
);

function flatKeys(node: unknown, prefix = ''): string[] {
	if (typeof node !== 'object' || node === null) return [prefix];
	return Object.entries(node).flatMap(([k, v]) => flatKeys(v, prefix ? `${prefix}.${k}` : k));
}
const shellDir = new URL('../src/lib/shell/', import.meta.url);
const readKeys = (folder: string, lang: string) =>
	new Set(
		flatKeys(JSON.parse(readFileSync(new URL(`${folder}/locale.${lang}.json`, shellDir), 'utf8')))
	);

suite.check(
	'locale files: English and Chinese define the same keys, and every key used exists',
	() => {
		for (const folder of ['column', 'settings']) {
			const en = readKeys(folder, 'en');
			const zh = readKeys(folder, 'zh');
			assertDeepEqual([...en].sort(), [...zh].sort(), `${folder}: same keys in both languages`);
		}
		const known = new Set([
			...readKeys('column', 'en'),
			...readKeys('settings', 'en'),
			...flatKeys(JSON.parse(readFileSync(new URL('locale.en.json', shellDir), 'utf8')))
		]);
		for (const folder of ['column', 'settings']) {
			for (const name of readdirSync(new URL(`${folder}/`, shellDir))) {
				if (!/\.(svelte|ts)$/.test(name)) continue;
				const text = readFileSync(new URL(`${folder}/${name}`, shellDir), 'utf8');
				for (const [, key] of text.matchAll(
					/'(shell\.(?:column|settings|sections|destinations)\.[A-Za-z.]+)'/g
				))
					assert(known.has(key), `${folder}/${name} uses ${key}`);
			}
		}
	}
);

finish(suite.result());
