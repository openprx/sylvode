/** Elements a keyboard user can reach with Tab. */
export const TABBABLE =
	'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), summary, [contenteditable=""], [contenteditable="true"], [tabindex]:not([tabindex="-1"])';

/** Controls the user types into; a dialog puts focus on the first of these. */
export const EDITABLE =
	'input:not([disabled]):not([type="hidden"]):not([type="checkbox"]):not([type="radio"]):not([type="button"]):not([type="submit"]), textarea:not([disabled]), select:not([disabled]), [contenteditable=""], [contenteditable="true"]';

function isHidden(el: HTMLElement): boolean {
	if (el.hidden || el.closest('[inert], [hidden]')) return true;
	const style = getComputedStyle(el);
	return style.display === 'none' || style.visibility === 'hidden';
}

/** Tabbable descendants of `root`, in DOM order, without hidden or inert ones. */
export function tabbables(root: HTMLElement): HTMLElement[] {
	return Array.from(root.querySelectorAll<HTMLElement>(TABBABLE)).filter(
		(el) => el.tabIndex >= 0 && !isHidden(el)
	);
}

/**
 * Where Tab or Shift+Tab goes inside a trapped scope, or `null` when the browser's own move is
 * right. Pure so it can be tested without a document.
 */
export function trapTarget<T>(
	items: readonly T[],
	active: T | null,
	shift: boolean,
	contains: (item: T | null) => boolean
): T | null {
	if (items.length === 0) return null;
	const first = items[0];
	const last = items[items.length - 1];
	if (!contains(active)) return shift ? last : first;
	if (shift && active === first) return last;
	if (!shift && active === last) return first;
	return null;
}

/**
 * Remembers the element that has focus now and returns a function that gives focus back to it
 * if it is still in the document. Call it before a layer opens, call the result when it closes.
 */
export function captureFocus(): () => void {
	const opener = typeof document === 'undefined' ? null : document.activeElement;
	return () => {
		if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
	};
}
