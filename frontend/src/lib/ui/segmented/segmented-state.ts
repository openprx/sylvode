export interface SegmentOption<V extends string = string> {
	value: V;
	label: string;
	disabled?: boolean;
	/** Visible to assistive technology and as a tooltip title while the segment is disabled. */
	disabledReason?: string;
}

/** The option that is checked, else the first enabled one: the segment that gets the tab stop. */
export function tabStopIndex<V extends string>(
	options: readonly SegmentOption<V>[],
	value: V | null | undefined
): number {
	const checked = options.findIndex((o) => o.value === value && !o.disabled);
	if (checked >= 0) return checked;
	return options.findIndex((o) => !o.disabled);
}
