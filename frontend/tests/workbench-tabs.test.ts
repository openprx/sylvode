import { TaskWorkbench, taskResource } from '../src/lib/workbench/tabs';
import { projectConflicts } from '../src/lib/workbench/project-session';
import { Suite, assertEqual, finish } from './support/harness';
const data = new Map<string, string>();
let writes = 0;
Object.defineProperty(globalThis, 'localStorage', {
	configurable: true,
	value: {
		getItem: (key: string) => data.get(key) ?? null,
		setItem: (key: string, value: string) => {
			writes++;
			data.set(key, value);
		},
		removeItem: (key: string) => data.delete(key)
	}
});
const suite = new Suite('task workbench');
suite.check(
	'full restored metadata yields to an explicit deep link without exceeding eight',
	() => {
		const first = new TaskWorkbench();
		first.activateUser('a');
		for (let i = 0; i < 8; i++) first.visit(`/flow/page-${i}`);
		const restored = new TaskWorkbench();
		restored.activateUser('a');
		restored.restoreRoute('/flow/deep-link');
		assertEqual(restored.current.tabs.length, 8, 'bounded restoration');
		assertEqual(restored.current.active, 'flow:deep-link', 'deep link active');
		assertEqual(restored.current.restoredTrimmed, true, 'visible limit notice');
		restored.register('flow:deep-link', {
			capture: () => ({ draft: 'SECRET-DRAFT' }),
			dirty: () => true,
			refresh: async () => {}
		});
		restored.capture();
		for (const tab of restored.current.tabs) restored.markDirty(tab.key, true);
		assertEqual(restored.canOpen('/flow/ninth'), false, 'ninth refused when all eight unsaved');
		assertEqual(restored.visit('/flow/ninth').kind, 'blocked', 'in-session ninth blocked');
		assertEqual(restored.current.pending?.key, 'flow:ninth', 'pending remembers the target');
		assertEqual(restored.current.active, 'flow:deep-link', 'active URL unchanged');
		restored.resolvePending(null);
		assertEqual(restored.current.pending, null, 'cancel clears pending');
		assertEqual(restored.close('flow:deep-link').kind, 'confirm', 'store close guarded');
		assertEqual(
			restored.snapshot<{ draft: string }>('flow:deep-link')?.draft,
			'SECRET-DRAFT',
			'draft retained'
		);
		assertEqual(
			[...data.values()].some((value) => value.includes('SECRET-DRAFT')),
			false,
			'business draft never persisted'
		);
	}
);
suite.check('distinct forms are independent while record selection remains within its form', () => {
	const base = '/workspace/w/projects/p/forms';
	assertEqual(taskResource(base + '?form=a')?.key, 'forms:p:a', 'form A');
	assertEqual(taskResource(base + '?form=b')?.key, 'forms:p:b', 'form B');
	assertEqual(
		taskResource(base + '?form=a&record=one')?.key,
		taskResource(base + '?form=a&record=two')?.key,
		'record selection internal'
	);
	assertEqual(taskResource(base)?.key, 'forms-entry:p', 'entry distinct');
	assertEqual(
		taskResource('/flow/one')?.key,
		taskResource('/workspace/w/flow/one')?.key,
		'canonical Flow dedupe'
	);
	assertEqual(
		taskResource('/flow/one?access_token=NEVER-STORE')?.url.includes('NEVER-STORE'),
		false,
		'token metadata excluded'
	);
});
suite.check(
	'account changes discard in-memory sessions and release pins but restore only own positions',
	() => {
		const state = new TaskWorkbench();
		state.activateUser('one');
		state.visit('/flow/one');
		state.saveSnapshot('flow:one', { draft: 'private' });
		let released = 0;
		state.retain('flow:one', () => released++);
		state.activateUser('two');
		assertEqual(released, 1, 'old pins released');
		assertEqual(state.snapshot('flow:one'), undefined, 'old draft inaccessible');
		assertEqual(state.current.tabs.length, 0, 'other account positions isolated');
		state.activateUser('one');
		assertEqual(state.current.tabs[0]?.key, 'flow:one', 'own position restored');
		assertEqual(state.snapshot('flow:one'), undefined, 'no relogin draft claim');
	}
);
suite.check(
	'same-field conflicts distinguish remote changes from untouched or already-matching fields',
	() => {
		const base = { name: 'old', description: 'base', updated_at: '2026-01-01T00:00:00.123456Z' };
		const local = { name: 'mine', description: 'base' };
		assertEqual(
			JSON.stringify(projectConflicts(base, local, { ...base, name: 'remote' })),
			JSON.stringify(['name']),
			'same field conflict'
		);
		assertEqual(
			projectConflicts(base, local, { ...base, name: 'mine' }).length,
			0,
			'matching value safe for explicit review'
		);
	}
);
suite.check('reloading the fifth active tab preserves every saved position', () => {
	const first = new TaskWorkbench();
	first.activateUser('order-case');
	for (let i = 0; i < 5; i++) first.visit(`/flow/order-${i}`);
	const before = first.current.tabs.map((tab) => tab.key);
	const restored = new TaskWorkbench();
	restored.activateUser('order-case');
	restored.restoreRoute('/flow/order-4');
	assertEqual(
		JSON.stringify(restored.current.tabs.map((tab) => tab.key)),
		JSON.stringify(before),
		'saved order unchanged'
	);
	assertEqual(restored.current.active, 'flow:order-4', 'last tab active');
});

suite.check(
	'moving tabs across positions preserves active resource, dirty snapshot, scroll and pins',
	() => {
		const state = new TaskWorkbench();
		state.activateUser('reorder');
		for (const id of ['a', 'b', 'c', 'd']) state.visit(`/flow/${id}`);
		state.visit('/flow/b');
		state.setScroll('flow:b', 87);
		state.markDirty('flow:b', true);
		state.markChanged('flow:c');
		const draft = { value: 'unsaved', version: 'raw-version' };
		state.saveSnapshot('flow:b', draft);
		let released = 0;
		state.retain('flow:b', () => released++);
		const tab = state.current.tabs.find((tab) => tab.key === 'flow:b');
		assertEqual(state.moveTabRelative('flow:a', 'flow:d', 'after'), true, 'first to last');
		assertEqual(
			state.current.tabs.map((tab) => tab.key).join(','),
			'flow:b,flow:c,flow:d,flow:a',
			'cross-position order'
		);
		assertEqual(state.moveTabRelative('flow:d', 'flow:b', 'before'), true, 'middle to first');
		assertEqual(state.current.active, 'flow:b', 'active identity unchanged');
		assertEqual(
			state.current.tabs.find((item) => item.key === 'flow:b'),
			tab,
			'tab object identity unchanged'
		);
		assertEqual(tab?.url, '/flow/b', 'href unchanged');
		assertEqual(tab?.dirty, true, 'dirty unchanged');
		assertEqual(tab?.scroll, 87, 'scroll unchanged');
		assertEqual(state.snapshot('flow:b'), draft, 'same draft retained');
		assertEqual(released, 0, 'session not released');
		const restored = new TaskWorkbench();
		restored.activateUser('reorder');
		restored.restoreRoute('/flow/b');
		assertEqual(
			restored.current.tabs.map((tab) => tab.key).join(','),
			state.current.tabs.map((tab) => tab.key).join(','),
			'persisted order restored'
		);
		assertEqual(restored.current.active, 'flow:b', 'active restored');
	}
);
suite.check('self, invalid, unchanged and single-tab reorder make no storage writes', () => {
	const state = new TaskWorkbench();
	state.activateUser('noop-reorder');
	state.visit('/flow/only');
	const before = writes;
	assertEqual(state.moveTab('flow:only', 0), false, 'single no-op');
	assertEqual(state.moveTabRelative('flow:only', 'flow:only', 'after'), false, 'self no-op');
	assertEqual(state.moveTab('missing', 0), false, 'missing source');
	assertEqual(state.moveTab('flow:only', -1), false, 'out of range');
	assertEqual(writes, before, 'no-op does not persist');
});
suite.check('eight full tabs can reorder without applying the open-tab capacity gate', () => {
	const state = new TaskWorkbench();
	state.activateUser('full-reorder');
	for (let i = 0; i < 8; i++) state.visit(`/flow/full-${i}`);
	assertEqual(state.moveTab('flow:full-7', 0), true, 'full-cap reorder works');
	assertEqual(state.current.tabs.length, 8, 'count unchanged');
	assertEqual(state.current.pending, null, 'capacity gate not triggered');
	assertEqual(state.current.active, 'flow:full-7', 'active stays same despite movement');
});
const open = (state: TaskWorkbench, n: number, prefix = 'p') => {
	for (let i = 0; i < n; i++) state.visit(`/flow/${prefix}${i}`);
};
const keys = (state: TaskWorkbench) => state.current.tabs.map((tab) => tab.key).join(',');
suite.check('the ninth item closes the least recently used saved tab and reports it', () => {
	const state = new TaskWorkbench();
	state.activateUser('lru');
	open(state, 8);
	state.visit('/flow/p0');
	state.visit('/flow/p1');
	const seen: string[] = [];
	state.onReplaced = (dropped, title) => seen.push(`${dropped.key}>${title}`);
	const result = state.visit('/flow/p8');
	assertEqual(result.kind, 'replaced', 'replaced');
	assertEqual(result.kind === 'replaced' ? result.dropped.key : '', 'flow:p2', 'LRU dropped');
	assertEqual(state.current.tabs.length, 8, 'still eight');
	assertEqual(state.current.active, 'flow:p8', 'new tab active');
	assertEqual(seen.join(), 'flow:p2>Flow', 'callback');
});
suite.check('unsaved tabs, including auxiliary drafts, are never evicted', () => {
	const state = new TaskWorkbench();
	state.activateUser('lru-dirty');
	open(state, 8);
	state.visit('/flow/p7');
	state.markDirty('flow:p0', true);
	let aux = true;
	state.retainAuxiliary('flow:p1', 'slot', { dirty: () => aux, dispose: () => {} });
	const result = state.visit('/flow/p8');
	assertEqual(result.kind === 'replaced' ? result.dropped.key : '', 'flow:p2', 'next saved tab');
	assertEqual(
		state.current.tabs.some((tab) => tab.key === 'flow:p0'),
		true,
		'dirty kept'
	);
	assertEqual(
		state.current.tabs.some((tab) => tab.key === 'flow:p1'),
		true,
		'auxiliary kept'
	);
	aux = false;
});
suite.check('the active tab is not evicted and all-unsaved blocks with resolvePending', () => {
	const state = new TaskWorkbench();
	state.activateUser('all-dirty');
	open(state, 8);
	for (const tab of state.current.tabs) state.markDirty(tab.key, true);
	assertEqual(state.canOpen('/flow/extra'), false, 'cannot open');
	assertEqual(state.canOpen('/flow/p3'), true, 'existing key opens');
	state.block('/flow/extra');
	assertEqual(state.current.pending?.href, '/flow/extra', 'pending href');
	assertEqual(state.resolvePending('flow:p3').kind, 'confirm', 'dirty close asks first');
	const done = state.resolvePending('flow:p3', true);
	assertEqual(done.kind === 'closed' ? done.href : '', '/flow/extra', 'returns pending href');
	assertEqual(state.current.pending, null, 'pending cleared');
	assertEqual(state.visit('/flow/extra').kind, 'opened', 'now opens');
	assertEqual(state.current.tabs.length, 8, 'eight');
	state.activateUser('active-only');
	open(state, 8);
	for (const tab of state.current.tabs) if (tab.key !== 'flow:p7') state.markDirty(tab.key, true);
	assertEqual(state.canOpen('/flow/extra'), false, 'active clean tab is not a candidate');
	assertEqual(state.visit('/flow/extra').kind, 'blocked', 'active clean tab is not evicted');
});
suite.check('reopening a dropped tab restores its position, scroll and title', () => {
	const state = new TaskWorkbench();
	state.activateUser('reopen');
	open(state, 8);
	state.setTitle('flow:p1', 'Named');
	state.setScroll('flow:p1', 120);
	for (const id of [0, 2, 3, 4, 5, 6, 7]) state.visit(`/flow/p${id}`);
	const before = keys(state);
	const result = state.visit('/flow/p8');
	assertEqual(result.kind === 'replaced' ? result.dropped.key : '', 'flow:p1', 'dropped p1');
	const dropped = result.kind === 'replaced' ? result.dropped : null;
	assertEqual(state.visit(dropped?.url ?? '').kind, 'replaced', 'reopen evicts another');
	const tab = state.current.tabs.find((item) => item.key === 'flow:p1');
	assertEqual(tab?.title, 'Named', 'title restored');
	assertEqual(tab?.scroll, 120, 'scroll restored');
	assertEqual(state.current.tabs.indexOf(tab!), 1, 'position restored');
	assertEqual(state.current.active, 'flow:p1', 'active');
	assertEqual(before.split(',').length, 8, 'eight before');
});
suite.check('closeSaved keeps unsaved tabs and the excepted tab', () => {
	const state = new TaskWorkbench();
	state.activateUser('close-saved');
	open(state, 4);
	state.markDirty('flow:p1', true);
	assertEqual(state.closeSaved('flow:p3').join(), 'flow:p0,flow:p2', 'closed saved');
	assertEqual(keys(state), 'flow:p1,flow:p3', 'remaining');
});
suite.check('lastActive persists and the restore trim uses it', () => {
	const first = new TaskWorkbench();
	first.activateUser('persist-lru');
	open(first, 8);
	first.visit('/flow/p0');
	const restored = new TaskWorkbench();
	restored.activateUser('persist-lru');
	assertEqual(
		restored.current.tabs.every((tab) => tab.lastActive > 0),
		true,
		'stamps restored'
	);
	restored.restoreRoute('/flow/new');
	assertEqual(
		restored.current.tabs.some((tab) => tab.key === 'flow:p1'),
		false,
		'LRU trimmed'
	);
	assertEqual(
		restored.current.tabs.some((tab) => tab.key === 'flow:p0'),
		true,
		'recent kept'
	);
	assertEqual(restored.current.restoredTrimmed, true, 'notice flag');
});
finish(suite.result());
