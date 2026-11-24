import type { FormField, FormRecord } from '#lib/api/forms.js';
import type {
	DetailLayoutSection,
	ParsedViewFilter,
	ParsedViewFilterExpression,
	ParsedViewFilterGroup,
	ViewFilterOperator,
	ViewSortDirection
} from '#lib/forms/page-types.js';
import { detailLayoutSectionDescription } from '#lib/forms/detail-layout-drafts.js';
import {
	detailLayoutFieldKeys,
	detailSectionColumns as detailLayoutSectionColumns,
	detailSectionDensity as detailLayoutSectionDensity
} from '#lib/forms/detail-layout.js';
import { translate } from '#lib/forms/i18n.js';
import {
	compareRecordField,
	isRecord,
	recordMatchesViewFilterExpression
} from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';
import {
	viewConfigRecord,
	viewFilterLogic,
	viewFilterNodeDisabled,
	viewFilterOperator,
	viewSortDirection
} from '#lib/forms/view-config.js';

export class ViewRecordsController extends FormsController {
	viewFilterConfigFromRecord = (
		filter: Record<string, unknown> | null
	): {
		field: FormField;
		operator: ViewFilterOperator;
		value: string;
	} | null => {
		if (!filter || typeof filter.field !== 'string') return null;

		const field = this.fieldByKey(filter.field);

		if (!field) return null;

		return {
			field,
			operator: viewFilterOperator(filter.operator),
			value: typeof filter.value === 'string' ? filter.value : ''
		};
	};

	parsedViewFilterExpressionFromValue = (value: unknown): ParsedViewFilterExpression | null => {
		if (!isRecord(value)) return null;

		const filterRecord = isRecord(value.filter) ? value.filter : null;
		const filter = this.viewFilterConfigFromRecord(filterRecord);

		if (filter) {
			return {
				kind: 'condition',
				filter,
				disabled: viewFilterNodeDisabled(value) || viewFilterNodeDisabled(filterRecord)
			};
		}

		if (typeof value.field === 'string') {
			const inlineFilter = this.viewFilterConfigFromRecord(value);

			return inlineFilter
				? {
						kind: 'condition',
						filter: inlineFilter,
						disabled: viewFilterNodeDisabled(value)
					}
				: null;
		}

		const rawChildren = Array.isArray(value.children)
			? value.children
			: Array.isArray(value.expressions)
				? value.expressions
				: [];

		const children = rawChildren
			.map(this.parsedViewFilterExpressionFromValue)
			.filter((child): child is ParsedViewFilterExpression => Boolean(child));

		return children.length > 0
			? {
					kind: 'group',
					logic: viewFilterLogic(value.logic),
					children,
					disabled: viewFilterNodeDisabled(value)
				}
			: null;
	};

	viewFilterGroups = (): ParsedViewFilterGroup[] => {
		const config = this.s.selectedView?.config ?? {};
		const groups = Array.isArray(config.filter_groups) ? config.filter_groups : [];

		const parsedGroups = groups
			.map((group) => {
				if (!isRecord(group) || !Array.isArray(group.filters)) return null;

				const filters = group.filters
					.map((filter) => (isRecord(filter) ? this.viewFilterConfigFromRecord(filter) : null))
					.filter((filter): filter is ParsedViewFilter => Boolean(filter));

				return filters.length > 0 ? { logic: viewFilterLogic(group.logic), filters } : null;
			})
			.filter((group): group is ParsedViewFilterGroup => Boolean(group));

		return parsedGroups;
	};

	viewFilterExpression = (): ParsedViewFilterExpression | null => {
		const config = this.s.selectedView?.config ?? {};
		const expression = this.parsedViewFilterExpressionFromValue(config.filter_expression);

		if (expression) return expression;

		const groups = this.viewFilterGroups();

		if (groups.length === 0) return null;

		return {
			kind: 'group',
			logic: 'all',
			children: groups.map((group) => ({
				kind: 'group',
				logic: group.logic,
				children: group.filters.map((filter) => ({ kind: 'condition', filter }))
			}))
		};
	};

	viewSortConfig = (): { field: FormField; direction: ViewSortDirection } | null => {
		const sort = viewConfigRecord(this.s.selectedView, 'sort');

		if (!sort || typeof sort.field !== 'string') return null;

		const field = this.fieldByKey(sort.field);

		if (!field) return null;

		return { field, direction: viewSortDirection(sort.direction) };
	};

	fieldByKey = (fieldKey: string): FormField | null => {
		return this.s.fields.find((field) => field.key === fieldKey) ?? null;
	};

	detailLayoutSections = (): DetailLayoutSection[] => {
		const detailFields = this.s.fields.filter((field) => field.visibility?.detail !== false);
		const byKey = new Map(detailFields.map((field) => [field.key, field]));
		const usedFieldKeys = new Set<string>();
		const rawSections = Array.isArray(this.s.selectedForm?.detail_layout?.sections)
			? this.s.selectedForm.detail_layout.sections
			: [];

		const sections = rawSections
			.filter(isRecord)
			.map((section, index) => {
				const layoutFields = detailLayoutFieldKeys(section)
					.map((key) => byKey.get(key))
					.filter((field): field is FormField => Boolean(field));

				if (layoutFields.length === 0) return null;

				for (const field of layoutFields) usedFieldKeys.add(field.key);

				const key =
					typeof section.key === 'string' && section.key.trim().length > 0
						? section.key.trim()
						: `section_${index + 1}`;

				const title =
					typeof section.title === 'string' && section.title.trim().length > 0
						? section.title.trim()
						: typeof section.name === 'string' && section.name.trim().length > 0
							? section.name.trim()
							: translate('forms.detailSectionMain');

				return {
					key,
					title,
					description: detailLayoutSectionDescription(section.description),
					columns: detailLayoutSectionColumns(section.columns),
					density: detailLayoutSectionDensity(section.density),
					hideEmptyFields: section.hide_empty_fields === true,
					collapsible: section.collapsible === true,
					defaultCollapsed: section.collapsible === true && section.default_collapsed === true,
					fields: layoutFields
				};
			})
			.filter((section): section is DetailLayoutSection => Boolean(section));

		const unplacedFields = detailFields.filter((field) => !usedFieldKeys.has(field.key));

		if (sections.length > 0 && unplacedFields.length > 0) {
			sections.push({
				key: 'additional',
				title: translate('forms.detailSectionAdditional'),
				description: '',
				columns: 2,
				density: 'comfortable',
				hideEmptyFields: false,
				collapsible: false,
				defaultCollapsed: false,
				fields: unplacedFields
			});
		}

		if (sections.length > 0) return sections;

		return [
			{
				key: 'main',
				title: translate('forms.detailSectionMain'),
				description: '',
				columns: 2,
				density: 'comfortable',
				hideEmptyFields: false,
				collapsible: false,
				defaultCollapsed: false,
				fields: detailFields
			}
		];
	};

	applyViewRecordConfig = (sourceRecords: FormRecord[]): FormRecord[] => {
		let nextRecords = [...sourceRecords];
		const filterExpression = this.viewFilterExpression();

		if (filterExpression) {
			nextRecords = nextRecords.filter((record) => {
				const matches = recordMatchesViewFilterExpression(record, filterExpression);

				return matches !== false;
			});
		}

		if (this.s.focusedRecordIds.length > 0) {
			const focusedIds = new Set(this.s.focusedRecordIds);

			nextRecords = nextRecords.filter((record) => focusedIds.has(record.id));
		}

		const sort = this.viewSortConfig();

		if (sort) {
			nextRecords.sort((left, right) =>
				compareRecordField(left, right, sort.field, sort.direction)
			);
		}

		return nextRecords;
	};
}
