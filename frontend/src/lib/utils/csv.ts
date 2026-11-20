/**
 * CSV cell encoding for downloads and clipboard copies.
 *
 * Spreadsheet programs evaluate a cell that starts with `=`, `+`, `-`, `@`, tab or carriage return
 * as a formula, so record content exported from a shared form could run in another member's
 * spreadsheet. Such cells get a leading `'`. The server export (`csv_escape`) uses the same rule.
 *
 * Numeric exception: in a numeric column, a cell that is a plain number (optionally followed by a
 * three-letter uppercase currency code, as amounts display) keeps its sign, since `-5` there is a
 * number and not a formula. Every other cell, including `-5` in a text column, is neutralised.
 */

const FORMULA_START = /^[=+\-@\t\r]/;
const PLAIN_NUMBER = /^-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?(?: [A-Z]{3})?$/;

/** Field types whose values are numbers (mirrors `is_numeric_field_type` in the API). */
export const NUMERIC_FIELD_TYPES: ReadonlySet<string> = new Set([
	'number',
	'integer',
	'amount',
	'rating',
	'progress'
]);

export function isPlainNumber(value: string): boolean {
	return PLAIN_NUMBER.test(value);
}

/** Prefixes `'` when the cell would be read as a formula. */
export function neutralizeCsvFormula(value: string, numeric = false): string {
	if (!FORMULA_START.test(value)) return value;
	return numeric && isPlainNumber(value) ? value : `'${value}`;
}

/** Neutralises, then quotes. Always quotes, like the page's import-side encoder. */
export function csvExportCell(value: string, numeric = false): string {
	return `"${neutralizeCsvFormula(value, numeric).replaceAll('"', '""')}"`;
}
