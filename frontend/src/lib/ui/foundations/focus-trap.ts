import { EDITABLE, tabbables, trapTarget } from './focus';

export interface TrapOptions {
	/** Trap only while true; turning it on moves focus into the node. Default true. */
	active?: boolean;
	/** Selector for the element that first receives focus; default is the first editable field. */
	initialFocus?: string;
}

/**
 * Svelte action: keeps Tab and Shift+Tab inside `node` and moves focus in when it becomes active.
 * Focus lands on `[data-autofocus]`, else on `initialFocus`, else on the first editable field,
 * else on the first tabbable element, else on the node itself (which then needs `tabindex="-1"`).
 */
export function focusTrap(node: HTMLElement, options: TrapOptions = {}) {
	let current = options;
	let active = false;

	function onKeydown(event: KeyboardEvent) {
		if (!active || event.key !== 'Tab' || event.defaultPrevented) return;
		const items = tabbables(node);
		if (items.length === 0) {
			event.preventDefault();
			node.focus();
			return;
		}
		const target = trapTarget(
			items,
			document.activeElement as HTMLElement | null,
			event.shiftKey,
			(item) => item !== null && node.contains(item)
		);
		if (target) {
			event.preventDefault();
			target.focus();
		}
	}

	function placeFocus() {
		const target =
			node.querySelector<HTMLElement>('[data-autofocus]') ??
			(current.initialFocus ? node.querySelector<HTMLElement>(current.initialFocus) : null) ??
			node.querySelector<HTMLElement>(EDITABLE) ??
			tabbables(node)[0] ??
			node;
		target.focus();
	}

	function apply(next: TrapOptions) {
		current = next;
		const wanted = next.active ?? true;
		if (wanted && !active) {
			active = true;
			// Deferred so a portal action on an ancestor has moved the node first; moving a focused
			// node would drop its focus.
			queueMicrotask(() => {
				if (active) placeFocus();
			});
		} else if (!wanted) {
			active = false;
		}
	}

	node.addEventListener('keydown', onKeydown);
	apply(options);
	return {
		update: apply,
		destroy() {
			active = false;
			node.removeEventListener('keydown', onKeydown);
		}
	};
}
