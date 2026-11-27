import { displayValue } from './refs';

export type ValueFormat = 'number' | 'amount' | 'percent' | 'date' | 'datetime';

const FORMATS = new Set<string>(['number', 'amount', 'percent', 'date', 'datetime']);

export function isValueFormat(value: unknown): value is ValueFormat {
	return typeof value === 'string' && FORMATS.has(value);
}

function tag(locale: string): string {
	return locale === 'en' ? 'en-US' : 'zh-CN';
}

/** Formats a resolved value for display; a value that does not fit the format is shown as is. */
export function formatValue(value: unknown, format: unknown, locale: string): string {
	if (!isValueFormat(format) || value === null || value === undefined || value === '') {
		return displayValue(value);
	}
	if (format === 'date' || format === 'datetime') {
		const date = new Date(String(value));
		if (Number.isNaN(date.getTime())) return displayValue(value);
		return format === 'date'
			? date.toLocaleDateString(tag(locale))
			: date.toLocaleString(tag(locale));
	}
	const number = typeof value === 'number' ? value : Number(value);
	if (!Number.isFinite(number)) return displayValue(value);
	if (format === 'amount') {
		return number.toLocaleString(tag(locale), {
			minimumFractionDigits: 2,
			maximumFractionDigits: 2
		});
	}
	if (format === 'percent') return `${number.toLocaleString(tag(locale))}%`;
	return number.toLocaleString(tag(locale));
}
