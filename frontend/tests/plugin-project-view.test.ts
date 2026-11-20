import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type { PluginContributionItem } from '../src/lib/api/plugin-packages';
import type { DiscoveryState, EntryOutcome } from '../src/lib/plugins/contributions';
import {
	closeView,
	initialProjectView,
	openView,
	reconcileView,
	retireFrame,
	retryView,
	type ProjectViewState
} from '../src/lib/plugins/project-view';

function item(overrides: Partial<PluginContributionItem> = {}): PluginContributionItem {
	return {
		package_id: 'pkg',
		key: 'order_board',
		version: '1.0.0',
		name: 'Order board',
		profile: 'ui',
		generation: 1,
		package_sha256: 'p',
		web_sha256: 'w',
		contribution: {
			id: 'board',
			kind: 'view',
			slot: 'project.view',
			label: 'Order board',
			view: 'board',
			route: 'board',
			form_key: null
		},
		project_position: 50,
		preference: { hidden: false, collapsed: false, position: null },
		effective_position: 50,
		grants: ['ui.display', 'read:orders'],
		...overrides
	};
}

const ready = (items: PluginContributionItem[]): DiscoveryState => ({ status: 'ready', items });

/**
 * Drives the state machine the way ProjectPluginView does: the discovery effect calls
 * `reconcile`, a returned target is opened, and `resume` is what the host's `onRetired` does
 * when a host that retired silently runs again.
 */
function view(entry: () => EntryOutcome = () => ({ ok: true, item: item(), html: '<p>x</p>' })) {
	const state: ProjectViewState = initialProjectView();
	let discovery: DiscoveryState = { status: 'loading', items: [] };
	const entries: number[] = [];
	const pending: Promise<void>[] = [];
	const deps = {
		loadEntry: async () => {
			entries.push(entries.length + 1);
			return entry();
		},
		projectName: async () => 'Demo',
		refreshDiscovery: async () => undefined,
		latest: () => discovery.items[0]
	};
	const open = (target: PluginContributionItem | undefined) => {
		if (target) pending.push(openView(state, target, deps));
	};
	return {
		state,
		entries,
		reconcile(next: DiscoveryState = discovery) {
			discovery = next;
			open(reconcileView(state, discovery, discovery.items[0]));
		},
		resume(token: number) {
			if (retireFrame(state, token)) open(reconcileView(state, discovery, discovery.items[0]));
		},
		retry() {
			open(retryView(state, discovery, discovery.items[0]));
		},
		settle: () => Promise.all(pending.splice(0))
	};
}

/** The view never shows nothing: a frame, loading, or an explicit notice. */
function assertVisible(state: ProjectViewState) {
	if (state.phase === 'ready') assert.ok(state.opened, 'ready without a frame');
	else assert.equal(state.opened, null, `${state.phase} with a frame`);
}

describe('project view state', () => {
	test('opens once discovery has the contribution', async () => {
		const v = view();
		v.reconcile();
		assert.equal(v.state.phase, 'loading');
		v.reconcile(ready([item()]));
		await v.settle();
		assert.equal(v.state.phase, 'ready');
		assert.equal(v.entries.length, 1);
		assert.equal(v.state.name, 'Demo');
		assertVisible(v.state);
	});

	test('X3: a view that comes back after its frame retired reopens with a fresh entry', async () => {
		const v = view();
		v.reconcile(ready([item()]));
		await v.settle();
		const first = v.state.opened!.token;
		// The route changed to a record page: the host retires its frame silently. The view itself
		// is kept alive for the grace period, and browser Back resumes it instead of remounting.
		// Discovery is unchanged, so only the host's report can bring the view back.
		v.reconcile();
		assert.equal(v.entries.length, 1, 'an unchanged discovery does not reopen a shown frame');
		v.resume(first);
		assert.equal(v.state.phase, 'loading');
		assertVisible(v.state);
		await v.settle();
		assert.equal(v.state.phase, 'ready');
		assert.equal(v.entries.length, 2, 'a fresh entry request after Back');
		assert.notEqual(v.state.opened!.token, first, 'a new frame, never the retired host');
		assertVisible(v.state);
	});

	test('a late report from an earlier frame does not close the current one', async () => {
		const v = view();
		v.reconcile(ready([item()]));
		await v.settle();
		const first = v.state.opened!.token;
		v.resume(first);
		await v.settle();
		const second = v.state.opened!.token;
		v.resume(first);
		assert.equal(v.state.opened?.token, second);
		assert.equal(v.entries.length, 2);
	});

	test('Back twice and Forward: every resume after a retire reopens exactly once', async () => {
		const v = view();
		v.reconcile(ready([item()]));
		await v.settle();
		for (let n = 0; n < 3; n++) {
			v.resume(v.state.opened!.token);
			await v.settle();
			assert.equal(v.state.phase, 'ready');
		}
		assert.equal(v.entries.length, 4);
	});

	test('a retire report while no frame is shown changes nothing', async () => {
		const v = view(() => ({ ok: false, reason: 'unavailable' }));
		v.reconcile(ready([item()]));
		await v.settle();
		assert.equal(v.state.phase, 'unavailable');
		assert.equal(retireFrame(v.state, 1), false);
		assert.equal(v.state.phase, 'unavailable');
	});

	test('a resumed view whose contribution was revoked shows the unavailable notice', async () => {
		const v = view();
		v.reconcile(ready([item()]));
		await v.settle();
		const token = v.state.opened!.token;
		v.reconcile(ready([]));
		assert.equal(v.state.phase, 'unavailable');
		assert.equal(v.state.opened, null);
		v.resume(token);
		assert.equal(v.state.phase, 'unavailable');
		assert.equal(v.entries.length, 1);
	});

	test('a resumed view whose grants changed opens the new entry, never the old frame', async () => {
		let grants: PluginContributionItem['grants'] = ['ui.display', 'read:orders'];
		const v = view(() => ({ ok: true, item: item({ grants }), html: '<p>x</p>' }));
		v.reconcile(ready([item()]));
		await v.settle();
		const token = v.state.opened!.token;
		grants = ['ui.display'];
		v.reconcile(ready([item({ grants })]));
		assert.equal(v.state.opened, null, 'the changed frame is closed at once');
		assert.equal(v.state.notice, 'plugins.versionChanged');
		await v.settle();
		assert.deepEqual(v.state.opened!.item.grants, ['ui.display']);
		v.resume(token);
		assert.deepEqual(v.state.opened!.item.grants, ['ui.display']);
		assert.equal(v.entries.length, 2);
	});

	test('an open superseded while in flight is dropped', async () => {
		let release: (outcome: EntryOutcome) => void = () => undefined;
		const state = initialProjectView();
		const opening = openView(state, item(), {
			loadEntry: () => new Promise((resolve) => (release = resolve)),
			projectName: async () => '',
			refreshDiscovery: async () => undefined,
			latest: () => undefined
		});
		reconcileView(state, { status: 'off', items: [] }, undefined);
		release({ ok: true, item: item(), html: '' });
		await opening;
		assert.equal(state.phase, 'unavailable');
		assert.equal(state.opened, null);
	});

	test('a host close with a reason shows it with a retry, and retry reopens', async () => {
		const v = view();
		v.reconcile(ready([item()]));
		await v.settle();
		assert.equal(closeView(v.state, 'plugins.host.suspended'), 'failed');
		assert.equal(v.state.phase, 'failed');
		assert.equal(v.state.notice, 'plugins.host.suspended');
		v.reconcile();
		assert.equal(v.entries.length, 1, 'a failure waits for the user');
		v.retry();
		await v.settle();
		assert.equal(v.state.phase, 'ready');
		assert.equal(v.entries.length, 2);
	});

	test('a close request from the plugin leaves the view', async () => {
		const v = view();
		v.reconcile(ready([item()]));
		await v.settle();
		assert.equal(closeView(v.state, ''), 'back');
		assert.equal(v.state.opened, null);
	});

	test('every open gets its own token', async () => {
		const v = view();
		v.reconcile(ready([item()]));
		await v.settle();
		const tokens = new Set([v.state.opened!.token]);
		closeView(v.state, 'x');
		v.retry();
		await v.settle();
		tokens.add(v.state.opened!.token);
		assert.equal(tokens.size, 2);
	});
});
