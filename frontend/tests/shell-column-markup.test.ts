/**
 * Server-rendered markup of the secondary column: roles, names, current items and states
 * (`lib/shell/column/*.svelte`). Text is the message key (no locale is loaded).
 *
 * Run standalone: `bun tests/shell-column-markup.test.ts`
 */
import { Suite, assert, assertEqual, finish } from './support/harness';
import { MemoryStore } from './support/memory-store';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import { installRuneModules } from './shell-rune-loader';
import { resolveRoute } from '../src/lib/shell/route-map';

await installRuneModules();
await installSvelteSsr();
const { ShellState } = await import('../src/lib/shell/shell-state.svelte');
const { ColumnData } = await import('../src/lib/shell/column/column-data.svelte');
const { default: NavList } = await import('../src/lib/shell/column/NavList.svelte');
const { default: ProjectsColumn } = await import('../src/lib/shell/column/ProjectsColumn.svelte');
const { default: SecondaryColumn } = await import('../src/lib/shell/column/SecondaryColumn.svelte');
const { default: AgentsColumn } = await import('../src/lib/shell/column/AgentsColumn.svelte');
const { default: FlowColumn } = await import('../src/lib/shell/column/FlowColumn.svelte');
const { default: BackToListRow } = await import('../src/lib/shell/column/BackToListRow.svelte');
type Apis = import('../src/lib/shell/column/column-data.svelte').ColumnApis;
type Options = import('../src/lib/shell/column/types').ProjectOptionsSource;

const suite = new Suite('shell-column-markup');
const W = 'w1';
const base = (id: string) => `/workspace/${W}/projects/${id}`;
const ok = <T>(data: T) => ({ code: 0, message: 'ok', data });
const page = <T>(items: T[]) => ok({ items, total: items.length, page: 1, per_page: 100 });
const count = (html: string, pattern: RegExp) => (html.match(pattern) ?? []).length;

function makeApis(over: Partial<Apis> = {}): Apis {
	return {
		workspaces: async () => page([{ id: W, name: 'North' }]),
		bots: async () => ok([{ id: 'b1', name: 'Deploy bot' }]),
		forms: async () => page([{ id: 'f1', name: 'Orders' }]),
		...over
	} as Apis;
}

function optionsOf(state: {
	items?: Array<{ id: string; name: string; workspace_id: string; type_key: string }>;
	loading?: boolean;
	loaded?: boolean;
	error?: string | null;
}): Options {
	const value = {
		items: (state.items ?? []).map((p) => ({ ...p, workspace_name: 'North' })),
		loading: state.loading ?? false,
		loaded: state.loaded ?? true,
		error: state.error ?? null
	};
	return {
		subscribe(run) {
			run(value);
			return () => {};
		},
		ensureLoaded: async () => {}
	};
}

function makeShell(scope = W, expanded: string[] = []) {
	const shell = new ShellState({
		userId: 'u1',
		local: new MemoryStore(),
		session: new MemoryStore()
	});
	shell.setScope(scope);
	for (const id of expanded) shell.setProjectExpanded(scope, id, true);
	return shell;
}

const viewer = (
	over: Partial<{ workspaceAdmin: boolean; instanceAdmin: boolean; unread: number }> = {}
) => ({
	workspaceAdmin: false,
	instanceAdmin: false,
	unread: 0,
	flow: { personal: true, workspaces: { [W]: true } },
	...over
});
const urlOf = (path: string) => new URL(path, 'http://x');

const projects = [
	{ id: 'p1', name: 'Core', workspace_id: W, type_key: 'software' },
	{ id: 'p2', name: 'Orders', workspace_id: W, type_key: 'custom_form' },
	{ id: 'p3', name: 'Elsewhere', workspace_id: 'w2', type_key: 'software' }
];

async function projectsHtml(
	path: string,
	opts: {
		expanded?: string[];
		admin?: boolean;
		options?: Options;
		scope?: string;
		data?: InstanceType<typeof ColumnData>;
	} = {}
) {
	const url = urlOf(path);
	return renderToHtml(ProjectsColumn, {
		shell: makeShell(opts.scope ?? W, opts.expanded ?? []),
		scope: opts.scope ?? W,
		url,
		ctx: resolveRoute(url, null),
		viewer: viewer({ workspaceAdmin: opts.admin ?? false }),
		data: opts.data ?? new ColumnData(makeApis()),
		optionsStore: opts.options ?? optionsOf({ items: projects })
	});
}

await suite.checkAsync(
	'nav list: links keep the link role, one current item, expandable rows say so',
	async () => {
		const html = await renderToHtml(NavList, {
			selectedId: 'b',
			rows: [
				{
					id: 'a',
					label: 'Alpha',
					href: '/a',
					glyph: 'folder',
					level: 0,
					parentId: null,
					expandable: true,
					expanded: false
				},
				{
					id: 'b',
					label: 'Beta',
					href: '/b',
					glyph: 'folder',
					level: 1,
					parentId: 'a',
					count: 120
				},
				{
					id: 'c',
					label: 'Gamma',
					href: '/c',
					glyph: 'folder',
					level: 0,
					parentId: null,
					attrs: { 'data-tree-kind': 'forms' }
				}
			]
		});
		assertEqual(count(html, /<a [^>]*data-nav-row/g), 3, 'three links');
		assertEqual(count(html, /aria-current="page"/g), 1, 'one current item');
		assert(
			/<a [^>]*href="\/b"[^>]*aria-current="page"|<a [^>]*aria-current="page"[^>]*href="\/b"/.test(
				html
			),
			'Beta is the current link'
		);
		assertEqual(
			count(html, /aria-expanded="false"/g),
			1,
			'only the expandable row has aria-expanded'
		);
		assert(html.includes('data-tree-kind="forms"'), 'data attributes are rendered');
		assert(html.includes('99+'), 'count is capped');
		assert(
			/class="chevron[^"]*"[^>]*aria-hidden="true"/.test(html) ||
				/aria-hidden="true"[^>]*class="chevron/.test(html),
			'chevron is hidden from assistive technology'
		);
		assertEqual(count(html, /<ul /g), 1, 'one list');
	}
);

await suite.checkAsync(
	'projects column: only the workspace projects, collapsed by default',
	async () => {
		const html = await projectsHtml(`${base('p1')}/issues`);
		assertEqual(count(html, /data-tree-kind="project"/g), 2, 'two projects of this workspace');
		assert(!html.includes('Elsewhere'), 'other workspace hidden');
		assertEqual(
			count(html, /aria-label="shell\.column\.projects\.current"/g),
			0,
			'no Project nav while collapsed'
		);
	}
);

await suite.checkAsync(
	'projects column: the current project children sit in nav "Project"',
	async () => {
		const html = await projectsHtml(`${base('p1')}/issues`, {
			expanded: ['p1', 'p2'],
			admin: true
		});
		assertEqual(
			count(html, /<nav aria-label="shell\.column\.projects\.current"/g),
			1,
			'exactly one Project nav'
		);
		const nav = html.slice(
			html.indexOf('<nav aria-label="shell.column.projects.current"'),
			html.indexOf('</nav>', html.indexOf('<nav aria-label="shell.column.projects.current"'))
		);
		for (const kind of ['overview', 'issues', 'board', 'cycles', 'forms', 'apps', 'settings'])
			assert(nav.includes(`data-tree-kind="${kind}"`), `${kind} inside the Project nav`);
		assert(
			nav.includes('data-project-id="p1"') && !nav.includes('data-project-id="p2"'),
			'only the current project'
		);
		assertEqual(
			count(html, /data-project-id="p2"/g) >= 2,
			true,
			'the other expanded project still lists its children outside the nav'
		);
		assert(
			/data-tree-kind="issues"[^>]*aria-current="page"|aria-current="page"[^>]*data-tree-kind="issues"/.test(
				html
			),
			'Issues is the current item'
		);
		assertEqual(count(html, /aria-current="page"/g), 1, 'one current item in the column');
	}
);

await suite.checkAsync(
	'projects column: a member sees no Settings row and no empty Apps row',
	async () => {
		const html = await projectsHtml(`${base('p1')}`, { expanded: ['p1'], admin: false });
		assert(!html.includes('data-tree-kind="settings"'), 'no Settings');
		assert(!html.includes('data-tree-kind="apps"'), 'no Apps while the count is zero or unknown');
	}
);

await suite.checkAsync(
	'projects column: a form of the open project is listed and current',
	async () => {
		const data = new ColumnData(makeApis());
		await data.loadForms('p2');
		const url = `${base('p2')}/forms?form=f1`;
		const html = await projectsHtml(url, { expanded: ['p2'], data });
		// The Forms group opens by itself when a form is on screen (an effect), so SSR shows it closed;
		// the group row must still exist and the project row must be the current item.
		assert(html.includes('data-tree-kind="forms"'), 'Forms group row');
		assertEqual(count(html, /aria-current="page"/g), 1, 'one current item');
	}
);

await suite.checkAsync('projects column: loading, failed and empty states', async () => {
	const loading = await projectsHtml(`/workspace/${W}/projects`, {
		options: optionsOf({ loading: true, loaded: false })
	});
	assert(
		loading.includes('data-column-skeleton') && loading.includes('data-kind="loading"'),
		'skeleton with a status'
	);
	const failed = await projectsHtml(`/workspace/${W}/projects`, {
		options: optionsOf({ loaded: false, error: 'x' })
	});
	assert(failed.includes('data-kind="loadFailed"'), 'load failed block');
	assert(failed.includes('<button'), 'retry button');
	const empty = await projectsHtml(`/workspace/${W}/projects`, {
		options: optionsOf({ items: [] })
	});
	assert(empty.includes('data-kind="empty"'), 'empty block');
});

await suite.checkAsync(
	'projects column: personal scope lists workspaces, with states',
	async () => {
		const data = new ColumnData(makeApis());
		await data.loadWorkspaces();
		const html = await projectsHtml('/workspace', { scope: 'personal', data });
		assertEqual(count(html, /data-workspace-id="w1"/g), 1, 'one workspace row');
		assert(html.includes('href="/workspace/w1/projects"'), 'row goes to the workspace projects');
		assert(html.includes('shell.column.projects.personalCaption'), 'caption');
		const idle = await projectsHtml('/workspace', { scope: 'personal' });
		assert(idle.includes('data-column-skeleton'), 'skeleton before the first load');
		const bad = new ColumnData(
			makeApis({ workspaces: async () => ({ code: 500, message: 'x', data: null }) })
		);
		await bad.loadWorkspaces();
		assert(
			(await projectsHtml('/workspace', { scope: 'personal', data: bad })).includes(
				'data-kind="loadFailed"'
			),
			'failed'
		);
		const denied = new ColumnData(
			makeApis({ workspaces: async () => ({ code: 403, message: 'x', data: null }) })
		);
		await denied.loadWorkspaces();
		assert(
			(await projectsHtml('/workspace', { scope: 'personal', data: denied })).includes(
				'data-kind="forbidden"'
			),
			'no permission'
		);
	}
);

async function agentsHtml(scope: string, admin: boolean, data = new ColumnData(makeApis())) {
	return renderToHtml(AgentsColumn, {
		scope,
		url: urlOf('/agents'),
		viewer: viewer({ workspaceAdmin: admin }),
		data
	});
}

await suite.checkAsync('agents column: personal scope is one empty state', async () => {
	const html = await agentsHtml('personal', true);
	assert(html.includes('data-kind="empty"'), 'empty state');
	assert(html.includes('shell.column.agents.personalEmpty'), 'its message');
	assert(!html.includes('data-column-group'), 'no lists');
});

await suite.checkAsync(
	'agents column: bots only for admins; connected bots group for everyone',
	async () => {
		const admin = await agentsHtml(W, true);
		assert(admin.includes('data-column-group="bots"'), 'admin sees bots group');
		assert(admin.includes('data-column-group="connected"'), 'connected group');
		assert(admin.includes('shell.column.agents.noConnection'), 'empty line until connections list');
		assert(!admin.includes('data-agent-kind="project"'), 'no participant rows');
		const member = await agentsHtml(W, false);
		assert(!member.includes('data-column-group="bots"'), 'member never requests or sees bots');
		assert(member.includes('data-column-group="connected"'), 'member sees the connected group');
		const data = new ColumnData(makeApis());
		await data.loadBots(W);
		const loaded = await agentsHtml(W, true, data);
		assert(
			loaded.includes('Deploy bot') && loaded.includes('href="/workspace/w1/members"'),
			'bot row links to members'
		);
	}
);

await suite.checkAsync(
	'agents column: a failed bots list does not hide the connected group',
	async () => {
		const data = new ColumnData(
			makeApis({ bots: async () => ({ code: 500, message: 'x', data: null }) })
		);
		await data.loadBots(W);
		const html = await agentsHtml(W, true, data);
		assert(html.includes('data-kind="loadFailed"'), 'bots failed');
		assert(html.includes('data-column-group="connected"'), 'connected group still shown');
	}
);

await suite.checkAsync('flow column: rows, notice and admin link', async () => {
	const render = (
		scope: string,
		flow: { personal: boolean; workspaces: Record<string, boolean | undefined> },
		admin = false
	) =>
		renderToHtml(FlowColumn, {
			scope,
			url: urlOf('/flow'),
			viewer: { ...viewer({ workspaceAdmin: admin }), flow },
			data: new ColumnData(makeApis()),
			optionsStore: optionsOf({ items: projects })
		});
	const on = await render(W, { personal: true, workspaces: { [W]: true } });
	assert(on.includes('data-testid="global-flow-link"'), 'Private row keeps its test id');
	assert(on.includes('data-flow-section="workspace"'), 'workspace row');
	assert(
		/data-flow-section="private"[^>]*aria-current|aria-current[^>]*data-flow-section="private"/.test(
			on
		),
		'Private is current at /flow'
	);
	const off = await render(W, { personal: false, workspaces: { [W]: false } }, true);
	assert(
		off.includes('shell.column.flow.off') && off.includes('href="/workspace/w1/settings/flow"'),
		'off notice with admin link'
	);
	const member = await render(W, { personal: false, workspaces: { [W]: false } }, false);
	assert(
		member.includes('shell.column.flow.off') && !member.includes('/settings/flow'),
		'member gets no link'
	);
	assert(
		(await render(W, { personal: false, workspaces: {} })).includes('data-kind="loading"'),
		'unknown flag shows loading'
	);
	assert(
		(await render('personal', { personal: false, workspaces: {} })).includes(
			'data-column-note="unavailable-personal"'
		),
		'personal unavailable'
	);
});

async function columnHtml(destination: string, path: string, over: Record<string, unknown> = {}) {
	const url = urlOf(path);
	const shell = (over.shell as ReturnType<typeof makeShell>) ?? makeShell();
	return renderToHtml(SecondaryColumn, {
		destination,
		ctx: resolveRoute(url, null),
		url,
		shell,
		viewer: viewer({ instanceAdmin: true, unread: 4 }),
		windowWidth: 1440,
		data: new ColumnData(makeApis()),
		...over
	});
}

await suite.checkAsync(
	'column frame: element, size, header controls and the destination nav',
	async () => {
		const wide = await columnHtml('home', '/inbox');
		assert(
			wide.includes('data-shell="column"') &&
				wide.includes('data-destination="home"') &&
				wide.includes('data-collapsed="false"'),
			'attributes'
		);
		assert(wide.includes('width: 264px'), '264 wide at 1440');
		assert(wide.includes('width: 232px') === false, 'not 232');
		assert(
			/<nav[^>]*aria-label="shell\.destinations\.home"/.test(wide),
			'nav named by the destination'
		);
		assert(
			wide.includes('data-shell="scope-switcher"') && wide.includes('data-scope="w1"'),
			'scope switcher with the scope'
		);
		assert(wide.includes('data-shell="collapse-column"'), 'collapse button');
		assert(wide.includes('data-shell="search"'), 'search field');
		assert(
			/href="\/inbox"[^>]*aria-current="page"|aria-current="page"[^>]*href="\/inbox"/.test(wide),
			'Notifications is current'
		);
		assert(wide.includes('>4<'), 'unread count');
		const mid = await columnHtml('home', '/inbox', { windowWidth: 1100 });
		assert(mid.includes('width: 232px'), '232 wide at 1100');
		const narrow = await columnHtml('home', '/inbox', { windowWidth: 700 });
		assert(narrow.includes('width: 100%'), 'full width at 700');
		assert(
			!narrow.includes('data-shell="collapse-column"'),
			'no collapse button in the narrow layout'
		);
	}
);

await suite.checkAsync('column frame: collapsed column is a zero-width placeholder', async () => {
	const shell = makeShell();
	shell.setCollapsed('home', true);
	const html = await columnHtml('home', '/inbox', { shell });
	assert(html.includes('data-collapsed="true"'), 'collapsed flag');
	assert(!html.includes('<nav'), 'nothing rendered inside');
	const narrow = await columnHtml('home', '/inbox', { shell, windowWidth: 700 });
	assert(narrow.includes('data-collapsed="false"'), 'collapse does not apply in the narrow layout');
});

await suite.checkAsync(
	'column frame: Flow starts collapsed on a canvas route until chosen',
	async () => {
		const shell = makeShell();
		const canvas = await columnHtml('flow', '/flow', { shell, routeId: '/(app)/flow' });
		assert(canvas.includes('data-collapsed="true"'), 'collapsed on the canvas route');
		const other = await columnHtml('flow', '/flow/package', {
			shell,
			routeId: '/(app)/flow/package'
		});
		assert(other.includes('data-collapsed="false"'), 'expanded elsewhere');
	}
);

await suite.checkAsync('column frame: governance destination lists its links', async () => {
	const html = await columnHtml('governance', '/proposals');
	assert(html.includes('data-governance-row="proposals"'), 'proposals row');
	assert(html.includes('data-governance-row="config"'), 'admin row');
	assert(
		/data-governance-row="proposals"[^>]*aria-current|aria-current[^>]*data-governance-row="proposals"/.test(
			html
		),
		'proposals current'
	);
});

await suite.checkAsync('back to list row: a named button', async () => {
	const html = await renderToHtml(BackToListRow, { label: 'Projects', onclick: () => {} });
	assert(html.includes('data-shell="back-to-list"') && html.includes('<button'), 'button');
	assert(html.includes('>Projects<'), 'shows the column title');
});

finish(suite.result());
