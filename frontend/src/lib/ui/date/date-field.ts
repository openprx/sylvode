import {
	displayDateTime,
	isDayDisabled,
	joinDateTime,
	parseDateInput,
	parseDateTimeInput,
	splitDateTime,
	type Bounds,
	type IsoDate
} from './calendar';

export type DateErrorKey =
	| 'pattern'
	| 'patternTime'
	| 'invalid'
	| 'min'
	| 'max'
	| 'unavailable'
	| 'order';

export type FieldParse =
	| { state: 'empty' }
	| { state: 'ok'; value: string }
	| { state: 'error'; key: DateErrorKey };

/** The text a field shows for a stored value: `2026-11-24` or `2026-11-24 14:30`. */
export function displayValue(kind: 'date' | 'datetime', value: string | null): string {
	if (!value) return '';
	if (kind === 'datetime') return displayDateTime(value);
	return splitDateTime(value)?.date ?? '';
}

/** Why a date may not be chosen, or `null`. */
export function boundsError(date: IsoDate, bounds: Bounds): DateErrorKey | null {
	if (bounds.min && date < bounds.min) return 'min';
	if (bounds.max && date > bounds.max) return 'max';
	return isDayDisabled(date, bounds) ? 'unavailable' : null;
}

/**
 * Parses what the user typed into the stored value. A date-time without a time keeps the time
 * already in the field, else `defaultTime`.
 */
export function parseField(
	kind: 'date' | 'datetime',
	text: string,
	bounds: Bounds,
	current: string | null,
	defaultTime: string
): FieldParse {
	if (text.trim() === '') return { state: 'empty' };
	if (kind === 'date') {
		const parsed = parseDateInput(text);
		if (!parsed.ok)
			return { state: 'error', key: parsed.reason === 'invalid' ? 'invalid' : 'pattern' };
		const error = boundsError(parsed.value, bounds);
		return error ? { state: 'error', key: error } : { state: 'ok', value: parsed.value };
	}
	const parsed = parseDateTimeInput(text);
	if (!parsed.ok)
		return { state: 'error', key: parsed.reason === 'invalid' ? 'invalid' : 'patternTime' };
	const error = boundsError(parsed.date, bounds);
	if (error) return { state: 'error', key: error };
	const time = parsed.time ?? splitDateTime(current)?.time ?? defaultTime;
	return { state: 'ok', value: joinDateTime(parsed.date, time) };
}

export interface RangeParse {
	start: FieldParse;
	end: FieldParse;
	/** Both ends are valid dates in the wrong order. */
	order: boolean;
	/** Both ends are valid: the range to emit. */
	range: { start: IsoDate; end: IsoDate } | null;
}

/** Parses both boxes of a range; the range is only complete when both ends are valid and ordered. */
export function parseRange(startText: string, endText: string, bounds: Bounds): RangeParse {
	const start = parseField('date', startText, bounds, null, '00:00');
	const end = parseField('date', endText, bounds, null, '00:00');
	if (start.state === 'ok' && end.state === 'ok') {
		const order = start.value > end.value;
		return {
			start,
			end,
			order,
			range: order ? null : { start: start.value as IsoDate, end: end.value as IsoDate }
		};
	}
	return { start, end, order: false, range: null };
}
