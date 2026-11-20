/**
 * Calendar and gantt date maths of the forms page. Every expectation is a fixed calendar fact, so
 * the suite gives the same result in any process timezone (run it with TZ=UTC, Asia/Shanghai and
 * America/Los_Angeles).
 *
 * Run standalone: `bun tests/forms-page/calendar-dates.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormField, FormRecord } from '../../src/lib/api/forms';
import type {
	CalendarGridGroup,
	CalendarRecordOccurrence,
	CalendarResourceRow
} from '../../src/lib/forms/page-types';

await installSvelteSsr();
const dates = await import('../../src/lib/forms/calendar-dates');

const suite = new Suite('forms-page/calendar-dates');

function record(id: string, values: Record<string, unknown>): FormRecord {
	return {
		id,
		workspace_id: 'w',
		project_id: 'p',
		form_id: 'f',
		title: id,
		values,
		source: {},
		created_at: '2026-01-02T03:04:05Z',
		updated_at: '2026-02-03T04:05:06Z'
	};
}

function field(type: FormField['type'] | undefined): FormField {
	return { key: 'when', type };
}

function occurrence(id: string, startDate: string, spanStart: boolean): CalendarRecordOccurrence {
	return {
		key: `${id}:${startDate}`,
		record: record(id, {}),
		date: startDate,
		startDate,
		endDate: startDate,
		recurring: false,
		spanStart,
		spanEnd: true,
		spanDays: 1
	};
}

function group(key: string, records: CalendarRecordOccurrence[]): CalendarGridGroup {
	return { key, label: key, date: '2026-11-20', records };
}

suite.check('addIsoDays moves within a month, over month and year ends, and backwards', () => {
	assertEqual(dates.addIsoDays('2026-11-20', 1), '2026-11-21', 'plain day');
	assertEqual(dates.addIsoDays('2026-11-30', 1), '2026-12-01', 'month end');
	assertEqual(dates.addIsoDays('2026-12-31', 1), '2027-01-01', 'year end');
	assertEqual(dates.addIsoDays('2027-01-01', -1), '2026-12-31', 'backwards over year');
	assertEqual(dates.addIsoDays('2026-03-01', -1), '2026-02-28', 'non-leap February');
	assertEqual(dates.addIsoDays('2028-03-01', -1), '2028-02-29', 'leap February');
	assertEqual(dates.addIsoDays('2028-02-28', 1), '2028-02-29', 'into the leap day');
	assertEqual(dates.addIsoDays('2028-02-29', 1), '2028-03-01', 'out of the leap day');
	assertEqual(dates.addIsoDays('2026-11-20', 0), '2026-11-20', 'zero');
	assertEqual(dates.addIsoDays('2026-01-01', 365), '2027-01-01', 'a full non-leap year');
	assertEqual(dates.addIsoDays('2028-01-01', 366), '2029-01-01', 'a full leap year');
});

suite.check('addIsoDays steps one calendar day over daylight-saving transitions', () => {
	// US spring forward, EU spring forward, EU fall back, US fall back (2026).
	assertEqual(dates.addIsoDays('2026-03-07', 1), '2026-03-08', 'US spring: the 7th to the 8th');
	assertEqual(dates.addIsoDays('2026-03-08', 1), '2026-03-09', 'US spring: the 8th to the 9th');
	assertEqual(dates.addIsoDays('2026-03-28', 2), '2026-03-30', 'EU spring over the 29th');
	assertEqual(dates.addIsoDays('2026-10-24', 2), '2026-10-26', 'EU fall over the 25th');
	assertEqual(dates.addIsoDays('2026-11-01', -1), '2026-10-31', 'US fall back day, backwards');
	assertEqual(dates.addIsoDays('2026-11-01', 1), '2026-11-02', 'US fall back day, forwards');
});

suite.check('addIsoDays throws RangeError on a malformed date', () => {
	let thrown: unknown = null;
	try {
		dates.addIsoDays('not-a-date', 1);
	} catch (error) {
		thrown = error;
	}
	assert(thrown instanceof RangeError, 'an invalid date makes toISOString throw');
});

suite.check('isoDayDiff counts whole days, signed, across months, leap days and DST', () => {
	assertEqual(dates.isoDayDiff('2026-11-20', '2026-11-20'), 0, 'same day');
	assertEqual(dates.isoDayDiff('2026-11-21', '2026-11-20'), 1, 'one day later');
	assertEqual(dates.isoDayDiff('2026-11-20', '2026-11-21'), -1, 'one day earlier');
	assertEqual(dates.isoDayDiff('2026-03-01', '2026-02-01'), 28, 'across non-leap February');
	assertEqual(dates.isoDayDiff('2028-03-01', '2028-02-01'), 29, 'across leap February');
	assertEqual(dates.isoDayDiff('2027-01-01', '2026-01-01'), 365, 'a year');
	assertEqual(dates.isoDayDiff('2026-03-09', '2026-03-08'), 1, 'US DST day');
	assertEqual(dates.isoDayDiff('2026-03-30', '2026-03-29'), 1, 'EU DST day');
	assertEqual(dates.isoDayDiff('2026-01-31', '2025-12-31'), 31, 'month boundary');
});

suite.check('isoDayDiff returns 0 when either side is malformed', () => {
	assertEqual(dates.isoDayDiff('x', '2026-01-01'), 0, 'bad left');
	assertEqual(dates.isoDayDiff('2026-01-01', ''), 0, 'empty right');
	assertEqual(dates.isoDayDiff('', ''), 0, 'both empty');
});

suite.check('addIsoMonths clamps to the end of the target month', () => {
	assertEqual(dates.addIsoMonths('2026-01-31', 1), '2026-02-28', '31 January plus one month');
	assertEqual(dates.addIsoMonths('2028-01-31', 1), '2028-02-29', 'same in a leap year');
	assertEqual(dates.addIsoMonths('2026-03-31', -1), '2026-02-28', 'backwards to February');
	assertEqual(dates.addIsoMonths('2026-05-31', 1), '2026-06-30', 'into a 30-day month');
	assertEqual(dates.addIsoMonths('2028-02-29', 12), '2029-02-28', 'leap day plus a year');
	assertEqual(dates.addIsoMonths('2028-02-29', 48), '2032-02-29', 'leap day plus four years');
	assertEqual(dates.addIsoMonths('2026-11-20', 0), '2026-11-20', 'zero months');
	assertEqual(dates.addIsoMonths('2026-11-20', 1), '2026-12-20', 'plain month');
	assertEqual(dates.addIsoMonths('2026-12-15', 1), '2027-01-15', 'over the year end');
	assertEqual(dates.addIsoMonths('2026-01-15', -1), '2025-12-15', 'backwards over the year start');
	assertEqual(dates.addIsoMonths('2026-01-31', 13), '2027-02-28', 'more than a year');
	assertEqual(dates.addIsoMonths('2026-01-31', -13), '2024-12-31', 'more than a year back');
});

suite.check('monthStartIso returns the first day of the month', () => {
	assertEqual(dates.monthStartIso('2026-11-20'), '2026-11-01', 'mid month');
	assertEqual(dates.monthStartIso('2026-11-01'), '2026-11-01', 'already the first');
	assertEqual(dates.monthStartIso('2026-12-31'), '2026-12-01', 'last day');
	assertEqual(dates.monthStartIso('2028-02-29'), '2028-02-01', 'leap day');
});

suite.check(
	'weekStartIso returns the Monday of the week (Sunday belongs to the week before)',
	() => {
		assertEqual(dates.weekStartIso('2026-11-16'), '2026-11-16', 'Monday stays');
		assertEqual(dates.weekStartIso('2026-11-18'), '2026-11-16', 'Wednesday');
		assertEqual(dates.weekStartIso('2026-11-20'), '2026-11-16', 'Friday');
		assertEqual(dates.weekStartIso('2026-11-21'), '2026-11-16', 'Saturday');
		assertEqual(dates.weekStartIso('2026-11-22'), '2026-11-16', 'Sunday goes back six days');
		assertEqual(dates.weekStartIso('2026-11-23'), '2026-11-23', 'next Monday');
		assertEqual(dates.weekStartIso('2026-12-02'), '2026-11-30', 'week across a month end');
		assertEqual(dates.weekStartIso('2027-01-01'), '2026-12-28', 'week across a year end');
		assertEqual(dates.weekStartIso('2028-03-01'), '2028-02-28', 'week across the leap day');
	}
);

suite.check('monthDayCount knows month lengths and leap years', () => {
	assertEqual(dates.monthDayCount('2026-01-15'), 31, 'January');
	assertEqual(dates.monthDayCount('2026-02-01'), 28, 'February');
	assertEqual(dates.monthDayCount('2028-02-29'), 29, 'leap February');
	assertEqual(dates.monthDayCount('2000-02-10'), 29, 'divisible by 400');
	assertEqual(dates.monthDayCount('1900-02-10'), 28, 'divisible by 100 only');
	assertEqual(dates.monthDayCount('2026-04-30'), 30, 'April');
	assertEqual(dates.monthDayCount('2026-12-31'), 31, 'December');
	assert(Number.isNaN(dates.monthDayCount('bad')), 'malformed input gives NaN');
});

suite.check('ganttBucketStartIso snaps to the bucket start of each scale', () => {
	assertEqual(dates.ganttBucketStartIso('2026-11-20', 'day'), '2026-11-20', 'day');
	assertEqual(dates.ganttBucketStartIso('2026-11-20', 'week'), '2026-11-16', 'week');
	assertEqual(dates.ganttBucketStartIso('2026-11-20', 'month'), '2026-11-01', 'month');
});

suite.check('addGanttBucket adds days, weeks and months', () => {
	assertEqual(dates.addGanttBucket('2026-11-20', 'day', 3), '2026-11-23', 'day');
	assertEqual(dates.addGanttBucket('2026-11-20', 'day', -20), '2026-10-31', 'day backwards');
	assertEqual(dates.addGanttBucket('2026-11-16', 'week', 2), '2026-11-30', 'week');
	assertEqual(dates.addGanttBucket('2026-11-16', 'week', -1), '2026-11-09', 'week backwards');
	assertEqual(
		dates.addGanttBucket('2026-12-28', 'week', 1),
		'2027-01-04',
		'week over the year end'
	);
	assertEqual(dates.addGanttBucket('2026-01-01', 'month', 1), '2026-02-01', 'month');
	assertEqual(dates.addGanttBucket('2026-01-31', 'month', 1), '2026-02-28', 'month clamps');
	assertEqual(dates.addGanttBucket('2026-11-01', 'month', 2), '2027-01-01', 'month over year end');
	assertEqual(dates.addGanttBucket('2026-11-20', 'day', 0), '2026-11-20', 'zero day');
	assertEqual(dates.addGanttBucket('2026-11-20', 'week', 0), '2026-11-20', 'zero week');
	assertEqual(dates.addGanttBucket('2026-11-20', 'month', 0), '2026-11-20', 'zero month');
});

suite.check('ganttBucketLimit is 45 for days and 18 for weeks and months', () => {
	assertEqual(dates.ganttBucketLimit('day'), 45, 'day');
	assertEqual(dates.ganttBucketLimit('week'), 18, 'week');
	assertEqual(dates.ganttBucketLimit('month'), 18, 'month');
});

suite.check('isIsoWorkday is true Monday to Friday only', () => {
	const expected: Array<[string, boolean]> = [
		['2026-11-16', true],
		['2026-11-17', true],
		['2026-11-18', true],
		['2026-11-19', true],
		['2026-11-20', true],
		['2026-11-21', false],
		['2026-11-22', false]
	];
	for (const [date, workday] of expected) {
		assertEqual(dates.isIsoWorkday(date), workday, date);
	}
});

suite.check('calendarGridLabel shows month, day and weekday in the zh-CN form', () => {
	const label = dates.calendarGridLabel('2026-11-20');
	assert(label.startsWith('11/20'), `month/day first, got ${JSON.stringify(label)}`);
	assert(label.includes('周五'), `Friday weekday name, got ${JSON.stringify(label)}`);
	assert(dates.calendarGridLabel('2026-11-22').includes('周日'), 'Sunday weekday name');
	assert(dates.calendarGridLabel('2028-02-29').startsWith('02/29'), 'leap day');
});

suite.check('ganttDateLabel formats days as MM/DD and months as year and month', () => {
	assertEqual(dates.ganttDateLabel('2026-11-05'), '11/05', 'default scale is day');
	assertEqual(dates.ganttDateLabel('2026-11-05', 'day'), '11/05', 'day');
	assertEqual(dates.ganttDateLabel('2026-11-05', 'week'), '11/05', 'week uses the day form');
	const month = dates.ganttDateLabel('2026-11-05', 'month');
	assert(month.includes('2026') && month.includes('11'), `year and month, got ${month}`);
	assert(!month.includes('05'), `no day in the month form, got ${month}`);
});

suite.check('calendarOccurrenceDates: no recurrence returns the base date only', () => {
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-11-05', '2026-11-01', '2026-11-30', 'none'),
		['2026-11-05'],
		'inside the grid'
	);
	assertDeepEqual(
		dates.calendarOccurrenceDates('2025-01-01', '2026-11-01', '2026-11-30', 'none'),
		['2025-01-01'],
		'outside the grid'
	);
});

suite.check('calendarOccurrenceDates: a base after the grid end returns the base date', () => {
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-12-01', '2026-11-01', '2026-11-30', 'daily'),
		['2026-12-01'],
		'daily'
	);
});

suite.check('calendarOccurrenceDates: daily recurrence lists each day inside the grid', () => {
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-11-05', '2026-11-01', '2026-11-07', 'daily'),
		['2026-11-05', '2026-11-06', '2026-11-07'],
		'base inside the grid'
	);
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-10-30', '2026-11-01', '2026-11-03', 'daily'),
		['2026-11-01', '2026-11-02', '2026-11-03'],
		'base before the grid start'
	);
});

suite.check('calendarOccurrenceDates: weekly recurrence keeps the weekday', () => {
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-10-01', '2026-11-01', '2026-11-30', 'weekly'),
		['2026-11-05', '2026-11-12', '2026-11-19', '2026-11-26'],
		'Thursdays of November'
	);
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-11-05', '2026-11-01', '2026-11-30', 'weekly'),
		['2026-11-05', '2026-11-12', '2026-11-19', '2026-11-26'],
		'base inside the grid'
	);
});

suite.check('calendarOccurrenceDates: monthly recurrence lists one date per month', () => {
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-01-15', '2026-01-01', '2026-04-30', 'monthly'),
		['2026-01-15', '2026-02-15', '2026-03-15', '2026-04-15'],
		'day 15'
	);
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-01-15', '2026-03-01', '2026-03-31', 'monthly'),
		['2026-03-15'],
		'grid of one month'
	);
});

suite.check('calendarOccurrenceDates: a monthly recurrence on the 31st drifts to the 28th', () => {
	// CURRENT BEHAVIOUR: each step adds one month to the previous (already clamped) date, so a
	// series that starts on 31 January stays on the 28th after February instead of returning to
	// the 31st in March. Computing every date from the base date would give 03-31.
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-01-31', '2026-01-01', '2026-04-30', 'monthly'),
		['2026-01-31', '2026-02-28', '2026-03-28', '2026-04-28'],
		'31 January series'
	);
});

suite.check(
	'calendarOccurrenceDates falls back to the base date when nothing lands in the grid',
	() => {
		// CURRENT BEHAVIOUR: the fallback returns a date that lies outside the grid.
		assertDeepEqual(
			dates.calendarOccurrenceDates('2026-01-01', '2026-01-15', '2026-01-20', 'monthly'),
			['2026-01-01'],
			'monthly series misses the grid'
		);
		assertDeepEqual(
			dates.calendarOccurrenceDates('2026-01-01', '2026-01-03', '2026-01-05', 'weekly'),
			['2026-01-01'],
			'weekly series misses the grid'
		);
	}
);

suite.check('calendarOccurrenceDates includes the grid boundary dates', () => {
	assertDeepEqual(
		dates.calendarOccurrenceDates('2026-11-01', '2026-11-01', '2026-11-01', 'daily'),
		['2026-11-01'],
		'one-day grid'
	);
});

suite.check('calendarDatePayloadValue appends midnight UTC for datetime fields only', () => {
	assertEqual(
		dates.calendarDatePayloadValue(field('datetime'), '2026-11-20'),
		'2026-11-20T00:00:00.000Z',
		'datetime'
	);
	assertEqual(dates.calendarDatePayloadValue(field('date'), '2026-11-20'), '2026-11-20', 'date');
	assertEqual(
		dates.calendarDatePayloadValue(field(undefined), '2026-11-20'),
		'2026-11-20',
		'missing type counts as date'
	);
	assertEqual(dates.calendarDatePayloadValue(field('text'), '2026-11-20'), '2026-11-20', 'text');
});

suite.check('calendarScheduleKey joins the record id and the start date', () => {
	assertEqual(
		dates.calendarScheduleKey(occurrence('rec-1', '2026-11-20', true)),
		'rec-1:2026-11-20',
		'key'
	);
});

suite.check('calendarDateInputValue reads the date part of date and datetime fields', () => {
	const r = record('r', { when: '2026-03-04T10:30:00Z' });
	assertEqual(dates.calendarDateInputValue(r, field('date')), '2026-03-04', 'date field');
	assertEqual(dates.calendarDateInputValue(r, field('datetime')), '2026-03-04', 'datetime field');
	assertEqual(dates.calendarDateInputValue(r, field(undefined)), '', 'missing type is text');
	assertEqual(dates.calendarDateInputValue(r, field('text')), '', 'text field');
	assertEqual(dates.calendarDateInputValue(r, null), '', 'no field');
	assertEqual(
		dates.calendarDateInputValue(record('r', { when: { date: '2026-05-06' } }), field('date')),
		'2026-05-06',
		'object value with a date'
	);
	assertEqual(
		dates.calendarDateInputValue(record('r', { when: { value: '2026-07-08' } }), field('date')),
		'2026-07-08',
		'object value with a value'
	);
	assertEqual(
		dates.calendarDateInputValue(record('r', { when: 'tomorrow' }), field('date')),
		'',
		'not an ISO date'
	);
	assertEqual(
		dates.calendarDateInputValue(record('r', { when: '' }), field('date')),
		'2026-02-03',
		'empty value falls back to updated_at'
	);
	assertEqual(
		dates.calendarDateInputValue(record('r', {}), field('date')),
		'2026-02-03',
		'missing value falls back to updated_at'
	);
});

suite.check('calendarRecordIsoDate returns the date part or null', () => {
	assertEqual(
		dates.calendarRecordIsoDate(record('r', { when: '2026-03-04' }), field('date')),
		'2026-03-04',
		'date string'
	);
	assertEqual(
		dates.calendarRecordIsoDate(
			record('r', { when: '2026-03-04T23:59:59+08:00' }),
			field('datetime')
		),
		'2026-03-04',
		'datetime keeps the written date, not a converted one'
	);
	assertEqual(
		dates.calendarRecordIsoDate(record('r', { when: 'tomorrow' }), field('date')),
		null,
		'not an ISO date'
	);
	assertEqual(
		dates.calendarRecordIsoDate(
			record('r', { when: { datetime: '2026-09-10T01:02:03Z' } }),
			field('date')
		),
		'2026-09-10',
		'object with a datetime'
	);
	assertEqual(
		dates.calendarRecordIsoDate(record('r', {}), null),
		'2026-02-03',
		'no field uses updated_at'
	);
	assertEqual(
		dates.calendarRecordIsoDate(record('r', { when: 5 }), field('date')),
		'2026-02-03',
		'number falls back to updated_at'
	);
});

suite.check('calendarAgendaGroups keeps span starts and drops groups left empty', () => {
	const resource: CalendarResourceRow = {
		key: 'res',
		label: 'Res',
		records: [],
		groups: [
			group('g1', [occurrence('a', '2026-11-20', true), occurrence('b', '2026-11-20', false)]),
			group('g2', [occurrence('c', '2026-11-21', false)]),
			group('g3', [])
		]
	};
	const result = dates.calendarAgendaGroups(resource);
	assertEqual(result.length, 1, 'only g1 remains');
	assertEqual(result[0].key, 'g1', 'group key kept');
	assertDeepEqual(
		result[0].records.map((item) => item.key),
		['a:2026-11-20'],
		'only the span start remains'
	);
	assertEqual(resource.groups[0].records.length, 2, 'the input is not modified');
	assertDeepEqual(
		dates.calendarAgendaGroups({ key: 'k', label: 'l', records: [], groups: [] }),
		[],
		'no groups'
	);
});

suite.check('calendarAgendaRecordCount totals the span starts of all groups', () => {
	const resource: CalendarResourceRow = {
		key: 'res',
		label: 'Res',
		records: [],
		groups: [
			group('g1', [occurrence('a', '2026-11-20', true), occurrence('b', '2026-11-20', true)]),
			group('g2', [occurrence('c', '2026-11-21', true), occurrence('d', '2026-11-21', false)]),
			group('g3', [occurrence('e', '2026-11-22', false)])
		]
	};
	assertEqual(dates.calendarAgendaRecordCount(resource), 3, 'three span starts');
	assertEqual(
		dates.calendarAgendaRecordCount({ key: 'k', label: 'l', records: [], groups: [] }),
		0,
		'empty resource'
	);
});

finish(suite.result());
