import type { FormField, FormRecord } from '#lib/api/forms.js';
import { translate } from '#lib/forms/i18n.js';
import { isRecord, recordDisplay } from '#lib/forms/record-format.js';

export function recordGroupLabel(record: FormRecord, field: FormField | null): string {
	const value = recordDisplay(record, field);

	return value && value !== '-' ? value : translate('forms.unspecified');
}

export function recordDateSource(record: FormRecord, field: FormField | null): string {
	const value = field ? record.values[field.key] : null;

	if (typeof value === 'string' && value.trim()) return value;

	if (isRecord(value)) {
		if (typeof value.value === 'string' && value.value.trim()) return value.value;
		if (typeof value.date === 'string' && value.date.trim()) return value.date;
		if (typeof value.datetime === 'string' && value.datetime.trim()) return value.datetime;
	}

	return record.updated_at || record.created_at;
}

export function groupRecordsByField(
	sourceRecords: FormRecord[],
	field: FormField | null
): Array<{ key: string; label: string; records: FormRecord[] }> {
	const groups = new Map<string, { key: string; label: string; records: FormRecord[] }>();

	if (field?.type === 'single_select' && Array.isArray(field.options)) {
		for (const option of field.options) {
			const label = option.trim();

			if (label) groups.set(label.toLowerCase(), { key: label.toLowerCase(), label, records: [] });
		}
	}

	for (const record of sourceRecords) {
		const label = recordGroupLabel(record, field);
		const key = label.toLowerCase();
		const existing = groups.get(key);

		if (existing) {
			existing.records.push(record);
		} else {
			groups.set(key, { key, label, records: [record] });
		}
	}

	return Array.from(groups.values()).sort((left, right) => left.label.localeCompare(right.label));
}

export function recordsByUpdatedAt(sourceRecords: FormRecord[]): FormRecord[] {
	return [...sourceRecords].sort(
		(left, right) => new Date(right.updated_at).getTime() - new Date(left.updated_at).getTime()
	);
}
