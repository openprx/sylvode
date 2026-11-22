import type { Component } from 'svelte';

/** An action a surface offers in its header; the type of the surface-host contract. */
export interface SurfaceAction {
	id: string;
	label: string;
	kind: 'primary' | 'secondary' | 'danger-quiet';
	run: () => void | Promise<void>;
	disabledReason?: string;
	/** Used when the primary action becomes an icon button on a narrow header. */
	icon?: Component<{ size?: number | string; 'aria-hidden'?: 'true' }>;
}

/** Content widths at which the header gives up parts. */
export const OVERFLOW_AT = 600;
export const NARROW_BELOW = 480;
export const MAX_INLINE_SECONDARY = 3;

export interface HeaderLayout {
	/** The context line is drawn (otherwise it stays in the DOM as the title's description). */
	contextVisible: boolean;
	/** How many secondary actions stay as buttons; the rest go into the overflow menu. */
	inlineSecondary: number;
	/** The primary action is a 32px icon button with its label as tooltip and name. */
	primaryAsIcon: boolean;
}

/**
 * As the header narrows: at 600 or less every secondary action moves into the overflow menu; below
 * 480 the context line is hidden visually and the primary action becomes an icon button. Up to
 * three secondary actions are buttons while there is room.
 */
export function headerLayout(width: number, secondaryCount: number): HeaderLayout {
	return {
		contextVisible: width >= NARROW_BELOW,
		inlineSecondary: width <= OVERFLOW_AT ? 0 : Math.min(MAX_INLINE_SECONDARY, secondaryCount),
		primaryAsIcon: width < NARROW_BELOW
	};
}
