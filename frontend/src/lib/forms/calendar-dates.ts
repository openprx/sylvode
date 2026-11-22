import type { FormField, FormRecord } from '#lib/api/forms.js';
import type {
	CalendarGridGroup,
	CalendarRecordOccurrence,
	CalendarResourceRow,
	GanttScale,
	ViewCalendarRecurrence
} from '#lib/forms/page-types.js';
import { currentLocale } from '#lib/forms/i18n.js';
import { recordDateSource } from '#lib/forms/record-grouping.js';

export function calendarScheduleKey(occurrence: CalendarRecordOccurrence): string {
	return `${occurrence.record.id}:${occurrence.startDate}`;
}

export function calendarDateInputValue(record: FormRecord, field: FormField | null): string {
	if (!field || !['date', 'datetime'].includes(field.type ?? 'text')) return '';

	const source = recordDateSource(record, field);
	const match = source.match(/^\d{4}-\d{2}-\d{2}/);

	return match?.[0] ?? '';
}

export function calendarRecordIsoDate(record: FormRecord, field: FormField | null): string | null {
	const source = recordDateSource(record, field);
	const match = source.match(/^\d{4}-\d{2}-\d{2}/);

	return match?.[0] ?? null;
}

export function addIsoDays(isoDate: string, days: number): string {
	const date = new Date(`${isoDate}T00:00:00.000Z`);

	date.setUTCDate(date.getUTCDate() + days);

	return date.toISOString().slice(0, 10);
}

export function isoDayDiff(left: string, right: string): number {
	const leftTime = new Date(`${left}T00:00:00.000Z`).getTime();
	const rightTime = new Date(`${right}T00:00:00.000Z`).getTime();

	if (!Number.isFinite(leftTime) || !Number.isFinite(rightTime)) return 0;

	return Math.round((leftTime - rightTime) / 86_400_000);
}

export function addIsoMonths(isoDate: string, months: number): string {
	const date = new Date(`${isoDate}T00:00:00.000Z`);
	const day = date.getUTCDate();

	date.setUTCMonth(date.getUTCMonth() + months, 1);

	const maxDay = new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 0)).getUTCDate();

	date.setUTCDate(Math.min(day, maxDay));

	return date.toISOString().slice(0, 10);
}

export function monthStartIso(isoDate: string): string {
	const date = new Date(`${isoDate}T00:00:00.000Z`);

	return new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), 1))
		.toISOString()
		.slice(0, 10);
}

export function weekStartIso(isoDate: string): string {
	const date = new Date(`${isoDate}T00:00:00.000Z`);
	const day = date.getUTCDay();
	const offset = day === 0 ? -6 : 1 - day;

	date.setUTCDate(date.getUTCDate() + offset);

	return date.toISOString().slice(0, 10);
}

export function monthDayCount(isoDate: string): number {
	const date = new Date(`${isoDate}T00:00:00.000Z`);

	return new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 0)).getUTCDate();
}

export function ganttBucketStartIso(isoDate: string, scale: GanttScale): string {
	if (scale === 'month') return monthStartIso(isoDate);
	if (scale === 'week') return weekStartIso(isoDate);

	return isoDate;
}

export function addGanttBucket(isoDate: string, scale: GanttScale, amount: number): string {
	if (scale === 'month') return addIsoMonths(isoDate, amount);
	if (scale === 'week') return addIsoDays(isoDate, amount * 7);

	return addIsoDays(isoDate, amount);
}

export function ganttBucketLimit(scale: GanttScale): number {
	if (scale === 'day') return 45;

	return 18;
}

export function calendarGridLabel(isoDate: string): string {
	const date = new Date(`${isoDate}T00:00:00.000Z`);

	return date.toLocaleDateString(currentLocale() === 'en' ? 'en-US' : 'zh-CN', {
		month: '2-digit',
		day: '2-digit',
		weekday: 'short',
		timeZone: 'UTC'
	});
}

export function isIsoWorkday(isoDate: string): boolean {
	const day = new Date(`${isoDate}T00:00:00.000Z`).getUTCDay();

	return day >= 1 && day <= 5;
}

export function ganttDateLabel(isoDate: string, scale: GanttScale = 'day'): string {
	const date = new Date(`${isoDate}T00:00:00.000Z`);

	if (scale === 'month') {
		return date.toLocaleDateString(currentLocale() === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			timeZone: 'UTC'
		});
	}

	return date.toLocaleDateString(currentLocale() === 'en' ? 'en-US' : 'zh-CN', {
		month: '2-digit',
		day: '2-digit',
		timeZone: 'UTC'
	});
}

export function calendarOccurrenceDates(
	baseDate: string,
	gridStartDate: string,
	gridEndDate: string,
	recurrence: ViewCalendarRecurrence
): string[] {
	if (recurrence === 'none') return [baseDate];
	if (baseDate > gridEndDate) return [baseDate];

	const dates: string[] = [];
	let cursor = baseDate;
	let step = 0;

	while (cursor <= gridEndDate) {
		if (cursor >= gridStartDate) dates.push(cursor);

		if (recurrence === 'daily') {
			cursor = addIsoDays(cursor, 1);
		} else if (recurrence === 'weekly') {
			cursor = addIsoDays(cursor, 7);
		} else {
			// Counted from the base date so a clamped month end (28 February) does not stick.
			step += 1;
			cursor = addIsoMonths(baseDate, step);
		}
	}

	return dates.length > 0 ? dates : [baseDate];
}

export function calendarDatePayloadValue(field: FormField, value: string): string {
	if ((field.type ?? 'date') === 'datetime') return `${value}T00:00:00.000Z`;

	return value;
}

export function calendarAgendaGroups(resource: CalendarResourceRow): CalendarGridGroup[] {
	return resource.groups
		.map((group) => ({
			...group,
			records: group.records.filter((occurrence) => occurrence.spanStart)
		}))
		.filter((group) => group.records.length > 0);
}

export function calendarAgendaRecordCount(resource: CalendarResourceRow): number {
	return calendarAgendaGroups(resource).reduce((total, group) => total + group.records.length, 0);
}
