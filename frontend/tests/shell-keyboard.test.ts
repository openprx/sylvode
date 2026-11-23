/**
 * Global shell keys (`lib/shell/keyboard.ts`) and the resolved theme (`lib/theme.ts`).
 *
 * Run standalone: `bun tests/shell-keyboard.test.ts`
 */
import { Suite, assertDeepEqual, assertEqual, finish } from './support/harness';
import { installSvelteSsr } from './support/svelte-ssr';
import {
	SEQUENCE_TIMEOUT_MS,
	createShellKeyHandler,
	isEditableTarget,
	type KeyEventLike
} from '../src/lib/shell/keyboard';

await installSvelteSsr();
const { resolveTheme } = await import('../src/lib/theme');

const suite = new Suite('shell-keyboard');

function setup(options: { modal?: boolean; global?: boolean; focusRow?: boolean } = {}) {
	const log: string[] = [];
	let clock = 1000;
	const handler = createShellKeyHandler({
		actions: {
			togglePalette: () => log.push('palette'),
			openSettings: () => log.push('settings'),
			goTo: (d) => log.push(`go:${d}`),
			addFocusedRow: () => {
				log.push('addRow');
				return options.focusRow ?? true;
			}
		},
		globalShortcuts: () => options.global ?? true,
		modalOpen: () => options.modal ?? false,
		now: () => clock
	});
	let prevented = 0;
	const press = (key: string, extra: Partial<KeyEventLike> = {}, advance = 0) => {
		clock += advance;
		const event: KeyEventLike = {
			key,
			metaKey: false,
			ctrlKey: false,
			altKey: false,
			shiftKey: false,
			defaultPrevented: false,
			target: null,
			preventDefault: () => void prevented++,
			...extra
		};
		return handler(event);
	};
	return { log, press, prevented: () => prevented };
}

const input = { tagName: 'INPUT' } as unknown as EventTarget;

suite.check('Cmd or Ctrl+K toggles the palette, even while typing', () => {
	const s = setup();
	assertEqual(s.press('k', { metaKey: true }), true, 'cmd');
	assertEqual(s.press('K', { ctrlKey: true, target: input }), true, 'ctrl in an input');
	assertDeepEqual(s.log, ['palette', 'palette'], 'twice');
	assertEqual(s.prevented(), 2, 'default prevented');
});

suite.check('an editor that already handled Ctrl+K keeps it', () => {
	const s = setup();
	assertEqual(s.press('k', { ctrlKey: true, defaultPrevented: true }), false, 'ignored');
	assertDeepEqual(s.log, [], 'no palette');
});

suite.check('Cmd or Ctrl+comma opens settings', () => {
	const s = setup();
	s.press(',', { metaKey: true });
	s.press(',', { ctrlKey: true, shiftKey: true });
	assertDeepEqual(s.log, ['settings'], 'plain only');
});

suite.check('G then H, P, F, A, S', () => {
	const s = setup();
	for (const key of ['h', 'p', 'f', 's']) {
		s.press('g');
		s.press(key);
	}
	assertDeepEqual(s.log, ['go:home', 'go:projects', 'go:flow', 'settings'], 'all four');
	assertEqual(s.prevented(), 4, 'second key prevented');
});

suite.check('the second key must follow within a second', () => {
	const s = setup();
	s.press('g');
	assertEqual(s.press('h', {}, SEQUENCE_TIMEOUT_MS), true, 'at the limit');
	s.press('g');
	assertEqual(s.press('h', {}, SEQUENCE_TIMEOUT_MS + 1), false, 'too late');
	assertDeepEqual(s.log, ['go:home'], 'only the first');
});

suite.check('unknown second key cancels; G then G starts over', () => {
	const s = setup();
	s.press('g');
	s.press('x');
	s.press('h');
	assertDeepEqual(s.log, [], 'cancelled');
	s.press('g');
	s.press('g');
	s.press('p');
	assertDeepEqual(s.log, ['go:projects'], 'restarted');
});

suite.check('sequences are ignored while typing, behind a modal, or with modifiers', () => {
	const typing = setup();
	typing.press('g', { target: input });
	typing.press('h', { target: input });
	const editor = setup();
	const pm = {
		tagName: 'DIV',
		closest: (sel: string) => (sel.includes('.ProseMirror') ? {} : null)
	} as unknown as EventTarget;
	editor.press('g', { target: pm });
	editor.press('h', { target: pm });
	const textbox = {
		tagName: 'DIV',
		getAttribute: (n: string) => (n === 'role' ? 'textbox' : null)
	} as unknown as EventTarget;
	editor.press('g', { target: textbox });
	editor.press('h', { target: textbox });
	const modal = setup({ modal: true });
	modal.press('g');
	modal.press('h');
	const alt = setup();
	alt.press('g');
	alt.press('h', { altKey: true });
	assertDeepEqual([...typing.log, ...editor.log, ...modal.log, ...alt.log], [], 'nothing fired');
});

suite.check('Shift does not block a sequence; key repeat does', () => {
	const s = setup();
	s.press('G', { shiftKey: true });
	s.press('H', { shiftKey: true });
	s.press('g', { repeat: true });
	s.press('p');
	assertDeepEqual(
		s.log,
		['go:home', 'addRow'],
		'repeat G started no sequence, so P is the add-row key'
	);
});

suite.check('P on a focused column row adds it; otherwise it is left alone', () => {
	const row = setup({ focusRow: true });
	assertEqual(row.press('p'), true, 'handled');
	const none = setup({ focusRow: false });
	assertEqual(none.press('p'), false, 'not handled');
	assertEqual(none.prevented(), 0, 'default kept');
	const typing = setup();
	typing.press('p', { target: input });
	assertDeepEqual(typing.log, [], 'not while typing');
});

suite.check('hosted frames get no global keys; Escape is never handled', () => {
	const s = setup({ global: false });
	s.press('k', { metaKey: true });
	s.press('g');
	s.press('h');
	assertDeepEqual(s.log, [], 'hosted');
	const free = setup();
	assertEqual(free.press('Escape'), false, 'escape');
});

suite.check('editable targets', () => {
	assertEqual(isEditableTarget(null), false, 'null');
	assertEqual(
		isEditableTarget({ tagName: 'TEXTAREA' } as unknown as EventTarget),
		true,
		'textarea'
	);
	assertEqual(isEditableTarget({ tagName: 'SELECT' } as unknown as EventTarget), true, 'select');
	assertEqual(
		isEditableTarget({ tagName: 'DIV', isContentEditable: true } as unknown as EventTarget),
		true,
		'contenteditable'
	);
	assertEqual(isEditableTarget({ tagName: 'BUTTON' } as unknown as EventTarget), false, 'button');
});

suite.check('resolveTheme resolves system against the OS preference', () => {
	assertEqual(resolveTheme('light', true), 'light', 'light');
	assertEqual(resolveTheme('dark', false), 'dark', 'dark');
	assertEqual(resolveTheme('system', true), 'dark', 'system dark');
	assertEqual(resolveTheme('system', false), 'light', 'system light');
});

finish(suite.result());
