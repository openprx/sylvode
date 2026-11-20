/**
 * Command palette: matching and ranking, grouping and limits, entry builders, the debounced
 * server search (stale responses discarded, short queries never sent), and rendered roles.
 *
 * Run standalone: `bun tests/shell-palette.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import { matchTier, rankLocal } from '../src/lib/shell/palette/ranking';
import {
	GROUP_LIMIT,
	TOTAL_LIMIT,
	actionEntries,
	buildPaletteView,
	flowEntries,
	formEntries,
	gotoEntries,
	issueEntries,
	projectEntries,
	recentEntries,
	searchResultHref,
	type PaletteEntry,
	type PaletteInput
} from '../src/lib/shell/palette/palette-sources';
import { createServerSearch, type TimerApi } from '../src/lib/shell/palette/palette-search';
import { settingsGroups } from '../src/lib/shell/settings-sections';
import type { SearchResult } from '../src/lib/api/search';

await installSvelteSsr();
const { default: Palette } = await import('../src/lib/shell/palette/Palette.svelte');

const suite = new Suite('shell-palette');
const t = (key: string, options?: { values?: Record<string, string | number> }) =>
	options?.values ? `${key}|${Object.values(options.values).join(',')}` : key;
const collator = new Intl.Collator('en');
const entry = (
	group: PaletteEntry['group'],
	title: string,
	extra: Partial<PaletteEntry> = {}
): PaletteEntry => ({
	id: `${group}:${title}`,
	group,
	kind: 'project',
	title,
	verb: 'Open',
	glyph: 'folder',
	href: `/x/${title}`,
	...extra
});
const input = (query: string, extra: Partial<PaletteInput> = {}): PaletteInput => ({
	query,
	t,
	collator,
	goto: [],
	recent: [],
	projects: [],
	forms: [],
	actions: [],
	flow: null,
	issues: null,
	flowEnabled: false,
	...extra
});

suite.check('match tiers: exact, prefix, word prefix, substring, none', () => {
	assertEqual(matchTier('alpha', 'Alpha'), 0, 'exact, case-insensitive');
	assertEqual(matchTier('alp', 'Alpha beta'), 1, 'prefix');
	assertEqual(matchTier('bet', 'Alpha beta'), 2, 'word prefix');
	assertEqual(matchTier('lph', 'Alpha beta'), 3, 'substring');
	assertEqual(matchTier('zzz', 'Alpha beta'), null, 'no match');
	assertEqual(matchTier('alpa', 'Alpha'), null, 'no fuzzy matching');
	assertEqual(matchTier('项目', '我的项目'), 3, 'CJK substring');
});

suite.check('ranking: tier first, then recency, then alphabetical', () => {
	const items = [
		{ title: 'Gamma tool' },
		{ title: 'Tool' },
		{ title: 'Beta tool', recency: 5 },
		{ title: 'Tooling', recency: 1 },
		{ title: 'Alpha tool' }
	];
	assertDeepEqual(
		rankLocal(items, 'tool', collator).map((i) => i.title),
		['Tool', 'Tooling', 'Beta tool', 'Alpha tool', 'Gamma tool'],
		'exact, prefix, then word-prefix group by recency then name'
	);
});

suite.check('ranking: recency beats alphabet inside a tier; non-matches dropped', () => {
	const items = [{ title: 'Apple pie' }, { title: 'Zebra pie', recency: 9 }, { title: 'Other' }];
	assertDeepEqual(
		rankLocal(items, 'pie', collator).map((i) => i.title),
		['Zebra pie', 'Apple pie'],
		'order'
	);
});

suite.check('ranking: an empty query keeps the input order, recent first', () => {
	const items = [{ title: 'B' }, { title: 'A' }, { title: 'C', recency: 3 }];
	assertDeepEqual(
		rankLocal(items, '', collator).map((i) => i.title),
		['C', 'B', 'A'],
		'order'
	);
});

suite.check('locale-aware alphabetical order', () => {
	const sv = new Intl.Collator('sv');
	const out = rankLocal([{ title: 'ab z' }, { title: 'Ab a' }, { title: 'äb' }], 'b', sv);
	assert(out.length === 3, 'all match');
	assertEqual(out[0].title, 'Ab a', 'uses the collator');
});

suite.check('groups appear in the fixed order and empty groups are hidden', () => {
	const view = buildPaletteView(
		input('al', {
			goto: [entry('goto', 'Alpha')],
			recent: [entry('recent', 'Alpine')],
			projects: [entry('projects', 'Alpha project')],
			forms: [],
			actions: [entry('actions', 'Alter')]
		})
	);
	assertDeepEqual(
		view.groups.map((g) => g.id),
		['goto', 'recent', 'projects', 'actions', 'results'],
		'ids'
	);
});

suite.check('query length gates the groups', () => {
	const base = {
		projects: [entry('projects', 'Alpha')],
		forms: [entry('forms', 'Alpha form')],
		flow: [entry('flow', 'Alpha page')],
		issues: [entry('issues', 'Alpha issue')],
		flowEnabled: true
	};
	assertDeepEqual(
		buildPaletteView(input('', base)).groups.map((g) => g.id),
		[],
		'empty query shows no project, form, flow, issue'
	);
	assertDeepEqual(
		buildPaletteView(input('a', base)).groups.map((g) => g.id),
		['projects', 'forms'],
		'one character: projects and forms, no server groups, no see-all'
	);
	assertDeepEqual(
		buildPaletteView(input('al', base)).groups.map((g) => g.id),
		['projects', 'forms', 'flow', 'issues', 'results'],
		'two characters: everything'
	);
});

suite.check('Flow pages need a workspace scope with Flow on', () => {
	const base = { flow: [entry('flow', 'Alpha page')] };
	assert(
		!buildPaletteView(input('al', { ...base, flowEnabled: false })).groups.some(
			(g) => g.id === 'flow'
		),
		'hidden'
	);
});

suite.check('server groups keep the server order; local groups are ranked', () => {
	const view = buildPaletteView(
		input('al', {
			issues: [entry('issues', 'Zed'), entry('issues', 'Alpha')],
			projects: [entry('projects', 'x al'), entry('projects', 'al')]
		})
	);
	assertDeepEqual(
		view.groups.find((g) => g.id === 'issues')?.entries.map((e) => e.title),
		['Zed', 'Alpha'],
		'server order'
	);
	assertDeepEqual(
		view.groups.find((g) => g.id === 'projects')?.entries.map((e) => e.title),
		['al', 'x al'],
		'ranked'
	);
});

suite.check('at most 5 rows per group and 30 in total', () => {
	const many = (g: PaletteEntry['group']) =>
		Array.from({ length: 12 }, (_, i) => entry(g, `item ${i}`));
	const one = buildPaletteView(input('item', { projects: many('projects') }));
	assertEqual(one.groups[0].entries.length, GROUP_LIMIT, 'per group');
	const all = buildPaletteView(
		input('item', {
			goto: many('goto'),
			recent: many('recent'),
			projects: many('projects'),
			forms: many('forms'),
			flow: many('flow'),
			issues: many('issues'),
			actions: many('actions'),
			flowEnabled: true
		})
	);
	assert(all.entries.length <= TOTAL_LIMIT, `total ${all.entries.length}`);
	assertEqual(all.entries.length, TOTAL_LIMIT, 'the cap is reached');
	assert(!all.groups.some((g) => g.id === 'results'), 'later groups are cut first');
});

suite.check('"See all results" goes to /search with the encoded query', () => {
	const view = buildPaletteView(input('a b'));
	const row = view.groups.find((g) => g.id === 'results')?.entries[0];
	assertEqual(row?.href, '/search?q=a%20b', 'href');
	assert(
		!buildPaletteView(input('a')).groups.some((g) => g.id === 'results'),
		'one character: none'
	);
});

suite.check('Go to lists reachable destinations, Settings, sections and All workspaces', () => {
	const groups = settingsGroups({
		instanceAdmin: true,
		workspaceAdmin: true,
		personalFlow: false,
		workspace: { id: 'w1', name: 'North' }
	});
	const entries = gotoEntries({
		t,
		scope: 'w1',
		flow: { personal: false, workspaces: { w1: false } },
		lastFor: () => undefined,
		settingsGroups: groups
	});
	const ids = entries.map((e) => e.id);
	assert(
		ids.includes('goto:home') && ids.includes('goto:projects') && ids.includes('goto:agents'),
		'destinations'
	);
	assert(!ids.includes('goto:flow'), 'Flow is off: not offered');
	assert(ids.includes('goto:settings:workspace:members'), 'workspace section for an admin');
	assert(ids.includes('goto:settings:instance:users'), 'instance section for an instance admin');
	assert(ids.includes('goto:all-workspaces'), 'all workspaces');
	const member = gotoEntries({
		t,
		scope: 'w1',
		flow: { personal: false, workspaces: {} },
		lastFor: () => undefined,
		settingsGroups: settingsGroups({
			instanceAdmin: false,
			workspaceAdmin: false,
			personalFlow: false,
			workspace: { id: 'w1', name: 'North' }
		})
	});
	assert(
		!member.some((e) => e.id.includes(':workspace:') || e.id.includes(':instance:')),
		'gates exclude sections'
	);
});

suite.check('project, form, recent entries carry hrefs and context', () => {
	const [p] = projectEntries(
		[
			{
				id: 'p1',
				name: 'Alpha',
				workspace_id: 'w1',
				workspace_name: 'North',
				type_key: 'custom_form'
			}
		],
		t,
		(href) => (href.endsWith('/forms') ? 42 : 0)
	);
	assertEqual(p.href, '/workspace/w1/projects/p1/forms', 'form project goes to forms');
	assertEqual(p.context, 'North', 'workspace context');
	assertEqual(p.recency, 42, 'tab recency');
	const [f] = formEntries(
		[{ id: 'f1', name: 'Orders', projectId: 'p1', projectName: 'Alpha', workspaceId: 'w1' }],
		t
	);
	assertEqual(f.href, '/workspace/w1/projects/p1/forms?form=f1', 'form href');
	const r = recentEntries(
		[
			{ href: '/a', title: 'A' },
			{ href: '/b', title: 'B' }
		],
		t
	);
	assert((r[0].recency ?? 0) > (r[1].recency ?? 0), 'most recent first');
});

suite.check('action entries: no write, current choices omitted, pin only when pinnable', () => {
	const a = actionEntries({
		t,
		scope: 'w1',
		workspaces: [
			{ id: 'w1', name: 'North' },
			{ id: 'w2', name: 'South' }
		],
		theme: 'dark',
		locale: 'en',
		pinnable: null
	});
	const ids = a.map((e) => e.id);
	assert(
		ids.includes('action:workspace:w2') && !ids.includes('action:workspace:w1'),
		'other workspace only'
	);
	assert(!ids.includes('action:theme:dark') && ids.includes('action:theme:light'), 'theme');
	assert(!ids.includes('action:language:en') && ids.includes('action:language:zh'), 'language');
	assert(!ids.includes('action:pin-current'), 'nothing to pin');
	assert(
		a.every((e) => e.href === undefined && e.action),
		'actions carry an action ref, never an href'
	);
	assert(
		actionEntries({
			t,
			scope: 'w1',
			workspaces: [],
			theme: 'dark',
			locale: 'en',
			pinnable: 'Alpha'
		}).some((e) => e.id === 'action:pin-current'),
		'pin offered'
	);
});

suite.check('search hit hrefs follow the previous search modal', () => {
	const issue = {
		type: 'issue',
		id: 'i1',
		title: 'T',
		state: 's',
		project_id: 'p1',
		workspace_id: 'w1'
	} as SearchResult;
	assertEqual(searchResultHref(issue, null), '/workspace/w1/projects/p1/issues/i1', 'issue');
	const comment = {
		type: 'comment',
		id: 'c1',
		body: 'b',
		issue_id: 'i1',
		project_id: 'p1',
		workspace_id: '',
		created_at: ''
	} as SearchResult;
	assertEqual(
		searchResultHref(comment, 'w9'),
		'/workspace/w9/projects/p1/issues/i1',
		'comment, fallback workspace'
	);
	assertEqual(searchResultHref(comment, null), null, 'no workspace');
	const out = issueEntries(
		[
			issue,
			comment,
			{ type: 'project', id: 'p1', key: 'K', name: 'N', workspace_id: 'w1' } as SearchResult
		],
		null,
		t
	);
	assertDeepEqual(
		out.map((e) => e.kind),
		['issue'],
		'comment without workspace and project hit are skipped'
	);
});

suite.check('Flow hits link to the workspace Flow page', () => {
	const [e] = flowEntries(
		[
			{
				object: { id: 'o 1', object_type: 'page', title: 'Plan', lifecycle_status: 'active' },
				matched_fields: [],
				snippets: {},
				indexed_seq: 0,
				head_seq: 0,
				projection_lag: 0,
				stale: false
			}
		],
		{ id: 'w1', name: 'North' },
		t
	);
	assertEqual(e.href, '/workspace/w1/flow/o%201', 'href');
	assertEqual(e.context, 'North', 'context');
});

function fakeTimers() {
	const pending = new Map<number, () => void>();
	let next = 1;
	const api: TimerApi = {
		set: (run) => {
			pending.set(next, run);
			return next++;
		},
		clear: (h) => void pending.delete(h as number)
	};
	return {
		api,
		fire() {
			const runs = [...pending.values()];
			pending.clear();
			runs.forEach((r) => r());
		},
		get count() {
			return pending.size;
		}
	};
}
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

await suite.checkAsync(
	'server search: debounced, one request for a burst of keystrokes',
	async () => {
		const timers = fakeTimers();
		const calls: string[] = [];
		const results: Array<string | null> = [];
		const search = createServerSearch<string>({
			fetch: async (q) => (calls.push(q), q.toUpperCase()),
			onresult: (_q, r) => results.push(r),
			timers: timers.api
		});
		search.request('al');
		search.request('alp');
		search.request('alph');
		assertEqual(timers.count, 1, 'one timer pending');
		timers.fire();
		await flush();
		assertDeepEqual(calls, ['alph'], 'one request');
		assertDeepEqual(results, ['ALPH'], 'one result');
	}
);

await suite.checkAsync(
	'server search: a stale response is discarded and the old request aborted',
	async () => {
		const timers = fakeTimers();
		const resolvers: Array<(v: string) => void> = [];
		const signals: AbortSignal[] = [];
		const results: Array<string | null> = [];
		const search = createServerSearch<string>({
			fetch: (_q, signal) =>
				new Promise((resolve) => (signals.push(signal), resolvers.push(resolve))),
			onresult: (_q, r) => results.push(r),
			timers: timers.api
		});
		search.request('ab');
		timers.fire();
		search.request('abc');
		assert(signals[0].aborted, 'first request aborted');
		timers.fire();
		resolvers[1]('fresh');
		await flush();
		resolvers[0]('stale');
		await flush();
		assertDeepEqual(results, ['fresh'], 'only the latest answer shows');
	}
);

await suite.checkAsync('server search: queries under 2 characters are never sent', async () => {
	const timers = fakeTimers();
	let fetched = 0;
	const results: Array<string | null> = [];
	const search = createServerSearch<string>({
		fetch: async () => (fetched++, 'x'),
		onresult: (_q, r) => results.push(r),
		timers: timers.api
	});
	search.request('a');
	search.request(' ');
	assertEqual(timers.count, 0, 'no timer');
	assertEqual(fetched, 0, 'no request');
	assertDeepEqual(results, [null, null], 'cleared');
});

await suite.checkAsync(
	'server search: a failure clears the group and cancel() stays silent',
	async () => {
		const timers = fakeTimers();
		const results: Array<string | null> = [];
		const search = createServerSearch<string>({
			fetch: async () => {
				throw new Error('boom');
			},
			onresult: (_q, r) => results.push(r),
			timers: timers.api
		});
		search.request('ab');
		timers.fire();
		await flush();
		assertDeepEqual(results, [null], 'failure yields null');
		search.request('ab');
		search.cancel();
		assertEqual(timers.count, 0, 'cancelled');
	}
);

await suite.checkAsync(
	'Palette renders a combobox, a listbox, labelled groups and options',
	async () => {
		const html = await renderToHtml(Palette, {
			open: true,
			onclose: () => {},
			context: {
				goto: [entry('goto', 'Home', { kind: 'destination' })],
				recent: [],
				projects: [],
				forms: [],
				actions: [entry('actions', 'Open settings', { kind: 'action', href: undefined })],
				flowWorkspace: null,
				fallbackWorkspaceId: null
			},
			sources: { searchAll: async () => null, searchFlow: async () => null },
			onnavigate: () => {},
			onaction: () => {}
		});
		const input = /<input[^>]*>/.exec(html)?.[0] ?? '';
		assert(input.includes('role="combobox"') && input.includes('aria-controls="'), input);
		assert(input.includes('aria-activedescendant="'), 'active descendant');
		assert(html.includes('role="listbox"'), 'listbox');
		assertEqual((html.match(/role="group"/g) ?? []).length, 2, 'two groups');
		assert(html.includes('aria-label="shell.palette.groups.goto"'), 'group name');
		assert(
			html.includes('data-palette-kind="destination"') &&
				html.includes('data-palette-kind="action"'),
			'kinds'
		);
		const selected = (html.match(/aria-selected="true"/g) ?? []).length;
		assertEqual(selected, 1, 'one option selected');
		assert(html.includes('data-shell="palette"'), 'marker');
		assert(html.includes('shell.palette.hint'), 'hint line');
	}
);

finish(suite.result());
