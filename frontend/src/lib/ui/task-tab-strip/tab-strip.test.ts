import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { computeOverflow, dropIndex, focusTarget, reorder, tabKeyAction } from './tab-strip';

await installSvelteSsr();
const { default: TaskTabStrip } = await import('./TaskTabStrip.svelte');

const suite = new Suite('ui-task-tab-strip');

suite.check('tabs that fit share the width, capped at 200', () => {
	const few = computeOverflow(3, 0, 1200);
	assertEqual(few.mode, 'tabs', 'mode');
	assertEqual(few.tabWidth, 200, 'cap');
	assertDeepEqual(few.hidden, [], 'none hidden');
});
suite.check('tabs shrink down to 120 before any is hidden', () => {
	// room = 900 - 16 - 80 = 804; 7 tabs: floor((804 - 12) / 7) = 113 < 120 -> hide; 6: 132.
	const six = computeOverflow(6, 0, 900);
	assertEqual(six.hidden.length, 0, 'six fit');
	assert(six.tabWidth >= 120 && six.tabWidth < 200, `shrunk to ${six.tabWidth}`);
});
suite.check('the rightmost inactive tabs are hidden and the active tab stays visible', () => {
	const layout = computeOverflow(8, 0, 724);
	assertEqual(layout.visible.length + layout.hidden.length, 8, 'all accounted for');
	assert(layout.hidden.length > 0, 'something hidden at 724');
	assert(layout.hidden.includes(7) && layout.visible.includes(0), 'right side goes first');
	const late = computeOverflow(8, 7, 724);
	assert(late.visible.includes(7), 'active last tab is shown');
	assert(
		!late.visible.includes(0) || late.visible.length > 1,
		'room is made on the right side of the rest'
	);
	assertEqual(late.tabWidth >= 120, true, 'minimum width');
});
suite.check('seven tabs at 724 show the "+n" overflow and the active tab', () => {
	const layout = computeOverflow(7, 4, 724);
	assert(layout.hidden.length >= 1, 'hidden count for +n');
	assert(layout.visible.includes(4), 'active visible');
});
suite.check('below 600 only the active tab remains', () => {
	const layout = computeOverflow(5, 2, 500);
	assertEqual(layout.mode, 'collapsed', 'mode');
	assertDeepEqual(layout.visible, [2], 'active only');
	assertDeepEqual(layout.hidden, [0, 1, 3, 4], 'rest in the menu');
});
suite.check('no tabs, no layout', () => {
	assertDeepEqual(computeOverflow(0, 0, 900).visible, [], 'empty');
});

suite.check('reorder moves one key and clamps', () => {
	const keys = ['a', 'b', 'c', 'd'];
	assertDeepEqual(reorder(keys, 'b', 2), ['a', 'c', 'b', 'd'], 'right');
	assertDeepEqual(reorder(keys, 'c', 0), ['c', 'a', 'b', 'd'], 'left');
	assertDeepEqual(reorder(keys, 'a', -5), keys, 'clamped at the start');
	assertDeepEqual(reorder(keys, 'd', 9), keys, 'clamped at the end');
	assertDeepEqual(reorder(keys, 'x', 1), keys, 'unknown key');
	assertDeepEqual(keys, ['a', 'b', 'c', 'd'], 'input untouched');
});
suite.check('dropIndex accounts for the removed source', () => {
	const keys = ['a', 'b', 'c', 'd'];
	assertEqual(dropIndex(keys, 'a', 'c', 'before'), 1, 'a before c');
	assertEqual(dropIndex(keys, 'a', 'c', 'after'), 2, 'a after c');
	assertEqual(dropIndex(keys, 'd', 'b', 'before'), 1, 'd before b');
	assertEqual(dropIndex(keys, 'b', 'a', 'after'), null, 'no move');
	assertEqual(dropIndex(keys, 'b', 'b', 'after'), null, 'onto itself');
});

suite.check('arrows wrap, Home and End jump', () => {
	assertEqual(focusTarget('ArrowRight', 3, 4), 0, 'wrap forward');
	assertEqual(focusTarget('ArrowLeft', 0, 4), 3, 'wrap back');
	assertEqual(focusTarget('Home', 2, 4), 0, 'home');
	assertEqual(focusTarget('End', 0, 4), 3, 'end');
	assertEqual(focusTarget('ArrowDown', 0, 4), null, 'vertical arrows are not used');
});
const ev = (init: Partial<Parameters<typeof tabKeyAction>[0]>) => ({
	key: '',
	altKey: false,
	shiftKey: false,
	ctrlKey: false,
	metaKey: false,
	...init
});
suite.check('key actions: Alt+Shift+Arrow moves, Delete closes, menu keys open the menu', () => {
	assertDeepEqual(
		tabKeyAction(ev({ key: 'ArrowRight', altKey: true, shiftKey: true }), 1, 4),
		{ kind: 'move', toIndex: 2 },
		'move right'
	);
	assertDeepEqual(
		tabKeyAction(ev({ key: 'ArrowLeft', altKey: true, shiftKey: true }), 0, 4),
		{ kind: 'move', toIndex: 0 },
		'clamped'
	);
	assertDeepEqual(tabKeyAction(ev({ key: 'Delete' }), 1, 4), { kind: 'close' }, 'close');
	assertDeepEqual(
		tabKeyAction(ev({ key: 'F10', shiftKey: true }), 1, 4),
		{ kind: 'menu' },
		'shift f10'
	);
	assertDeepEqual(tabKeyAction(ev({ key: 'ContextMenu' }), 1, 4), { kind: 'menu' }, 'menu key');
	assertDeepEqual(
		tabKeyAction(ev({ key: 'ArrowRight' }), 1, 4),
		{ kind: 'focus', index: 2 },
		'focus'
	);
});
suite.check('browser-owned shortcuts are never handled', () => {
	assertEqual(tabKeyAction(ev({ key: 'ArrowRight', ctrlKey: true }), 1, 4), null, 'ctrl arrow');
	assertEqual(tabKeyAction(ev({ key: 'Tab', ctrlKey: true }), 1, 4), null, 'ctrl tab');
	assertEqual(tabKeyAction(ev({ key: 'Delete', metaKey: true }), 1, 4), null, 'cmd delete');
});

await suite.checkAsync(
	'markup: tablist, one tab stop, selected tab, markers named in text',
	async () => {
		const html = await renderToHtml(TaskTabStrip, {
			tabs: [
				{ key: 'a', title: 'Alpha', dirty: false, changed: false },
				{ key: 'b', title: 'Beta', dirty: true, changed: true }
			],
			active: 'b',
			width: 1000,
			panelId: (key: string) => `panel-${key}`,
			onselect: () => {},
			onclose: () => {},
			onconfirmdiscard: () => {},
			onmove: () => {},
			onresolvepending: () => {},
			oncloseSaved: () => {}
		});
		assert(
			html.includes('role="tablist"') && html.includes('data-testid="task-tablist"'),
			'tablist'
		);
		const tabs = [...html.matchAll(/<button[^>]*role="tab"[^>]*>/g)].map((m) => m[0]);
		assertEqual(tabs.length, 2, 'two tabs');
		assert(tabs[0].includes('tabindex="-1"') && tabs[0].includes('aria-selected="false"'), tabs[0]);
		assert(tabs[1].includes('tabindex="0"') && tabs[1].includes('aria-selected="true"'), tabs[1]);
		assert(tabs[1].includes('aria-controls="panel-b"'), 'controls');
		assert(
			tabs[1].includes('ui.taskTabStrip.unsaved') || html.includes('ui.taskTabStrip.unsaved'),
			'unsaved text'
		);
		assert(
			html.includes('aria-keyshortcuts="Alt+Shift+ArrowLeft Alt+Shift+ArrowRight"'),
			'shortcut hint'
		);
	}
);
await suite.checkAsync('no tabs, no strip', async () => {
	const html = await renderToHtml(TaskTabStrip, {
		tabs: [],
		active: null,
		panelId: () => '',
		onselect: () => {},
		onclose: () => {},
		onconfirmdiscard: () => {},
		onmove: () => {},
		onresolvepending: () => {},
		oncloseSaved: () => {}
	});
	assert(!html.includes('role="tablist"'), 'not rendered');
});

finish(suite.result());
