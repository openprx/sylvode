import { isModalOpen } from '../ui/foundations/modal';
import type { Destination } from './route-map';

/** Second key of a "G then ..." sequence must follow within this time. */
export const SEQUENCE_TIMEOUT_MS = 1000;

/** The part of a keyboard event the shell reads; real events satisfy it. */
export interface KeyEventLike {
	key: string;
	metaKey: boolean;
	ctrlKey: boolean;
	altKey: boolean;
	shiftKey: boolean;
	repeat?: boolean;
	defaultPrevented: boolean;
	target: EventTarget | null;
	preventDefault(): void;
}

export interface ShellKeyActions {
	togglePalette(): void;
	openSettings(): void;
	goTo(destination: Exclude<Destination, 'governance'>): void;
	/** Adds the focused column row to the rail; returns true when there was one. */
	addFocusedRow(target: EventTarget | null): boolean;
}

export interface ShellKeyDeps {
	actions: ShellKeyActions;
	/** False when the surface is hosted: no global keys at all. */
	globalShortcuts?: () => boolean;
	/** Default: the `lib/ui` modal counter. */
	modalOpen?: () => boolean;
	now?: () => number;
}

interface ElementLike {
	tagName?: string;
	isContentEditable?: boolean;
	getAttribute?(name: string): string | null;
	closest?(selector: string): unknown;
}

const EDITABLE_TAGS = new Set(['INPUT', 'TEXTAREA', 'SELECT']);

/** Focus is in something the user types into: sequence keys must not fire. */
export function isEditableTarget(target: EventTarget | null): boolean {
	const el = target as ElementLike | null;
	if (!el || typeof el !== 'object') return false;
	if (el.tagName && EDITABLE_TAGS.has(el.tagName.toUpperCase())) return true;
	if (el.isContentEditable) return true;
	const editable = el.getAttribute?.('contenteditable');
	if (editable !== null && editable !== undefined && editable !== 'false') return true;
	if (el.getAttribute?.('role') === 'textbox') return true;
	return Boolean(el.closest?.('.ProseMirror, [contenteditable]:not([contenteditable="false"])'));
}

const SEQUENCE_TARGETS: Record<string, Exclude<Destination, 'governance'> | 'settings'> = {
	h: 'home',
	p: 'projects',
	f: 'flow',
	s: 'settings'
};

/**
 * Builds the keydown handler for the global shortcuts: Cmd/Ctrl+K (palette), Cmd/Ctrl+, and
 * G then S (settings), G then H, P, F (destinations), P on a focused column row (add to
 * rail). Escape is never handled here: it belongs to the topmost layer. Returns true when the
 * event was consumed.
 */
export function createShellKeyHandler(deps: ShellKeyDeps): (event: KeyEventLike) => boolean {
	const now = deps.now ?? (() => Date.now());
	const globalOn = deps.globalShortcuts ?? (() => true);
	const modalOpen = deps.modalOpen ?? isModalOpen;
	let pendingG: number | null = null;

	return (event) => {
		if (!globalOn()) {
			pendingG = null;
			return false;
		}
		const key = event.key.toLowerCase();
		const command = event.metaKey || event.ctrlKey;

		if (command) {
			pendingG = null;
			if (event.altKey || event.defaultPrevented) return false;
			if (key === 'k' && !event.shiftKey) {
				event.preventDefault();
				deps.actions.togglePalette();
				return true;
			}
			if (key === ',' && !event.shiftKey) {
				event.preventDefault();
				deps.actions.openSettings();
				return true;
			}
			return false;
		}

		// Sequence keys: bare keys only (Shift allowed), never while typing or behind a modal.
		if (event.altKey || event.repeat || event.defaultPrevented) {
			pendingG = null;
			return false;
		}
		if (isEditableTarget(event.target) || modalOpen()) {
			pendingG = null;
			return false;
		}

		if (pendingG !== null) {
			const fresh = now() - pendingG <= SEQUENCE_TIMEOUT_MS;
			pendingG = null;
			const target = fresh ? SEQUENCE_TARGETS[key] : undefined;
			if (target) {
				event.preventDefault();
				if (target === 'settings') deps.actions.openSettings();
				else deps.actions.goTo(target);
				return true;
			}
			// Not a sequence key: fall through, so "G" then "G" starts a new sequence.
		}

		if (key === 'g') {
			pendingG = now();
			return false;
		}
		if (key === 'p' && deps.actions.addFocusedRow(event.target)) {
			event.preventDefault();
			return true;
		}
		return false;
	};
}

/** Registers the handler on `window`; returns the function that removes it. */
export function installShellKeys(deps: ShellKeyDeps): () => void {
	if (typeof window === 'undefined') return () => {};
	const handler = createShellKeyHandler(deps);
	const listener = (event: KeyboardEvent) => void handler(event);
	window.addEventListener('keydown', listener);
	return () => window.removeEventListener('keydown', listener);
}
