export type PanelMode = 'docked' | 'overlay' | 'sheet';
export type PanelStatus = 'loading' | 'ready' | 'failed' | 'forbidden' | 'missing';

export const PANEL_MIN = 360;
export const PANEL_MAX = 720;
export const DOCK_FROM = 1000;
export const SHEET_BELOW = 768;
export const WIDE_WINDOW = 1280;
export const STEP = 16;
export const STEP_LARGE = 64;

/** 600 when the window is at least 1280 wide, else 480. */
export function defaultWidth(windowWidth: number): number {
	return windowWidth >= WIDE_WINDOW ? 600 : 480;
}

/** Widest the panel may get: 720, or 60% of its container if that is less. */
export function maxWidth(containerWidth: number): number {
	return Math.max(PANEL_MIN, Math.min(PANEL_MAX, Math.floor(containerWidth * 0.6)));
}

export function clampWidth(width: number, containerWidth: number): number {
	return Math.max(PANEL_MIN, Math.min(maxWidth(containerWidth), Math.round(width)));
}

/** Sheet below 768 viewport, docked from 1000 container, overlay in between. */
export function panelMode(containerWidth: number, viewportWidth: number): PanelMode {
	if (viewportWidth < SHEET_BELOW) return 'sheet';
	return containerWidth >= DOCK_FROM ? 'docked' : 'overlay';
}

/**
 * New width for a key on the separator, or null when the key is not one of its keys. The panel
 * sits at the inline end, so the arrow pointing away from it (toward the list) widens it.
 * `rtl` mirrors the arrows.
 */
export function separatorWidth(
	key: string,
	shift: boolean,
	width: number,
	containerWidth: number,
	windowWidth: number,
	rtl = false
): number | null {
	const step = shift ? STEP_LARGE : STEP;
	const widen = rtl ? 'ArrowRight' : 'ArrowLeft';
	const narrow = rtl ? 'ArrowLeft' : 'ArrowRight';
	if (key === widen) return clampWidth(width + step, containerWidth);
	if (key === narrow) return clampWidth(width - step, containerWidth);
	if (key === 'Home') return PANEL_MIN;
	if (key === 'End') return maxWidth(containerWidth);
	if (key === 'Enter') return clampWidth(defaultWidth(windowWidth), containerWidth);
	return null;
}

/** Width for a pointer drag: the distance from the pointer to the container's inline end. */
export function dragWidth(
	pointerX: number,
	containerRight: number,
	containerWidth: number
): number {
	return clampWidth(containerRight - pointerX, containerWidth);
}
