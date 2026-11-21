import type { SurfaceWidthClass } from './context';

/** Thresholds of the surface container: xs < 480 <= sm < 600 <= md < 900 <= full. */
export const SURFACE_SM = 480;
export const SURFACE_MD = 600;
export const SURFACE_FULL = 900;

export function widthClass(px: number): SurfaceWidthClass {
	if (px < SURFACE_SM) return 'xs';
	if (px < SURFACE_MD) return 'sm';
	if (px < SURFACE_FULL) return 'md';
	return 'full';
}

/**
 * Svelte action: reports the element's content width to `onwidth` now and on every resize. The
 * element should carry `container-type: inline-size; container-name: surface`.
 */
export function measureWidth(node: HTMLElement, onwidth: (px: number) => void) {
	let callback = onwidth;
	const report = (px: number) => callback(Math.round(px));
	report(node.clientWidth);
	if (typeof ResizeObserver === 'undefined')
		return { update: (next: typeof onwidth) => (callback = next) };
	const observer = new ResizeObserver((entries) => {
		const entry = entries[entries.length - 1];
		if (entry) report(entry.contentRect.width);
	});
	observer.observe(node);
	return {
		update(next: (px: number) => void) {
			callback = next;
		},
		destroy() {
			observer.disconnect();
		}
	};
}
