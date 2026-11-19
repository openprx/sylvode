/**
 * The record detail layout admins configure in "Detail layout": sections, header fields and
 * highlights. Shared by the record side panel, the forms page and the full record page so the
 * three show a record the same way.
 */

import type { FormField, FormRecord, UniversalForm } from '#lib/api/forms.js';

export type DetailSectionDensity = 'comfortable' | 'compact';

export interface DetailSection {
	key: string;
	title: string;
	description: string;
	columns: 1 | 2 | 3;
	density: DetailSectionDensity;
	hideEmptyFields: boolean;
	collapsible: boolean;
	defaultCollapsed: boolean;
	fields: FormField[];
}

export interface DetailHeaderFields {
	badge: FormField | null;
	subtitle: FormField | null;
	highlights: FormField[];
}

type LayoutSource = Pick<UniversalForm, 'detail_layout'> | null | undefined;

function isRecord(input: unknown): input is Record<string, unknown> {
	return typeof input === 'object' && input !== null && !Array.isArray(input);
}

export function detailLayoutFieldKeys(section: Record<string, unknown>): string[] {
	const rawFields = Array.isArray(section.fields) ? section.fields : [];
	return rawFields
		.map((field) => {
			if (typeof field === 'string') return field;
			if (isRecord(field) && typeof field.key === 'string') return field.key;
			return '';
		})
		.filter((key) => key.trim().length > 0);
}

export function detailSectionColumns(value: unknown, fallback: 1 | 2 | 3 = 2): 1 | 2 | 3 {
	const numeric =
		typeof value === 'number' ? value : typeof value === 'string' ? Number(value) : NaN;
	return numeric === 1 || numeric === 2 || numeric === 3 ? numeric : fallback;
}

export function detailSectionDensity(value: unknown): DetailSectionDensity {
	return value === 'compact' ? 'compact' : 'comfortable';
}

function trimmedString(value: unknown): string {
	return typeof value === 'string' ? value.trim() : '';
}

/** Fields shown on a record detail: everything not hidden from the detail view. */
export function detailFields(fields: FormField[]): FormField[] {
	return fields.filter((field) => field.visibility?.detail !== false);
}

/**
 * Sections in configured order. Fields not placed in any section go to a final section titled
 * `labels.other`; with no usable section at all, every detail field goes into one `labels.main`.
 */
export function buildDetailSections(
	form: LayoutSource,
	fields: FormField[],
	labels: { main: string; other: string },
	defaultColumns: 1 | 2 | 3 = 2
): DetailSection[] {
	const visible = detailFields(fields);
	const byKey = new Map(visible.map((field) => [field.key, field]));
	const used = new Set<string>();
	const rawSections = Array.isArray(form?.detail_layout?.sections)
		? form.detail_layout.sections
		: [];
	const sections: DetailSection[] = [];

	rawSections.filter(isRecord).forEach((section, index) => {
		const sectionFields = detailLayoutFieldKeys(section)
			.map((key) => byKey.get(key))
			.filter((field): field is FormField => Boolean(field));
		if (sectionFields.length === 0) return;
		for (const field of sectionFields) used.add(field.key);
		sections.push({
			key: trimmedString(section.key) || `section_${index + 1}`,
			title: trimmedString(section.title) || trimmedString(section.name) || labels.main,
			description: trimmedString(section.description),
			columns: detailSectionColumns(section.columns, defaultColumns),
			density: detailSectionDensity(section.density),
			hideEmptyFields: section.hide_empty_fields === true,
			collapsible: section.collapsible === true,
			defaultCollapsed: section.collapsible === true && section.default_collapsed === true,
			fields: sectionFields
		});
	});

	const unplaced = visible.filter((field) => !used.has(field.key));
	const base = {
		description: '',
		columns: defaultColumns,
		density: 'comfortable' as const,
		hideEmptyFields: false,
		collapsible: false,
		defaultCollapsed: false
	};
	if (sections.length > 0) {
		if (unplaced.length > 0) {
			sections.push({ ...base, key: 'additional', title: labels.other, fields: unplaced });
		}
		return sections;
	}
	return [{ ...base, key: 'main', title: labels.main, fields: visible }];
}

/** True when a stored value carries nothing to show. */
export function recordValueIsEmpty(value: unknown): boolean {
	if (value === null || value === undefined) return true;
	if (typeof value === 'string') return value.trim().length === 0;
	if (Array.isArray(value)) return value.length === 0 || value.every(recordValueIsEmpty);
	if (isRecord(value)) {
		if (typeof value.display === 'string') return value.display.trim().length === 0;
		if (typeof value.name === 'string') return value.name.trim().length === 0;
		if (typeof value.email === 'string') return value.email.trim().length === 0;
		if (typeof value.title === 'string') return value.title.trim().length === 0;
		if (typeof value.decimal === 'string') return value.decimal.trim().length === 0;
		if ('value' in value) return recordValueIsEmpty(value.value);
		return Object.keys(value).length === 0;
	}
	return false;
}

/** The fields a section shows for one record: minus fields hidden by a condition, minus empty
 * fields when the section hides them. */
export function sectionFieldsForRecord(
	section: DetailSection,
	record: FormRecord,
	isHiddenByCondition: (field: FormField, record: FormRecord) => boolean = () => false,
	isEmpty: (field: FormField, value: unknown) => boolean = (_field, value) =>
		recordValueIsEmpty(value)
): FormField[] {
	return section.fields.filter((field) => {
		if (isHiddenByCondition(field, record)) return false;
		return !section.hideEmptyFields || !isEmpty(field, record.values?.[field.key]);
	});
}

/** Header badge and subtitle fields and up to four highlights, as configured. */
export function headerFields(form: LayoutSource, fields: FormField[]): DetailHeaderFields {
	const layout = isRecord(form?.detail_layout) ? form.detail_layout : {};
	const header = isRecord(layout.header) ? layout.header : {};
	const visible = detailFields(fields);
	const pick = (key: 'badge_field' | 'subtitle_field') => {
		const fieldKey = trimmedString(header[key]);
		return fieldKey ? (visible.find((field) => field.key === fieldKey) ?? null) : null;
	};
	const highlights: FormField[] = [];
	const seen = new Set<string>();
	for (const raw of Array.isArray(layout.highlights) ? layout.highlights : []) {
		if (typeof raw !== 'string' || seen.has(raw)) continue;
		const field = visible.find((item) => item.key === raw);
		if (!field) continue;
		highlights.push(field);
		seen.add(raw);
		if (highlights.length >= 4) break;
	}
	return { badge: pick('badge_field'), subtitle: pick('subtitle_field'), highlights };
}
