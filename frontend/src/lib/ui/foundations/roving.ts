export type RovingOrientation = 'horizontal' | 'vertical' | 'both';

export interface RovingOptions {
	/** Matches the items inside the container. */
	selector: string;
	orientation?: RovingOrientation;
	/** Wrap from the last item to the first and back; default true. */
	loop?: boolean;
	/** Right-to-left layout swaps the horizontal arrows. */
	rtl?: boolean;
	/** Fired with the element that took focus. */
	onmove?: (el: HTMLElement) => void;
}

/**
 * The index focus should move to for `key`, or `null` when the key is not a movement key.
 * `enabled[i]` is false for items that must be skipped.
 */
export function nextRovingIndex(
	key: string,
	current: number,
	enabled: readonly boolean[],
	{ orientation = 'vertical', loop = true, rtl = false }: Partial<RovingOptions> = {}
): number | null {
	const horizontal = orientation === 'horizontal' || orientation === 'both';
	const vertical = orientation === 'vertical' || orientation === 'both';
	let step = 0;
	let edge: 'first' | 'last' | null = null;
	if (key === 'Home') edge = 'first';
	else if (key === 'End') edge = 'last';
	else if (vertical && key === 'ArrowDown') step = 1;
	else if (vertical && key === 'ArrowUp') step = -1;
	else if (horizontal && key === 'ArrowRight') step = rtl ? -1 : 1;
	else if (horizontal && key === 'ArrowLeft') step = rtl ? 1 : -1;
	else return null;

	const count = enabled.length;
	if (edge) {
		const order = [...enabled.keys()];
		const found = (edge === 'first' ? order : order.reverse()).find((i) => enabled[i]);
		return found ?? null;
	}
	let index = current;
	for (let tries = 0; tries < count; tries += 1) {
		index += step;
		if (index < 0 || index >= count) {
			if (!loop) return null;
			index = (index + count) % count;
		}
		if (enabled[index]) return index;
	}
	return null;
}

/**
 * Svelte action: roving focus over the items matched by `selector`. Exactly one item has
 * `tabindex="0"` (the focused one, else the checked (`aria-checked`, `aria-selected`, or a checked input) one, else the first
 * enabled one); the others have `-1`. Items marked `aria-disabled="true"` or `disabled` are skipped.
 */
export function roving(node: HTMLElement, options: RovingOptions) {
	const items = () => Array.from(node.querySelectorAll<HTMLElement>(options.selector));
	const isEnabled = (el: HTMLElement) =>
		el.getAttribute('aria-disabled') !== 'true' && !(el as HTMLButtonElement).disabled;

	function sync(active?: HTMLElement) {
		const list = items();
		const current =
			active ??
			list.find((el) => el === document.activeElement) ??
			list.find(
				(el) => el.getAttribute('aria-checked') === 'true' || (el as HTMLInputElement).checked
			) ??
			list.find((el) => el.getAttribute('aria-selected') === 'true') ??
			list.find(isEnabled);
		for (const el of list) el.tabIndex = el === current ? 0 : -1;
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.altKey || event.ctrlKey || event.metaKey) return;
		const list = items();
		const current = list.findIndex((el) => el === document.activeElement);
		if (current < 0) return;
		const next = nextRovingIndex(event.key, current, list.map(isEnabled), options);
		if (next === null) return;
		event.preventDefault();
		const el = list[next];
		sync(el);
		el.focus();
		options.onmove?.(el);
	}

	function onFocusin(event: FocusEvent) {
		const el = event.target as HTMLElement;
		if (items().includes(el)) sync(el);
	}

	node.addEventListener('keydown', onKeydown);
	node.addEventListener('focusin', onFocusin);
	sync();
	// Item state set by the framework right after mount (a checked radio) is read on the next tick.
	queueMicrotask(() => sync());
	const observer = new MutationObserver(() => sync());
	observer.observe(node, { childList: true, subtree: true });
	return {
		update(next: RovingOptions) {
			options = next;
			sync();
		},
		destroy() {
			observer.disconnect();
			node.removeEventListener('keydown', onKeydown);
			node.removeEventListener('focusin', onFocusin);
		}
	};
}
