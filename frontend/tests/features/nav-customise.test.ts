/**
 * What a person changes in the navigation (`lib/nav/customise.ts`).
 *
 * Run standalone: `bun test tests/features/nav-customise.test.ts`
 */
import { test } from 'node:test';
import nodeAssert from 'node:assert/strict';
import {
	applyOrder,
	buildSections,
	emptyPrefs,
	moveWithin,
	setHidden,
	setPinned,
	setSectionCollapsed,
	stepWithin
} from '../../src/lib/nav/customise';
import type { ResolvedEntry, ResolvedNav } from '../../src/lib/nav/types';

function entry(id: string, position: number, extra: Partial<ResolvedEntry> = {}): ResolvedEntry {
	return {
		id,
		section_id: 'main',
		kind: 'builtin',
		target: { kind: 'builtin', page: `project.${id}` },
		label: null,
		name: null,
		icon: null,
		href: null,
		pinned: false,
		locked: false,
		user_hidden: false,
		position,
		...extra
	};
}

const menu: ResolvedNav = {
	schema: 'sv.nav.resolved.v1',
	scope: 'project',
	scope_id: 'p',
	config_version: 1,
	customised: true,
	sections: [
		{
			id: '_pinned',
			label: null,
			collapsed: false,
			entries: [entry('cycles', 3, { pinned: true })]
		},
		{
			id: 'main',
			label: null,
			collapsed: true,
			entries: [entry('overview', 0, { locked: true }), entry('issues', 1), entry('board', 2)]
		}
	]
};
const ids = (prefs = emptyPrefs()) =>
	buildSections(menu, emptyPrefs(), prefs).flatMap((s) => s.entries.map((e) => e.entry.id));

test('pinned entries return to the section they are stored in, at their position', () => {
	nodeAssert.deepEqual(ids(), ['overview', 'issues', 'board', 'cycles']);
});

test('a locked entry ignores hide, pin and move; the others obey', () => {
	const locked = menu.sections[1].entries[0];
	const issues = menu.sections[1].entries[1];
	nodeAssert.deepEqual(setHidden(emptyPrefs(), locked, true), emptyPrefs());
	nodeAssert.deepEqual(setPinned(emptyPrefs(), locked, true), emptyPrefs());
	nodeAssert.deepEqual(setHidden(emptyPrefs(), issues, true).hidden, ['issues']);
	nodeAssert.deepEqual(setHidden(setHidden(emptyPrefs(), issues, true), issues, false).hidden, []);
});

test('moving keeps locked entries at their index', () => {
	const section = buildSections(menu, emptyPrefs(), emptyPrefs())[0];
	const prefs = moveWithin(emptyPrefs(), section, 'cycles', 0);
	nodeAssert.deepEqual(prefs.order.main, ['cycles', 'issues', 'board']);
	nodeAssert.deepEqual(ids(prefs), ['overview', 'cycles', 'issues', 'board']);
	const down = stepWithin(emptyPrefs(), section, 'issues', 1);
	nodeAssert.deepEqual(ids(down), ['overview', 'board', 'issues', 'cycles']);
	nodeAssert.deepEqual(stepWithin(emptyPrefs(), section, 'overview', 1), emptyPrefs());
});

test('applyOrder lists the listed ids first and leaves locked ones in place', () => {
	const items = [
		{ id: 'a', locked: false },
		{ id: 'b', locked: true },
		{ id: 'c', locked: false },
		{ id: 'd', locked: false }
	];
	nodeAssert.deepEqual(
		applyOrder(items, ['d']).map((i) => i.id),
		['d', 'b', 'a', 'c']
	);
	nodeAssert.deepEqual(
		applyOrder(items, undefined).map((i) => i.id),
		['a', 'b', 'c', 'd']
	);
});

test('an administrator pin is dropped by unpinning, the own pin by unpinning too', () => {
	const cycles = menu.sections[0].entries[0];
	const unpinned = setPinned(emptyPrefs(), cycles, false);
	const view = buildSections(menu, emptyPrefs(), unpinned);
	nodeAssert.equal(view[0].entries.find((e) => e.entry.id === 'cycles')?.pinned, false);
	const board = menu.sections[1].entries[2];
	const pinned = setPinned(emptyPrefs(), board, true);
	nodeAssert.equal(
		buildSections(menu, emptyPrefs(), pinned)[0].entries.find((e) => e.entry.id === 'board')
			?.pinned,
		true
	);
	nodeAssert.deepEqual(setPinned(pinned, board, false).pinned, []);
});

test('collapse overrides the configured default in both directions', () => {
	const sections = (p = emptyPrefs()) => buildSections(menu, emptyPrefs(), p)[0].collapsed;
	nodeAssert.equal(sections(), true);
	nodeAssert.equal(sections(setSectionCollapsed(emptyPrefs(), 'main', false)), false);
	const open = setSectionCollapsed(emptyPrefs(), 'main', false);
	nodeAssert.equal(sections(setSectionCollapsed(open, 'main', true)), true);
});
