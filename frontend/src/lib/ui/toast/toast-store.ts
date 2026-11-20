import { ToastQueue, type TimerApi, type ToastInput, type ToastItem } from './toast-queue';

export interface ToastLabels {
	undo: string;
	open: string;
	retry: string;
}

/**
 * The toast store: one instance per app, passed to `ToastRegion`. Plain class with `subscribe`
 * (the Svelte store contract) so the same logic runs under Bun tests and in the browser.
 */
export class Toasts {
	private readonly queue: ToastQueue;
	private listeners = new Set<() => void>();
	private live = { text: '', seq: 0 };
	labels: ToastLabels = { undo: '', open: '', retry: '' };

	constructor(
		options: { timers?: TimerApi; announce?: (m: string, a: string | null) => string } = {}
	) {
		this.queue = new ToastQueue({
			timers: options.timers,
			announce: (message, action) => this.announceText(message, action),
			onchange: () => this.emit(),
			onannounce: (text) => {
				this.live = { text, seq: this.live.seq + 1 };
				this.emit();
			}
		});
		this.formatter = options.announce ?? null;
	}

	private formatter: ((message: string, action: string | null) => string) | null;

	private announceText(message: string, action: string | null): string {
		if (this.formatter) return this.formatter(message, action);
		return action ? `${message}, ${action}` : message;
	}

	/** Sets how the live region phrases "message, Undo available" (localised by the region). */
	setFormatter(format: (message: string, action: string | null) => string): void {
		this.formatter = format;
	}

	setLabels(labels: ToastLabels): void {
		this.labels = labels;
	}

	get items(): ToastItem[] {
		return this.queue.items;
	}

	get announcement(): { text: string; seq: number } {
		return this.live;
	}

	subscribe(run: (store: Toasts) => void): () => void {
		const listener = () => run(this);
		this.listeners.add(listener);
		run(this);
		return () => this.listeners.delete(listener);
	}

	private emit(): void {
		for (const listener of this.listeners) listener();
	}

	show(input: ToastInput): string {
		return this.queue.show(input);
	}

	/** A result that is no longer on screen, with an optional "Open" action. */
	result(
		message: string,
		open?: { label?: string; href: string; go?: (href: string) => void }
	): string {
		return this.queue.show({
			message,
			tone: 'success',
			action: open
				? {
						label: open.label ?? this.labels.open,
						kind: 'open',
						run: () => (open.go ? open.go(open.href) : window.location.assign(open.href))
					}
				: undefined
		});
	}

	/** A toast with Undo. If the undo fails the toast turns into a warning with Retry. */
	undo(
		message: string,
		onUndo: () => Promise<void>,
		failMessage: (error: unknown) => string
	): string {
		const id = this.queue.show({
			message,
			action: { label: this.labels.undo, kind: 'undo', run: onUndo }
		});
		this.undoFailures.set(id, failMessage);
		return id;
	}

	private undoFailures = new Map<string, (error: unknown) => string>();

	dismiss(id: string): void {
		this.undoFailures.delete(id);
		this.queue.dismiss(id);
	}

	holdWhileModal(on: boolean): void {
		this.queue.holdWhileModal(on);
	}

	hold(id: string, reason: string): void {
		this.queue.hold(id, reason);
	}

	release(id: string, reason: string): void {
		this.queue.release(id, reason);
	}

	setDocumentHidden(hidden: boolean): void {
		this.queue.setDocumentHidden(hidden);
	}

	newestUndo(): ToastItem | undefined {
		return this.queue.newestUndo();
	}

	/** Runs the action of a toast (Undo, Open, Retry). */
	run(id: string): Promise<void> {
		const failMessage = this.undoFailures.get(id);
		const item = this.queue.items.find((i) => i.id === id);
		if (!item) return Promise.resolve();
		const retryRun = item.action?.run;
		return this.queue.runAction(
			id,
			failMessage && retryRun
				? (error) => ({
						message: failMessage(error),
						tone: 'warning',
						persistent: true,
						action: { label: this.labels.retry, kind: 'retry', run: retryRun }
					})
				: undefined
		);
	}

	runNewestUndo(): boolean {
		const item = this.queue.newestUndo();
		if (!item) return false;
		void this.run(item.id);
		return true;
	}
}
