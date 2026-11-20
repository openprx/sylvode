import type { FormField, FormRecordImportRow } from '#lib/api/forms.js';
import { parseJsonish } from '#lib/forms/record-values.js';
import type { ImportMappingTransform } from '#lib/forms/page-types.js';
import { fieldLabel, fieldOptions, fieldType } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { isRecord } from '#lib/forms/record-format.js';

export function csvCell(value: string): string {
	return `"${value.replaceAll('"', '""')}"`;
}

export function sampleImportValue(field: FormField): string {
	const type = fieldType(field);

	if (type === 'boolean') return 'true';
	if (type === 'integer') return '1';
	if (type === 'number' || type === 'amount') return '1.00';
	if (type === 'single_select') return fieldOptions(field)[0] ?? '';
	if (type === 'multi_select') return (fieldOptions(field)[0] ?? '').replaceAll(',', ' ');

	return fieldLabel(field);
}

export function isValidImportMappingTransform(value: string): value is ImportMappingTransform {
	return [
		'none',
		'trim',
		'uppercase',
		'lowercase',
		'capitalize',
		'titlecase',
		'normalize_spaces',
		'strip_non_digits',
		'slugify'
	].includes(value);
}

export function applyImportMappingTransform(
	value: string,
	transform: ImportMappingTransform
): string {
	if (transform === 'none') return value;

	const trimmed = value.trim();

	if (transform === 'uppercase') return trimmed.toUpperCase();
	if (transform === 'lowercase') return trimmed.toLowerCase();
	if (transform === 'capitalize') return capitalizeImportValue(trimmed);
	if (transform === 'titlecase')
		return trimmed.split(/\s+/).filter(Boolean).map(capitalizeImportValue).join(' ');
	if (transform === 'normalize_spaces') return trimmed.split(/\s+/).filter(Boolean).join(' ');
	if (transform === 'strip_non_digits') return trimmed.replace(/\D+/g, '');
	if (transform === 'slugify') return slugifyImportValue(trimmed);

	return trimmed;
}

export function capitalizeImportValue(value: string): string {
	if (!value) return '';

	return `${value.slice(0, 1).toUpperCase()}${value.slice(1).toLowerCase()}`;
}

export function slugifyImportValue(value: string): string {
	return value
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '');
}

export function normalizeImportItem(item: unknown, rowNumber: number): FormRecordImportRow {
	if (!isRecord(item))
		throw new Error(translate('forms.importInvalidRow', { values: { row: rowNumber } }));

	const values = isRecord(item.values) ? item.values : item;
	const title = typeof item.title === 'string' ? item.title : undefined;

	return {
		row_number: rowNumber,
		title,
		values: values as Record<string, unknown>,
		source: { type: 'web', origin: 'import' }
	};
}

export function parseCsvLines(text: string): string[][] {
	const rows: string[][] = [];
	let row: string[] = [];
	let cell = '';
	let quoted = false;

	for (let index = 0; index < text.length; index += 1) {
		const char = text[index];
		const next = text[index + 1];

		if (quoted) {
			if (char === '"' && next === '"') {
				cell += '"';
				index += 1;
			} else if (char === '"') {
				quoted = false;
			} else {
				cell += char;
			}

			continue;
		}

		if (char === '"') {
			quoted = true;
		} else if (char === ',') {
			row.push(cell);
			cell = '';
		} else if (char === '\n') {
			row.push(cell);
			rows.push(row);
			row = [];
			cell = '';
		} else if (char !== '\r') {
			cell += char;
		}
	}

	row.push(cell);
	rows.push(row);

	return rows;
}

export function importCellValue(field: FormField, raw: string): unknown {
	const trimmed = raw.trim();

	if (!trimmed) return undefined;

	const type = fieldType(field);

	if (type === 'boolean') return ['true', '1', 'yes', 'y', '是'].includes(trimmed.toLowerCase());

	if (type === 'multi_select') {
		return trimmed
			.split(/[;|]/)
			.map((item) => item.trim())
			.filter(Boolean);
	}

	if (['attachment', 'image', 'relation', 'child_table', 'formula'].includes(type)) {
		return parseJsonish(trimmed);
	}

	return trimmed;
}
