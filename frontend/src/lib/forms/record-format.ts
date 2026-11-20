import type { BusinessEvent, FormField, FormRecord } from '#lib/api/forms.js';
import type {
	ParsedViewFilter,
	ParsedViewFilterExpression,
	ViewSortDirection
} from '#lib/forms/page-types.js';
import { translate, currentLocale } from '#lib/forms/i18n.js';
import { recordDateSource } from '#lib/forms/record-grouping.js';

export function formsReviewText(value: unknown): string {
	return typeof value === 'object' ? JSON.stringify(value, null, 2) : String(value ?? '');
}

export function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function recordMatchesViewFilterExpression(
	record: FormRecord,
	expression: ParsedViewFilterExpression
): boolean | null {
	if (expression.disabled) return null;

	if (expression.kind === 'condition') {
		return recordMatchesViewFilter(record, expression.filter);
	}

	const matches = expression.children
		.map((child) => recordMatchesViewFilterExpression(record, child))
		.filter((match): match is boolean => match !== null);

	if (matches.length === 0) return null;

	return expression.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
}

export function recordMatchesViewFilter(record: FormRecord, filter: ParsedViewFilter): boolean {
	const expected = filter.value.trim().toLowerCase();
	const actual = recordComparableText(record, filter.field);

	if (filter.operator === 'not_empty') return actual.length > 0;
	if (!expected) return true;
	if (filter.operator === 'equals') return actual.toLowerCase() === expected;
	if (filter.operator === 'not_equals') return actual.toLowerCase() !== expected;

	return actual.toLowerCase().includes(expected);
}

export function recordComparableText(record: FormRecord, field: FormField): string {
	const value = displayValue(record.values[field.key]);

	return value === '-' ? '' : value;
}

export function cardFieldHasDisplayValue(record: FormRecord, field: FormField): boolean {
	const value = displayValue(record.values[field.key]).trim();

	return value.length > 0 && value !== '-';
}

export function recordSortValue(record: FormRecord, field: FormField): string | number {
	const raw = record.values[field.key];
	const type = field.type ?? 'text';

	if (['amount', 'number', 'integer'].includes(type)) {
		if (typeof raw === 'number') return raw;

		if (typeof raw === 'string') {
			const parsed = Number(raw);

			return Number.isNaN(parsed) ? raw : parsed;
		}

		if (isRecord(raw)) {
			const value = typeof raw.decimal === 'string' ? raw.decimal : raw.value;

			if (typeof value === 'number') return value;

			if (typeof value === 'string') {
				const parsed = Number(value);

				return Number.isNaN(parsed) ? value : parsed;
			}
		}
	}

	if (['date', 'datetime'].includes(type)) {
		const time = new Date(recordDateSource(record, field)).getTime();

		return Number.isNaN(time) ? 0 : time;
	}

	return recordComparableText(record, field).toLowerCase();
}

export function compareRecordField(
	left: FormRecord,
	right: FormRecord,
	field: FormField,
	direction: ViewSortDirection
): number {
	const leftValue = recordSortValue(left, field);
	const rightValue = recordSortValue(right, field);
	const multiplier = direction === 'desc' ? -1 : 1;

	if (typeof leftValue === 'number' && typeof rightValue === 'number') {
		return (leftValue - rightValue) * multiplier;
	}

	return String(leftValue).localeCompare(String(rightValue)) * multiplier;
}

export function recordNumericValue(record: FormRecord, field: FormField | null): number {
	if (!field) return 1;

	const raw = record.values[field.key];

	if (typeof raw === 'number') return Number.isFinite(raw) ? raw : 0;

	if (typeof raw === 'string') {
		const parsed = Number(raw);

		return Number.isFinite(parsed) ? parsed : 0;
	}

	if (isRecord(raw)) {
		const value = typeof raw.decimal === 'string' ? raw.decimal : raw.value;

		if (typeof value === 'number') return Number.isFinite(value) ? value : 0;

		if (typeof value === 'string') {
			const parsed = Number(value);

			return Number.isFinite(parsed) ? parsed : 0;
		}
	}

	return 0;
}

export function recordAmountCurrency(record: FormRecord, field: FormField | null): string {
	if ((field?.type ?? 'text') !== 'amount') return '';

	const configured = field?.amount?.currency;

	if (configured) return configured;

	const raw = field ? record.values[field.key] : null;

	if (isRecord(raw) && typeof raw.currency === 'string') return raw.currency;

	return '';
}

export function recordDisplay(record: FormRecord, field: FormField | null): string {
	return field ? displayValue(record.values[field.key]) : '-';
}

export function eventValue(event: BusinessEvent, keys: string[]): string {
	for (const source of [event.payload, event.metadata, event.source]) {
		for (const key of keys) {
			const value = source[key];

			if (typeof value === 'string' && value.trim()) return value.trim();
			if (typeof value === 'number' || typeof value === 'boolean') return String(value);
		}
	}

	return '';
}

export function displayValue(value: unknown): string {
	if (value === null || value === undefined || value === '') return '-';
	if (typeof value === 'boolean') return value ? translate('forms.yes') : translate('forms.no');
	if (typeof value === 'string' || typeof value === 'number') return String(value);
	if (Array.isArray(value)) return value.map(displayValue).join(', ');

	if (isRecord(value)) {
		if (typeof value.display === 'string') return value.display;
		if (typeof value.name === 'string') return value.name;
		if (typeof value.email === 'string') return value.email;
		if (typeof value.title === 'string') return value.title;
		if (typeof value.decimal === 'string')
			return `${value.decimal}${typeof value.currency === 'string' ? ` ${value.currency}` : ''}`;
		if (typeof value.value === 'number') return String(value.value);

		return JSON.stringify(value);
	}

	return String(value);
}

export function formatDate(value: string): string {
	return new Date(value).toLocaleString(currentLocale() === 'en' ? 'en-US' : 'zh-CN', {
		year: 'numeric',
		month: '2-digit',
		day: '2-digit',
		hour: '2-digit',
		minute: '2-digit'
	});
}

export function inputValue(event: Event): string {
	return event.currentTarget instanceof HTMLInputElement ||
		event.currentTarget instanceof HTMLTextAreaElement
		? event.currentTarget.value
		: '';
}

export function selectValue(event: Event): string {
	return event.currentTarget instanceof HTMLSelectElement ? event.currentTarget.value : '';
}
