// A work item's due date is a calendar date, not an instant. The date picker works in
// `YYYY-MM-DD`; the API stores an RFC 3339 timestamp. The two are tied together by one rule: the
// date is stored as midnight UTC of that day and read back from the UTC date parts. Reading the
// UTC parts (never the viewer's local ones) is what makes a date chosen in one time zone show as
// the same day in every other, and a value saved and loaded again never moves by a day.

const DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

/** The stored timestamp of a chosen date, or `null` for none (which clears the due date). */
export function dueDateToTimestamp(date: string | null | undefined): string | null {
	if (!date) return null;
	const match = DATE.exec(date);
	if (!match) return null;
	const [, year, month, day] = match;
	const parsed = new Date(Date.UTC(Number(year), Number(month) - 1, Number(day)));
	// `2026-02-30` rolls over to March: that is not a date.
	if (
		parsed.getUTCFullYear() !== Number(year) ||
		parsed.getUTCMonth() !== Number(month) - 1 ||
		parsed.getUTCDate() !== Number(day)
	) {
		return null;
	}
	return `${date}T00:00:00Z`;
}

/** The date to show for a stored timestamp, or `null` for none or an unreadable value. */
export function timestampToDueDate(timestamp: string | null | undefined): string | null {
	if (!timestamp) return null;
	const parsed = new Date(timestamp);
	if (Number.isNaN(parsed.getTime())) return null;
	const year = String(parsed.getUTCFullYear()).padStart(4, '0');
	const month = String(parsed.getUTCMonth() + 1).padStart(2, '0');
	const day = String(parsed.getUTCDate()).padStart(2, '0');
	return `${year}-${month}-${day}`;
}
