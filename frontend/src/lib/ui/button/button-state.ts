export type ButtonVariant = 'primary' | 'secondary' | 'text' | 'danger' | 'danger-quiet';

/** Filled buttons draw their focus ring 2px outside the edge (design decision D2). */
export function isFilled(variant: ButtonVariant): boolean {
	return variant === 'primary' || variant === 'danger';
}

/** A busy button stays focusable but ignores activation. */
export function isInert(disabled: boolean, loading: boolean): boolean {
	return disabled || loading;
}
