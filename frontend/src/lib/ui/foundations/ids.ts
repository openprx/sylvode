let counter = 0;

/** A document-unique id for wiring labels, descriptions and controls (`ui-<prefix>-<n>`). */
export function uid(prefix: string): string {
	counter += 1;
	return `ui-${prefix}-${counter}`;
}

/** Test hook: restart the sequence so ids are predictable. */
export function resetIds(): void {
	counter = 0;
}
