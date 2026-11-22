/**
 * Which modal layers are open. A dialog calls `enterModal()` while it is open; anything that must
 * wait behind a modal (the toast region holds new toasts, the design's toast rule) listens with
 * `onModalChange`. A counter, so nested modals close one at a time.
 */
let depth = 0;
const listeners = new Set<(open: boolean) => void>();

function notify(): void {
	for (const listener of [...listeners]) listener(depth > 0);
}

/** Marks a modal as open; call the returned function once when it closes. */
export function enterModal(): () => void {
	depth += 1;
	notify();
	let left = false;
	return () => {
		if (left) return;
		left = true;
		depth -= 1;
		notify();
	};
}

export function isModalOpen(): boolean {
	return depth > 0;
}

/** Calls `listener` now and whenever a modal opens or the last one closes; returns the stop function. */
export function onModalChange(listener: (open: boolean) => void): () => void {
	listeners.add(listener);
	listener(depth > 0);
	return () => listeners.delete(listener);
}
