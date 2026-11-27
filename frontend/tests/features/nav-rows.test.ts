/**
 * The resolved navigation as rows (`lib/nav/rows.ts`) and the project tree built from it.
 *
 * Run standalone: `bun test tests/features/nav-rows.test.ts`
 */
import { test } from 'node:test';
import nodeAssert from 'node:assert/strict';
import { projectTree } from '../../src/lib/project/nav';
import { flattenTree } from '../../src/lib/shell/column/rows';
import { pickLabel, projectChildren, workspaceGroups } from '../../src/lib/nav/rows';
import type { ResolvedEntry, ResolvedNav } from '../../src/lib/nav/types';

const tr = (key: string) => `T(${key})`;
const W = 'w1';
const P = 'p1';

function entry(id: string, target: ResolvedEntry['target'], extra: Partial<ResolvedEntry> = {}) {
	return {
		id,
		section_id: 'main',
		kind: target.kind,
		target,
		label: null,
		name: null,
		icon: null,
		href: null,
		pinned: false,
		locked: false,
		user_hidden: false,
		...extra
	} satisfies ResolvedEntry;
}

function menu(scope: 'workspace' | 'project', sections: ResolvedNav['sections']): ResolvedNav {
	return {
		schema: 'sv.nav.resolved.v1',
		scope,
		scope_id: scope === 'workspace' ? W : P,
		config_version: 1,
		customised: true,
		sections
	};
}

const flow = { personal: false, workspaces: { [W]: true } };
const homeInput = { workspaceId: W, locale: 'en', tr, unread: 3, flow, recent: [] };

test('labels fall back from the locale to its language, English, then any text', () => {
	nodeAssert.equal(pickLabel({ en: 'Intake', zh: '受理' }, 'zh'), '受理');
	nodeAssert.equal(pickLabel({ en: 'Intake', zh: '受理' }, 'zh-CN'), '受理');
	nodeAssert.equal(pickLabel({ en: 'Intake' }, 'zh'), 'Intake');
	nodeAssert.equal(pickLabel({ de: 'Eingang' }, 'zh'), 'Eingang');
	nodeAssert.equal(pickLabel(null, 'en'), null);
});

test('the workspace groups carry captions, external links and the unread count', () => {
	const groups = workspaceGroups(
		menu('workspace', [
			{
				id: '_pinned',
				label: null,
				collapsed: false,
				entries: [
					entry(
						'intake',
						{ kind: 'form', form_id: 'f1' },
						{
							href: '/workspace/w1/projects/p1/forms?form=f1',
							name: 'Customer intake',
							label: { en: 'Intake', zh: '受理' },
							pinned: true
						}
					)
				]
			},
			{
				id: 'main',
				label: null,
				collapsed: false,
				entries: [
					entry('notifications', { kind: 'builtin', page: 'home.notifications' }),
					entry(
						'handbook',
						{ kind: 'external', url: 'https://wiki.example.com/handbook' },
						{ href: 'https://wiki.example.com/handbook', label: { en: 'Handbook' } }
					)
				]
			},
			{
				id: 'ops',
				label: { en: 'Ops' },
				collapsed: true,
				entries: [entry('flow', { kind: 'builtin', page: 'workspace.flow' })]
			}
		]),
		{ ...homeInput, locale: 'zh' }
	);
	nodeAssert.deepEqual(
		groups.map((g) => [g.id, g.caption, g.collapsed]),
		[
			['_pinned', 'T(nav.sections.pinned)', false],
			['main', null, false],
			['ops', 'Ops', true]
		]
	);
	nodeAssert.equal(groups[0].rows[0].label, '受理');
	const [notifications, handbook] = groups[1].rows;
	nodeAssert.equal(notifications.href, '/inbox');
	nodeAssert.equal(notifications.count, 3);
	nodeAssert.equal(handbook.external, true);
	nodeAssert.equal(handbook.selectable, false);
	nodeAssert.equal(groups[2].rows[0].href, `/workspace/${W}/flow`);
});

test('a Flow entry disappears while the workspace has Flow off', () => {
	const groups = workspaceGroups(
		menu('workspace', [
			{
				id: 'main',
				label: null,
				collapsed: false,
				entries: [entry('flow', { kind: 'builtin', page: 'workspace.flow' })]
			}
		]),
		{ ...homeInput, flow: { personal: false, workspaces: { [W]: false } } }
	);
	nodeAssert.deepEqual(groups, []);
});

const projectInput = {
	workspaceId: W,
	projectId: P,
	locale: 'en',
	tr,
	isWorkspaceAdmin: false,
	apps: null,
	forms: null,
	formsTotal: null,
	formsInTree: 100
};

test('the project children follow the menu: captions, renamed built-ins, links and app routes', () => {
	const children = projectChildren(
		menu('project', [
			{
				id: 'main',
				label: null,
				collapsed: false,
				entries: [
					entry('issues', { kind: 'builtin', page: 'project.issues' }, { label: { en: 'Work' } }),
					entry('forms', { kind: 'builtin', page: 'project.forms' }),
					entry('desk', { kind: 'plugin_app', package_key: 'acme/desk', route: 'board' }),
					entry('gone', { kind: 'plugin_app', package_key: 'acme/none', route: 'x' })
				]
			},
			{
				id: 'more',
				label: { en: 'More' },
				collapsed: false,
				entries: [
					entry('wiki', { kind: 'external', url: 'https://x.test/' }, { href: 'https://x.test/' })
				]
			}
		]),
		{ ...projectInput, apps: [{ key: 'acme/desk', route: 'board', label: 'Desk' }] }
	);
	nodeAssert.deepEqual(
		children.map((c) => [c.kind, c.label]),
		[
			['link', 'Work'],
			['forms', 'T(shell.projectNav.forms)'],
			['link', 'Desk'],
			['caption', 'More'],
			['link', 'https://x.test/']
		]
	);
	nodeAssert.equal(children[0].href, `/workspace/${W}/projects/${P}/issues`);
	nodeAssert.equal(children[2].href, `/workspace/${W}/projects/${P}/apps/acme%2Fdesk/board`);
	nodeAssert.equal(children[4].external, true);
});

test('the Apps row lists the apps as children and shows for admins even when there are none', () => {
	const apps = menu('project', [
		{
			id: 'main',
			label: null,
			collapsed: false,
			entries: [entry('apps', { kind: 'builtin', page: 'project.apps' })]
		}
	]);
	nodeAssert.deepEqual(projectChildren(apps, projectInput), []);
	nodeAssert.equal(projectChildren(apps, { ...projectInput, isWorkspaceAdmin: true }).length, 1);
	const withApps = projectChildren(apps, {
		...projectInput,
		apps: [{ key: 'acme/desk', route: 'board', label: 'Desk' }]
	});
	nodeAssert.equal(withApps[0].children?.[0].label, 'Desk');
});

test('a project tree with a menu flattens to rows with captions and external links', () => {
	const tree = projectTree({
		workspaceId: W,
		project: { id: P, name: 'Core', type_key: 'custom_form' },
		isWorkspaceAdmin: false,
		appsCount: null,
		forms: [],
		formsTotal: 0,
		translate: tr,
		menu: menu('project', [
			{
				id: 'main',
				label: null,
				collapsed: false,
				entries: [entry('forms', { kind: 'builtin', page: 'project.forms' })]
			},
			{
				id: 'more',
				label: { en: 'More' },
				collapsed: false,
				entries: [
					entry('wiki', { kind: 'external', url: 'https://x.test/' }, { href: 'https://x.test/' })
				]
			}
		])
	});
	const rows = flattenTree(tree, new Set([P]), { workspaceId: W });
	nodeAssert.deepEqual(
		rows.map((r) => [r.caption === true, r.external === true, r.selectable !== false]),
		[
			[false, false, true],
			[false, false, true],
			[true, false, false],
			[false, true, false]
		]
	);
});

test('without a menu the project tree is the built-in one', () => {
	const tree = projectTree({
		workspaceId: W,
		project: { id: P, name: 'Core', type_key: 'custom_form' },
		isWorkspaceAdmin: false,
		appsCount: 2,
		forms: null,
		formsTotal: null,
		translate: tr,
		apps: [{ key: 'acme/desk', route: 'board', label: 'Desk' }]
	});
	nodeAssert.deepEqual(
		(tree.children ?? []).map((c) => c.kind),
		['forms', 'apps']
	);
	nodeAssert.equal(tree.children?.[1].children?.[0].kind, 'app');
});
