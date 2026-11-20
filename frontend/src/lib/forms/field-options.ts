import type { FormField, UniversalForm } from '#lib/api/forms.js';
import { parseFields } from '#lib/forms/designer-schema.js';
import { translate } from '#lib/forms/i18n.js';

export function fieldLabel(field: FormField): string {
	return field.label?.trim() || field.key.replaceAll('_', ' ');
}

export function fieldType(field: FormField): string {
	return field.type ?? 'text';
}

export function fieldOptions(field: FormField): string[] {
	return field.options ?? [];
}

export function fieldDisabledOptions(field: FormField): string[] {
	return field.option_disabled ?? [];
}

export function fieldArchivedOptions(field: FormField): string[] {
	return field.option_archived ?? [];
}

export function optionDisabled(field: FormField, option: string): boolean {
	return fieldDisabledOptions(field).includes(option);
}

export function optionArchived(field: FormField, option: string): boolean {
	return fieldArchivedOptions(field).includes(option);
}

export function recordSelectOptions(field: FormField, draft: Record<string, unknown>): string[] {
	const selected = new Set<string>();
	const value = draft[field.key];

	if (typeof value === 'string') selected.add(value);

	if (Array.isArray(value)) {
		for (const item of value) {
			if (typeof item === 'string') selected.add(item);
		}
	}

	return fieldOptions(field).filter(
		(option) => !optionArchived(field, option) || selected.has(option)
	);
}

export function optionDescription(field: FormField, option: string): string {
	return (
		field.option_descriptions?.[option]?.trim() ??
		field.option_descriptions?.[option.trim()]?.trim() ??
		''
	);
}

export function optionGroup(field: FormField, option: string): string {
	return (
		field.option_groups?.[option]?.trim() ?? field.option_groups?.[option.trim()]?.trim() ?? ''
	);
}

export function optionDisplayLabel(field: FormField, option: string): string {
	const group = optionGroup(field, option);

	return group ? `${group} / ${option}` : option;
}

export function sanitizeOptionColor(value: unknown): string {
	if (typeof value !== 'string') return '';

	const trimmed = value.trim();

	if (/^#[0-9a-f]{6}$/i.test(trimmed) || /^#[0-9a-f]{3}$/i.test(trimmed)) return trimmed;

	return '';
}

export function fieldPlaceholder(field: FormField): string {
	if (field.placeholder) return field.placeholder;
	if (fieldType(field) === 'amount') return field.amount?.currency ?? 'CNY';
	if (fieldType(field) === 'multi_select') return fieldOptions(field).join(', ');

	return '';
}

export function formListFields(form: UniversalForm): FormField[] {
	return parseFields(form.schema).filter((field) => field.visibility?.list !== false);
}

export function formFieldSummary(form: UniversalForm): string {
	const labels = formListFields(form)
		.slice(0, 3)
		.map((field) => fieldLabel(field));

	return labels.length > 0 ? labels.join(' / ') : translate('forms.noFields');
}
