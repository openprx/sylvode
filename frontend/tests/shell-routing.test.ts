/**
 * Shell routing contract: route to destination or settings mode, destination roots, settings
 * sections and document titles (`lib/shell/{route-map,destinations,settings-sections,page-titles}.ts`).
 *
 * Run standalone: `bun tests/shell-routing.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { resolveRoute } from '../src/lib/shell/route-map';
import { destinationRoot, railHref, DEFAULT_DESTINATIONS } from '../src/lib/shell/destinations';
import { sectionFor, settingsGroups } from '../src/lib/shell/settings-sections';
import { pageTitle, pageTitleKey } from '../src/lib/shell/page-titles';

const suite = new Suite('shell-routing');
const W = 'w1';
const P = 'p1';
const route = (path: string) => resolveRoute(new URL(path, 'http://x'), null);

suite.check('destinations by route', () => {
	const cases: Array<[string, string]> = [
		['/workspace', 'home'],
		[`/workspace/${W}`, 'home'],
		['/inbox', 'home'],
		['/my-work', 'home'],
		['/search?q=a', 'home'],
		[`/workspace/${W}/projects`, 'projects'],
		[`/workspace/${W}/projects/${P}/issues/i1`, 'projects'],
		[`/workspace/${W}/projects/${P}/forms/records/r1`, 'projects'],
		[`/workspace/${W}/projects/${P}/apps`, 'projects'],
		[`/workspace/${W}/projects/${P}/audit-reports/a`, 'projects'],
		[`/projects/${P}/audit-reports`, 'projects'],
		['/flow', 'flow'],
		['/flow/package', 'flow'],
		[`/workspace/${W}/flow/o1`, 'flow'],
		['/shared-blocks/s1', 'flow'],
		['/agents', 'agents'],
		['/ai-agents/a1/learning', 'agents'],
		['/governance', 'governance'],
		['/proposals/abc/chain', 'governance'],
		['/trust-scores/appeals', 'governance'],
		['/users/u1/trust', 'governance']
	];
	for (const [path, destination] of cases) {
		const ctx = route(path);
		assertEqual(ctx.destination, destination, path);
		assertEqual(ctx.settings, null, `${path} is not settings`);
	}
});

suite.check('settings mode by route', () => {
	const cases: Array<[string, string, string]> = [
		[`/workspace/${W}/projects/${P}/settings/plugins`, 'project', 'plugins'],
		[`/workspace/${W}/projects/${P}/settings/plugins/creator`, 'project', 'plugins'],
		[`/workspace/${W}/projects/${P}/settings`, 'project', 'plugins'],
		[`/workspace/${W}/settings/flow/operations`, 'workspace', 'flow'],
		[`/workspace/${W}/settings/flow/package/imports/i`, 'workspace', 'flow'],
		[`/workspace/${W}/settings`, 'workspace', 'general'],
		[`/workspace/${W}/members`, 'workspace', 'members'],
		[`/workspace/${W}/webhooks`, 'workspace', 'webhooks'],
		[`/workspace/${W}/connections`, 'workspace', 'connections'],
		[`/workspace/${W}/workflows`, 'workspace', 'workflows'],
		['/settings', 'account', 'profile'],
		['/plugins', 'account', 'plugins'],
		['/plugins/creator', 'account', 'plugins'],
		['/admin', 'instance', 'dashboard'],
		['/admin/users', 'instance', 'users'],
		['/admin/settings', 'instance', 'system'],
		['/admin/ui', 'instance', 'gallery']
	];
	for (const [path, group, section] of cases) {
		const ctx = route(path);
		assertEqual(ctx.destination, null, `${path} has no destination`);
		assertDeepEqual(ctx.settings, { group, section }, path);
	}
});

suite.check('ids are read from the URL', () => {
	const ctx = route(`/workspace/${W}/projects/${P}/forms?form=f9&record=r1`);
	assertEqual(ctx.workspaceId, W, 'workspace');
	assertEqual(ctx.projectId, P, 'project');
	assertEqual(ctx.formId, 'f9', 'form');
	assertEqual(
		route(`/workspace/${W}/projects/${P}/issues?form=x`).formId,
		null,
		'form only on forms'
	);
	assertEqual(route('/flow/o1').flowObjectId, 'o1', 'personal flow object');
	assertEqual(route(`/workspace/${W}/flow/o2/convert`).flowObjectId, 'o2', 'convert');
	assertEqual(route('/flow/package').flowObjectId, null, 'package is not an object');
	assertEqual(route('/flow/conversions/j').flowObjectId, null, 'conversions are not objects');
	assertEqual(route('/projects/legacy/audit-reports').projectId, 'legacy', 'legacy project id');
	assertEqual(route('/workspace').workspaceId, null, 'no workspace on the list');
});

suite.check('unknown paths fall back to Home', () => {
	assertEqual(route('/nothing-here').destination, 'home', 'unknown');
});

suite.check('destination roots by scope', () => {
	const off = { personal: false, workspaces: {} };
	const on = { personal: true, workspaces: { [W]: true } };
	assertDeepEqual([...DEFAULT_DESTINATIONS], ['home', 'projects', 'flow', 'agents'], 'defaults');
	assertDeepEqual(
		destinationRoot('home', 'personal', off),
		{ available: true, href: '/workspace' },
		'home'
	);
	assertDeepEqual(
		destinationRoot('projects', 'personal', off),
		{ available: true, href: '/workspace' },
		'projects personal'
	);
	assertDeepEqual(
		destinationRoot('projects', W, off),
		{ available: true, href: `/workspace/${W}/projects` },
		'projects ws'
	);
	assertDeepEqual(
		destinationRoot('flow', 'personal', on),
		{ available: true, href: '/flow' },
		'flow personal'
	);
	assertDeepEqual(
		destinationRoot('flow', W, on),
		{ available: true, href: `/workspace/${W}/flow` },
		'flow ws'
	);
	assertEqual(destinationRoot('flow', 'personal', off).available, false, 'flow personal off');
	assertEqual(
		destinationRoot('flow', W, { personal: true, workspaces: { [W]: undefined } }).available,
		false,
		'flow unknown'
	);
	assertDeepEqual(
		destinationRoot('agents', W, off),
		{ available: true, href: '/agents' },
		'agents'
	);
	assertDeepEqual(
		destinationRoot('governance', W, off),
		{ available: true, href: '/governance' },
		'governance'
	);
});

suite.check('rail href prefers the remembered location when reachable', () => {
	const on = { personal: true, workspaces: { [W]: true } };
	assertDeepEqual(
		railHref('projects', W, `/workspace/${W}/projects/p9`, on),
		{ available: true, href: `/workspace/${W}/projects/p9` },
		'last'
	);
	assertDeepEqual(
		railHref('projects', W, undefined, on),
		{ available: true, href: `/workspace/${W}/projects` },
		'root'
	);
	assertEqual(
		railHref('flow', 'personal', '/flow/o', { personal: false, workspaces: {} }).available,
		false,
		'unavailable wins'
	);
});

const viewer = {
	instanceAdmin: false,
	workspaceAdmin: true,
	personalFlow: true,
	workspace: { id: W, name: 'North' }
};

suite.check('settings groups follow the viewer', () => {
	const ids = (v: Parameters<typeof settingsGroups>[0]) => settingsGroups(v).map((g) => g.id);
	assertDeepEqual(ids(viewer), ['account', 'workspace'], 'workspace admin');
	assertDeepEqual(
		ids({ ...viewer, instanceAdmin: true }),
		['account', 'workspace', 'instance'],
		'instance admin'
	);
	assertDeepEqual(
		ids({ ...viewer, workspaceAdmin: false }),
		['account'],
		'member sees no workspace group'
	);
	const personal = settingsGroups({ ...viewer, workspace: null });
	assertEqual(personal[1].chooseWorkspace, true, 'personal scope asks for a workspace');
	assertEqual(personal[1].sections.length, 0, 'no sections');
	const account = settingsGroups({ ...viewer, personalFlow: false })[0];
	assert(
		!account.sections.some((s) => s.id === 'flow-packages'),
		'flow packages need personal Flow'
	);
	const flowPackages = settingsGroups(viewer)[0].sections.find((s) => s.id === 'flow-packages');
	assertEqual(flowPackages?.external, true, 'outbound link');
	assertEqual(flowPackages?.href, '/flow/package', 'href');
	const ws = settingsGroups(viewer)[1];
	assertDeepEqual(
		ws.sections.map((s) => s.href),
		[
			`/workspace/${W}/settings`,
			`/workspace/${W}/members`,
			`/workspace/${W}/webhooks`,
			`/workspace/${W}/connections`,
			`/workspace/${W}/workflows`,
			`/workspace/${W}/settings/flow`
		],
		'workspace hrefs keep today URLs'
	);
	assertEqual(ws.caption, 'North', 'workspace name');
});

suite.check('project settings show the project group only', () => {
	const project = { workspaceId: W, id: P, name: 'Orders', typeKey: 'custom_form' };
	const groups = settingsGroups({ ...viewer, project });
	assertEqual(groups.length, 1, 'one group');
	assertEqual(groups[0].id, 'project', 'project');
	assertEqual(
		groups[0].sections[0].href,
		`/workspace/${W}/projects/${P}/settings/plugins`,
		'plugins href'
	);
	assertEqual(
		settingsGroups({ ...viewer, workspaceAdmin: false, project }).length,
		0,
		'members see nothing'
	);
});

suite.check('sectionFor finds the listed section', () => {
	const groups = settingsGroups({ ...viewer, instanceAdmin: true });
	assertEqual(
		sectionFor(route(`/workspace/${W}/members`), groups)?.section.id,
		'members',
		'members'
	);
	assertEqual(sectionFor(route('/plugins/creator'), groups)?.section.id, 'plugins', 'plugins');
	assertEqual(sectionFor(route('/admin/ui'), groups), null, 'gallery is not listed');
	assertEqual(sectionFor(route('/inbox'), groups), null, 'not settings');
});

suite.check('page titles', () => {
	const t = (key: string, options?: { values?: Record<string, string> }) =>
		options?.values ? `${key}:${JSON.stringify(options.values)}` : key;
	assertEqual(pageTitle('/plugins/creator', t), 'plugins.library.title', 'plugins');
	assertEqual(pageTitle('/flow/package', t), 'nav.flow', 'flow package');
	assertEqual(pageTitle('/flow/o/convert', t), 'pageTitle.flowConvert', 'convert');
	assertEqual(pageTitle('/workspace', t), 'pageTitle.workspace', 'workspace list');
	assertEqual(pageTitle(`/workspace/${W}`, t), 'pageTitle.workspaceHome', 'home without name');
	assertEqual(
		pageTitle(`/workspace/${W}`, t, { [W]: ' North ' }),
		'pageTitle.workspaceHomeWithName:{"name":"North"}',
		'home with name'
	);
	assertEqual(
		pageTitle(`/workspace/${W}/settings/flow/operations`, t),
		'pageTitle.flowOperations',
		'operations'
	);
	assertEqual(
		pageTitle(`/workspace/${W}/projects/${P}/issues/i`, t),
		'pageTitle.issueDetail',
		'issue'
	);
	assertEqual(
		pageTitle(`/workspace/${W}/projects/${P}/settings/plugins/creator`, t),
		'plugins.creator.title',
		'creator'
	);
	assertEqual(
		pageTitle(`/workspace/${W}/projects/${P}/settings`, t),
		'plugins.projectSettings',
		'settings landing'
	);
	assertEqual(pageTitle(`/workspace/${W}/projects/${P}`, t), 'pageTitle.projectDetail', 'project');
	assertEqual(pageTitle('/agents', t), 'shell.pageTitle.agents', 'agents');
	assertEqual(
		pageTitle(`/workspace/${W}/projects/${P}/apps`, t),
		'shell.pageTitle.projectApps',
		'apps'
	);
	assertEqual(
		pageTitle(`/workspace/${W}/projects/${P}/audit-reports/a`, t),
		'pageTitle.auditReportDetail',
		'moved report'
	);
	assertEqual(
		pageTitle(`/workspace/${W}/projects/${P}/apps/x/y`, t),
		'pageTitle.appDefault',
		'plugin page keeps default'
	);
	assertEqual(pageTitleKey('/nope').key, 'pageTitle.appDefault', 'default');
});

finish(suite.result());
