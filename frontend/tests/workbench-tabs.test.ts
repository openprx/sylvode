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
		assertEqual(restored.visit('/flow/ninth'), false, 'in-session ninth blocked');
		assertEqual(restored.current.active, 'flow:deep-link', 'active URL unchanged');
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
	assertEqual(state.current.limit, false, 'capacity warning not triggered');
	assertEqual(state.current.active, 'flow:full-7', 'active stays same despite movement');
});
finish(suite.result());
