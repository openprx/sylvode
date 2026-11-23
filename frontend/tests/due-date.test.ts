/**
 * The due-date mapping (`src/lib/utils/due-date.ts`): a chosen calendar date is stored as midnight
 * UTC and read back from the UTC date parts, so it round-trips in every time zone.
 *
 * Run standalone: `bun tests/due-date.test.ts`
 */

import { Suite, assertEqual, finish } from './support/harness';
import { dueDateToTimestamp, timestampToDueDate } from '../src/lib/utils/due-date';

const suite = new Suite('due-date');

suite.check('a date is stored as midnight UTC of that day', () => {
	assertEqual(dueDateToTimestamp('2030-01-02'), '2030-01-02T00:00:00Z', 'plain date');
	assertEqual(dueDateToTimestamp('2028-02-29'), '2028-02-29T00:00:00Z', 'leap day');
	assertEqual(dueDateToTimestamp(null), null, 'no date clears');
	assertEqual(dueDateToTimestamp(''), null, 'empty clears');
	assertEqual(dueDateToTimestamp('2029-02-29'), null, 'a day that does not exist');
	assertEqual(dueDateToTimestamp('2030-13-01'), null, 'a month that does not exist');
	assertEqual(dueDateToTimestamp('01/02/2030'), null, 'not an ISO date');
});

suite.check('a stored timestamp shows its UTC date, whatever the offset it was sent with', () => {
	assertEqual(timestampToDueDate('2030-01-02T00:00:00Z'), '2030-01-02', 'start of day');
	assertEqual(timestampToDueDate('2030-01-02T23:59:59Z'), '2030-01-02', 'end of day');
	assertEqual(timestampToDueDate('2030-01-02T00:00:00+00:00'), '2030-01-02', 'the API form');
	assertEqual(timestampToDueDate('2030-01-03T01:00:00+05:00'), '2030-01-02', 'a positive offset');
	assertEqual(timestampToDueDate('2030-01-01T20:00:00-05:00'), '2030-01-02', 'a negative offset');
	assertEqual(timestampToDueDate(null), null, 'none');
	assertEqual(timestampToDueDate(undefined), null, 'absent');
	assertEqual(timestampToDueDate('not a date'), null, 'unreadable');
});

suite.check('edit then display never shifts the day, in any time zone', () => {
	const days = ['2030-01-01', '2030-03-31', '2030-10-27', '2030-12-31', '2028-02-29', '2031-01-01'];
	for (const day of days) {
		assertEqual(timestampToDueDate(dueDateToTimestamp(day)), day, `${day} round-trips`);
	}
});

finish(suite.result());
