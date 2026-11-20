/**
 * Project tree data of the secondary column (`lib/project/nav.ts`).
 *
 * Run standalone: `bun tests/project-nav.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	projectHome,
	projectTree,
	settingsLanding,
	settingsSections,
	type TreeNode
} from '../src/lib/project/nav';

const suite = new Suite('project-nav');
const W = 'w1';
const code = { id: 'p1', name: 'Core', type_key: 'software' };
const forms = { id: 'p2', name: 'Orders', type_key: 'custom_form' };
const base = (id: string) => `/workspace/${W}/projects/${id}`;
const t = (key: string) => `T(${key})`;
const input = {
	workspaceId: W,
	isWorkspaceAdmin: false,
	appsCount: null,
	forms: null,
	formsTotal: null,
	translate: t
};
const kinds = (node: TreeNode) => (node.children ?? []).map((c) => c.kind);

function ids(node: TreeNode, out: string[] = []): string[] {
	out.push(node.id);
	for (const child of node.children ?? []) ids(child, out);
	return out;
}

suite.check('code project: overview, issues, board, cycles, forms', () => {
	const tree = projectTree({ ...input, project: code });
	assertEqual(tree.kind, 'project', 'kind');
	assertEqual(tree.label, 'Core', 'label');
	assertEqual(tree.href, base('p1'), 'row goes to the overview');
	assertDeepEqual(kinds(tree), ['overview', 'issues', 'board', 'cycles', 'forms'], 'children');
	assertEqual(tree.children?.[0].label, 'T(shell.projectNav.overview)', 'translated');
	assertEqual(tree.children?.[1].href, `${base('p1')}/issues`, 'issues href');
});

suite.check('custom_form project has no overview and opens on forms', () => {
	const tree = projectTree({ ...input, project: forms });
	assertEqual(tree.href, `${base('p2')}/forms`, 'row href');
	assertDeepEqual(kinds(tree), ['forms'], 'children');
	assertEqual(projectHome(W, forms), `${base('p2')}/forms`, 'projectHome');
	assertEqual(projectHome(W, code), base('p1'), 'projectHome code');
});

suite.check('forms load lazily and list under the Forms row', () => {
	const lazy = projectTree({ ...input, project: forms }).children?.[0];
	assertEqual(lazy?.lazy, true, 'lazy until loaded');
	assertEqual(lazy?.children, undefined, 'no children yet');
	const loaded = projectTree({
		...input,
		project: forms,
		forms: [{ id: 'f 1', name: 'Lead' }],
		formsTotal: 1
	}).children?.[0];
	assertEqual(loaded?.lazy, undefined, 'not lazy');
	assertEqual(loaded?.children?.[0].kind, 'form', 'form row');
	assertEqual(loaded?.children?.[0].href, `${base('p2')}/forms?form=f%201`, 'form href is encoded');
	assertEqual(loaded?.children?.length, 1, 'no All forms row');
});

suite.check('All forms appears past 100 forms', () => {
	const many = Array.from({ length: 100 }, (_, i) => ({ id: `f${i}`, name: `F${i}` }));
	const at100 = projectTree({ ...input, project: forms, forms: many, formsTotal: 100 });
	assertEqual(at100.children?.[0].children?.length, 100, '100 is not past');
	const over = projectTree({ ...input, project: forms, forms: many, formsTotal: 101 });
	const last = over.children?.[0].children?.at(-1);
	assertEqual(last?.kind, 'all-forms', 'All forms row');
	assertEqual(last?.href, `${base('p2')}/forms`, 'href');
});

suite.check('Apps row: count or admin; Settings row: admin only', () => {
	assert(
		!kinds(projectTree({ ...input, project: forms })).includes('apps'),
		'hidden for member, unknown count'
	);
	assert(
		!kinds(projectTree({ ...input, project: forms, appsCount: 0 })).includes('apps'),
		'hidden for member, zero'
	);
	assert(
		kinds(projectTree({ ...input, project: forms, appsCount: 2 })).includes('apps'),
		'shown with apps'
	);
	const admin = projectTree({ ...input, project: forms, isWorkspaceAdmin: true });
	assertDeepEqual(kinds(admin), ['forms', 'apps', 'settings'], 'admin sees both');
	assertEqual(admin.children?.[1].href, `${base('p2')}/apps`, 'apps href');
	assertEqual(admin.children?.[2].href, `${base('p2')}/settings`, 'settings href');
	assert(
		!kinds(projectTree({ ...input, project: forms, appsCount: 3 })).includes('settings'),
		'member has no settings'
	);
});

suite.check('node ids are unique within a tree', () => {
	const tree = projectTree({
		...input,
		project: code,
		isWorkspaceAdmin: true,
		appsCount: 1,
		forms: [
			{ id: 'f1', name: 'A' },
			{ id: 'f2', name: 'B' }
		],
		formsTotal: 2
	});
	const all = ids(tree);
	assertEqual(new Set(all).size, all.length, `ids ${all.join(',')}`);
});

suite.check('settings sections and landing', () => {
	assertDeepEqual(settingsSections('custom_form', false), [], 'members get none');
	const sections = settingsSections('custom_form', true, { base: base('p2'), translate: t });
	assertEqual(sections.length, 1, 'plugins only today');
	assertEqual(sections[0].id, 'plugins', 'id');
	assertEqual(sections[0].href, `${base('p2')}/settings/plugins`, 'href');
	assertEqual(sections[0].label, 'T(shell.sections.projectPlugins)', 'label');
	assertEqual(settingsLanding(W, 'p2', 'custom_form'), `${base('p2')}/settings/plugins`, 'landing');
});

finish(suite.result());
