/**
 * Settings mode frame: which groups each viewer sees, the section list model, the close target and
 * the server-rendered section list (`lib/shell/settings/*`).
 *
 * Run standalone: `bun tests/shell-settings-frame.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { MemoryStore } from './support/memory-store';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import { installRuneModules } from './shell-rune-loader';
import { resolveRoute } from '../src/lib/shell/route-map';
import {
	buildSettingsViewer,
	closeTarget,
	settingsNavModel,
	type SettingsViewerInput
} from '../src/lib/shell/settings/frame';

await installRuneModules();
await installSvelteSsr();
const { ShellState } = await import('../src/lib/shell/shell-state.svelte');
const { ColumnData } = await import('../src/lib/shell/column/column-data.svelte');
const { default: SettingsColumn } = await import('../src/lib/shell/settings/SettingsColumn.svelte');

const suite = new Suite('shell-settings-frame');
const W = 'w1';
const ctx = (path: string) => resolveRoute(new URL(path, 'http://x'), null);
const user = (role: string) =>
	({ id: 'u1', email: 'u@example.test', role, name: 'U' }) as unknown as NonNullable<
		SettingsViewerInput['user']
	>;

function input(over: Partial<SettingsViewerInput> = {}): SettingsViewerInput {
	return {
		user: user('member'),
		workspaceRole: null,
		personalFlow: false,
		scope: W,
		workspaces: [{ id: W, name: 'North' }],
		ctx: ctx('/settings'),
		projects: [{ id: 'p1', name: 'Core', type_key: 'software' }],
		...over
	};
}
const groupIds = (over: Partial<SettingsViewerInput> = {}) => {
	const i = input(over);
	return settingsNavModel(buildSettingsViewer(i), i.ctx).map((g) => g.id);
};

suite.check('groups by viewer: member, workspace admin, instance admin, personal scope', () => {
	assertDeepEqual(groupIds({ workspaceRole: 'member' }), ['account'], 'plain member: Account only');
	assertDeepEqual(groupIds({ workspaceRole: null }), ['account'], 'role unknown fails closed');
	assertDeepEqual(
		groupIds({ workspaceRole: 'admin' }),
		['account', 'workspace'],
		'workspace admin'
	);
	assertDeepEqual(
		groupIds({ workspaceRole: 'owner' }),
		['account', 'workspace'],
		'workspace owner'
	);
	assertDeepEqual(
		groupIds({ user: user('admin'), workspaceRole: 'member' }),
		['account', 'instance'],
		'instance admin who is only a member: no workspace group'
	);
	assertDeepEqual(
		groupIds({ user: user('admin'), workspaceRole: 'owner' }),
		['account', 'workspace', 'instance'],
		'both'
	);
	assertDeepEqual(
		groupIds({ scope: 'personal', workspaceRole: 'owner' }),
		['account', 'workspace'],
		'personal scope: workspace group is the prompt'
	);
	const personal = settingsNavModel(
		buildSettingsViewer(input({ scope: 'personal' })),
		ctx('/settings')
	).find((g) => g.id === 'workspace');
	assertEqual(personal?.chooseWorkspace, true, 'choose a workspace prompt');
	assertEqual(personal?.sections.length, 0, 'no sections in personal scope');
});

suite.check(
	'account sections: profile, my plugins, Flow packages only when personal Flow exists',
	() => {
		const sections = (personalFlow: boolean) => {
			const i = input({ personalFlow });
			return settingsNavModel(buildSettingsViewer(i), i.ctx)[0].sections;
		};
		assertDeepEqual(
			sections(false).map((s) => s.id),
			['profile', 'plugins'],
			'without Flow'
		);
		const withFlow = sections(true);
		assertDeepEqual(
			withFlow.map((s) => s.id),
			['profile', 'plugins', 'flow-packages'],
			'with Flow'
		);
		assertEqual(withFlow[1].href, '/plugins', 'plugin library URL unchanged');
		assertEqual(withFlow[2].external, true, 'Flow packages leaves settings');
		assertEqual(withFlow[2].href, '/flow/package', 'Flow packages URL unchanged');
	}
);

suite.check('workspace sections and the instance sections keep their URLs', () => {
	const i = input({ user: user('admin'), workspaceRole: 'owner' });
	const model = settingsNavModel(buildSettingsViewer(i), i.ctx);
	const ws = model.find((g) => g.id === 'workspace');
	assertEqual(ws?.caption, 'North', 'workspace name beside the caption');
	assertDeepEqual(
		ws?.sections.map((s) => s.href),
		[
			`/workspace/${W}/settings`,
			`/workspace/${W}/members`,
			`/workspace/${W}/webhooks`,
			`/workspace/${W}/connections`,
			`/workspace/${W}/workflows`,
			`/workspace/${W}/settings/flow`,
			`/workspace/${W}/settings/navigation`,
			`/workspace/${W}/settings/installations`
		],
		'workspace hrefs'
	);
	assertDeepEqual(
		model.find((g) => g.id === 'instance')?.sections.map((s) => s.href),
		['/admin', '/admin/users', '/admin/settings'],
		'instance hrefs'
	);
});

suite.check('current section from the route', () => {
	const selected = (path: string, over: Partial<SettingsViewerInput> = {}) => {
		const i = input({ user: user('admin'), workspaceRole: 'owner', ctx: ctx(path), ...over });
		return settingsNavModel(buildSettingsViewer(i), i.ctx)
			.flatMap((g) => g.sections.filter((s) => s.selected).map((s) => `${g.id}/${s.id}`))
			.join(',');
	};
	assertEqual(selected('/settings'), 'account/profile', 'profile');
	assertEqual(selected('/plugins/creator'), 'account/plugins', 'creator belongs to My plugins');
	assertEqual(selected(`/workspace/${W}/settings`), 'workspace/general', 'general');
	assertEqual(
		selected(`/workspace/${W}/settings/flow/operations`),
		'workspace/flow',
		'flow child page'
	);
	assertEqual(selected(`/workspace/${W}/members`), 'workspace/members', 'members');
	assertEqual(selected(`/workspace/${W}/workflows`), 'workspace/workflows', 'workflows');
	assertEqual(selected('/admin/users'), 'instance/users', 'users');
	assertEqual(selected('/admin/ui'), '', 'gallery is not listed');
});

suite.check('project settings: the project group only, for owners and admins', () => {
	const path = `/workspace/${W}/projects/p1/settings/plugins`;
	const admin = input({ ctx: ctx(path), workspaceRole: 'admin' });
	const model = settingsNavModel(buildSettingsViewer(admin), admin.ctx);
	assertDeepEqual(
		model.map((g) => g.id),
		['project'],
		'project group only'
	);
	assertEqual(model[0].caption, 'Core', 'project name as caption');
	assertEqual(
		model[0].sections[0].href,
		`/workspace/${W}/projects/p1/settings/plugins`,
		'plugins section'
	);
	assertEqual(model[0].sections[0].selected, true, 'plugins selected');
	const member = input({ ctx: ctx(path), workspaceRole: 'member' });
	assertEqual(
		settingsNavModel(buildSettingsViewer(member), member.ctx).length,
		0,
		'member sees nothing'
	);
});

suite.check('close target: stored return URL, else Home root', () => {
	assertEqual(
		closeTarget('/workspace/w1/projects?x=1'),
		'/workspace/w1/projects?x=1',
		'return URL'
	);
	assertEqual(closeTarget(null), '/workspace', 'fallback');
	assertEqual(closeTarget(''), '/workspace', 'empty');
});

function render(over: Partial<SettingsViewerInput> = {}, extra: Record<string, unknown> = {}) {
	const i = input(over);
	const shell = new ShellState({
		userId: 'u1',
		local: new MemoryStore(),
		session: new MemoryStore()
	});
	return renderToHtml(SettingsColumn, {
		ctx: i.ctx,
		shell,
		viewer: buildSettingsViewer(i),
		windowWidth: 1440,
		data: new ColumnData({} as never),
		...extra
	});
}
const count = (html: string, pattern: RegExp) => (html.match(pattern) ?? []).length;

await suite.checkAsync(
	'markup: nav named Settings, group headings, one current section, close button',
	async () => {
		const html = await render({
			user: user('admin'),
			workspaceRole: 'owner',
			personalFlow: true,
			ctx: ctx(`/workspace/${W}/members`)
		});
		assert(/<nav[^>]*aria-label="shell\.destinations\.settings"/.test(html), 'nav name');
		assert(html.includes('data-shell="settings-column"'), 'data-shell');
		assert(html.includes('width: 264px'), '264 wide');
		assertEqual(count(html, /<h2 /g), 3, 'three group headings');
		for (const group of ['account', 'workspace', 'instance'])
			assert(html.includes(`data-settings-group="${group}"`), `${group} group`);
		assertEqual(count(html, /aria-current="page"/g), 1, 'one current section');
		assert(
			/data-settings-section="members"[^>]*aria-current="page"|aria-current="page"[^>]*data-settings-section="members"/.test(
				html
			),
			'members is current'
		);
		assert(html.includes('data-shell="close-settings"'), 'close button');
		assert(html.includes('shell.settings.leavesSettings'), 'external link says it leaves settings');
		assertEqual(
			count(html, /<a [^>]*data-settings-section/g),
			3 + 8 + 3,
			'every section is a link'
		);
	}
);

await suite.checkAsync(
	'markup: a member sees Account only; personal scope shows the workspace prompt',
	async () => {
		const member = await render({ workspaceRole: 'member' });
		assertEqual(count(member, /<h2 /g), 1, 'one heading');
		assert(!member.includes('data-settings-group="workspace"'), 'no workspace group');
		const personal = await render({ scope: 'personal' });
		assert(personal.includes('data-settings-choose'), 'choose a workspace');
		assert(!personal.includes('data-settings-section="members"'), 'no workspace sections');
	}
);

await suite.checkAsync('markup: narrow window is full width', async () => {
	const html = await render({}, { windowWidth: 700 });
	assert(html.includes('width: 100%'), 'full width');
});

finish(suite.result());
