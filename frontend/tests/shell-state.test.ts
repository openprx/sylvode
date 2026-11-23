/**
 * Shell state: storage parsing, scope, collapse, expanded projects, last locations, recent items,
 * settings return URL, and the rail shortcut store (`lib/shell/{storage,shell-state,shortcuts}`).
 *
 * Run standalone: `bun tests/shell-state.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { MemoryStore } from './support/memory-store';
import { installRuneModules } from './shell-rune-loader';
import { resolveRoute } from '../src/lib/shell/route-map';
import {
	SETTINGS_RETURN_KEY,
	parseShellFile,
	safeInAppPath,
	shellStateKey,
	shortcutsKey
} from '../src/lib/shell/storage';

await installRuneModules();
const { ShellState, scopeOfRoute } = await import('../src/lib/shell/shell-state.svelte');
const shortcutsModule = await import('../src/lib/shell/shortcuts.svelte');
const { RailShortcuts, MAX_SHORTCUTS, railSelection, selectedShortcutId, shortcutId } =
	shortcutsModule;

const suite = new Suite('shell-state');
const url = (path: string) => new URL(path, 'http://x');
const ctx = (path: string) => resolveRoute(url(path), null);

suite.check('safeInAppPath accepts in-app paths only and strips secrets', () => {
	assertEqual(
		safeInAppPath('/workspace/w/projects?x=1#h'),
		'/workspace/w/projects?x=1',
		'query kept, fragment dropped'
	);
	assertEqual(safeInAppPath('/a?token=s&b=2'), '/a?b=2', 'secret removed');
	for (const bad of [
		'//evil.com',
		'https://evil.com',
		'/\\evil',
		'relative',
		'',
		42,
		null,
		'/a\nb'
	])
		assertEqual(safeInAppPath(bad), null, String(bad));
});

suite.check('parseShellFile keeps valid data and drops the rest', () => {
	assertDeepEqual(parseShellFile('junk'), parseShellFile(null), 'junk is defaults');
	assertEqual(parseShellFile({ version: 2, scope: 'x' }).scope, 'personal', 'unknown version');
	const file = parseShellFile({
		version: 1,
		scope: 'w1',
		collapsed: { flow: true, home: 'yes', nope: true },
		expanded: { w1: ['a', 'a', 7, 'b'], w2: 'x' },
		last: { projects: { w1: '/workspace/w1/projects/p', w2: '//evil' }, bogus: { w1: '/x' } },
		recent: [
			{ href: '/flow/a', title: 'A' },
			{ href: '/flow/a', title: 'dup' },
			{ href: 'https://x', title: 'bad' },
			{ href: '/flow/b' }
		]
	});
	assertEqual(file.scope, 'w1', 'scope');
	assertDeepEqual(file.collapsed, { flow: true }, 'collapsed');
	assertDeepEqual(file.expanded, { w1: ['a', 'b'] }, 'expanded');
	assertDeepEqual(file.last, { projects: { w1: '/workspace/w1/projects/p' } }, 'last');
	assertDeepEqual(file.recent, [{ href: '/flow/a', title: 'A' }], 'recent');
	const many = parseShellFile({
		version: 1,
		expanded: { w: Array.from({ length: 80 }, (_, i) => `p${i}`) }
	});
	assertEqual(many.expanded.w.length, 50, 'expanded capped at 50');
	assertEqual(many.expanded.w[49], 'p79', 'keeps the newest');
});

suite.check('scope: workspace URL wins, personal Flow is personal, else unchanged', () => {
	assertEqual(
		scopeOfRoute(url('/workspace/w9/projects'), ctx('/workspace/w9/projects'), 'personal'),
		'w9',
		'workspace'
	);
	assertEqual(scopeOfRoute(url('/flow/o'), ctx('/flow/o'), 'w1'), 'personal', 'personal flow');
});

suite.check('state persists per user and survives a reload', () => {
	const local = new MemoryStore();
	const session = new MemoryStore();
	const a = new ShellState({ userId: 'a', local, session });
	assertEqual(a.scope, 'personal', 'default scope');
	a.setScope('w1');
	a.setCollapsed('projects', true);
	a.setProjectExpanded('w1', 'p1', true);
	a.setProjectExpanded('w1', 'p2', true);
	a.setProjectExpanded('w1', 'p1', false);
	const b = new ShellState({ userId: 'a', local, session });
	assertEqual(b.scope, 'w1', 'scope restored');
	assertEqual(b.isCollapsed('projects'), true, 'collapse restored');
	assertDeepEqual([...b.expandedProjects('w1')], ['p2'], 'expansion restored');
	const other = new ShellState({ userId: 'b', local, session });
	assertEqual(other.scope, 'personal', 'another user starts clean');
	assert(local.data.has(shellStateKey('a')), 'key per user');
	const anonymous = new ShellState({ userId: null, local, session });
	anonymous.setScope('w3');
	assertEqual(local.data.size, 1, 'no user, no writes');
});

suite.check('unreadable storage never throws', () => {
	const local = new MemoryStore();
	local.failWrites = true;
	const state = new ShellState({ userId: 'a', local, session: null });
	state.setScope('w1');
	state.toggleCollapsed('home');
	assertEqual(state.scope, 'w1', 'memory state still changes');
	local.data.set(shellStateKey('z'), '{not json');
	assertEqual(new ShellState({ userId: 'z', local, session: null }).scope, 'personal', 'bad JSON');
});

suite.check('collapse defaults: expanded, Flow collapsed on canvas routes', () => {
	const state = new ShellState({ userId: 'a', local: new MemoryStore(), session: null });
	assertEqual(state.isCollapsed('home'), false, 'home');
	assertEqual(state.isCollapsed('flow'), false, 'flow off canvas');
	assertEqual(state.isCollapsed('flow', { flowCanvas: true }), true, 'flow on canvas');
	assertEqual(state.toggleCollapsed('flow', { flowCanvas: true }), false, 'toggle expands');
	assertEqual(state.isCollapsed('flow', { flowCanvas: true }), false, 'stored choice wins');
});

suite.check('last location is remembered per destination and scope', () => {
	const state = new ShellState({ userId: 'a', local: new MemoryStore(), session: null });
	state.observe(
		url('/workspace/w1/projects/p1/issues?x=1'),
		ctx('/workspace/w1/projects/p1/issues')
	);
	state.observe(url('/workspace/w2/projects'), ctx('/workspace/w2/projects'));
	state.observe(url('/flow/o1'), ctx('/flow/o1'));
	assertEqual(state.lastFor('projects', 'w1'), '/workspace/w1/projects/p1/issues?x=1', 'w1');
	assertEqual(state.lastFor('projects', 'w2'), '/workspace/w2/projects', 'w2');
	assertEqual(state.lastFor('flow', 'personal'), '/flow/o1', 'personal flow');
	assertEqual(state.scope, 'personal', 'personal flow switches scope');
	state.observe(url('/workspace/w1/members'), ctx('/workspace/w1/members'));
	assertEqual(state.lastFor('settings', 'w1'), '/workspace/w1/members', 'settings location');
});

suite.check('settings return URL is the last non-settings location', () => {
	const session = new MemoryStore();
	const state = new ShellState({ userId: 'a', local: new MemoryStore(), session });
	assertEqual(state.settingsReturn, null, 'unknown at first');
	state.observe(url('/workspace/w1/projects'), ctx('/workspace/w1/projects'));
	state.observe(url('/settings'), ctx('/settings'));
	assertEqual(state.settingsReturn, '/workspace/w1/projects', 'stored on entry');
	state.observe(url('/plugins'), ctx('/plugins'));
	assertEqual(state.settingsReturn, '/workspace/w1/projects', 'moving inside settings keeps it');
	assertEqual(
		JSON.parse(session.data.get(SETTINGS_RETURN_KEY) ?? 'null'),
		'/workspace/w1/projects',
		'sessionStorage'
	);
	const reopened = new ShellState({ userId: 'a', local: new MemoryStore(), session });
	assertEqual(reopened.settingsReturn, '/workspace/w1/projects', 'survives a reload in the tab');
});

suite.check('recent keeps task resources only, newest first, max 8', () => {
	const state = new ShellState({ userId: 'a', local: new MemoryStore(), session: null });
	state.recordVisit('/inbox', 'Inbox');
	assertEqual(state.recent.length, 0, 'not a task resource');
	for (let i = 0; i < 10; i++) state.recordVisit(`/flow/page-${i}`, `Page ${i}`);
	assertEqual(state.recent.length, 8, 'capped');
	assertEqual(state.recent[0].href, '/flow/page-9', 'newest first');
	state.recordVisit('/flow/page-5?token=s', 'Five');
	assertEqual(state.recent[0].href, '/flow/page-5', 'secret stripped');
	assertEqual(state.recent.filter((r) => r.href === '/flow/page-5').length, 1, 'no duplicates');
	state.clearRecent();
	assertEqual(state.recent.length, 0, 'cleared');
});

suite.check('narrow view is memory only', () => {
	const local = new MemoryStore();
	const state = new ShellState({ userId: 'a', local, session: null });
	state.showContent();
	assertEqual(state.narrowView, 'content', 'content');
	assertEqual(new ShellState({ userId: 'a', local, session: null }).narrowView, 'list', 'resets');
});

const sc = (
	kind: 'project' | 'form' | 'flow-page' | 'governance' | 'view',
	objectId: string,
	href: string,
	extra = {}
) => ({
	id: shortcutId(kind, objectId),
	kind,
	label: objectId,
	href,
	objectId,
	...extra
});

suite.check('shortcuts: add, duplicate, cap, move, remove, restore, persist', () => {
	const storage = new MemoryStore();
	const rail = new RailShortcuts({ userId: 'a', storage });
	assertEqual(rail.add(sc('project', 'p1', '/workspace/w/projects/p1')).ok, true, 'add');
	const again = rail.add({ ...sc('project', 'p1', '/workspace/w/projects/p1'), label: 'Renamed' });
	assertDeepEqual(again, { ok: true, added: false }, 'duplicate adds nothing');
	assertEqual(rail.items[0].label, 'Renamed', 'label refreshed');
	rail.add(sc('flow-page', 'o1', '/flow/o1'));
	assertEqual(rail.move('flow-page:o1', -1), true, 'move up');
	assertDeepEqual(
		rail.items.map((i) => i.id),
		['flow-page:o1', 'project:p1'],
		'order'
	);
	assertEqual(rail.move('flow-page:o1', -1), false, 'top edge');
	assertEqual(new RailShortcuts({ userId: 'a', storage }).items.length, 2, 'persisted');
	assert(storage.data.has(shortcutsKey('a')), 'key per user');
	for (let i = 0; i < MAX_SHORTCUTS - 2; i++)
		rail.add(sc('project', `x${i}`, `/workspace/w/projects/x${i}`));
	const full = rail.add(sc('project', 'late', '/workspace/w/projects/late'));
	assertEqual(full.ok, false, 'ninth refused');
	assert(!full.ok && full.reason === 'shell.shortcuts.full', 'inline reason key');
	rail.remove('project:p1');
	assertEqual(rail.has('project:p1'), false, 'removed');
	rail.restoreDefault();
	assertEqual(rail.items.length, 0, 'restored');
});

suite.check('shortcuts: invalid stored data is ignored; hrefs must be in-app', () => {
	const storage = new MemoryStore();
	storage.data.set(
		shortcutsKey('a'),
		JSON.stringify({
			version: 1,
			items: [
				{ id: 'x', kind: 'project', label: 'X', href: 'https://evil' },
				{ id: 'project:ok', kind: 'project', label: 'OK', href: '/ok' },
				5
			]
		})
	);
	const rail = new RailShortcuts({ userId: 'a', storage });
	assertDeepEqual(
		rail.items.map((i) => i.id),
		['project:ok'],
		'only the valid item'
	);
	assertEqual(rail.add(sc('project', 'bad', 'https://evil')).ok, false, 'external href refused');
});

await suite.checkAsync(
	'shortcuts: availability is checked once, two at a time, only gone is unavailable',
	async () => {
		const rail = new RailShortcuts({ userId: 'a', storage: new MemoryStore() });
		rail.add(sc('project', 'p1', '/p1'));
		rail.add(sc('form', 'f1', '/f1', { formId: 'f1' }));
		rail.add(sc('flow-page', 'o1', '/flow/o1'));
		rail.add(sc('flow-page', 'o2', '/flow/o2'));
		rail.add(sc('governance', 'governance', '/governance'));
		let running = 0;
		let peak = 0;
		const calls: string[] = [];
		const outcomes: Record<string, 'available' | 'gone' | 'error'> = {
			'project:p1': 'available',
			'form:f1': 'gone',
			'flow-page:o1': 'error',
			'flow-page:o2': 'available'
		};
		const check = async (s: { id: string }) => {
			calls.push(s.id);
			running++;
			peak = Math.max(peak, running);
			await new Promise((r) => setTimeout(r, 5));
			running--;
			return outcomes[s.id];
		};
		await rail.validate(check);
		assertEqual(peak, 2, 'two at a time');
		assertEqual(calls.includes('governance:governance'), false, 'governance is not checked');
		assertEqual(
			rail.availabilityOf('governance:governance').state,
			'available',
			'governance always available'
		);
		assertEqual(rail.availabilityOf('project:p1').state, 'available', 'available');
		const gone = rail.availabilityOf('form:f1');
		assert(
			gone.state === 'unavailable' && gone.reason === 'shell.shortcuts.unavailable',
			'gone is unavailable with a reason'
		);
		assertEqual(rail.availabilityOf('flow-page:o1').state, 'unknown', 'error stays unknown');
		await rail.validate(check);
		assertEqual(calls.length, 4, 'once per session');
		assertEqual(rail.items.length, 5, 'unavailable shortcuts stay until removed');
	}
);

suite.check('shortcut selection: segment boundary, longest path, form id', () => {
	const items = [
		sc('project', 'p1', '/workspace/w/projects/p1'),
		sc('form', 'f1', '/workspace/w/projects/p1/forms?form=f1', { formId: 'f1' }),
		sc('project', 'p10', '/workspace/w/projects/p10')
	];
	assertEqual(
		selectedShortcutId(items, url('/workspace/w/projects/p1/issues')),
		'project:p1',
		'project'
	);
	assertEqual(
		selectedShortcutId(items, url('/workspace/w/projects/p10')),
		'project:p10',
		'no prefix confusion'
	);
	assertEqual(
		selectedShortcutId(items, url('/workspace/w/projects/p1/forms?form=f1')),
		'form:f1',
		'form wins'
	);
	assertEqual(
		selectedShortcutId(items, url('/workspace/w/projects/p1/forms?form=f2')),
		'project:p1',
		'other form falls back'
	);
	assertEqual(selectedShortcutId(items, url('/inbox')), null, 'none');
});

suite.check('rail selection: a shortcut takes the mark from its destination', () => {
	const items = [sc('project', 'p1', '/workspace/w/projects/p1')];
	const on = railSelection(
		ctx('/workspace/w/projects/p1/board'),
		url('/workspace/w/projects/p1/board'),
		items
	);
	assertDeepEqual(
		on,
		{ column: 'projects', destination: null, shortcutId: 'project:p1', settings: false },
		'shortcut'
	);
	const off = railSelection(ctx('/workspace/w/projects'), url('/workspace/w/projects'), items);
	assertEqual(off.destination, 'projects', 'destination');
	const settings = railSelection(ctx('/settings'), url('/settings'), items);
	assertDeepEqual(
		settings,
		{ column: null, destination: null, shortcutId: null, settings: true },
		'settings'
	);
});

finish(suite.result());
