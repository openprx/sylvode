/**
 * Rail, top bar and shortcut editor: model derivation (selection, unavailable items, unread chip,
 * pinning the current page) and server-rendered markup (roles, names, current item).
 *
 * Run standalone: `bun tests/shell-rail.test.ts`
 */
import { Suite, assert, assertEqual, finish } from './support/harness';
import { MemoryStore } from './support/memory-store';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import { installRuneModules } from './shell-rune-loader';
import { resolveRoute } from '../src/lib/shell/route-map';

await installSvelteSsr();
await installRuneModules();
const { buildRailModel, currentPageShortcut, unreadBadge } =
	await import('../src/lib/shell/rail/rail-model');
const { RailShortcuts, governanceShortcut } = await import('../src/lib/shell/shortcuts.svelte');
const { default: Rail } = await import('../src/lib/shell/rail/Rail.svelte');
const { default: NarrowTopBar } = await import('../src/lib/shell/rail/NarrowTopBar.svelte');
const { default: RailItem } = await import('../src/lib/shell/rail/RailItem.svelte');
const { default: ShortcutEditor } = await import('../src/lib/shell/rail/ShortcutEditor.svelte');

const suite = new Suite('shell-rail');
const url = (path: string) => new URL(path, 'http://x');
const t = (key: string, options?: { values?: Record<string, string | number> }) =>
	options?.values ? `${key}|${Object.values(options.values).join(',')}` : key;
const flowOn = { personal: true, workspaces: { w1: true } };

function model(path: string, extra: Partial<Parameters<typeof buildRailModel>[0]> = {}) {
	const u = url(path);
	return buildRailModel({
		ctx: resolveRoute(u, null),
		url: u,
		scope: 'w1',
		flow: flowOn,
		lastFor: () => undefined,
		shortcuts: [],
		availability: () => ({ state: 'unknown' }),
		unread: 0,
		t,
		...extra
	});
}

const account = {
	user: { name: 'Ada Lovelace', email: 'ada@example.test' },
	theme: 'system' as const,
	locale: 'en' as const,
	ontheme: () => {},
	onlocale: () => {},
	onsignout: () => {}
};
const shortcuts = () => new RailShortcuts({ userId: 'u1', storage: new MemoryStore() });
const noop = () => {};

suite.check('three destinations in order, the current one selected', () => {
	const m = model('/workspace/w1/projects/p1/issues');
	assertEqual(m.destinations.map((d) => d.id).join(','), 'home,projects,flow', 'order');
	assertEqual(
		m.destinations
			.filter((d) => d.selected)
			.map((d) => d.id)
			.join(','),
		'projects',
		'selected'
	);
	assertEqual(m.settings.selected, false, 'settings not selected');
});

suite.check('settings mode selects Settings and no destination', () => {
	const m = model('/workspace/w1/members');
	assertEqual(
		m.destinations.some((d) => d.selected),
		false,
		'no destination'
	);
	assertEqual(m.settings.selected, true, 'settings');
});

suite.check('an item links to the remembered location, else the scope root', () => {
	const none = model('/workspace/w1/projects');
	assertEqual(none.destinations[1].href, '/workspace/w1/projects', 'root');
	const remembered = model('/workspace/w1/projects', {
		lastFor: (key) => (key === 'projects' ? '/workspace/w1/projects/p9/issues' : undefined)
	});
	assertEqual(remembered.destinations[1].href, '/workspace/w1/projects/p9/issues', 'last');
});

suite.check('Flow off in the workspace: unavailable, no href, with a reason', () => {
	const m = model('/workspace/w2', {
		scope: 'w2',
		flow: { personal: true, workspaces: { w2: false } }
	});
	const flow = m.destinations[2];
	assertEqual(flow.href, null, 'not a link');
	assert(flow.unavailableReason !== null, 'reason');
});

suite.check('unread count: chip text, 99+ cap, accessible name', () => {
	assertEqual(unreadBadge(0), null, 'zero');
	assertEqual(unreadBadge(7), '7', 'seven');
	assertEqual(unreadBadge(100), '99+', 'cap');
	const home = model('/workspace', { unread: 120 }).destinations[0];
	assertEqual(home.badge, '99+', 'badge');
	assert(home.accessibleName.startsWith('shell.rail.homeUnread'), home.accessibleName);
	const quiet = model('/workspace').destinations[0];
	assertEqual(quiet.accessibleName, quiet.label, 'name without badge');
});

suite.check('a selected shortcut takes the mark from its destination', () => {
	const items = [
		{
			id: 'project:p1',
			kind: 'project' as const,
			label: 'Alpha',
			href: '/workspace/w1/projects/p1'
		}
	];
	const m = model('/workspace/w1/projects/p1/issues', { shortcuts: items });
	assertEqual(
		m.destinations.some((d) => d.selected),
		false,
		'destination not selected'
	);
	assertEqual(m.shortcuts[0].selected, true, 'shortcut selected');
	const other = model('/workspace/w1/projects/p2', { shortcuts: items });
	assertEqual(other.destinations[1].selected, true, 'projects selected elsewhere');
});

suite.check('the Governance shortcut is selected on any governance page', () => {
	const m = model('/proposals/abc', { shortcuts: [governanceShortcut('Governance')] });
	assertEqual(m.shortcuts[0].selected, true, 'selected');
});

suite.check('an unavailable shortcut is not a link and not selected', () => {
	const items = [
		{
			id: 'project:p1',
			kind: 'project' as const,
			label: 'Alpha',
			href: '/workspace/w1/projects/p1'
		}
	];
	const m = model('/workspace/w1/projects/p1', {
		shortcuts: items,
		availability: () => ({ state: 'unavailable', reason: 'shell.shortcuts.unavailable' })
	});
	assertEqual(m.shortcuts[0].href, null, 'href');
	assertEqual(m.shortcuts[0].selected, false, 'selected');
	assertEqual(m.shortcuts[0].unavailableReason, 'shell.shortcuts.unavailable', 'reason');
});

suite.check('currentPageShortcut: form, project, flow page, nothing', () => {
	const pin = (path: string, typeKey?: string) =>
		currentPageShortcut({
			ctx: resolveRoute(url(path), null),
			url: url(path),
			label: 'Name',
			projectTypeKey: typeKey
		});
	assertEqual(pin('/workspace/w1/projects/p1/forms?form=f1')?.id, 'form:f1', 'form');
	assertEqual(pin('/workspace/w1/projects/p1/issues')?.id, 'project:p1', 'project');
	assertEqual(
		pin('/workspace/w1/projects/p1/issues', 'custom_form')?.href,
		'/workspace/w1/projects/p1/forms',
		'custom form home'
	);
	assertEqual(pin('/workspace/w1/flow/o1')?.kind, 'flow-page', 'flow');
	assertEqual(pin('/workspace/w1/members'), null, 'settings page');
	assertEqual(pin('/inbox'), null, 'inbox');
});

const attrs = (html: string, tag: RegExp) => tag.exec(html)?.[0] ?? '';

await suite.checkAsync('Rail renders nav[aria-label=Primary] with the current item', async () => {
	const m = model('/workspace/w1/projects');
	const html = await renderToHtml(Rail, {
		model: m,
		columnExpanded: true,
		ontogglecolumn: noop,
		account,
		shortcuts: shortcuts(),
		currentPage: null,
		confirmRestore: async () => true
	});
	const nav = attrs(html, /<nav[^>]*>/);
	assert(nav.includes('aria-label="shell.rail.label"') && nav.includes('data-layout="rail"'), nav);
	assertEqual((html.match(/data-rail-destination="/g) ?? []).length, 3, 'three destinations');
	const projects = attrs(html, /<a[^>]*data-rail-destination="projects"[^>]*>/);
	assert(
		projects.includes('aria-current="page"') && projects.includes('aria-expanded="true"'),
		projects
	);
	const home = attrs(html, /<a[^>]*data-rail-destination="home"[^>]*>/);
	assert(!home.includes('aria-current'), home);
	assert(
		html.includes('data-shell="settings"') && html.includes('data-shell="account"'),
		'bottom group'
	);
	assert(html.includes('data-shell="edit-navigation"'), 'edit navigation');
	assert(html.includes('aria-label="shell.rail.account"'), 'account name');
});

await suite.checkAsync('Rail: exactly one destination is a Tab stop', async () => {
	const html = await renderToHtml(Rail, {
		model: model('/workspace/w1/projects'),
		columnExpanded: false,
		ontogglecolumn: noop,
		account,
		shortcuts: shortcuts(),
		currentPage: null,
		confirmRestore: async () => true
	});
	const stops = [...html.matchAll(/<a[^>]*data-rail-destination[^>]*>/g)].filter((m) =>
		m[0].includes('tabindex="0"')
	);
	assertEqual(stops.length, 1, 'one tab stop');
	assert(stops[0][0].includes('data-rail-destination="projects"'), 'on the selected item');
});

await suite.checkAsync('RailItem: unavailable is a described, aria-disabled button', async () => {
	const html = await renderToHtml(RailItem, {
		item: {
			...model('/workspace/w2', { scope: 'w2', flow: { personal: false, workspaces: {} } })
				.destinations[2]
		}
	});
	assert(html.includes('<button') && html.includes('aria-disabled="true"'), html);
	assert(html.includes('aria-describedby=') && !html.includes('href='), html);
	assert(!html.includes('<a '), 'not a link');
	assert(html.includes('shell.destinations.flowUnavailableWorkspace'), 'reason text');
});

await suite.checkAsync(
	'RailItem: the Home unread chip is hidden from the name and badge shows',
	async () => {
		const item = model('/workspace', { unread: 3 }).destinations[0];
		const html = await renderToHtml(RailItem, { item });
		assert(html.includes('aria-label="shell.rail.homeUnread'), html);
		assert(html.includes('data-shell="rail-badge"') && html.includes('>3<'), html);
	}
);

await suite.checkAsync(
	'NarrowTopBar: same nav name, data-layout=topbar, text destinations',
	async () => {
		const html = await renderToHtml(NarrowTopBar, {
			model: model('/workspace/w1/projects'),
			ondestination: noop,
			account,
			shortcuts: shortcuts(),
			currentPage: null,
			confirmRestore: async () => true
		});
		const nav = attrs(html, /<nav[^>]*>/);
		assert(
			nav.includes('aria-label="shell.rail.label"') && nav.includes('data-layout="topbar"'),
			nav
		);
		assertEqual((html.match(/data-rail-destination="/g) ?? []).length, 3, 'three destinations');
		assert(html.includes('data-shell="rail-more"'), 'more button');
	}
);

await suite.checkAsync('ShortcutEditor lists locked destinations and the shortcuts', async () => {
	const store = shortcuts();
	store.add(governanceShortcut('Governance'));
	const html = await renderToHtml(ShortcutEditor, {
		open: true,
		anchor: null,
		onclose: noop,
		shortcuts: store,
		destinations: model('/workspace').destinations,
		current: null,
		confirmRestore: async () => true
	});
	assertEqual((html.match(/data-editor-fixed=/g) ?? []).length, 3, 'fixed rows');
	assert(html.includes('data-editor-shortcut="governance:governance"'), 'shortcut row');
	assert(html.includes('shell.rail.editor.restore'), 'restore');
});

await suite.checkAsync('ShortcutEditor: nothing to add disables Add with a reason', async () => {
	const store = shortcuts();
	store.add(governanceShortcut('Governance'));
	const html = await renderToHtml(ShortcutEditor, {
		open: true,
		anchor: null,
		onclose: noop,
		shortcuts: store,
		destinations: [],
		current: null,
		confirmRestore: async () => true
	});
	assert(html.includes('shell.rail.editor.nothingToAdd'), 'reason shown');
});

finish(suite.result());
