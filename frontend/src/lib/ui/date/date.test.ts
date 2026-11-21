import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { boundsError, displayValue, parseField, parseRange } from './date-field';
import type { IsoDate } from './calendar';

await installSvelteSsr();
const { default: DatePicker } = await import('./DatePicker.svelte');
const { default: DateRangePicker } = await import('./DateRangePicker.svelte');
const { default: CalendarPanel } = await import('./CalendarPanel.svelte');

const suite = new Suite('ui-date');
const iso = (s: string) => s as IsoDate;

suite.check('displayValue: ISO date, and the date-time with a space', () => {
	assertEqual(displayValue('date', '2026-11-24'), '2026-11-24', 'date');
	assertEqual(displayValue('date', null), '', 'empty');
	assertEqual(displayValue('datetime', '2026-11-24T14:30'), '2026-11-24 14:30', 'date-time');
	assertEqual(
		displayValue('datetime', '2026-11-24T14:30:59'),
		'2026-11-24 14:30',
		'seconds dropped'
	);
});
suite.check('parseField: typed forms normalise to the stored value', () => {
	assertDeepEqual(
		parseField('date', '2026/1/5', {}, null, '00:00'),
		{ state: 'ok', value: '2026-01-05' },
		'date'
	);
	assertDeepEqual(
		parseField('date', '  ', {}, null, '00:00'),
		{ state: 'empty' },
		'blank is empty'
	);
	assertDeepEqual(
		parseField('datetime', '2026-11-24 9:05', {}, null, '00:00'),
		{ state: 'ok', value: '2026-11-24T09:05' },
		'with time'
	);
	assertDeepEqual(
		parseField('datetime', '2026-11-24', {}, null, '08:00'),
		{ state: 'ok', value: '2026-11-24T08:00' },
		'default time'
	);
	assertDeepEqual(
		parseField('datetime', '2026-11-25', {}, '2026-11-24T14:30', '08:00'),
		{ state: 'ok', value: '2026-11-25T14:30' },
		'keeps the existing time'
	);
});
suite.check('parseField: pattern, invalid and bounds errors', () => {
	assertDeepEqual(
		parseField('date', '24/11/2026', {}, null, '00:00'),
		{ state: 'error', key: 'pattern' },
		'ambiguous'
	);
	assertDeepEqual(
		parseField('date', '2026-02-30', {}, null, '00:00'),
		{ state: 'error', key: 'invalid' },
		'impossible'
	);
	assertDeepEqual(
		parseField('datetime', '2026-11-24 99:00', {}, null, '00:00'),
		{ state: 'error', key: 'patternTime' },
		'time'
	);
	const bounds = { min: iso('2026-11-10'), max: iso('2026-11-20') };
	assertDeepEqual(
		parseField('date', '2026-11-01', bounds, null, '00:00'),
		{ state: 'error', key: 'min' },
		'before min'
	);
	assertEqual(boundsError(iso('2026-11-21'), bounds), 'max', 'after max');
	assertEqual(boundsError(iso('2026-11-12'), { isDisabled: () => true }), 'unavailable', 'custom');
	assertEqual(boundsError(iso('2026-11-12'), bounds), null, 'free');
});
suite.check('parseRange: complete only when both ends are valid and ordered', () => {
	assertDeepEqual(
		parseRange('2026-11-10', '2026-11-12', {}).range,
		{ start: '2026-11-10', end: '2026-11-12' },
		'ordered'
	);
	const reversed = parseRange('2026-11-12', '2026-11-10', {});
	assert(reversed.order && reversed.range === null, 'reversed');
	assertEqual(parseRange('2026-11-12', '', {}).range, null, 'draft');
	assertEqual(parseRange('x', '2026-11-12', {}).start.state, 'error', 'error end');
});

const picker = (props: Record<string, unknown>) =>
	renderToHtml(DatePicker, { id: 'due', label: 'Due', onchange: () => {}, ...props });

await suite.checkAsync(
	'the field shows the same ISO text whatever the browser language',
	async () => {
		const original = Object.getOwnPropertyDescriptor(globalThis.navigator, 'language');
		const render = async (language: string) => {
			Object.defineProperty(globalThis.navigator, 'language', {
				value: language,
				configurable: true
			});
			return picker({ value: '2026-11-24' });
		};
		try {
			const us = await render('en-US');
			const de = await render('de-DE');
			assertEqual(us, de, 'markup does not depend on the browser language');
			assert(us.includes('value="2026-11-24"'), 'ISO value');
		} finally {
			if (original) Object.defineProperty(globalThis.navigator, 'language', original);
		}
	}
);
await suite.checkAsync('the date-time field shows a space between date and time', async () => {
	const html = await picker({ kind: 'datetime', value: '2026-11-24T14:30' });
	assert(html.includes('value="2026-11-24 14:30"'), html);
	assert(
		html.includes('ui.date.placeholderTime') === false ||
			html.includes('placeholder="ui.date.placeholderTime"'),
		'placeholder key'
	);
});
await suite.checkAsync(
	'the field is a combobox with a dialog popup, collapsed, required and described',
	async () => {
		const html = await picker({ value: null, required: true, helper: 'Pick a day' });
		assert(html.includes('role="combobox"') && html.includes('aria-haspopup="dialog"'), html);
		assert(
			html.includes('aria-expanded="false"') && html.includes('autocomplete="off"'),
			'collapsed, no autofill'
		);
		assert(html.includes('aria-required="true"'), 'required');
		assert(/aria-describedby="[^"]+-message"/.test(html), 'described by the helper');
		assert(!html.includes('inputmode'), 'no numeric keypad: the value has dashes');
	}
);
await suite.checkAsync(
	'an error from the caller marks the field invalid; disabled shows its reason',
	async () => {
		const invalid = await picker({ value: null, error: 'Choose a due date.' });
		assert(
			invalid.includes('aria-invalid="true"') && invalid.includes('Choose a due date.'),
			invalid
		);
		const off = await picker({ value: null, disabled: true, disabledReason: 'Locked' });
		assert(/<input[^>]*\sdisabled/.test(off) && off.includes('Locked'), off);
	}
);
await suite.checkAsync('the range picker renders two named boxes in a group', async () => {
	const html = await renderToHtml(DateRangePicker, {
		label: 'Period',
		value: { start: '2026-11-10', end: '2026-11-18' },
		onchange: () => {}
	});
	assert(html.includes('role="group"'), html);
	assert(html.includes('value="2026-11-10"') && html.includes('value="2026-11-18"'), 'both ends');
	assert(
		html.includes('aria-label="ui.date.start"') && html.includes('aria-label="ui.date.end"'),
		'box names'
	);
});

const panel = (props: Record<string, unknown>) => renderToHtml(CalendarPanel, props);
await suite.checkAsync('the panel is a 6 by 7 grid with a header for each weekday', async () => {
	const html = await panel({ selected: '2026-11-24' });
	assertEqual((html.match(/<tr>/g) ?? []).length, 7, 'header row plus six rows');
	assertEqual((html.match(/<th /g) ?? []).length, 7, 'weekdays');
	assertEqual((html.match(/role="gridcell"/g) ?? []).length, 42, 'cells');
	assertEqual((html.match(/data-date=/g) ?? []).length, 30, 'only the days of November');
	assert(
		html.includes('abbr="Monday"') || html.includes('abbr="星期一"'),
		'abbr carries the full name'
	);
});
await suite.checkAsync('the heading follows the app language (zh in the SSR stub)', async () => {
	const html = await panel({ selected: '2026-11-24' });
	assert(/<h2[^>]*>2026年11月<\/h2>/.test(html), html);
});
await suite.checkAsync(
	'one tab stop, one selected day, disabled days are aria-disabled',
	async () => {
		const html = await panel({ selected: '2026-11-24', min: '2026-11-10' });
		assertEqual((html.match(/tabindex="0"/g) ?? []).length >= 1, true, 'a roving stop exists');
		assert(
			/aria-selected="true"[^>]*data-date="2026-11-24"|data-date="2026-11-24"[^>]*aria-selected="true"/.test(
				html
			),
			'selected day'
		);
		assertEqual((html.match(/aria-selected="true"/g) ?? []).length, 1, 'one selected');
		assertEqual(
			(html.match(/aria-disabled="true"/g) ?? []).length >= 9,
			true,
			'days before the minimum'
		);
	}
);
await suite.checkAsync('exactly one cell is today when today is on the shown month', async () => {
	const html = await panel({});
	assertEqual((html.match(/aria-current="date"/g) ?? []).length, 1, 'today');
});
await suite.checkAsync(
	'date-time adds the time field and the zone line; date does not',
	async () => {
		const withTime = await panel({
			kind: 'datetime',
			time: '14:30',
			zoneLabel: 'No time zone: saved as entered.'
		});
		assert(
			withTime.includes('value="14:30"') && withTime.includes('No time zone: saved as entered.'),
			withTime
		);
		const plain = await panel({ kind: 'date', zoneLabel: 'ignored' });
		assert(!plain.includes('class="time') && !plain.includes('ignored'), 'no time field');
	}
);
await suite.checkAsync('range mode marks the ends and the band', async () => {
	const html = await panel({ mode: 'range', range: { start: '2026-11-10', end: '2026-11-14' } });
	assertEqual((html.match(/aria-selected="true"/g) ?? []).length, 5, 'every day of the range');
	assert(
		html.includes('ui.date.rangeStart') && html.includes('ui.date.rangeEnd'),
		'end markers in the hidden text'
	);
});

finish(suite.result());
