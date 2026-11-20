/**
 * Timeline event keys, titles, JSON, CSV, filters and date boundaries of the forms page.
 *
 * Run standalone: `bun tests/forms-page/views/timeline-events.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../../support/harness';
import { installSvelteSsr } from '../../support/svelte-ssr';
import type { BusinessEvent } from '../../../src/lib/api/forms';

await installSvelteSsr();
const timeline = await import('../../../src/lib/forms/views/timeline-events');

const suite = new Suite('forms-page/views/timeline-events');

function event(overrides: Partial<BusinessEvent> = {}): BusinessEvent {
	return {
		id: 'e1',
		workspace_id: 'w',
		event_type: 'record.created',
		aggregate_type: 'record',
		aggregate_id: 'r1',
		source: {},
		payload: {},
		metadata: {},
		created_at: '2026-11-20T10:00:00Z',
		...overrides
	};
}

suite.check(
	'timelineEventAggregateKey and timelineEventAggregateKeys join type and id and de-duplicate',
	() => {
		assertEqual(timeline.timelineEventAggregateKey(event()), 'record:r1', 'key');
		assertDeepEqual(
			timeline.timelineEventAggregateKeys([
				event({ id: 'a', aggregate_id: 'r2' }),
				event({ id: 'b', aggregate_id: 'r1' }),
				event({ id: 'c', aggregate_id: 'r2' }),
				event({ id: 'd', aggregate_type: 'form', aggregate_id: 'r1' })
			]),
			['record:r2', 'record:r1', 'form:r1'],
			'first-seen order, no repeats'
		);
		assertDeepEqual(timeline.timelineEventAggregateKeys([]), [], 'no events');
	}
);

suite.check('timelineEventDateTimeLocal writes the local date and time to the minute', () => {
	// Dates are built from local components, so the result is the same in any timezone.
	assertEqual(
		timeline.timelineEventDateTimeLocal(new Date(2026, 10, 20, 9, 5, 59)),
		'2026-11-20T09:05',
		'morning'
	);
	assertEqual(
		timeline.timelineEventDateTimeLocal(new Date(2026, 0, 1, 0, 0)),
		'2026-01-01T00:00',
		'local midnight'
	);
	assertEqual(
		timeline.timelineEventDateTimeLocal(new Date(2026, 11, 31, 23, 59)),
		'2026-12-31T23:59',
		'end of the year'
	);
	assertEqual(
		timeline.timelineEventDateTimeLocal(new Date(2028, 1, 29, 12, 30)),
		'2028-02-29T12:30',
		'leap day'
	);
	assertEqual(
		timeline.timelineEventDateTimeLocal(new Date(2026, 6, 15, 12, 0)),
		'2026-07-15T12:00',
		'summer noon'
	);
	assertEqual(
		timeline.timelineEventDateTimeLocal(new Date(2026, 0, 15, 12, 0)),
		'2026-01-15T12:00',
		'winter noon'
	);
});

suite.check('timelineEventDateTimeLocal throws RangeError for an invalid date', () => {
	let thrown: unknown = null;
	try {
		timeline.timelineEventDateTimeLocal(new Date('nope'));
	} catch (error) {
		thrown = error;
	}
	assert(thrown instanceof RangeError, 'an invalid date makes toISOString throw');
});

suite.check('timelineEventLayout is detailed only for the exact word', () => {
	assertEqual(timeline.timelineEventLayout('detailed'), 'detailed', 'detailed');
	assertEqual(timeline.timelineEventLayout('summary'), 'summary', 'summary');
	assertEqual(timeline.timelineEventLayout('Detailed'), 'summary', 'case matters');
	assertEqual(timeline.timelineEventLayout(undefined), 'summary', 'undefined');
	assertEqual(timeline.timelineEventLayout(null), 'summary', 'null');
	assertEqual(timeline.timelineEventLayout(1), 'summary', 'number');
});

suite.check('timelineEventTitle reads title, record_title, form_name, then the event type', () => {
	assertEqual(
		timeline.timelineEventTitle(event({ payload: { title: ' Hello ' } })),
		'Hello',
		'payload title, trimmed'
	);
	assertEqual(
		timeline.timelineEventTitle(event({ payload: { record_title: 'RT' } })),
		'RT',
		'record_title'
	);
	assertEqual(
		timeline.timelineEventTitle(event({ payload: { form_name: 'FN' } })),
		'FN',
		'form_name'
	);
	assertEqual(
		timeline.timelineEventTitle(event({ payload: { title: 'T', record_title: 'RT' } })),
		'T',
		'title first'
	);
	assertEqual(
		timeline.timelineEventTitle(event({ payload: { form_name: 'FN' }, metadata: { title: 'MT' } })),
		'FN',
		'payload is searched before metadata'
	);
	assertEqual(
		timeline.timelineEventTitle(event({ metadata: { title: 'MT' }, source: { title: 'ST' } })),
		'MT',
		'metadata before source'
	);
	assertEqual(timeline.timelineEventTitle(event({ source: { title: 'ST' } })), 'ST', 'source');
	assertEqual(timeline.timelineEventTitle(event({ payload: { title: 42 } })), '42', 'number');
	assertEqual(
		timeline.timelineEventTitle(event({ payload: { title: false } })),
		'false',
		'boolean'
	);
	assertEqual(
		timeline.timelineEventTitle(event({ payload: { title: '   ' } })),
		'record.created',
		'blank title falls back'
	);
	assertEqual(
		timeline.timelineEventTitle(event({ payload: { title: { a: 1 } } })),
		'record.created',
		'object title falls back'
	);
	assertEqual(timeline.timelineEventTitle(event()), 'record.created', 'nothing configured');
});

suite.check('timelineEventSubtitle shows the aggregate and the actor when there is one', () => {
	assertEqual(timeline.timelineEventSubtitle(event()), 'record:r1', 'no actor');
	assertEqual(timeline.timelineEventSubtitle(event({ actor_id: null })), 'record:r1', 'null actor');
	assertEqual(timeline.timelineEventSubtitle(event({ actor_id: '' })), 'record:r1', 'empty actor');
	assertEqual(
		timeline.timelineEventSubtitle(event({ actor_id: 'u7' })),
		'record:r1 · forms.eventActor u7',
		'actor'
	);
});

suite.check('timelineEventJson writes {} for empty values and two-space JSON otherwise', () => {
	for (const value of [null, undefined, '', 0, false, {}]) {
		assertEqual(timeline.timelineEventJson(value), '{}', `${JSON.stringify(value)} gives {}`);
	}
	assertEqual(timeline.timelineEventJson({ a: 1 }), '{\n  "a": 1\n}', 'one key');
	assertEqual(
		timeline.timelineEventJson({ a: { b: [1, 2] } }),
		'{\n  "a": {\n    "b": [\n      1,\n      2\n    ]\n  }\n}',
		'nested'
	);
	assertEqual(timeline.timelineEventJson('text'), '"text"', 'a non-empty string is quoted');
	assertEqual(timeline.timelineEventJson(5), '5', 'a number');
	assertEqual(timeline.timelineEventJson(['x']), '[\n  "x"\n]', 'an array');
	assertEqual(timeline.timelineEventJson([]), '[]', 'an empty array is not turned into {}');
});

suite.check('timelineEventDetailsPayload and the JSON helpers carry all event properties', () => {
	const e = event({
		actor_id: 'u7',
		payload: { title: 'T', n: 1 },
		metadata: { m: true },
		source: { s: 'web' }
	});
	const expected = {
		id: 'e1',
		event_type: 'record.created',
		created_at: '2026-11-20T10:00:00Z',
		aggregate_type: 'record',
		aggregate_id: 'r1',
		actor_id: 'u7',
		title: 'T',
		subtitle: 'record:r1 · forms.eventActor u7',
		payload: { title: 'T', n: 1 },
		metadata: { m: true },
		source: { s: 'web' }
	};
	assertDeepEqual(timeline.timelineEventDetailsPayload(e), expected, 'payload object');
	assertEqual(
		timeline.timelineEventDetailsJson(e),
		JSON.stringify(expected, null, 2),
		'single JSON'
	);
	assertEqual(
		timeline.timelineEventsDetailsJson([e, event({ id: 'e2' })]),
		JSON.stringify(
			[
				expected,
				{
					id: 'e2',
					event_type: 'record.created',
					created_at: '2026-11-20T10:00:00Z',
					aggregate_type: 'record',
					aggregate_id: 'r1',
					title: 'record.created',
					subtitle: 'record:r1',
					payload: {},
					metadata: {},
					source: {}
				}
			],
			null,
			2
		),
		'list JSON'
	);
	assertEqual(timeline.timelineEventsDetailsJson([]), '[]', 'empty list');
	assertEqual(
		timeline.timelineEventDetailsPayload(event()).actor_id,
		undefined,
		'a missing actor stays undefined'
	);
});

suite.check('timelineEventsCsv writes a quoted header and one quoted row per event', () => {
	const header =
		'"Event ID","Type","Title","Subtitle","Aggregate Type","Aggregate ID","Actor","Created At","Payload","Metadata","Source"';
	assertEqual(timeline.timelineEventsCsv([]), header, 'header only');
	assertEqual(
		timeline.timelineEventsCsv([event({ actor_id: 'u7', payload: { title: 'Say "hi"' } })]),
		`${header}\n"e1","record.created","Say ""hi""","record:r1 · forms.eventActor u7","record","r1","u7","2026-11-20T10:00:00Z","{\n  ""title"": ""Say \\""hi\\""""\n}","{}","{}"`,
		'one row; quotes doubled; JSON cells keep their line breaks'
	);
	const two = timeline.timelineEventsCsv([event(), event({ id: 'e2', aggregate_id: 'r2' })]);
	assertEqual(two.split('\n').length, 3, 'header plus two single-line rows');
	assert(
		two.includes('\n"e2","record.created","record.created","record:r2","record","r2","","2026'),
		'second row, empty actor cell'
	);
});

suite.check('timelineEventsCsv neutralises titles that start with = + - @', () => {
	// Titles come from user content; the export prefixes a ' so a spreadsheet reads text.
	const csv = timeline.timelineEventsCsv([event({ payload: { title: '=1+1' } })]);
	assert(csv.includes(`,"'=1+1",`), 'formula-like title is prefixed with a quote');
	assert(!csv.includes(',"=1+1",'), 'no cell starts with the bare formula');
});

suite.check('timelineEventSearchText is one lower-case text of every searchable property', () => {
	const text = timeline.timelineEventSearchText(
		event({
			id: 'E-1',
			actor_id: 'U-7',
			payload: { title: 'Hello WORLD', note: 'Ünï' },
			metadata: { Tag: 'Blue' }
		})
	);
	assertEqual(text, text.toLowerCase(), 'all lower case');
	for (const part of [
		'e-1',
		'record.created',
		'record:r1',
		'u-7',
		'hello world',
		'"note": "ünï"',
		'"tag": "blue"',
		'forms.eventactor u-7'
	]) {
		assert(text.includes(part), `contains ${part}`);
	}
	assertEqual(
		timeline.timelineEventSearchText(event()),
		[
			'e1',
			'record.created',
			'record',
			'r1',
			'',
			'record.created',
			'record:r1',
			'{}',
			'{}',
			'{}'
		].join('\n'),
		'a bare event: ten lines in a fixed order'
	);
});

suite.check('eventsByCreatedAt sorts newest first without changing the input', () => {
	const events = [
		event({ id: 'old', created_at: '2026-11-18T08:00:00Z' }),
		event({ id: 'new', created_at: '2026-11-20T08:00:00Z' }),
		event({ id: 'mid', created_at: '2026-11-19T08:00:00Z' }),
		event({ id: 'new-offset', created_at: '2026-11-20T09:00:00+02:00' })
	];
	const sorted = timeline.eventsByCreatedAt(events);
	assertDeepEqual(
		sorted.map((e) => e.id),
		['new', 'new-offset', 'mid', 'old'],
		'newest first; time zone offsets are respected'
	);
	assertDeepEqual(
		events.map((e) => e.id),
		['old', 'new', 'mid', 'new-offset'],
		'input unchanged'
	);
	assertDeepEqual(
		timeline.eventsByCreatedAt([event({ id: 'a' }), event({ id: 'b' })]).map((e) => e.id),
		['a', 'b'],
		'equal times keep their order'
	);
	assertDeepEqual(timeline.eventsByCreatedAt([]), [], 'empty');
});

suite.check('eventTypes, eventActors and eventAggregates are sorted distinct lists', () => {
	const events = [
		event({ event_type: 'record.updated', actor_id: 'u2', aggregate_id: 'r2' }),
		event({ event_type: 'record.created', actor_id: 'u1', aggregate_id: 'r1' }),
		event({ event_type: 'record.updated', actor_id: null, aggregate_id: 'r2' }),
		event({ event_type: '', actor_id: '', aggregate_type: 'form', aggregate_id: 'f1' }),
		event({ event_type: 'record.created', actor_id: 'u2', aggregate_id: 'r1' })
	];
	assertDeepEqual(
		timeline.eventTypes(events),
		['record.created', 'record.updated'],
		'types, empty dropped'
	);
	assertDeepEqual(timeline.eventActors(events), ['u1', 'u2'], 'actors, null and empty dropped');
	assertDeepEqual(
		timeline.eventAggregates(events),
		['form:f1', 'record:r1', 'record:r2'],
		'aggregates'
	);
	assertDeepEqual(timeline.eventTypes([]), [], 'no events: types');
	assertDeepEqual(timeline.eventActors([]), [], 'no events: actors');
	assertDeepEqual(timeline.eventAggregates([]), [], 'no events: aggregates');
});

suite.check('timelineEventBoundary parses a date or date and time into milliseconds', () => {
	assertEqual(
		timeline.timelineEventBoundary('2026-11-20'),
		1795132800000,
		'date only is UTC midnight'
	);
	assertEqual(timeline.timelineEventBoundary(' 2026-11-20 '), 1795132800000, 'padding trimmed');
	assertEqual(timeline.timelineEventBoundary('2026-11-20T12:30:00Z'), 1795177800000, 'UTC time');
	assertEqual(
		timeline.timelineEventBoundary('2026-11-20T14:30:00+02:00'),
		1795177800000,
		'offset time'
	);
	assertEqual(
		timeline.timelineEventBoundary('2026-11-20T09:05'),
		new Date(2026, 10, 20, 9, 5).getTime(),
		'a time without a zone is local time'
	);
});

suite.check('timelineEventBoundary returns null for blank or unparseable input', () => {
	assertEqual(timeline.timelineEventBoundary(''), null, 'empty');
	assertEqual(timeline.timelineEventBoundary('   '), null, 'blank');
	assertEqual(timeline.timelineEventBoundary('yesterday'), null, 'text');
	assertEqual(timeline.timelineEventBoundary('2026-13-45'), null, 'impossible date');
});

finish(suite.result());
