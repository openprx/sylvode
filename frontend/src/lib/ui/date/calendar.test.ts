import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import {
	addMonths,
	bandRole,
	compareIso,
	daysInMonth,
	displayDateTime,
	fullDateLabel,
	isDayDisabled,
	monthGrid,
	monthLabel,
	moveFocus,
	moveMonth,
	nearestEnabled,
	parseDateInput,
	parseDateTimeInput,
	parseTimeInput,
	pickRangeDate,
	shiftView,
	splitDateTime,
	stepTime,
	todayIso,
	weekdayLabels,
	type IsoDate
} from './calendar';

const suite = new Suite('ui-date-calendar');
const iso = (s: string) => s as IsoDate;

suite.check('monthGrid: November 2026, Monday start, six rows of seven', () => {
	const grid = monthGrid(2026, 11, 1);
	assertEqual(grid.length, 6, 'rows');
	assert(
		grid.every((row) => row.length === 7),
		'columns'
	);
	assertDeepEqual(grid[0], [null, null, null, null, null, null, '2026-11-01'], 'first row');
	assertEqual(grid.flat().filter(Boolean).length, 30, 'days');
	assertEqual(grid[5][0], '2026-11-30', 'last day');
});
suite.check('monthGrid: Sunday start and a month that fills a first row', () => {
	assertEqual(monthGrid(2026, 11, 0)[0][0], '2026-11-01', 'Sunday start');
	assertEqual(monthGrid(2026, 2, 1)[0][6], '2026-02-01', 'Feb 2026 starts on Sunday');
});
suite.check('daysInMonth: leap years', () => {
	assertEqual(daysInMonth(2028, 2), 29, 'leap');
	assertEqual(daysInMonth(2026, 2), 28, 'common');
	assertEqual(monthGrid(2028, 2, 1).flat().filter(Boolean).length, 29, 'grid');
});

suite.check('moveFocus: arrows cross month ends', () => {
	assertEqual(moveFocus(iso('2026-11-01'), 'ArrowLeft', false), '2026-10-31', 'left');
	assertEqual(moveFocus(iso('2026-11-30'), 'ArrowRight', false), '2026-12-01', 'right');
	assertEqual(moveFocus(iso('2026-11-03'), 'ArrowUp', false), '2026-10-27', 'up');
	assertEqual(moveFocus(iso('2026-11-27'), 'ArrowDown', false), '2026-12-04', 'down');
});
suite.check('moveFocus: page keys clamp to the last day, shift moves years', () => {
	assertEqual(moveFocus(iso('2026-01-31'), 'PageDown', false), '2026-02-28', 'month clamp');
	assertEqual(moveFocus(iso('2026-03-31'), 'PageUp', false), '2026-02-28', 'month back');
	assertEqual(moveFocus(iso('2028-02-29'), 'PageUp', true), '2027-02-28', 'year back');
	assertEqual(moveFocus(iso('2028-02-29'), 'PageDown', true), '2029-02-28', 'year forward');
});
suite.check('moveFocus: Home and End follow the week start', () => {
	// 2026-11-25 is a Wednesday.
	assertEqual(moveFocus(iso('2026-11-25'), 'Home', false, 1), '2026-11-23', 'Monday');
	assertEqual(moveFocus(iso('2026-11-25'), 'End', false, 1), '2026-11-29', 'Sunday');
	assertEqual(moveFocus(iso('2026-11-25'), 'Home', false, 0), '2026-11-22', 'Sunday start');
	assertEqual(moveFocus(iso('2026-11-25'), 'End', false, 0), '2026-11-28', 'Saturday');
	assertEqual(moveFocus(iso('2026-11-23'), 'Home', false, 1), '2026-11-23', 'already Monday');
});
suite.check('month navigation: shiftView and addMonths cross years', () => {
	assertDeepEqual(shiftView({ year: 2026, month: 1 }, -1), { year: 2025, month: 12 }, 'back');
	assertDeepEqual(shiftView({ year: 2026, month: 12 }, 1), { year: 2027, month: 1 }, 'forward');
	assertEqual(addMonths(iso('2026-12-31'), 2), '2027-02-28', 'clamp across the year');
});
suite.check('month view keys move by one and three', () => {
	assertDeepEqual(
		moveMonth({ year: 2026, month: 11 }, 'ArrowDown'),
		{ year: 2027, month: 2 },
		'down'
	);
	assertDeepEqual(
		moveMonth({ year: 2026, month: 1 }, 'ArrowLeft'),
		{ year: 2025, month: 12 },
		'left'
	);
	assertDeepEqual(moveMonth({ year: 2026, month: 5 }, 'Home'), { year: 2026, month: 4 }, 'home');
	assertDeepEqual(moveMonth({ year: 2026, month: 5 }, 'End'), { year: 2026, month: 6 }, 'end');
	assertEqual(moveMonth({ year: 2026, month: 5 }, 'x'), null, 'other keys');
});

suite.check('parseDateInput accepts the documented forms', () => {
	for (const text of [
		'2026-11-24',
		'2026-11-24 ',
		'2026/11/24',
		'2026.11.24',
		'20261124',
		'2026年11月24日'
	]) {
		assertDeepEqual(parseDateInput(text), { ok: true, value: '2026-11-24' }, text);
	}
	assertDeepEqual(parseDateInput('2026-1-5'), { ok: true, value: '2026-01-05' }, 'short parts');
});
suite.check('parseDateInput rejects ambiguous and impossible dates', () => {
	assertDeepEqual(parseDateInput('24/11/2026'), { ok: false, reason: 'pattern' }, 'day first');
	assertDeepEqual(parseDateInput('11/24/2026'), { ok: false, reason: 'pattern' }, 'month first');
	assertDeepEqual(parseDateInput('26-11-24'), { ok: false, reason: 'pattern' }, 'two-digit year');
	assertDeepEqual(parseDateInput('2026-02-30'), { ok: false, reason: 'invalid' }, 'no such day');
	assertDeepEqual(
		parseDateInput('2027-02-29'),
		{ ok: false, reason: 'invalid' },
		'not a leap year'
	);
	assertDeepEqual(parseDateInput(''), { ok: false, reason: 'pattern' }, 'empty');
});
suite.check('parseTimeInput normalises H:mm, HH:mm and HHmm', () => {
	assertDeepEqual(parseTimeInput('9:05'), { ok: true, value: '09:05' }, 'H:mm');
	assertDeepEqual(parseTimeInput('14:30'), { ok: true, value: '14:30' }, 'HH:mm');
	assertDeepEqual(parseTimeInput('1430'), { ok: true, value: '14:30' }, 'HHmm');
	assertEqual(parseTimeInput('24:00').ok, false, 'hour');
	assertEqual(parseTimeInput('12:60').ok, false, 'minute');
	assertEqual(parseTimeInput('noon').ok, false, 'word');
});
suite.check('parseDateTimeInput: space or T, time optional', () => {
	assertDeepEqual(
		parseDateTimeInput('2026-11-24 14:30'),
		{ ok: true, date: '2026-11-24', time: '14:30' },
		'space'
	);
	assertDeepEqual(
		parseDateTimeInput('2026-11-24T9:05'),
		{ ok: true, date: '2026-11-24', time: '09:05' },
		'T'
	);
	assertDeepEqual(
		parseDateTimeInput('2026/11/24'),
		{ ok: true, date: '2026-11-24', time: null },
		'date only'
	);
	assertEqual(parseDateTimeInput('2026-11-24 25:00').ok, false, 'bad time');
	assertEqual(parseDateTimeInput('2026-11-24 1 2').ok, false, 'three parts');
});
suite.check('stored and displayed date-time forms', () => {
	assertDeepEqual(
		splitDateTime('2026-11-24T14:30:15'),
		{ date: '2026-11-24', time: '14:30' },
		'seconds dropped'
	);
	assertDeepEqual(splitDateTime('2026-11-24'), { date: '2026-11-24', time: '00:00' }, 'date only');
	assertEqual(splitDateTime('nope'), null, 'junk');
	assertEqual(displayDateTime('2026-11-24T14:30'), '2026-11-24 14:30', 'display');
	assertEqual(displayDateTime(null), '', 'empty');
});

suite.check('stepTime: 15 minutes, shift one hour, clamped', () => {
	assertEqual(stepTime('14:30', 'ArrowUp', false), '14:45', 'up');
	assertEqual(stepTime('14:30', 'ArrowDown', true), '13:30', 'hour down');
	assertEqual(stepTime('23:50', 'ArrowUp', false), '23:59', 'upper clamp');
	assertEqual(stepTime('00:05', 'ArrowDown', false), '00:00', 'lower clamp');
});

suite.check('bounds and nearest enabled day', () => {
	const bounds = {
		min: iso('2026-11-10'),
		max: iso('2026-11-20'),
		isDisabled: (d: IsoDate) => d === '2026-11-15'
	};
	assert(isDayDisabled(iso('2026-11-09'), bounds), 'before min');
	assert(isDayDisabled(iso('2026-11-21'), bounds), 'after max');
	assert(isDayDisabled(iso('2026-11-15'), bounds), 'custom');
	assert(!isDayDisabled(iso('2026-11-16'), bounds), 'free');
	assertEqual(nearestEnabled(iso('2026-11-01'), bounds), '2026-11-10', 'moves to min');
	assertEqual(nearestEnabled(iso('2026-12-25'), bounds), '2026-11-20', 'moves to max');
	assertEqual(nearestEnabled(iso('2026-11-12'), bounds), '2026-11-12', 'already enabled');
});

suite.check('range: two picks give an ordered range whatever the order', () => {
	const first = pickRangeDate(null, iso('2026-11-20'));
	assertDeepEqual(first, { pending: '2026-11-20', range: null }, 'first');
	const forward = pickRangeDate(first.pending, iso('2026-11-25'));
	assertDeepEqual(
		forward,
		{ pending: null, range: { start: '2026-11-20', end: '2026-11-25' } },
		'forward'
	);
	const backward = pickRangeDate(first.pending, iso('2026-11-02'));
	assertDeepEqual(backward.range, { start: '2026-11-02', end: '2026-11-20' }, 'swapped');
	assertDeepEqual(
		pickRangeDate(first.pending, iso('2026-11-20')).range,
		{ start: '2026-11-20', end: '2026-11-20' },
		'same day'
	);
});
suite.check('range band roles: complete range and preview', () => {
	const range = { start: iso('2026-11-10'), end: iso('2026-11-14') };
	assertEqual(bandRole(iso('2026-11-10'), range, null, null).role, 'start', 'start');
	assertEqual(bandRole(iso('2026-11-12'), range, null, null).role, 'middle', 'middle');
	assertEqual(bandRole(iso('2026-11-14'), range, null, null).role, 'end', 'end');
	assertEqual(bandRole(iso('2026-11-15'), range, null, null).role, null, 'outside');
	const single = { start: iso('2026-11-10'), end: iso('2026-11-10') };
	assertEqual(bandRole(iso('2026-11-10'), single, null, null).role, 'single', 'single day');
	const preview = bandRole(iso('2026-11-12'), null, iso('2026-11-10'), iso('2026-11-14'));
	assertDeepEqual(preview, { role: 'middle', preview: true }, 'preview middle');
	assertEqual(
		bandRole(iso('2026-11-10'), null, iso('2026-11-10'), iso('2026-11-14')).role,
		'start',
		'pending start'
	);
	assertEqual(
		bandRole(iso('2026-11-12'), null, iso('2026-11-14'), iso('2026-11-10')).role,
		'middle',
		'preview backwards'
	);
	assertEqual(
		bandRole(iso('2026-11-14'), null, iso('2026-11-14'), iso('2026-11-10')).role,
		'end',
		'pending end'
	);
});

suite.check('compareIso orders strings as dates', () => {
	assertEqual(compareIso(iso('2026-11-01'), iso('2026-11-02')), -1, 'less');
	assertEqual(compareIso(iso('2026-11-02'), iso('2026-11-02')), 0, 'equal');
	assertEqual(compareIso(iso('2027-01-01'), iso('2026-12-31')), 1, 'greater');
});
suite.check('todayIso follows the zone, not the machine', () => {
	const instant = new Date('2026-11-20T23:30:00Z');
	assertEqual(todayIso('UTC', instant), '2026-11-20', 'UTC');
	assertEqual(todayIso('Asia/Shanghai', instant), '2026-11-21', 'ahead of UTC');
	assertEqual(todayIso('America/Los_Angeles', instant), '2026-11-20', 'behind UTC');
});
suite.check('labels follow the app language, never the browser locale', () => {
	assertEqual(monthLabel(2026, 11, 'en'), 'November 2026', 'en month');
	assert(/^2026年11月$/.test(monthLabel(2026, 11, 'zh')), `zh month ${monthLabel(2026, 11, 'zh')}`);
	assertDeepEqual(
		weekdayLabels('en', 1).map((d) => d.short),
		['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su'],
		'en weekdays'
	);
	assertDeepEqual(
		weekdayLabels('zh', 1).map((d) => d.short),
		['一', '二', '三', '四', '五', '六', '日'],
		'zh weekdays'
	);
	assertEqual(weekdayLabels('en', 0)[0].full, 'Sunday', 'Sunday start');
	assertEqual(fullDateLabel(iso('2026-11-24'), 'en'), 'Tuesday, November 24, 2026', 'full date');
	// CLDR releases differ on whether a space separates the date from the weekday in zh, so the
	// separator is not part of what is checked.
	const zhFull = fullDateLabel(iso('2026-11-24'), 'zh');
	assert(/^2026年11月24日 ?星期二$/.test(zhFull), `zh full date ${zhFull}`);
});

finish(suite.result());
