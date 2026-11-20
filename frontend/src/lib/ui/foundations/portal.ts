/**
 * Svelte action: moves `node` out of its parent so it cannot be clipped or trapped under a
 * stacking context. The target is the nearest themed container (`.dark` or `[data-theme]`) so
 * the layer keeps the theme of the region that opened it, else `document.body`.
 */
export function portal(node: HTMLElement, from?: HTMLElement | null) {
	const origin = from ?? node.parentElement;
	const target = origin?.closest<HTMLElement>('.dark, [data-theme]') ?? document.body;
	target.appendChild(node);
	return {
		destroy() {
			node.remove();
		}
	};
}
