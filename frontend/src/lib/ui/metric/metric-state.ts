/**
 * The figure as text: a missing value is an em dash, text is shown as given, and a number is
 * grouped with the app language (never the browser language).
 */
export function formatMetric(
	value: number | string | null,
	locale: string | undefined,
	format?: Intl.NumberFormatOptions
): string {
	if (value === null) return '—';
	if (typeof value === 'string') return value;
	return new Intl.NumberFormat(locale, format).format(value);
}
