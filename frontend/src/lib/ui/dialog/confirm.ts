/**
 * The awaited confirmation helper that replaces `window.confirm()`.
 *
 *   if (!(await confirmAction({ tone: 'destructive', title, body, verb: 'delete', object }))) return;
 *
 * One shared dialog (`ConfirmHost.svelte`, mounted once by the app shell) shows the requests one
 * after the other. This module is plain TypeScript with no DOM or framework imports, so the
 * queueing and resolution rules are unit-tested directly.
 *
 * Rules:
 * - Resolves `true` only when the user presses the confirm button; Escape, the close button,
 *   Cancel, navigation to another path and unmounting the host all resolve `false`.
 * - Calls made while a request is open are queued and shown in call order.
 * - With no host mounted the call resolves `false` at once (the action is never run) and a
 *   warning is logged once.
 * - With several hosts mounted only the first one renders; the others stay inert.
 */

export type ConfirmTone = 'destructive' | 'neutral';

/** Verbs with a message template `ui.confirm.verb.<verb>` taking `{object}`. */
export const CONFIRM_VERBS = ['delete', 'remove', 'discard', 'revoke', 'leave', 'restore'] as const;
export type ConfirmVerb = (typeof CONFIRM_VERBS)[number];

interface ConfirmBase {
	/** `destructive` (default) uses the danger button; `neutral` a primary one. */
	tone?: ConfirmTone;
	/** A question that names the object ("Delete project Alpha?"). */
	title: string;
	/** What happens, what is kept, and whether it can be undone. */
	body?: string;
	/** Defaults to the shared "Cancel". */
	cancelLabel?: string;
}

/** The caller composed the confirm label itself (with `$t` and parameters). */
export interface ConfirmWithLabel extends ConfirmBase {
	/** Verb and object ("Delete project Alpha"). Never "OK". */
	confirmLabel: string;
	verb?: never;
	object?: never;
}

/** The label is composed from a verb template, so word order is right in every language. */
export interface ConfirmWithVerb extends ConfirmBase {
	verb: ConfirmVerb;
	/** What the verb applies to ("member Ana"). */
	object: string;
	confirmLabel?: never;
}

export type ConfirmOptions = ConfirmWithLabel | ConfirmWithVerb;

export type Translate = (key: string, values?: Record<string, string>) => string;

/** The text of the confirm button. */
export function confirmLabelOf(options: ConfirmOptions, translate: Translate): string {
	if (options.verb !== undefined) {
		return translate(`ui.confirm.verb.${options.verb}`, { object: options.object });
	}
	return options.confirmLabel;
}

export interface ConfirmRequest {
	readonly id: number;
	readonly options: ConfirmOptions;
	/** `location.pathname` when the request was made ('' without a browser). */
	readonly scope: string;
}

interface Entry extends ConfirmRequest {
	resolve: (value: boolean) => void;
	settled: boolean;
}

export interface ConfirmHostHandle {
	/** True for the first mounted host: only that one renders requests. */
	isActive(): boolean;
	detach(): void;
}

export class ConfirmQueue {
	private queue: Entry[] = [];
	private hosts: object[] = [];
	private listeners = new Set<() => void>();
	private nextId = 1;
	private warned = false;

	constructor(
		private readonly currentScope: () => string = () =>
			typeof location === 'undefined' ? '' : location.pathname,
		private readonly warn: (message: string) => void = (message) => console.warn(message)
	) {}

	/** Opens a request; resolves with the user's answer. */
	request(options: ConfirmOptions): Promise<boolean> {
		if (this.hosts.length === 0) {
			if (!this.warned) {
				this.warned = true;
				this.warn(
					'confirmAction() was called while no ConfirmHost is mounted; the request was declined.'
				);
			}
			return Promise.resolve(false);
		}
		return new Promise<boolean>((resolve) => {
			this.queue.push({
				id: this.nextId++,
				options,
				scope: this.currentScope(),
				resolve,
				settled: false
			});
			this.notify();
		});
	}

	/** The request being shown, or null. */
	current(): ConfirmRequest | null {
		return this.queue[0] ?? null;
	}

	pending(): number {
		return this.queue.length;
	}

	/** Answers a request; a second answer for the same request is ignored. */
	settle(request: ConfirmRequest, result: boolean): void {
		const entry = this.queue.find((candidate) => candidate.id === request.id);
		if (!entry || entry.settled) return;
		entry.settled = true;
		this.queue = this.queue.filter((candidate) => candidate !== entry);
		entry.resolve(result);
		this.notify();
	}

	/** Declines every request (current and queued). */
	cancelAll(): void {
		this.cancelWhere(() => true);
	}

	/** Declines the requests made on another path: their page is gone. */
	cancelOutside(scope: string): void {
		this.cancelWhere((entry) => entry.scope !== scope);
	}

	private cancelWhere(match: (entry: Entry) => boolean): void {
		const gone = this.queue.filter(match);
		if (gone.length === 0) return;
		this.queue = this.queue.filter((entry) => !match(entry));
		for (const entry of gone) {
			entry.settled = true;
			entry.resolve(false);
		}
		this.notify();
	}

	/** Registers a host. When the last host detaches, pending requests are declined. */
	attachHost(): ConfirmHostHandle {
		const token = {};
		this.hosts.push(token);
		this.notify();
		return {
			isActive: () => this.hosts[0] === token,
			detach: () => {
				const index = this.hosts.indexOf(token);
				if (index < 0) return;
				this.hosts.splice(index, 1);
				if (this.hosts.length === 0) this.cancelAll();
				else this.notify();
			}
		};
	}

	hostCount(): number {
		return this.hosts.length;
	}

	subscribe(listener: () => void): () => void {
		this.listeners.add(listener);
		return () => this.listeners.delete(listener);
	}

	private notify(): void {
		for (const listener of [...this.listeners]) listener();
	}
}

/** The app's one queue. */
export const confirmQueue = new ConfirmQueue();

/** Shows a confirmation and resolves `true` when the user confirms. */
export function confirmAction(options: ConfirmOptions): Promise<boolean> {
	return confirmQueue.request(options);
}
