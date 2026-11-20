/**
 * Where a portalled layer is mounted. An open modal `<dialog>` lives in the browser's top layer,
 * which paints above every z-index in the document: a layer appended to the themed container
 * would open behind it and the dialog's own content would take its clicks. So the layer goes
 * inside the nearest open `<dialog>`; that keeps it in the top layer, in the dialog's stacking
 * context, with the theme of the region that opened it (the dialog sits inside that region).
 * Otherwise the target is the nearest themed container (`.dark` or `[data-theme]`), else
 * `document.body`.
 */
export function portalTarget(
	origin: Pick<Element, 'closest'> | null,
	fallback: () => HTMLElement
): HTMLElement {
	return (
		origin?.closest<HTMLElement>('dialog[open]') ??
		origin?.closest<HTMLElement>('.dark, [data-theme]') ??
		fallback()
	);
}

/**
 * Svelte action: moves `node` out of its parent so it cannot be clipped or trapped under a
 * stacking context; see `portalTarget` for where it lands.
 */
export function portal(node: HTMLElement, from?: HTMLElement | null) {
	const origin = from ?? node.parentElement;
	portalTarget(origin, () => document.body).appendChild(node);
	return {
		destroy() {
			node.remove();
		}
	};
}
