/** Pure logic behind the focus trap, roving focus, Escape stack, positioning and messages. */
import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import { resetIds, uid } from './ids';
import { trapTarget } from './focus';
import { isTopLayer, openLayerCount, pushLayer } from './layers';
import { mergeMessages } from './messages';
import { computePosition } from './position';
import { nextRovingIndex } from './roving';

const suite = new Suite('ui-foundations');

suite.check('uid yields distinct, prefixed ids', () => {
	resetIds();
	assertEqual(uid('a'), 'ui-a-1', 'first');
	assertEqual(uid('a'), 'ui-a-2', 'second');
});

// Focus trap: Tab wraps from the last to the first item and Shift+Tab from the first to the last.
const items = ['a', 'b', 'c'];
const inside = (item: string | null) => item !== null && items.includes(item);
suite.check('trap: Tab on the last item wraps to the first', () => {
	assertEqual(trapTarget(items, 'c', false, inside), 'a', 'wrap forward');
});
suite.check('trap: Shift+Tab on the first item wraps to the last', () => {
	assertEqual(trapTarget(items, 'a', true, inside), 'c', 'wrap back');
});
suite.check('trap: inside the scope the browser moves focus itself', () => {
	assertEqual(trapTarget(items, 'b', false, inside), null, 'forward');
	assertEqual(trapTarget(items, 'b', true, inside), null, 'back');
});
suite.check('trap: focus outside the scope is pulled in', () => {
	assertEqual(trapTarget(items, 'x', false, inside), 'a', 'forward enters at first');
	assertEqual(trapTarget(items, null, true, inside), 'c', 'back enters at last');
});
suite.check('trap: no tabbable items leaves the decision to the caller', () => {
	assertEqual(trapTarget([], null, false, inside), null, 'empty');
});

// Roving focus.
const all = [true, true, true, true];
suite.check('roving: arrows step and wrap', () => {
	assertEqual(nextRovingIndex('ArrowDown', 0, all), 1, 'down');
	assertEqual(nextRovingIndex('ArrowDown', 3, all), 0, 'wrap down');
	assertEqual(nextRovingIndex('ArrowUp', 0, all), 3, 'wrap up');
});
suite.check('roving: no wrap when loop is off', () => {
	assertEqual(nextRovingIndex('ArrowDown', 3, all, { loop: false }), null, 'end');
});
suite.check('roving: Home and End go to the first and last enabled item', () => {
	const mixed = [false, true, true, false];
	assertEqual(nextRovingIndex('Home', 2, mixed), 1, 'home');
	assertEqual(nextRovingIndex('End', 1, mixed), 2, 'end');
});
suite.check('roving: disabled items are skipped', () => {
	assertEqual(nextRovingIndex('ArrowDown', 0, [true, false, true]), 2, 'skip');
});
suite.check('roving: orientation filters keys and rtl mirrors horizontal arrows', () => {
	assertEqual(nextRovingIndex('ArrowRight', 0, all, { orientation: 'vertical' }), null, 'v');
	assertEqual(nextRovingIndex('ArrowDown', 0, all, { orientation: 'horizontal' }), null, 'h');
	assertEqual(nextRovingIndex('ArrowRight', 1, all, { orientation: 'horizontal' }), 2, 'ltr');
	assertEqual(
		nextRovingIndex('ArrowRight', 1, all, { orientation: 'horizontal', rtl: true }),
		0,
		'rtl'
	);
	assertEqual(nextRovingIndex('a', 0, all), null, 'other key');
});
suite.check('roving: a single enabled item has nowhere to go', () => {
	assertEqual(nextRovingIndex('ArrowDown', 1, [false, true, false]), 1, 'stays');
});

// Escape stack, with a stand-in document.
suite.check('layers: Escape reaches only the topmost layer, then the next', () => {
	const handlers: ((event: unknown) => void)[] = [];
	(globalThis as unknown as { document: unknown }).document = {
		addEventListener: (_type: string, fn: (event: unknown) => void) => handlers.push(fn),
		removeEventListener: (_type: string, fn: (event: unknown) => void) => {
			const index = handlers.indexOf(fn);
			if (index >= 0) handlers.splice(index, 1);
		}
	};
	const calls: string[] = [];
	const bottom = { onEscape: () => calls.push('bottom') };
	const top = { onEscape: () => calls.push('top') };
	const removeBottom = pushLayer(bottom);
	const removeTop = pushLayer(top);
	assert(isTopLayer(top) && !isTopLayer(bottom), 'top is on top');
	const press = () =>
		handlers.forEach((fn) => fn({ key: 'Escape', preventDefault() {}, stopPropagation() {} }));
	press();
	assertDeepEqual(calls, ['top'], 'first Escape');
	removeTop();
	removeTop();
	press();
	assertDeepEqual(calls, ['top', 'bottom'], 'second Escape');
	removeBottom();
	assertEqual(openLayerCount(), 0, 'stack empty');
	assertEqual(handlers.length, 0, 'listener removed');
	delete (globalThis as { document?: unknown }).document;
});

// Positioning.
const viewport = { width: 1000, height: 800 };
suite.check('position: below the anchor, aligned to its start', () => {
	const p = computePosition({
		anchor: { left: 100, top: 100, width: 80, height: 32 },
		floating: { width: 200, height: 100 },
		viewport
	});
	assertDeepEqual(p, { x: 100, y: 138, placement: 'bottom-start' }, 'placement');
});
suite.check('position: flips above when there is no room below', () => {
	const p = computePosition({
		anchor: { left: 100, top: 700, width: 80, height: 32 },
		floating: { width: 200, height: 150 },
		viewport
	});
	assertEqual(p.placement, 'top-start', 'flipped');
	assertEqual(p.y, 700 - 150 - 6, 'above the anchor');
});
suite.check('position: end alignment, rtl mirror and viewport clamp', () => {
	const anchor = { left: 900, top: 100, width: 80, height: 32 };
	const floating = { width: 300, height: 50 };
	assertEqual(computePosition({ anchor, floating, viewport }).x, 692, 'clamped to the right edge');
	assertEqual(
		computePosition({ anchor, floating, viewport, placement: 'bottom-end' }).x,
		680,
		'end aligned'
	);
	assertEqual(
		computePosition({
			anchor: { left: 100, top: 100, width: 80, height: 32 },
			floating,
			viewport,
			rtl: true
		}).x,
		8,
		'rtl start aligns to the right edge, then clamps'
	);
});
suite.check('position: centred placement for tooltips', () => {
	const p = computePosition({
		anchor: { left: 200, top: 100, width: 32, height: 32 },
		floating: { width: 60, height: 24 },
		viewport,
		placement: 'bottom'
	});
	assertEqual(p.x, 186, 'centre');
	assertEqual(p.placement, 'bottom', 'kept');
});

// Message merging.
suite.check('messages: deep merge adds keys and rejects conflicts', () => {
	const merged = mergeMessages({ a: { x: '1' } }, { a: { y: '2' } }, { ui: { b: '3' } });
	assertDeepEqual(merged, { a: { x: '1', y: '2' }, ui: { b: '3' } }, 'merged');
	let failed = false;
	try {
		mergeMessages({}, { ui: { b: '1' } }, { ui: { b: '2' } });
	} catch {
		failed = true;
	}
	assert(failed, 'conflicting leaf must throw');
	failed = false;
	try {
		mergeMessages({}, { ui: 'leaf' }, { ui: { b: '2' } });
	} catch {
		failed = true;
	}
	assert(failed, 'leaf versus branch must throw');
});

finish(suite.result());
