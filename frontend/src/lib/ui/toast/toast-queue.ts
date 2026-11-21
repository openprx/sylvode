export type ToastTone = 'neutral' | 'success' | 'warning';
export type ToastActionKind = 'undo' | 'open' | 'retry';

export interface ToastAction {
	label: string;
	run: () => void | Promise<void>;
	kind?: ToastActionKind;
}

export interface ToastInput {
	message: string;
	tone?: ToastTone;
	action?: ToastAction;
	/** A toast with the same key replaces the one already shown. */
	key?: string;
	/** Stays until dismissed. */
	persistent?: boolean;
}

export interface ToastItem {
	readonly id: string;
	readonly key?: string;
	readonly message: string;
	readonly tone: ToastTone;
	readonly action?: ToastAction;
	readonly persistent: boolean;
	readonly busy: boolean;
}

export interface TimerApi {
	setTimeout(fn: () => void, ms: number): unknown;
	clearTimeout(handle: unknown): void;
	now(): number;
}

export const NEUTRAL_MS = 6000;
export const ACTION_MS = 10000;
/** After a pause the toast never shows for less than this. */
export const MIN_RESUME_MS = 2000;
export const MAX_VISIBLE = 3;

const realTimers: TimerApi = {
	setTimeout: (fn, ms) => setTimeout(fn, ms),
	clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
	now: () => Date.now()
};

interface Entry {
	item: ToastItem;
	remaining: number;
	startedAt: number | null;
	handle: unknown;
	holds: Set<string>;
}

export interface QueueOptions {
	timers?: TimerApi;
	/** Called after every change of the visible list. */
	onchange?: () => void;
	/** Called with the text that goes into the live region. */
	onannounce?: (text: string) => void;
	/** Builds the live-region text; default "message, action available". */
	announce?: (message: string, actionLabel: string | null) => string;
}

/**
 * The toast list and its timers, free of the DOM so it can be tested with fake timers. Timing:
 * 6 s neutral, 10 s with an action, no timer for persistent or busy toasts; hover, focus and a
 * hidden document pause the timer, and resuming leaves at least 2 s. Dedupe by `key`. At most
 * three are visible; a fourth removes the oldest one without a pending Undo, else the oldest.
 * While a modal dialog is open new toasts wait.
 */
export class ToastQueue {
	private entries: Entry[] = [];
	private waiting: Array<{ id: string; input: ToastInput }> = [];
	private nextId = 1;
	private modal = false;
	private hidden = false;
	private readonly timers: TimerApi;
	private readonly options: QueueOptions;

	constructor(options: QueueOptions = {}) {
		this.options = options;
		this.timers = options.timers ?? realTimers;
	}

	get items(): ToastItem[] {
		return this.entries.map((e) => e.item);
	}

	get queued(): number {
		return this.waiting.length;
	}

	show(input: ToastInput): string {
		const id = `toast-${this.nextId++}`;
		if (this.modal) {
			this.waiting = this.waiting.filter((w) => !input.key || w.input.key !== input.key);
			this.waiting.push({ id, input });
			return id;
		}
		this.insert(id, input);
		return id;
	}

	dismiss(id: string): void {
		this.waiting = this.waiting.filter((w) => w.id !== id);
		const entry = this.find(id);
		if (!entry) return;
		this.stop(entry);
		this.entries = this.entries.filter((e) => e !== entry);
		this.changed();
	}

	/** Adds a reason to keep the toast on screen ('hover', 'focus', ...). */
	hold(id: string, reason: string): void {
		const entry = this.find(id);
		if (!entry) return;
		entry.holds.add(reason);
		this.sync(entry);
	}

	release(id: string, reason: string): void {
		const entry = this.find(id);
		if (!entry || !entry.holds.delete(reason)) return;
		if (entry.holds.size === 0) entry.remaining = Math.max(entry.remaining, MIN_RESUME_MS);
		this.sync(entry);
	}

	/** Timers stop while the document is hidden. */
	setDocumentHidden(hidden: boolean): void {
		if (this.hidden === hidden) return;
		this.hidden = hidden;
		for (const entry of this.entries) {
			if (!hidden) entry.remaining = Math.max(entry.remaining, MIN_RESUME_MS);
			this.sync(entry);
		}
	}

	holdWhileModal(on: boolean): void {
		this.modal = on;
		if (on) return;
		const pending = this.waiting;
		this.waiting = [];
		for (const { id, input } of pending) this.insert(id, input);
	}

	/** Runs the toast's action. An Undo that fails turns into a persistent warning with Retry. */
	async runAction(id: string, failure?: (error: unknown) => ToastInput): Promise<void> {
		const entry = this.find(id);
		const action = entry?.item.action;
		if (!entry || !action || entry.item.busy) return;
		this.update(entry, { busy: true });
		try {
			await action.run();
			this.dismiss(id);
		} catch (error) {
			if (!this.find(id)) return;
			if (!failure) {
				this.update(entry, { busy: false });
				return;
			}
			const next = failure(error);
			this.update(entry, {
				busy: false,
				message: next.message,
				tone: next.tone ?? 'warning',
				action: next.action,
				persistent: next.persistent ?? true
			});
			this.announceItem(entry.item);
		}
	}

	/** The newest toast that has an Undo action and is not already running it. */
	newestUndo(): ToastItem | undefined {
		for (let i = this.entries.length - 1; i >= 0; i -= 1) {
			const item = this.entries[i].item;
			if (item.action?.kind === 'undo' && !item.busy) return item;
		}
		return undefined;
	}

	private insert(id: string, input: ToastInput): void {
		if (input.key) {
			const same = this.entries.find((e) => e.item.key === input.key);
			if (same) {
				this.stop(same);
				this.entries = this.entries.filter((e) => e !== same);
			}
		}
		const item: ToastItem = {
			id,
			key: input.key,
			message: input.message,
			tone: input.tone ?? 'neutral',
			action: input.action,
			persistent: input.persistent ?? false,
			busy: false
		};
		const entry: Entry = {
			item,
			remaining: item.action ? ACTION_MS : NEUTRAL_MS,
			startedAt: null,
			handle: null,
			holds: new Set()
		};
		this.entries.push(entry);
		while (this.entries.length > MAX_VISIBLE) {
			const older = this.entries.filter((e) => e !== entry);
			const drop = older.find((e) => e.item.action?.kind !== 'undo') ?? older[0];
			this.stop(drop);
			this.entries = this.entries.filter((e) => e !== drop);
		}
		this.sync(entry);
		this.announceItem(item);
		this.changed();
	}

	private announceItem(item: ToastItem): void {
		const label = item.action?.label ?? null;
		const text = this.options.announce
			? this.options.announce(item.message, label)
			: label
				? `${item.message}, ${label} available`
				: item.message;
		this.options.onannounce?.(text);
	}

	private find(id: string): Entry | undefined {
		return this.entries.find((e) => e.item.id === id);
	}

	private update(entry: Entry, patch: Partial<Omit<ToastItem, 'id'>>): void {
		entry.item = { ...entry.item, ...patch };
		this.sync(entry);
		this.changed();
	}

	private shouldRun(entry: Entry): boolean {
		return !entry.item.persistent && !entry.item.busy && entry.holds.size === 0 && !this.hidden;
	}

	private sync(entry: Entry): void {
		const run = this.shouldRun(entry);
		if (run && entry.startedAt === null) {
			entry.startedAt = this.timers.now();
			entry.handle = this.timers.setTimeout(() => this.dismiss(entry.item.id), entry.remaining);
		} else if (!run && entry.startedAt !== null) {
			this.stop(entry);
		}
	}

	private stop(entry: Entry): void {
		if (entry.startedAt !== null) {
			entry.remaining = Math.max(0, entry.remaining - (this.timers.now() - entry.startedAt));
			entry.startedAt = null;
		}
		if (entry.handle !== null) this.timers.clearTimeout(entry.handle);
		entry.handle = null;
	}

	private changed(): void {
		this.options.onchange?.();
	}
}

/** Shape of the parts of a keyboard event the toast shortcuts read. */
export interface KeyLike {
	key: string;
	code?: string;
	ctrlKey: boolean;
	metaKey: boolean;
	altKey: boolean;
	shiftKey: boolean;
	defaultPrevented?: boolean;
}

/** Ctrl or Cmd with Z and nothing else: runs the newest Undo. */
export function isUndoKey(event: KeyLike): boolean {
	return (
		(event.ctrlKey || event.metaKey) &&
		!event.altKey &&
		!event.shiftKey &&
		event.key.toLowerCase() === 'z'
	);
}

/** Alt+Shift+N: moves focus into the notification region. */
export function isRegionKey(event: KeyLike): boolean {
	return (
		event.altKey &&
		event.shiftKey &&
		!event.ctrlKey &&
		!event.metaKey &&
		(event.code === 'KeyN' || event.key.toLowerCase() === 'n')
	);
}

/** True for elements the user types into; the toast shortcuts leave those alone. */
export function isEditableElement(
	el: {
		tagName?: string;
		isContentEditable?: boolean;
		type?: string;
	} | null
): boolean {
	if (!el) return false;
	const tag = (el.tagName ?? '').toLowerCase();
	if (tag === 'textarea' || tag === 'select') return true;
	if (tag === 'input') {
		return !['button', 'checkbox', 'radio', 'submit', 'reset', 'range', 'color', 'file'].includes(
			el.type ?? 'text'
		);
	}
	return el.isContentEditable === true;
}
