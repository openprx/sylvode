import type { FormField, FormRecord } from '#lib/api/forms.js';
import { detailPageWidgetKeys, detailPageWidgetOptions } from '#lib/forms/page-constants.js';
import type {
	DetailLayoutSection,
	DetailLayoutSectionDensity,
	DetailPageWidgetEntry,
	DetailPageWidgetKey,
	DetailPageWidgetRegion
} from '#lib/forms/page-types.js';
import { recordValueIsEmpty } from '#lib/forms/detail-layout.js';
import { fieldType } from '#lib/forms/field-options.js';
import { isRecord } from '#lib/forms/record-format.js';

export function detailPageWidgetKey(value: unknown): DetailPageWidgetKey | null {
	return typeof value === 'string' && detailPageWidgetKeys.has(value as DetailPageWidgetKey)
		? (value as DetailPageWidgetKey)
		: null;
}

export function detailPageWidgetEntriesFromLayout(
	layout: Record<string, unknown> | null | undefined
): DetailPageWidgetEntry[] {
	const entries: DetailPageWidgetEntry[] = [];
	const seen = new Set<DetailPageWidgetKey>();
	const rawWidgets = isRecord(layout) && Array.isArray(layout.widgets) ? layout.widgets : [];

	for (const rawWidget of rawWidgets) {
		const key = isRecord(rawWidget)
			? detailPageWidgetKey(rawWidget.key)
			: detailPageWidgetKey(rawWidget);

		if (!key || seen.has(key)) continue;

		entries.push({
			key,
			enabled: isRecord(rawWidget) ? rawWidget.enabled !== false : true,
			title: isRecord(rawWidget) && typeof rawWidget.title === 'string' ? rawWidget.title : '',
			description:
				isRecord(rawWidget) && typeof rawWidget.description === 'string'
					? rawWidget.description
					: '',
			emptyText:
				isRecord(rawWidget) && typeof rawWidget.empty_text === 'string' ? rawWidget.empty_text : '',
			showCount: isRecord(rawWidget) ? rawWidget.show_count === true : false,
			hideEmpty: isRecord(rawWidget) ? rawWidget.hide_empty === true : false,
			collapsible: isRecord(rawWidget) ? rawWidget.collapsible === true : false,
			defaultCollapsed:
				isRecord(rawWidget) && rawWidget.collapsible === true
					? rawWidget.default_collapsed === true
					: false,
			density: isRecord(rawWidget) && rawWidget.density === 'compact' ? 'compact' : 'comfortable',
			limit: isRecord(rawWidget) ? detailPageWidgetLimitText(rawWidget.limit) : ''
		});

		seen.add(key);
	}

	for (const option of detailPageWidgetOptions) {
		if (!seen.has(option.key)) {
			entries.push({
				key: option.key,
				enabled: true,
				title: '',
				description: '',
				emptyText: '',
				showCount: false,
				hideEmpty: false,
				collapsible: false,
				defaultCollapsed: false,
				density: 'comfortable',
				limit: ''
			});
		}
	}

	return entries;
}

export function detailPageWidgetOption(key: DetailPageWidgetKey) {
	return detailPageWidgetOptions.find((option) => option.key === key) ?? detailPageWidgetOptions[0];
}

export function detailPageWidgetPayload(entries: DetailPageWidgetEntry[]) {
	return entries.map((entry) => ({
		key: entry.key,
		enabled: entry.enabled,
		...(entry.title.trim() ? { title: entry.title.trim() } : {}),
		...(entry.description.trim() ? { description: entry.description.trim() } : {}),
		...(entry.emptyText.trim() ? { empty_text: entry.emptyText.trim() } : {}),
		...(entry.showCount ? { show_count: true } : {}),
		...(entry.hideEmpty ? { hide_empty: true } : {}),
		...(entry.collapsible ? { collapsible: true } : {}),
		...(entry.collapsible && entry.defaultCollapsed ? { default_collapsed: true } : {}),
		...(entry.density === 'compact' ? { density: 'compact' } : {}),
		...(detailPageWidgetLimitNumber(entry.limit)
			? { limit: detailPageWidgetLimitNumber(entry.limit) }
			: {})
	}));
}

export function detailPageWidgetLimitNumber(value: unknown): number | null {
	const numericValue =
		typeof value === 'number' ? value : typeof value === 'string' ? Number(value) : NaN;

	if (!Number.isFinite(numericValue)) return null;

	const limit = Math.trunc(numericValue);

	if (limit < 1 || limit > 50) return null;

	return limit;
}

export function detailPageWidgetLimitText(value: unknown): string {
	const limit = detailPageWidgetLimitNumber(value);

	return limit ? String(limit) : '';
}

export function detailPageWidgetRegion(value: unknown): DetailPageWidgetRegion {
	return value === 'below' ? 'below' : 'sidebar';
}

export function detailPageFieldColumns(value: unknown): 1 | 2 {
	const numericValue =
		typeof value === 'number' ? value : typeof value === 'string' ? Number(value) : NaN;

	return numericValue === 1 ? 1 : 2;
}

export function detailLayoutSectionDescription(value: unknown): string {
	return typeof value === 'string' ? value.trim() : '';
}

export function keepDetailSectionOpen(event: Event, collapsible: boolean) {
	if (collapsible) return;

	const details = event.currentTarget;

	if (details instanceof HTMLDetailsElement && !details.open) details.open = true;
}

export function detailLayoutSectionGridClass(
	columns: 1 | 2 | 3,
	density: DetailLayoutSectionDensity = 'comfortable'
): string {
	const gap = density === 'compact' ? 'gap-2' : 'gap-3';

	if (columns === 1) return `grid ${gap}`;
	if (columns === 3) return `grid ${gap} sm:grid-cols-2 xl:grid-cols-3`;

	return `grid ${gap} sm:grid-cols-2`;
}

export function detailLayoutSectionFieldClass(field: FormField, columns: 1 | 2 | 3): string {
	if (fieldType(field) !== 'child_table' || columns === 1) return 'min-w-0';

	return columns === 3 ? 'min-w-0 sm:col-span-2 xl:col-span-3' : 'min-w-0 sm:col-span-2';
}

export function detailLayoutSectionFieldsForRecord(
	section: DetailLayoutSection,
	record: FormRecord
): FormField[] {
	if (!section.hideEmptyFields) return section.fields;

	return section.fields.filter((field) => !recordValueIsEmpty(record.values[field.key]));
}
