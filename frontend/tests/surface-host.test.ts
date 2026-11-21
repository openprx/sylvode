/**
 * Surface host contract: width classes, the standalone frame (ownership, tip, report, close,
 * syncState, task tab bridge), nested scopes, and the spec helpers of `useSurface`.
 *
 * Run standalone: `bun tests/surface-host.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { installRuneModules } from './shell-rune-loader';
import { widthClass } from '../src/lib/surface/width';
import type { SurfaceHost } from '../src/lib/surface/context';
import type { SurfaceSpec } from '../src/lib/surface/use-surface.svelte';

await installRuneModules();
const { StandaloneSurfaceHost, SurfaceScopeProvider, SURFACE_REPORT_TIP_ID, formatReport } =
	await import('../src/lib/surface/standalone-host.svelte');
const { applySpec, createInertHost } = await import('../src/lib/surface/use-surface.svelte');

const suite = new Suite('surface-host');
const translate = (key: string, values: Record<string, string> = {}) =>
	`${key}|${Object.entries(values)
		.map(([k, v]) => `${k}=${v}`)
		.join(';')}`;

function frame(initial = '/workspace/w/projects/p/issues') {
	let current = new URL(initial, 'http://x');
	const navigations: Array<{ url: string; options?: object }> = [];
	const taskLog: string[] = [];
	let active: string | null = 'forms:p:f1';
	const host = new StandaloneSurfaceHost({
		navigate: (url, options) => navigations.push({ url, options }),
		currentUrl: () => current,
		rootHref: () => '/workspace',
		translate,
		locale: () => 'en',
		tasks: {
			activeKey: () => active,
			setTitle: (key, title) => taskLog.push(`title:${key}:${title}`),
			markDirty: (key, dirty) => taskLog.push(`dirty:${key}:${dirty}`)
		},
		taskKey: () => 'forms:p:f1'
	});
	return {
		host,
		navigations,
		taskLog,
		go: (path: string) => {
			current = new URL(path, 'http://x');
			host.noteNavigation(current);
		},
		setActive: (key: string | null) => (active = key)
	};
}

suite.check('width classes: xs < 480 <= sm < 600 <= md < 900 <= full', () => {
	const cases: Array<[number, string]> = [
		[0, 'xs'],
		[340, 'xs'],
		[479, 'xs'],
		[480, 'sm'],
		[599, 'sm'],
		[600, 'md'],
		[899, 'md'],
		[900, 'full'],
		[1440, 'full']
	];
	for (const [px, expected] of cases) assertEqual(widthClass(px), expected, String(px));
});

suite.check(
	'the frame is empty until a surface attaches, then loading until a title arrives',
	() => {
		const { host } = frame();
		assertEqual(host.hasSurface, false, 'no surface');
		assertEqual(host.loading, false, 'not loading without a surface');
		const owner = Symbol('a');
		const surface = host.attach(owner);
		assertEqual(host.hasSurface, true, 'attached');
		assertEqual(host.loading, true, 'skeleton until the first title');
		surface.setTitle({ title: 'Issues' });
		assertEqual(host.loading, false, 'loaded');
		assertEqual(host.documentTitle, 'Issues · Sylvode', 'document title');
		surface.setTitle({ title: 'Issues', loading: true });
		assertEqual(host.documentTitle, null, 'no title while loading');
	}
);

suite.check('a newer owner supersedes the old one; a late detach clears nothing', () => {
	const { host } = frame();
	const oldOwner = Symbol('old');
	const newOwner = Symbol('new');
	const oldSurface = host.attach(oldOwner);
	oldSurface.setTitle({ title: 'Old' });
	const newSurface = host.attach(newOwner);
	newSurface.setTitle({ title: 'New' });
	oldSurface.setTitle({ title: 'Stale' });
	oldSurface.setTip({ id: 't', tone: 'danger', message: 'x' });
	oldSurface.setDirty(true);
	host.detach(oldOwner);
	assertEqual(host.title?.title, 'New', 'old setters are ignored');
	assertEqual(host.tip, null, 'stale tip ignored');
	assertEqual(host.dirty, false, 'stale dirty ignored');
	host.detach(newOwner);
	assertEqual(host.title, null, 'current owner clears');
	assertEqual(host.hasSurface, false, 'detached');
	newSurface.setTitle({ title: 'After destroy' });
	assertEqual(host.title, null, 'detached owner is ignored');
});

suite.check('actions, header, density and tip replace in place', () => {
	const { host } = frame();
	const s = host.attach(Symbol());
	const primary = { id: 'new', label: 'New', kind: 'primary' as const, run: () => {} };
	s.setActions({ primary, secondary: [] });
	s.setHeader('compact');
	s.setDensity('compact');
	assertEqual(host.actions.primary?.id, 'new', 'primary');
	assertEqual(host.header, 'compact', 'header');
	assertEqual(host.density, 'compact', 'density');
	s.setTip({ id: 'e', tone: 'danger', message: 'Failed' });
	s.setTip({ id: 'e', tone: 'danger', message: 'Failed again' });
	assertEqual(host.tip?.message, 'Failed again', 'replaced');
	s.setTip(null);
	assertEqual(host.tip, null, 'cleared');
});

suite.check('the report line lists what was written and what was left alone', () => {
	const { host, go } = frame();
	const s = host.attach(Symbol());
	s.report({ object: 'HOME-12', written: ['1 comment', 'status'], unchanged: ['assignee'] });
	assertEqual(host.tip?.id, SURFACE_REPORT_TIP_ID, 'tip id');
	assertEqual(host.tip?.tone, 'success', 'success tone');
	assertEqual(
		host.tip?.message,
		'surface.reportUnchanged|object=HOME-12;written=1 comment and status;unchanged=assignee',
		'message'
	);
	go('/workspace/w/projects/p/issues?view=x');
	assert(host.tip !== null, 'a query change keeps it');
	go('/workspace/w/projects/p/board');
	assertEqual(host.tip, null, 'next route clears it');
	assertEqual(
		formatReport(translate, 'en', { object: 'A', written: ['x'], unchanged: [] }),
		'surface.report|object=A;written=x',
		'written only'
	);
	assertEqual(
		formatReport(translate, 'en', { object: 'A', written: [], unchanged: ['y'] }),
		'surface.reportNone|object=A',
		'nothing written'
	);
});

suite.check('syncState merges the query, replaces by default, and skips no-ops', () => {
	const { host, navigations } = frame('/workspace/w/projects/p/forms?form=f1&record=r1#top');
	const s = host.attach(Symbol());
	s.syncState({ record: 'r2', view: 'v' });
	s.syncState({ form: null });
	s.syncState({ record: 'r3' }, 'push');
	s.syncState({ record: 'r1' });
	assertEqual(navigations.length, 3, 'unchanged query does not navigate');
	assertDeepEqual(
		navigations[0],
		{
			url: '/workspace/w/projects/p/forms?form=f1&record=r2&view=v#top',
			options: { replace: true, keepFocus: true, noScroll: true }
		},
		'merge'
	);
	assertEqual(navigations[1].url, '/workspace/w/projects/p/forms?record=r1#top', 'null removes');
	assertEqual((navigations[2].options as { replace: boolean }).replace, false, 'push mode');
});

suite.check('open goes to the target URL; panel uses panelHref; foreign URLs are refused', () => {
	const { host, navigations } = frame();
	const s = host.attach(Symbol());
	const target = {
		kind: 'record' as const,
		id: 'r',
		href: '/workspace/w/projects/p/forms/records/r',
		panelHref: '/workspace/w/projects/p/forms?form=f&record=r'
	};
	s.open(target, 'main');
	s.open(target, 'panel');
	s.open({ ...target, panelHref: undefined }, 'panel');
	s.open(target, 'replace');
	s.open({ ...target, href: 'https://evil.example/x' }, 'main');
	s.open({ ...target, href: '//evil.example/x' }, 'main');
	assertDeepEqual(
		navigations.map((n) => n.url),
		[target.href, target.panelHref, target.href, target.href],
		'targets'
	);
});

suite.check('close returns to the previous in-app URL, else the fallback, else the root', () => {
	const a = frame('/workspace/w/projects/p/forms');
	const s = a.host.attach(Symbol());
	a.host.noteNavigation(new URL('/workspace/w/projects/p/forms', 'http://x'));
	s.close('/fallback');
	assertEqual(a.navigations.at(-1)?.url, '/fallback', 'fallback with no history');
	s.close();
	assertEqual(a.navigations.at(-1)?.url, '/workspace', 'root');
	a.go('/workspace/w/projects/p/forms/records/r');
	s.close('/fallback');
	assertEqual(a.navigations.at(-1)?.url, '/workspace/w/projects/p/forms', 'previous');
	s.close('//evil');
	assertEqual(
		a.navigations.at(-1)?.url,
		'/workspace/w/projects/p/forms',
		'previous still wins over a bad fallback'
	);
});

suite.check('title and dirty reach the active task tab only', () => {
	const f = frame();
	const s = f.host.attach(Symbol());
	s.setTitle({ title: 'Orders' });
	s.setDirty(true);
	assertDeepEqual(f.taskLog, ['title:forms:p:f1:Orders', 'dirty:forms:p:f1:true'], 'active tab');
	s.setDirty(false);
	assertEqual(f.taskLog.at(-1), 'dirty:forms:p:f1:false', 'cleared');
	f.taskLog.length = 0;
	f.setActive('flow:other');
	s.setTitle({ title: 'Elsewhere' });
	s.setDirty(true);
	assertDeepEqual(f.taskLog, [], 'another tab is active');
	f.setActive('forms:p:f1');
	f.host.syncTasks();
	assertDeepEqual(
		f.taskLog,
		['title:forms:p:f1:Elsewhere', 'dirty:forms:p:f1:true'],
		'syncTasks catches up'
	);
});

suite.check('detach clears the dirty flag it set on the task tab', () => {
	const f = frame();
	const owner = Symbol();
	const s = f.host.attach(owner);
	s.setDirty(true);
	f.host.detach(owner);
	assertEqual(f.taskLog.at(-1), 'dirty:forms:p:f1:false', 'tab no longer dirty');
});

suite.check('a surface sees width, class, theme and frame kind', () => {
	const { host } = frame();
	const s = host.attach(Symbol());
	host.setWidth(500);
	host.setTheme('dark');
	assertEqual(s.width, 500, 'width');
	assertEqual(s.widthClass, 'sm', 'class');
	assertEqual(s.theme, 'dark', 'theme');
	assertEqual(s.frame, 'standalone', 'frame');
	assertEqual(s.slot, 'main', 'slot');
	assertEqual(s.globalShortcuts, true, 'shortcuts');
});

suite.check('a scope reports to its owner and never touches the content header', () => {
	const f = frame();
	const page = f.host.attach(Symbol());
	page.setTitle({ title: 'List' });
	const specs: Array<{ title: string; tip?: unknown; dirty?: boolean }> = [];
	const opened: string[] = [];
	const scope = new SurfaceScopeProvider({
		slot: 'panel',
		parent: f.host,
		onspec: (spec) => specs.push(spec),
		onopen: (target, how) => opened.push(`${how}:${target.href}`),
		translate,
		locale: () => 'en'
	});
	const owner = Symbol('panel');
	const panel = scope.attach(owner);
	panel.setTitle({ title: 'Record' });
	panel.setDirty(true);
	scope.setWidth(480);
	assertEqual(f.host.title?.title, 'List', 'content header untouched');
	assertEqual(specs.at(-1)?.title, 'Record', 'spec reported');
	assertEqual(specs.at(-1)?.dirty, true, 'dirty reported');
	assertEqual(panel.slot, 'panel', 'slot');
	assertEqual(panel.widthClass, 'sm', 'own width');
	assertEqual(panel.frame, 'standalone', 'frame inherited');
	panel.open({ kind: 'record', id: 'r', href: '/r' }, 'replace');
	assertDeepEqual(opened, ['replace:/r'], 'onopen');
	panel.report({ object: 'R', written: ['x'], unchanged: [] });
	assertEqual(
		(specs.at(-1)?.tip as { message: string }).message,
		'surface.report|object=R;written=x',
		'report becomes a tip'
	);
	scope.detach(owner);
	assertEqual(specs.at(-1)?.title, '', 'detach resets');
});

suite.check('a scope without handlers falls back to the enclosing frame', () => {
	const f = frame();
	const scope = new SurfaceScopeProvider({
		slot: 'inline',
		parent: f.host,
		onspec: () => {},
		translate,
		locale: () => 'en'
	});
	const panel = scope.attach(Symbol());
	panel.open({ kind: 'issue', id: 'i', href: '/workspace/w/projects/p/issues/i' }, 'main');
	assertEqual(f.navigations.at(-1)?.url, '/workspace/w/projects/p/issues/i', 'open falls back');
});

suite.check('applySpec sends the whole spec and leaves tip and dirty alone when undefined', () => {
	const calls: string[] = [];
	const host: SurfaceHost = {
		...createInertHost(),
		setTitle: (t) => calls.push(`title:${t.title}:${t.identifier ?? ''}`),
		setActions: (a) => calls.push(`actions:${a.primary?.id ?? '-'}:${a.secondary.length}`),
		setHeader: (h) => calls.push(`header:${h}`),
		setDensity: (d) => calls.push(`density:${d}`),
		setTip: (t) => calls.push(`tip:${t === null ? 'null' : t.id}`),
		setDirty: (d) => calls.push(`dirty:${d}`)
	};
	const spec: SurfaceSpec = { title: 'Lead', identifier: 'LEAD-1' };
	applySpec(host, spec);
	assertDeepEqual(
		calls,
		['title:Lead:LEAD-1', 'actions:-:0', 'header:standard', 'density:comfortable'],
		'defaults'
	);
	calls.length = 0;
	applySpec(host, {
		...spec,
		header: 'none',
		tip: null,
		dirty: false,
		secondary: [{ id: 's', label: 'S', kind: 'secondary', run: () => {} }]
	});
	assertDeepEqual(
		calls,
		[
			'title:Lead:LEAD-1',
			'actions:-:1',
			'header:none',
			'density:comfortable',
			'tip:null',
			'dirty:false'
		],
		'explicit'
	);
});

suite.check('the inert host accepts every call', () => {
	const host = createInertHost();
	host.setTitle({ title: 'x' });
	host.setActions({ secondary: [] });
	host.setTip(null);
	host.syncState({ a: '1' });
	host.close();
	assertEqual(host.frame, 'standalone', 'frame');
	assertEqual(host.width, 1024, 'width');
	assertEqual(host.widthClass, 'full', 'class');
});

finish(suite.result());
