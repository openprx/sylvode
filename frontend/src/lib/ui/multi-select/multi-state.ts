/** Pure reducers for the multi-select: toggling, chip removal with Backspace, chips and names. */

export function toggleValue<V>(values: readonly V[], value: V): V[] {
	return values.includes(value) ? values.filter((v) => v !== value) : [...values, value];
}

export interface BackspaceResult<V> {
	values: V[];
	/** The chip marked for removal, waiting for the second press. */
	marked: V | null;
	/** The value removed by this press, if any (announced to assistive technology). */
	removed: V | null;
}

/** First Backspace marks the last chip, the second removes it. */
export function backspace<V>(values: readonly V[], marked: V | null): BackspaceResult<V> {
	if (values.length === 0) return { values: [], marked: null, removed: null };
	const last = values[values.length - 1];
	if (marked !== null && marked === last)
		return { values: values.slice(0, -1), marked: null, removed: last };
	return { values: [...values], marked: last, removed: null };
}

export interface ChipView {
	chips: Array<{ value: string; label: string }>;
	hidden: Array<{ value: string; label: string }>;
}

/** At most `max` chips, the rest hidden behind "+n". */
export function chipView(
	items: ReadonlyArray<{ value: string; label: string }>,
	max: number
): ChipView {
	const limit = Math.max(0, max);
	return { chips: items.slice(0, limit), hidden: items.slice(limit) };
}

export type Translate = (key: string, values?: Record<string, string | number>) => string;

/** Accessible name of the trigger: "Labels: Bug, Urgent and 3 more". */
export function summaryName(
	label: string,
	labels: readonly string[],
	max: number,
	t: Translate
): string {
	if (labels.length === 0) return label;
	const shown = labels.slice(0, Math.max(1, max)).join(', ');
	const more = labels.length - Math.max(1, max);
	return more > 0
		? t('ui.multiSelect.summaryMore', { label, labels: shown, n: more })
		: t('ui.multiSelect.summary', { label, labels: shown });
}
