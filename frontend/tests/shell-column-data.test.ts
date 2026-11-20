/**
 * Column data loading: each list loads and fails on its own, 403 is its own state, a newer
 * request wins, and a loaded list is not requested again unless forced (`column-data.svelte.ts`).
 *
 * Run standalone: `bun tests/shell-column-data.test.ts`
 */
import { Suite, assertDeepEqual, assertEqual, finish } from './support/harness';
import { installRuneModules } from './shell-rune-loader';

await installRuneModules();
const { ColumnData, loadFromResult } = await import('../src/lib/shell/column/column-data.svelte');
type Apis = import('../src/lib/shell/column/column-data.svelte').ColumnApis;

const suite = new Suite('shell-column-data');
const ok = <T>(data: T) => ({ code: 0, message: 'ok', data });
const fail = (code: number) => ({ code, message: 'no', data: null });
const page = <T>(items: T[]) =>
	ok({ items, total: items.length, page: 1, per_page: 100, total_pages: 1 });

function apis(over: Partial<Apis> = {}): { apis: Apis; calls: string[] } {
	const calls: string[] = [];
	return {
		calls,
		apis: {
			workspaces: async () => (calls.push('workspaces'), page([{ id: 'w1', name: 'North' }])),
			bots: async (w) => (calls.push(`bots:${w}`), ok([{ id: 'b1', name: 'Bot' }])),
			aiAgents: async (p) => (calls.push(`agents:${p}`), page([{ id: 'a1', name: 'Agent' }])),
			forms: async (p) => (calls.push(`forms:${p}`), page([{ id: 'f1', name: 'Orders' }])),
			...over
		} as Apis
	};
}

suite.check('loadFromResult: ready, forbidden, failed', () => {
	const pick = (d: { items: number[] }) => ({ items: d.items, total: null });
	assertEqual(loadFromResult(ok({ items: [1] }), pick).status, 'ready', 'ready');
	assertEqual(loadFromResult(fail(403), pick).status, 'forbidden', '403');
	assertEqual(loadFromResult(fail(500), pick).status, 'error', '500');
	assertEqual(loadFromResult(null, pick).status, 'error', 'null');
});

await suite.checkAsync('lists load on their own and are cached', async () => {
	const { apis: a, calls } = apis();
	const data = new ColumnData(a);
	assertEqual(data.workspaces.status, 'idle', 'idle first');
	await data.loadWorkspaces();
	await data.loadWorkspaces();
	assertEqual(data.workspaces.status, 'ready', 'ready');
	assertEqual(data.workspaces.items[0].name, 'North', 'items');
	await data.loadBots('w1');
	await data.loadAgents('p1');
	await data.loadForms('p1');
	assertEqual(data.botsOf('w1').items[0].name, 'Bot', 'bots');
	assertEqual(data.agentsOf('p1').items[0].name, 'Agent', 'agents');
	assertEqual(data.formsOf('p1').items[0].id, 'f1', 'forms');
	assertEqual(data.formsOf('p9').status, 'idle', 'other project untouched');
	await data.loadForms('p1');
	assertDeepEqual(calls, ['workspaces', 'bots:w1', 'agents:p1', 'forms:p1'], 'no repeat requests');
	await data.loadForms('p1', true);
	assertEqual(calls.filter((c) => c === 'forms:p1').length, 2, 'force reloads');
});

await suite.checkAsync(
	'one failed list does not touch the others; failure can be retried',
	async () => {
		let botCalls = 0;
		const { apis: a } = apis({
			bots: async () => (++botCalls === 1 ? fail(500) : ok([{ id: 'b1', name: 'Bot' }])),
			aiAgents: async () => fail(403)
		});
		const data = new ColumnData(a);
		await Promise.all([data.loadWorkspaces(), data.loadBots('w1'), data.loadAgents('p1')]);
		assertEqual(data.workspaces.status, 'ready', 'workspaces fine');
		assertEqual(data.botsOf('w1').status, 'error', 'bots failed');
		assertEqual(data.agentsOf('p1').status, 'forbidden', 'agents forbidden');
		await data.loadBots('w1');
		assertEqual(data.botsOf('w1').status, 'ready', 'retry after failure succeeds without force');
	}
);

await suite.checkAsync('a thrown request is an error, not an unhandled rejection', async () => {
	const { apis: a } = apis({
		workspaces: async () => {
			throw new Error('network');
		}
	});
	const data = new ColumnData(a);
	await data.loadWorkspaces();
	assertEqual(data.workspaces.status, 'error', 'error');
});

await suite.checkAsync('only the newest request stores its result', async () => {
	const resolvers: Array<(items: Array<{ id: string; name: string }>) => void> = [];
	const { apis: a } = apis({
		forms: () => new Promise((resolve) => resolvers.push((items) => resolve(page(items))))
	});
	const data = new ColumnData(a);
	const first = data.loadForms('p1');
	const second = data.loadForms('p1', true);
	assertEqual(data.formsOf('p1').status, 'loading', 'loading');
	resolvers[1]([{ id: 'new', name: 'New' }]);
	await second;
	resolvers[0]([{ id: 'old', name: 'Old' }]);
	await first;
	assertEqual(data.formsOf('p1').items[0].id, 'new', 'stale response dropped');
});

suite.check('apps count: unknown until set', () => {
	const data = new ColumnData(apis().apis);
	assertEqual(data.appsCountOf('p1'), null, 'unknown');
	data.setAppsCount('p1', 2);
	assertEqual(data.appsCountOf('p1'), 2, 'set');
	data.setAppsCount('p1', null);
	assertEqual(data.appsCountOf('p1'), null, 'reset');
});

finish(suite.result());
