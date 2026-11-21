/**
 * Run an internal handler first, then the caller's, so a primitive never discards a handler the
 * caller passed through `{...rest}`. The caller's handler is read through a getter at call time,
 * so a prop that changes later is still the one that runs; it is skipped when absent.
 */
export function composeHandlers<E extends Event>(
	internal: (event: E) => void,
	external: () => ((event: E) => void) | null | undefined
): (event: E) => void {
	return (event) => {
		internal(event);
		external()?.(event);
	};
}
