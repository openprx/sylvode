import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { ApiResult } from '../src/lib/api/client';
import type { PluginContributionItem } from '../src/lib/api/plugin-packages';
import {
	clearAllPreferences,
	clearPreference,
	compareItems,
	createDiscoveryStore,
	discoveryState,
	effectivePosition,
	enabledRoutes,
	findByRoute,
	frameVerdict,
	isExtensionsOff,
	layoutSlot,
	loadEntry,
	moveDownPosition,
	moveUpPosition,
	navigationHref,
	parseState,
	partitionInline,
	preferenceError,
	safeFrom,
	sameFrameIdentity,
	sameItem,
	sameItems,
	savePreference,
	sortItems,
	withPreference,
	type DiscoveryState
} from '../src/lib/plugins/contributions';
import {
	contributionEntries,
	sameEntry,
	validPlacementPositions
} from '../src/lib/plugins/record-entries';

function item(
	key: string,
	id: string,
	overrides: Partial<PluginContributionItem> = {},
	slot: PluginContributionItem['contribution']['slot'] = 'project.view'
): PluginContributionItem {
	return {
		package_id: `pkg-${key}`,
		key,
		version: '1.0.0',
		name: key,
		profile: 'ui',
		generation: 1,
		package_sha256: 'p',
		web_sha256: 'w',
		contribution: {
			id,
			kind: slot === 'project.view' ? 'view' : 'panel',
			slot,
			label: id,
			view: id,
			route: slot === 'project.view' ? id : null,
			form_key: slot === 'project.view' ? null : 'order_line'
		},
		project_position: 50,
		preference: { hidden: false, collapsed: false, position: null },
		effective_position: 50,
		grants: ['ui.display'],
		...overrides
	};
}

const ok = <T>(data: T): ApiResult<T> => ({ code: 0, message: 'ok', data });
const fail = (code: number, details?: unknown): ApiResult<never> =>
	({ code, message: 'x', data: null, ...(details === undefined ? {} : { details }) }) as never;

test('order is (effective position, plugin key, contribution id) and preference position wins', () => {
	const a = item('b', 'x');
	const b = item('a', 'z');
	const c = item('a', 'y');
	assert.deepEqual(
		sortItems([a, b, c]).map((entry) => `${entry.key}:${entry.contribution.id}`),
		['a:y', 'a:z', 'b:x']
	);
	const personal = withPreference(a, { hidden: false, collapsed: false, position: 5 });
	assert.equal(personal.effective_position, 5);
	assert.equal(effectivePosition(personal), 5);
	assert.equal(sortItems([b, c, personal])[0], personal);
	// A zero personal position is a real position, not "no preference".
	assert.equal(
		effectivePosition(withPreference(a, { hidden: false, collapsed: false, position: 0 })),
		0
	);
	assert.ok(
		compareItems(withPreference(a, { hidden: false, collapsed: false, position: null }), b) > 0
	);
});

test('layout hides personal hidden entries and moves collapsed and overflow entries to More', () => {
	const many = Array.from({ length: 15 }, (_, i) =>
		item('k', `v${String(i).padStart(2, '0')}`, { project_position: i })
	);
	many[0] = withPreference(many[0], { hidden: true, collapsed: false, position: null });
	many[1] = withPreference(many[1], { hidden: false, collapsed: true, position: null });
	const layout = layoutSlot(many, 'project.view');
	assert.equal(layout.hidden.length, 1);
	assert.equal(layout.inline.length, 12);
	assert.deepEqual(
		layout.more.map((entry) => entry.contribution.id),
		['v01', 'v14']
	);
	assert.equal(layoutSlot(many, 'form.list.toolbar').inline.length, 0);
	assert.deepEqual(
		partitionInline([1, 2, 3, 4, 5], 2, (n) => n === 1),
		{ inline: [2, 3], more: [1, 4, 5] }
	);
});

test('record entries are ordered by position, plugin key and contribution id and skip hidden ones', () => {
	const v3 = [
		item('zzz', 'summary', { project_position: 10 }, 'form.record.detail'),
		item('aaa', 'tie', {}, 'form.record.detail'),
		withPreference(item('bbb', 'mine', {}, 'form.record.detail'), {
			hidden: true,
			collapsed: false,
			position: null
		})
	];
	const merged = contributionEntries(v3, 'form.record.detail');
	assert.deepEqual(
		merged.map((entry) => entry.id),
		['summary', 'tie']
	);
	assert.equal(sameEntry(merged[0], merged[0]), true);
	assert.equal(sameEntry(merged[0], merged[1]), false);
	assert.equal(sameEntry(null, merged[1]), false);
});

test('each entry stays in its declared slot and a tied position sorts by plugin key', () => {
	const items = [
		item('zzz', 'act', {}, 'form.record.actions'),
		item('aaa', 'act', {}, 'form.record.actions'),
		item('aaa', 'panel', {}, 'form.record.detail')
	];
	const actions = contributionEntries(items, 'form.record.actions');
	assert.deepEqual(
		actions.map((entry) => entry.item.key),
		['aaa', 'zzz']
	);
	const [detail] = contributionEntries(items, 'form.record.detail');
	assert.equal(detail.slot, 'form.record.detail');
	assert.equal(sameEntry(detail, actions[0]), false);
	assert.equal(sameEntry(detail, detail), true);
});

test('layout positions must be integers from 0 to 99', () => {
	assert.equal(validPlacementPositions(undefined), true);
	assert.equal(validPlacementPositions([{ id: 'a', enabled: false, position: 0 }]), true);
	assert.equal(validPlacementPositions([{ id: 'a', enabled: true, position: NaN }]), false);
	assert.equal(validPlacementPositions([{ id: 'a', enabled: true, position: 100 }]), false);
});

test('safeFrom only accepts same-project in-app paths', () => {
	const home = '/workspace/w1/projects/p1';
	const good = '/workspace/w1/projects/p1/forms?x=1';
	assert.equal(safeFrom(good, 'w1', 'p1'), good);
	for (const bad of [
		null,
		'',
		'//evil.example/workspace/w1/projects/p1/x',
		'https://evil.example/workspace/w1/projects/p1/x',
		'/workspace/w1/projects/p2/forms',
		'/workspace/w2/projects/p1/forms',
		'/workspace/w1/projects/p1',
		'/workspace/w1/projects/p1x/forms',
		'/workspace/w1/projects/p1/../p2/forms',
		'/workspace/w1/projects/p1/%2e%2e/p2/forms',
		'/workspace/w1/projects/p1/..%2fp2',
		'/workspace/w1/projects/p1/\\evil',
		'/\\evil.example',
		'/workspace/w1/projects/p1/a\nb',
		'javascript:alert(1)',
		`/workspace/w1/projects/p1/${'a'.repeat(3000)}`
	])
		assert.equal(safeFrom(bad, 'w1', 'p1'), home, String(bad));
});

test('state is capped at 512 characters', () => {
	assert.deepEqual(parseState(null), { state: null, tooLong: false });
	assert.deepEqual(parseState(''), { state: null, tooLong: false });
	assert.deepEqual(parseState('a'.repeat(512)), { state: 'a'.repeat(512), tooLong: false });
	assert.deepEqual(parseState('a'.repeat(513)), { state: null, tooLong: true });
});

test('equivalence ignores grant order but sees every change that matters', () => {
	const base = item('k', 'v', { grants: ['ui.display', 'read:orders'] });
	const reordered = { ...base, grants: ['read:orders', 'ui.display'] as typeof base.grants };
	assert.equal(sameItem(base, reordered), true);
	assert.equal(sameItems([base], [reordered]), true);
	assert.equal(sameItems([base], []), false);
	for (const changed of [
		{ ...base, generation: 2 },
		{ ...base, version: '1.0.1' },
		{ ...base, web_sha256: 'other' },
		{ ...base, package_sha256: 'other' },
		{ ...base, grants: ['ui.display'] as typeof base.grants }
	]) {
		assert.equal(sameFrameIdentity(base, changed), false);
		assert.equal(sameItem(base, changed), false);
	}
	// A personal preference reorders but does not recreate frames.
	const pref = withPreference(base, { hidden: false, collapsed: true, position: 3 });
	assert.equal(sameFrameIdentity(base, pref), true);
	assert.equal(sameItem(base, pref), false);
});

test('discovery state classification', () => {
	const previous = [item('k', 'v')];
	assert.equal(discoveryState(ok({ items: [] }), previous).status, 'ready');
	assert.equal(discoveryState(fail(404), previous).status, 'off');
	assert.equal(
		discoveryState(fail(403, { reason: 'extensions_disabled' }), previous).status,
		'off'
	);
	assert.equal(discoveryState(fail(401), previous).status, 'denied');
	assert.deepEqual(discoveryState(fail(403), previous).items, []);
	const transient = discoveryState(fail(500), previous);
	assert.equal(transient.status, 'error');
	assert.equal(transient.items, previous);
	assert.equal(isExtensionsOff(fail(404)), true);
	assert.equal(isExtensionsOff(fail(500)), false);
});

test('the discovery store notifies only when the result really changed and coalesces refreshes', async () => {
	let calls = 0;
	let response: ApiResult<{ items: PluginContributionItem[] }> = ok({ items: [item('k', 'v')] });
	const store = createDiscoveryStore(
		async () => {
			calls++;
			return response;
		},
		{ idleMs: 1_000_000 }
	);
	const seen: DiscoveryState[] = [];
	const stop = store.subscribe((value) => seen.push(value));
	assert.equal(seen[0].status, 'loading');
	const release = store.retain();
	await store.refresh();
	assert.equal(seen.at(-1)?.status, 'ready');
	const count = seen.length;
	// Equivalent response (same content, fresh objects, reordered grants): no new state.
	response = ok({ items: [{ ...item('k', 'v'), grants: ['ui.display'] }] });
	await store.refresh();
	assert.equal(seen.length, count);
	// Concurrent refreshes collapse into the running one plus a single re-run.
	const before = calls;
	await Promise.all([store.refresh(), store.refresh(), store.refresh()]);
	assert.ok(calls - before <= 2);
	// A real change notifies.
	response = ok({ items: [{ ...item('k', 'v'), generation: 2 }] });
	await store.refresh();
	assert.equal(seen.length, count + 1);
	assert.equal(store.snapshot().items[0].generation, 2);
	// Extension flag off: silently empty.
	response = fail(404);
	await store.refresh();
	assert.deepEqual(store.snapshot(), { status: 'off', items: [] });
	release();
	stop();
});

test('an open frame is kept, changed or closed from the latest discovery', () => {
	const opened = item('k', 'v', { grants: ['ui.display', 'read:orders'] });
	const state = (status: DiscoveryState['status'], items: PluginContributionItem[]) => ({
		status,
		items
	});
	assert.equal(frameVerdict(state('loading', []), opened), 'ok');
	assert.equal(frameVerdict(state('ready', [{ ...opened }]), opened), 'ok');
	assert.equal(frameVerdict(state('ready', []), opened), 'gone');
	assert.equal(frameVerdict(state('off', []), opened), 'gone');
	assert.equal(frameVerdict(state('denied', []), opened), 'gone');
	assert.equal(frameVerdict(state('error', [opened]), opened), 'error');
	assert.equal(
		frameVerdict(state('ready', [{ ...opened, grants: ['ui.display'] }]), opened),
		'changed'
	);
	assert.equal(frameVerdict(state('ready', [{ ...opened, generation: 9 }]), opened), 'changed');
});

test('routes are found and enabled from the current discovery only', () => {
	const items = [
		item('a', 'board'),
		item('a', 'other'),
		item('b', 'board'),
		item('a', 'rec', {}, 'form.record.detail')
	];
	assert.equal(findByRoute(items, 'a', 'board')?.package_id, 'pkg-a');
	assert.equal(findByRoute(items, 'b', 'other'), undefined);
	assert.equal(findByRoute(items, 'a', 'rec'), undefined);
	assert.deepEqual(enabledRoutes(items, 'a'), ['board', 'other']);
	assert.deepEqual(enabledRoutes(items, 'missing'), []);
});

test('navigation targets map to in-app URLs only', () => {
	const where = {
		workspaceId: 'w',
		projectId: 'p',
		key: 'k',
		from: '/workspace/w/projects/p/forms'
	};
	assert.equal(
		navigationHref({ target: 'view', route: 'board', state: 'a b' }, where),
		'/workspace/w/projects/p/apps/k/board?state=a+b&from=%2Fworkspace%2Fw%2Fprojects%2Fp%2Fforms'
	);
	assert.equal(
		navigationHref({ target: 'view', route: 'board', state: null }, { ...where, from: null }),
		'/workspace/w/projects/p/apps/k/board'
	);
	assert.equal(
		navigationHref({ target: 'record', record_id: 'r1' }, where),
		'/workspace/w/projects/p/forms/records/r1'
	);
	assert.equal(navigationHref({ target: 'project' }, where), '/workspace/w/projects/p');
});

test('entry loading separates unavailable from transient failures and checks the answer', async () => {
	const asked = item('k', 'v', {}, 'form.record.detail');
	const api = (response: unknown) => ({ entry: async () => response as never });
	const good = await loadEntry(
		'p',
		asked,
		{ recordId: 'r' },
		api(ok({ item: asked, html: '<p/>' }))
	);
	assert.equal(good.ok, true);
	for (const code of [401, 403, 404])
		assert.deepEqual(await loadEntry('p', asked, {}, api(fail(code))), {
			ok: false,
			reason: 'unavailable'
		});
	assert.deepEqual(
		await loadEntry('p', asked, {}, api(fail(400, { reason: 'contribution_unavailable' }))),
		{ ok: false, reason: 'unavailable' }
	);
	assert.deepEqual(await loadEntry('p', asked, {}, api(fail(500))), {
		ok: false,
		reason: 'failed'
	});
	// An answer for another contribution is rejected.
	assert.deepEqual(
		await loadEntry(
			'p',
			asked,
			{},
			api(ok({ item: item('k', 'other', {}, 'form.record.detail'), html: 'x' }))
		),
		{ ok: false, reason: 'failed' }
	);
	const throwing = { entry: async () => Promise.reject(new Error('network')) };
	assert.deepEqual(await loadEntry('p', asked, {}, throwing), { ok: false, reason: 'failed' });
});

test('preference helpers call the API and map failures', async () => {
	const calls: string[] = [];
	const api = {
		setPreference: async () => {
			calls.push('set');
			return ok({
				key: 'k',
				contribution_id: 'v',
				updated_at: '',
				hidden: true,
				collapsed: false,
				position: null
			});
		},
		resetPreference: async () => {
			calls.push('reset');
			return ok({ reset: true as const });
		},
		resetAllPreferences: async () => {
			calls.push('all');
			return ok({ reset: 2 });
		}
	};
	const target = item('k', 'v');
	assert.equal(
		(await savePreference('p', target, { hidden: true, collapsed: false, position: null }, api))
			.code,
		0
	);
	assert.equal((await clearPreference('p', target, api)).code, 0);
	assert.equal((await clearAllPreferences('p', api)).code, 0);
	assert.deepEqual(calls, ['set', 'reset', 'all']);
	assert.equal(
		preferenceError({ details: { reason: 'preference_limit' } }),
		'plugins.prefs.limitReached'
	);
	assert.equal(
		preferenceError({ details: { reason: 'contribution_unavailable' } }),
		'plugins.prefs.unavailable'
	);
	assert.equal(preferenceError({}), 'plugins.prefs.saveFailed');
});

test('move helpers stay inside 0-99 and never tie', () => {
	const ordered = [
		item('a', 'one', { project_position: 0 }),
		item('a', 'two', { project_position: 10 }),
		item('a', 'three', { project_position: 99 })
	];
	assert.equal(moveUpPosition(ordered, ordered[0]), null);
	assert.equal(moveUpPosition(ordered, ordered[1]), null);
	assert.equal(moveUpPosition(ordered, ordered[2]), 9);
	assert.equal(moveDownPosition(ordered, ordered[0]), 11);
	assert.equal(moveDownPosition(ordered, ordered[1]), null);
	assert.equal(moveDownPosition(ordered, ordered[2]), null);
});
