/**
 * Pure calendar logic for the date pickers: ISO parsing and formatting, the month grid, keyboard
 * movement, month and year navigation, range selection. No DOM, no browser locale: dates are
 * wall-clock strings (`YYYY-MM-DD`, `HH:mm`) and every calculation runs on UTC dates so no time
 * zone moves a calendar day.
 */

export type IsoDate = `${number}-${number}-${number}`;
export type WeekStart = 0 | 1 | 6;
export type MoveKey =
	| 'ArrowLeft'
	| 'ArrowRight'
	| 'ArrowUp'
	| 'ArrowDown'
	| 'Home'
	| 'End'
	| 'PageUp'
	| 'PageDown';

export interface DateRange {
	start: IsoDate;
	end: IsoDate;
}

export type DateParse = { ok: true; value: IsoDate } | { ok: false; reason: 'pattern' | 'invalid' };
export type TimeParse = { ok: true; value: string } | { ok: false };
export type DateTimeParse =
	| { ok: true; date: IsoDate; time: string | null }
	| { ok: false; reason: 'pattern' | 'invalid' };

const pad = (n: number, width = 2) => String(n).padStart(width, '0');

export function daysInMonth(year: number, month: number): number {
	return new Date(Date.UTC(year, month, 0)).getUTCDate();
}

/** Builds an ISO date from parts (month 1 to 12); the caller guarantees the day exists. */
export function toIso(year: number, month: number, day: number): IsoDate {
	return `${pad(year, 4)}-${pad(month)}-${pad(day)}` as IsoDate;
}

export function isValidParts(year: number, month: number, day: number): boolean {
	return (
		Number.isInteger(year) &&
		year >= 1 &&
		year <= 9999 &&
		month >= 1 &&
		month <= 12 &&
		day >= 1 &&
		day <= daysInMonth(year, month)
	);
}

export function partsOf(date: IsoDate): { year: number; month: number; day: number } {
	const [year, month, day] = date.split('-').map(Number);
	return { year, month, day };
}

export function isIsoDate(text: string): text is IsoDate {
	const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
	return !!match && isValidParts(+match[1], +match[2], +match[3]);
}

const DATE_PATTERNS = [
	/^(\d{4})[-/.](\d{1,2})[-/.](\d{1,2})$/,
	/^(\d{4})(\d{2})(\d{2})$/,
	/^(\d{4})年(\d{1,2})月(\d{1,2})日?$/
];

/**
 * Accepts `2026-11-24`, `2026-1-5`, `2026/11/24`, `2026.11.24`, `20261124` and `2026年11月24日`.
 * Day-first and month-first forms are ambiguous across locales and are rejected as `pattern`.
 */
export function parseDateInput(text: string): DateParse {
	const source = text.trim();
	for (const pattern of DATE_PATTERNS) {
		const match = pattern.exec(source);
		if (!match) continue;
		const [year, month, day] = [+match[1], +match[2], +match[3]];
		if (!isValidParts(year, month, day)) return { ok: false, reason: 'invalid' };
		return { ok: true, value: toIso(year, month, day) };
	}
	return { ok: false, reason: 'pattern' };
}

/** Accepts `H:mm`, `HH:mm` and `HHmm`; returns `HH:mm`. */
export function parseTimeInput(text: string): TimeParse {
	const source = text.trim();
	const match = /^(\d{1,2}):(\d{2})$/.exec(source) ?? /^(\d{2})(\d{2})$/.exec(source);
	if (!match) return { ok: false };
	const hours = +match[1];
	const minutes = +match[2];
	if (hours > 23 || minutes > 59) return { ok: false };
	return { ok: true, value: `${pad(hours)}:${pad(minutes)}` };
}

/** `date`, or `date time` separated by a space or `T`. A missing time comes back as `null`. */
export function parseDateTimeInput(text: string): DateTimeParse {
	const tokens = text
		.trim()
		.split(/[\sT]+/)
		.filter(Boolean);
	if (tokens.length === 0 || tokens.length > 2) return { ok: false, reason: 'pattern' };
	const date = parseDateInput(tokens[0]);
	if (!date.ok) return date;
	if (tokens.length === 1) return { ok: true, date: date.value, time: null };
	const time = parseTimeInput(tokens[1]);
	if (!time.ok) return { ok: false, reason: 'pattern' };
	return { ok: true, date: date.value, time: time.value };
}

/** `2026-11-24T14:30` to `{ date, time }`; seconds are dropped. Anything else is `null`. */
export function splitDateTime(value: string | null): { date: IsoDate; time: string } | null {
	if (!value) return null;
	const match = /^(\d{4}-\d{2}-\d{2})(?:[T ](\d{2}):(\d{2})(?::\d{2}(?:\.\d+)?)?)?$/.exec(value);
	if (!match || !isIsoDate(match[1])) return null;
	return { date: match[1], time: match[2] ? `${match[2]}:${match[3]}` : '00:00' };
}

/** The stored form: `2026-11-24T14:30`. */
export function joinDateTime(date: IsoDate, time: string): string {
	return `${date}T${time}`;
}

/** The form shown in a field: `2026-11-24 14:30`. */
export function displayDateTime(value: string | null): string {
	const parts = splitDateTime(value);
	return parts ? `${parts.date} ${parts.time}` : '';
}

export function compareIso(a: IsoDate, b: IsoDate): -1 | 0 | 1 {
	return a < b ? -1 : a > b ? 1 : 0;
}

function utc(date: IsoDate): Date {
	const { year, month, day } = partsOf(date);
	return new Date(Date.UTC(year, month - 1, day));
}
function fromUtc(d: Date): IsoDate {
	return toIso(d.getUTCFullYear(), d.getUTCMonth() + 1, d.getUTCDate());
}

export function addDays(date: IsoDate, days: number): IsoDate {
	const d = utc(date);
	d.setUTCDate(d.getUTCDate() + days);
	return fromUtc(d);
}

/** Same day in another month; clamped to that month's last day. */
export function addMonths(date: IsoDate, months: number): IsoDate {
	const { year, month, day } = partsOf(date);
	const index = year * 12 + (month - 1) + months;
	const y = Math.floor(index / 12);
	const m = (index % 12) + 1;
	return toIso(y, m, Math.min(day, daysInMonth(y, m)));
}

export function addYears(date: IsoDate, years: number): IsoDate {
	return addMonths(date, years * 12);
}

/** 0 = Sunday ... 6 = Saturday. */
export function weekdayOf(date: IsoDate): number {
	return utc(date).getUTCDay();
}

/** Days since the first day of the week (0 to 6). */
function offsetInWeek(date: IsoDate, weekStart: WeekStart): number {
	return (weekdayOf(date) - weekStart + 7) % 7;
}

export function startOfWeek(date: IsoDate, weekStart: WeekStart): IsoDate {
	return addDays(date, -offsetInWeek(date, weekStart));
}

/** A 6 by 7 grid for the month (1 to 12); days of other months are `null`. */
export function monthGrid(
	year: number,
	month: number,
	weekStart: WeekStart = 1
): Array<Array<IsoDate | null>> {
	const first = toIso(year, month, 1);
	const lead = offsetInWeek(first, weekStart);
	const total = daysInMonth(year, month);
	const rows: Array<Array<IsoDate | null>> = [];
	for (let row = 0; row < 6; row += 1) {
		const cells: Array<IsoDate | null> = [];
		for (let col = 0; col < 7; col += 1) {
			const day = row * 7 + col - lead + 1;
			cells.push(day >= 1 && day <= total ? toIso(year, month, day) : null);
		}
		rows.push(cells);
	}
	return rows;
}

/** Where a key in the day grid moves the focused date. `shift` turns the page keys into years. */
export function moveFocus(
	date: IsoDate,
	key: MoveKey,
	shift: boolean,
	weekStart: WeekStart = 1
): IsoDate {
	switch (key) {
		case 'ArrowLeft':
			return addDays(date, -1);
		case 'ArrowRight':
			return addDays(date, 1);
		case 'ArrowUp':
			return addDays(date, -7);
		case 'ArrowDown':
			return addDays(date, 7);
		case 'Home':
			return startOfWeek(date, weekStart);
		case 'End':
			return addDays(startOfWeek(date, weekStart), 6);
		case 'PageUp':
			return shift ? addYears(date, -1) : addMonths(date, -1);
		case 'PageDown':
			return shift ? addYears(date, 1) : addMonths(date, 1);
	}
}

export interface MonthView {
	year: number;
	month: number;
}

/** Previous or next month (`delta` months) of a view; the year follows. */
export function shiftView(view: MonthView, delta: number): MonthView {
	const index = view.year * 12 + (view.month - 1) + delta;
	return { year: Math.floor(index / 12), month: (index % 12) + 1 };
}

/** Month view keys: left and right move one month, up and down three; Home and End the row ends. */
export function moveMonth(view: MonthView, key: string): MonthView | null {
	switch (key) {
		case 'ArrowLeft':
			return shiftView(view, -1);
		case 'ArrowRight':
			return shiftView(view, 1);
		case 'ArrowUp':
			return shiftView(view, -3);
		case 'ArrowDown':
			return shiftView(view, 3);
		case 'Home':
			return { year: view.year, month: view.month - ((view.month - 1) % 3) };
		case 'End':
			return { year: view.year, month: view.month + (2 - ((view.month - 1) % 3)) };
		case 'PageUp':
			return { year: view.year - 1, month: view.month };
		case 'PageDown':
			return { year: view.year + 1, month: view.month };
		default:
			return null;
	}
}

/** `now` in the given IANA zone (default: the browser's zone) as an ISO date. */
export function todayIso(zone?: string, now: Date = new Date()): IsoDate {
	const parts = new Intl.DateTimeFormat('en-US', {
		timeZone: zone,
		year: 'numeric',
		month: '2-digit',
		day: '2-digit'
	}).formatToParts(now);
	const get = (type: string) => +(parts.find((p) => p.type === type)?.value ?? 0);
	return toIso(get('year'), get('month'), get('day'));
}

export interface Bounds {
	min?: IsoDate;
	max?: IsoDate;
	isDisabled?: (date: IsoDate) => boolean;
}

export function isDayDisabled(date: IsoDate, { min, max, isDisabled }: Bounds): boolean {
	return (!!min && date < min) || (!!max && date > max) || !!isDisabled?.(date);
}

/** `date` if enabled, else the closest enabled day (earlier wins a tie), else `date`. */
export function nearestEnabled(date: IsoDate, bounds: Bounds, limit = 400): IsoDate {
	if (!isDayDisabled(date, bounds)) return date;
	for (let step = 1; step <= limit; step += 1) {
		const before = addDays(date, -step);
		if (!isDayDisabled(before, bounds)) return before;
		const after = addDays(date, step);
		if (!isDayDisabled(after, bounds)) return after;
	}
	return date;
}

/** Up and down in the time field: 15 minutes, with Shift one hour; clamped to the day. */
export function stepTime(time: string, key: 'ArrowUp' | 'ArrowDown', shift: boolean): string {
	const parsed = parseTimeInput(time);
	const [h, m] = (parsed.ok ? parsed.value : '00:00').split(':').map(Number);
	const delta = (shift ? 60 : 15) * (key === 'ArrowUp' ? 1 : -1);
	const total = Math.max(0, Math.min(23 * 60 + 59, h * 60 + m + delta));
	return `${pad(Math.floor(total / 60))}:${pad(total % 60)}`;
}

export interface RangePick {
	/** The start chosen first, waiting for the end; `null` when nothing is pending. */
	pending: IsoDate | null;
	/** Set when the pick completed a range (earlier date first). */
	range: DateRange | null;
}

/** First pick sets the start and stays open; the second sets the end, swapped if earlier. */
export function pickRangeDate(pending: IsoDate | null, picked: IsoDate): RangePick {
	if (!pending) return { pending: picked, range: null };
	return {
		pending: null,
		range:
			compareIso(picked, pending) < 0
				? { start: picked, end: pending }
				: { start: pending, end: picked }
	};
}

export type BandRole = 'start' | 'end' | 'middle' | 'single' | null;

/**
 * Role of a day in the shown band. A complete range gives start, middle and end; while the end
 * is being chosen the band runs from the pending start to the previewed day.
 */
export function bandRole(
	date: IsoDate,
	range: DateRange | null,
	pending: IsoDate | null,
	preview: IsoDate | null
): { role: BandRole; preview: boolean } {
	let start: IsoDate | null = null;
	let end: IsoDate | null = null;
	let isPreview = false;
	if (pending) {
		start = pending;
		end = preview ?? pending;
		if (end < start) [start, end] = [end, start];
		isPreview = true;
	} else if (range) {
		start = range.start;
		end = range.end;
	}
	if (!start || !end || date < start || date > end) return { role: null, preview: false };
	// While previewing, only the pending start carries the end marker.
	if (isPreview) {
		if (date === pending)
			return { role: start === end ? 'single' : date === start ? 'start' : 'end', preview: true };
		return { role: 'middle', preview: true };
	}
	if (start === end) return { role: 'single', preview: false };
	if (date === start) return { role: 'start', preview: false };
	if (date === end) return { role: 'end', preview: false };
	return { role: 'middle', preview: false };
}

/** Month name and year in the app language, e.g. "November 2026" or "2026年11月". */
export function monthLabel(year: number, month: number, locale: string): string {
	return new Intl.DateTimeFormat(locale, {
		year: 'numeric',
		month: 'long',
		timeZone: 'UTC'
	}).format(new Date(Date.UTC(year, month - 1, 1)));
}

export function yearLabel(year: number, locale: string): string {
	return new Intl.DateTimeFormat(locale, { year: 'numeric', timeZone: 'UTC' }).format(
		new Date(Date.UTC(year, 0, 1))
	);
}

export function monthName(month: number, locale: string): string {
	return new Intl.DateTimeFormat(locale, { month: 'short', timeZone: 'UTC' }).format(
		new Date(Date.UTC(2026, month - 1, 1))
	);
}

/** Weekday headers from the week start: short and full names. Latin languages show two letters. */
export function weekdayLabels(
	locale: string,
	weekStart: WeekStart = 1
): Array<{ short: string; full: string }> {
	const latin = locale.toLowerCase().startsWith('en');
	const short = new Intl.DateTimeFormat(locale, {
		weekday: latin ? 'short' : 'narrow',
		timeZone: 'UTC'
	});
	const full = new Intl.DateTimeFormat(locale, { weekday: 'long', timeZone: 'UTC' });
	// 2026-11-01 is a Sunday.
	return Array.from({ length: 7 }, (_, i) => {
		const day = new Date(Date.UTC(2026, 10, 1 + ((weekStart + i) % 7)));
		const label = short.format(day);
		return { short: latin ? label.slice(0, 2) : label, full: full.format(day) };
	});
}

/** The full date for a cell's hidden text, e.g. "Tuesday, 24 November 2026". */
export function fullDateLabel(date: IsoDate, locale: string): string {
	return new Intl.DateTimeFormat(locale, { dateStyle: 'full', timeZone: 'UTC' }).format(utc(date));
}
