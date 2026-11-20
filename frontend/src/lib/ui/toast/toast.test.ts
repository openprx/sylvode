import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import {
	ACTION_MS,
	MAX_VISIBLE,
	NEUTRAL_MS,
	ToastQueue,
	isEditableElement,
	isRegionKey,
	isUndoKey,
	type TimerApi
} from './toast-queue';
import { Toasts } from './toast-store';

class FakeTimers implements TimerApi {
	time = 0;
	private next = 1;
	private pending = new Map<number, { at: number; fn: () => void }>();
	setTimeout(fn: () => void, ms: number) {
		const id = this.next++;
		this.pending.set(id, { at: this.time + ms, fn });
		return id;
	}
	clearTimeout(handle: unknown) {
		this.pending.delete(handle as number);
	}
	now() {
		return this.time;
	}
	advance(ms: number) {
		const target = this.time + ms;
		for (;;) {
			const due = [...this.pending.entries()]
				.filter(([, t]) => t.at <= target)
				.sort((a, b) => a[1].at - b[1].at)[0];
			if (!due) break;
			this.pending.delete(due[0]);
			this.time = due[1].at;
			due[1].fn();
		}
		this.time = target;
	}
}

function setup() {
	const timers = new FakeTimers();
	const announced: string[] = [];
	const queue = new ToastQueue({ timers, onannounce: (text) => announced.push(text) });
	return { timers, announced, queue };
}
const noop = () => {};
const undoAction = { label: 'Undo', kind: 'undo' as const, run: noop };

const suite = new Suite('ui-toast');

suite.check('a neutral toast goes after 6 s, an action toast after 10 s', () => {
	const { timers, queue } = setup();
	queue.show({ message: 'a' });
	queue.show({ message: 'b', action: undoAction });
	timers.advance(NEUTRAL_MS - 1);
	assertEqual(queue.items.length, 2, 'both at 5999');
	timers.advance(1);
	assertEqual(queue.items.length, 1, 'neutral gone at 6000');
	timers.advance(ACTION_MS - NEUTRAL_MS - 1);
	assertEqual(queue.items.length, 1, 'action toast at 9999');
	timers.advance(1);
	assertEqual(queue.items.length, 0, 'action toast gone at 10000');
});
suite.check('hover pauses; resuming leaves at least 2 s and otherwise what was left', () => {
	const { timers, queue } = setup();
	const id = queue.show({ message: 'a' });
	timers.advance(1000);
	queue.hold(id, 'hover');
	timers.advance(60000);
	assertEqual(queue.items.length, 1, 'still there while held');
	queue.release(id, 'hover');
	timers.advance(4999);
	assertEqual(queue.items.length, 1, '5 s were left');
	timers.advance(1);
	assertEqual(queue.items.length, 0, 'gone after the remaining 5 s');

	const late = queue.show({ message: 'b' });
	timers.advance(5500);
	queue.hold(late, 'focus');
	queue.release(late, 'focus');
	timers.advance(1999);
	assertEqual(queue.items.length, 1, 'never less than 2 s after resuming');
	timers.advance(1);
	assertEqual(queue.items.length, 0, 'gone after 2 s');
});
suite.check('hover and focus are separate holds', () => {
	const { timers, queue } = setup();
	const id = queue.show({ message: 'a' });
	queue.hold(id, 'hover');
	queue.hold(id, 'focus');
	queue.release(id, 'hover');
	timers.advance(60000);
	assertEqual(queue.items.length, 1, 'focus still holds');
	queue.release(id, 'focus');
	timers.advance(NEUTRAL_MS);
	assertEqual(queue.items.length, 0, 'released');
});
suite.check('a hidden document stops timers', () => {
	const { timers, queue } = setup();
	queue.show({ message: 'a' });
	queue.setDocumentHidden(true);
	timers.advance(60000);
	assertEqual(queue.items.length, 1, 'kept while hidden');
	queue.setDocumentHidden(false);
	timers.advance(NEUTRAL_MS);
	assertEqual(queue.items.length, 0, 'runs again when visible');
});
suite.check('the same key replaces a toast instead of stacking', () => {
	const { queue } = setup();
	queue.show({ message: 'first', key: 'export' });
	queue.show({ message: 'second', key: 'export' });
	assertDeepEqual(
		queue.items.map((i) => i.message),
		['second'],
		'one toast'
	);
});
suite.check('a fourth toast removes the oldest one without a pending Undo, else the oldest', () => {
	const { queue } = setup();
	queue.show({ message: 'undo-1', action: undoAction });
	queue.show({ message: 'plain-2' });
	queue.show({ message: 'undo-3', action: undoAction });
	queue.show({ message: 'plain-4' });
	assertEqual(MAX_VISIBLE, 3, 'limit');
	assertDeepEqual(
		queue.items.map((i) => i.message),
		['undo-1', 'undo-3', 'plain-4'],
		'plain-2 dropped'
	);
	queue.show({ message: 'undo-5', action: undoAction });
	assertDeepEqual(
		queue.items.map((i) => i.message),
		['undo-1', 'undo-3', 'undo-5'],
		'plain dropped first'
	);
	queue.show({ message: 'undo-6', action: undoAction });
	assertDeepEqual(
		queue.items.map((i) => i.message),
		['undo-3', 'undo-5', 'undo-6'],
		'then the oldest'
	);
});
suite.check('announcements carry the message and the action', () => {
	const { announced, queue } = setup();
	queue.show({ message: 'Deleted it.', action: undoAction });
	queue.show({ message: 'Done.' });
	assertDeepEqual(announced, ['Deleted it., Undo available', 'Done.'], 'live region texts');
});
suite.check('toasts wait while a modal is open and appear when it closes', () => {
	const { queue } = setup();
	queue.holdWhileModal(true);
	queue.show({ message: 'a' });
	queue.show({ message: 'b' });
	assertEqual(queue.items.length, 0, 'nothing visible');
	assertEqual(queue.queued, 2, 'queued');
	queue.holdWhileModal(false);
	assertDeepEqual(
		queue.items.map((i) => i.message),
		['a', 'b'],
		'shown in order'
	);
});

await suite.checkAsync('Undo: busy while running, removed on success', async () => {
	const timers = new FakeTimers();
	const toasts = new Toasts({ timers });
	toasts.setLabels({ undo: 'Undo', open: 'Open', retry: 'Retry' });
	let release: () => void = noop;
	let calls = 0;
	const id = toasts.undo(
		'Deleted.',
		() =>
			new Promise<void>((resolve) => {
				calls += 1;
				release = resolve;
			}),
		() => 'failed'
	);
	const running = toasts.run(id);
	assertEqual(toasts.items[0].busy, true, 'busy');
	timers.advance(60000);
	assertEqual(toasts.items.length, 1, 'timer stopped while busy');
	void toasts.run(id);
	assertEqual(calls, 1, 'a second click does not start a second undo');
	release();
	await running;
	assertEqual(toasts.items.length, 0, 'removed');
});
await suite.checkAsync('Undo failure: warning, persistent, Retry; retry can succeed', async () => {
	const timers = new FakeTimers();
	const toasts = new Toasts({ timers });
	toasts.setLabels({ undo: 'Undo', open: 'Open', retry: 'Retry' });
	let fail = true;
	const id = toasts.undo(
		'Deleted.',
		async () => {
			if (fail) throw new Error('no');
		},
		() => 'Could not restore.'
	);
	await toasts.run(id);
	const [item] = toasts.items;
	assertEqual(item.tone, 'warning', 'tone');
	assertEqual(item.persistent, true, 'persistent');
	assertEqual(item.message, 'Could not restore.', 'reason in words');
	assertEqual(item.action?.label, 'Retry', 'retry action');
	assertEqual(item.busy, false, 'not busy');
	timers.advance(60000);
	assertEqual(toasts.items.length, 1, 'no timeout');
	fail = false;
	await toasts.run(id);
	assertEqual(toasts.items.length, 0, 'retry removed it');
});
suite.check('newestUndo skips toasts without Undo and runs the newest', () => {
	const toasts = new Toasts({ timers: new FakeTimers() });
	toasts.setLabels({ undo: 'Undo', open: 'Open', retry: 'Retry' });
	const order: string[] = [];
	toasts.undo(
		'one',
		async () => void order.push('one'),
		() => 'x'
	);
	toasts.undo(
		'two',
		async () => void order.push('two'),
		() => 'x'
	);
	toasts.show({ message: 'plain' });
	assertEqual(toasts.newestUndo()?.message, 'two', 'newest undo');
	assertEqual(toasts.runNewestUndo(), true, 'ran');
	assertDeepEqual(order, ['two'], 'handler');
});
suite.check('subscribers see the live-region text', () => {
	const toasts = new Toasts({ timers: new FakeTimers() });
	let seen = '';
	toasts.subscribe((s) => (seen = s.announcement.text));
	toasts.show({ message: 'Hello' });
	assertEqual(seen, 'Hello', 'announcement');
});

const key = (init: Partial<Parameters<typeof isUndoKey>[0]>) => ({
	key: '',
	ctrlKey: false,
	metaKey: false,
	altKey: false,
	shiftKey: false,
	...init
});
suite.check('Ctrl/Cmd+Z is the Undo key; with Shift or Alt it is not', () => {
	assert(isUndoKey(key({ key: 'z', ctrlKey: true })), 'ctrl z');
	assert(isUndoKey(key({ key: 'Z', metaKey: true })), 'cmd z');
	assert(!isUndoKey(key({ key: 'z', ctrlKey: true, shiftKey: true })), 'redo');
	assert(!isUndoKey(key({ key: 'z' })), 'plain z');
});
suite.check('Alt+Shift+N is the region key, read from the physical key', () => {
	assert(isRegionKey(key({ key: 'N', code: 'KeyN', altKey: true, shiftKey: true })), 'plain');
	assert(
		isRegionKey(key({ key: 'Dead', code: 'KeyN', altKey: true, shiftKey: true })),
		'mac dead key'
	);
	assert(!isRegionKey(key({ key: 'n', code: 'KeyN', altKey: true })), 'no shift');
});
suite.check('text fields and editable content are left alone', () => {
	assert(isEditableElement({ tagName: 'INPUT', type: 'text' }), 'text input');
	assert(isEditableElement({ tagName: 'TEXTAREA' }), 'textarea');
	assert(isEditableElement({ tagName: 'DIV', isContentEditable: true }), 'contenteditable');
	assert(!isEditableElement({ tagName: 'INPUT', type: 'checkbox' }), 'checkbox');
	assert(!isEditableElement({ tagName: 'BODY' }), 'body');
	assert(!isEditableElement(null), 'nothing');
});

finish(suite.result());
