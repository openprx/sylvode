import type { FormField, FormRecord } from '#lib/api/forms.js';
import { draftValueFromRecordValue, emptyDraftValue } from '#lib/forms/record-values.js';

export function defaultDraftForFields(formFields: FormField[]): Record<string, unknown> {
	return Object.fromEntries(
		formFields.map((field) => [field.key, defaultDraftValueForField(field)])
	);
}

export function defaultDraftValueForField(field: FormField): unknown {
	if (field.default_value !== undefined && field.default_value !== null) {
		return draftValueFromRecordValue(field, field.default_value);
	}

	return emptyDraftValue(field);
}

export function draftValueForField(field: FormField, record: FormRecord): unknown {
	return draftValueFromRecordValue(field, record.values[field.key]);
}

export function draftArrayFrom(draft: Record<string, unknown>, fieldKey: string): string[] {
	const value = draft[fieldKey];

	return Array.isArray(value)
		? value.filter((item): item is string => typeof item === 'string')
		: [];
}

export function draftStringFrom(draft: Record<string, unknown>, fieldKey: string): string {
	const value = draft[fieldKey];

	return typeof value === 'string' ? value : '';
}

export function childRelationKeyForField(field: FormField): string {
	return field.relation?.relation_key || field.key;
}
