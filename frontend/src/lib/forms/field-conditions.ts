import type { FormField, FormRecord } from '#lib/api/forms.js';
import type {
	FieldConditionDraft,
	FieldConditionItem,
	FieldConditionOperator
} from '#lib/forms/page-types.js';
import { isRecord } from '#lib/forms/record-format.js';

export function fieldHasConditionalHidden(field: FormField): boolean {
	return Boolean(field.conditional?.hidden_when);
}

export function fieldHasConditionalReadOnly(field: FormField): boolean {
	return Boolean(field.conditional?.read_only_when);
}

export function recordFieldHiddenByCondition(field: FormField, record: FormRecord): boolean {
	return fieldHasConditionalHidden(field) && fieldHiddenByCondition(field, record.values);
}

export function recordFieldReadOnlyByCondition(field: FormField, record: FormRecord): boolean {
	return (
		fieldHasConditionalReadOnly(field) &&
		fieldConditionMatches(field.conditional?.read_only_when, record.values)
	);
}

export function recordsFieldHiddenByCondition(
	field: FormField | null,
	sourceRecords: FormRecord[]
): boolean {
	return Boolean(
		field &&
		fieldHasConditionalHidden(field) &&
		sourceRecords.some((record) => recordFieldHiddenByCondition(field, record))
	);
}

export function recordsFieldReadOnlyByCondition(
	field: FormField | null,
	sourceRecords: FormRecord[]
): boolean {
	return Boolean(
		field &&
		fieldHasConditionalReadOnly(field) &&
		sourceRecords.some((record) => recordFieldReadOnlyByCondition(field, record))
	);
}

export function normalizeFieldConditionOperator(value: string): FieldConditionOperator | null {
	const normalized = value.trim().toLowerCase();

	if (normalized === 'equals' || normalized === 'eq' || normalized === '==' || normalized === '=') {
		return 'equals';
	}

	if (
		normalized === 'not_equals' ||
		normalized === 'neq' ||
		normalized === '!=' ||
		normalized === '<>'
	) {
		return 'not_equals';
	}

	if (normalized === 'contains') return 'contains';
	if (normalized === 'not_contains' || normalized === 'not-contains') return 'not_contains';
	if (normalized === 'is_empty' || normalized === 'empty') return 'is_empty';
	if (normalized === 'not_empty' || normalized === 'exists') return 'not_empty';
	if (normalized === 'gt' || normalized === '>') return 'gt';
	if (normalized === 'gte' || normalized === '>=') return 'gte';
	if (normalized === 'lt' || normalized === '<') return 'lt';
	if (normalized === 'lte' || normalized === '<=') return 'lte';

	return null;
}

export function fieldConditionOperatorNeedsValue(operator: FieldConditionOperator): boolean {
	return operator !== 'is_empty' && operator !== 'not_empty';
}

export function parseFieldConditionLine(value: string): FieldConditionItem | null {
	const trimmed = value.trim();

	if (!trimmed) return null;

	const notEqualsIndex = trimmed.indexOf('!=');

	if (notEqualsIndex >= 0) {
		const field = trimmed.slice(0, notEqualsIndex).trim();
		const conditionValue = trimmed.slice(notEqualsIndex + 2).trim();

		return field && conditionValue
			? { field, operator: 'not_equals', value: conditionValue }
			: null;
	}

	const [operatorField, rawOperator, ...rawOperatorValueParts] = trimmed.split(/\s+/);
	const operator = normalizeFieldConditionOperator(rawOperator ?? '');

	if (operator && operatorField) {
		const conditionValue = rawOperatorValueParts.join(' ').trim();

		if (fieldConditionOperatorNeedsValue(operator) && !conditionValue) return null;

		return fieldConditionOperatorNeedsValue(operator)
			? { field: operatorField, operator, value: conditionValue }
			: { field: operatorField, operator };
	}

	const [rawField, ...rawValueParts] = trimmed.split('=');
	const field = rawField?.trim();
	const equals = rawValueParts.join('=').trim();

	return rawValueParts.length > 0 && field && equals ? { field, equals } : null;
}

export function parseFieldConditionText(value: string): FieldConditionDraft | null {
	const lines = value
		.split('\n')
		.map((line) => line.trim())
		.filter(Boolean);

	if (lines.length === 0) return null;

	const first = lines[0]?.toLowerCase();
	const hasLogicPrefix = first === 'all:' || first === 'any:';
	const logic = hasLogicPrefix ? (first.slice(0, -1) as 'all' | 'any') : 'all';
	const conditionLines = hasLogicPrefix ? lines.slice(1) : lines;
	const conditions = conditionLines
		.map(parseFieldConditionLine)
		.filter((item): item is FieldConditionItem => Boolean(item));

	if (conditions.length === 0) return null;
	if (!hasLogicPrefix && conditions.length === 1) return conditions[0];

	return { logic, conditions };
}

export function fieldConditionToText(value: unknown): string {
	if (!isRecord(value)) return '';

	if (Array.isArray(value.conditions)) {
		const logic = value.logic === 'any' ? 'any' : 'all';

		const lines = value.conditions
			.filter(isRecord)
			.map((condition) => {
				const field = typeof condition.field === 'string' ? condition.field.trim() : '';
				const equals = typeof condition.equals === 'string' ? condition.equals.trim() : '';

				const operator =
					typeof condition.operator === 'string'
						? (normalizeFieldConditionOperator(condition.operator) ?? 'equals')
						: 'equals';

				const conditionValue =
					typeof condition.value === 'string' ? condition.value.trim() : equals;

				if (!field || (fieldConditionOperatorNeedsValue(operator) && !conditionValue)) return '';
				if (operator === 'equals') return `${field}=${conditionValue}`;
				if (operator === 'not_equals') return `${field}!=${conditionValue}`;

				return fieldConditionOperatorNeedsValue(operator)
					? `${field} ${operator} ${conditionValue}`
					: `${field} ${operator}`;
			})
			.filter(Boolean);

		return lines.length > 0 ? [`${logic}:`, ...lines].join('\n') : '';
	}

	const field = typeof value.field === 'string' ? value.field.trim() : '';
	const equals = typeof value.equals === 'string' ? value.equals.trim() : '';

	const operator =
		typeof value.operator === 'string'
			? (normalizeFieldConditionOperator(value.operator) ?? 'equals')
			: 'equals';

	const conditionValue = typeof value.value === 'string' ? value.value.trim() : equals;

	if (!field || (fieldConditionOperatorNeedsValue(operator) && !conditionValue)) return '';
	if (operator === 'equals') return `${field}=${conditionValue}`;
	if (operator === 'not_equals') return `${field}!=${conditionValue}`;

	return fieldConditionOperatorNeedsValue(operator)
		? `${field} ${operator} ${conditionValue}`
		: `${field} ${operator}`;
}

export function draftConditionValue(draft: Record<string, unknown>, fieldKey: string): string {
	const value = draft[fieldKey];

	if (Array.isArray(value))
		return value.filter((item): item is string => typeof item === 'string').join(',');

	if (isRecord(value)) {
		if (typeof value.record_id === 'string') return value.record_id;
		if (typeof value.user_id === 'string') return value.user_id;
		if (typeof value.value === 'string' || typeof value.value === 'number')
			return String(value.value);
	}

	if (value === null || value === undefined) return '';

	return String(value);
}

export function fieldConditionMatches(condition: unknown, draft: Record<string, unknown>): boolean {
	if (!isRecord(condition)) return false;

	if (Array.isArray(condition.conditions)) {
		const checks = condition.conditions
			.filter(isRecord)
			.map((item) => fieldConditionMatches(item, draft));

		if (checks.length === 0) return false;

		return condition.logic === 'any' ? checks.some(Boolean) : checks.every(Boolean);
	}

	const field = typeof condition.field === 'string' ? condition.field.trim() : '';

	const operator =
		typeof condition.operator === 'string'
			? (normalizeFieldConditionOperator(condition.operator) ?? 'equals')
			: 'equals';

	const equals = typeof condition.equals === 'string' ? condition.equals.trim() : '';
	const conditionValue = typeof condition.value === 'string' ? condition.value.trim() : equals;

	if (!field || (fieldConditionOperatorNeedsValue(operator) && !conditionValue)) return false;

	const draftValue = draftConditionValue(draft, field);

	if (operator === 'equals') return draftValue === conditionValue;
	if (operator === 'not_equals') return draftValue !== conditionValue;
	if (operator === 'contains') return draftValue.includes(conditionValue);
	if (operator === 'not_contains') return !draftValue.includes(conditionValue);
	if (operator === 'is_empty') return !draftValue.trim();
	if (operator === 'not_empty') return Boolean(draftValue.trim());

	const actualNumber = Number(draftValue);
	const expectedNumber = Number(conditionValue);

	if (!Number.isFinite(actualNumber) || !Number.isFinite(expectedNumber)) return false;
	if (operator === 'gt') return actualNumber > expectedNumber;
	if (operator === 'gte') return actualNumber >= expectedNumber;
	if (operator === 'lt') return actualNumber < expectedNumber;
	if (operator === 'lte') return actualNumber <= expectedNumber;

	return false;
}

export function fieldHiddenByCondition(field: FormField, draft: Record<string, unknown>): boolean {
	return fieldConditionMatches(field.conditional?.hidden_when, draft);
}

export function fieldReadOnlyByCondition(
	field: FormField,
	draft: Record<string, unknown>
): boolean {
	return (
		field.policy?.read_only === true ||
		fieldConditionMatches(field.conditional?.read_only_when, draft)
	);
}
