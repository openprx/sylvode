import type { FormRecord, FormView } from '#lib/api/forms.js';
import type {
	CalendarExceptionDateMap,
	ViewFilterDraft,
	ViewFilterGroupDraft,
	ViewFilterLogic,
	ViewFilterOperator
} from '#lib/forms/page-types.js';
import { isRecord } from '#lib/forms/record-format.js';
import {
	calendarExceptionDateMap,
	viewFilterLogic,
	viewFilterNodeDisabled,
	viewFilterOperator
} from '#lib/forms/view-config.js';

export function viewDraftFilterExpressionRecordIdsText(records: FormRecord[] | null): string {
	return records?.map((record) => record.id).join('\n') ?? '';
}

export function filterOperatorSummary(operator: ViewFilterOperator): string {
	if (operator === 'equals') return 'equals';
	if (operator === 'not_equals') return 'does not equal';
	if (operator === 'not_empty') return 'is not empty';

	return 'contains';
}

export function parseViewFilterDrafts(rawFilters: unknown[]): ViewFilterDraft[] {
	return rawFilters
		.map((filter): ViewFilterDraft | null => {
			if (!isRecord(filter) || typeof filter.field !== 'string') return null;

			return {
				field: filter.field,
				operator: viewFilterOperator(filter.operator),
				value: typeof filter.value === 'string' ? filter.value : '',
				disabled: viewFilterNodeDisabled(filter) ? true : undefined
			};
		})
		.filter((filter): filter is ViewFilterDraft => Boolean(filter))
		.slice(0, 5);
}

export function viewDraftExpressionFromView(view: FormView): Record<string, unknown> | null {
	return isRecord(view.config?.filter_expression) ? view.config.filter_expression : null;
}

export function viewDraftFilterGroupsFromExpression(
	expression: Record<string, unknown>
): ViewFilterGroupDraft[] {
	const rawChildren = Array.isArray(expression.children)
		? expression.children
		: Array.isArray(expression.expressions)
			? expression.expressions
			: [];

	const groups = rawChildren
		.map((child): ViewFilterGroupDraft | null => {
			if (!isRecord(child)) return null;

			if (typeof child.field === 'string' || isRecord(child.filter)) {
				const filter = parseViewFilterDrafts([isRecord(child.filter) ? child.filter : child])[0];

				if (filter && viewFilterNodeDisabled(child)) filter.disabled = true;

				return filter
					? {
							logic: 'all' as ViewFilterLogic,
							filters: [filter],
							disabled: viewFilterNodeDisabled(child) ? true : undefined
						}
					: null;
			}

			const childFilters = Array.isArray(child.children)
				? child.children
						.map((item) => {
							if (!isRecord(item)) return null;

							const filter = isRecord(item.filter) ? item.filter : item;

							if (viewFilterNodeDisabled(item) && !viewFilterNodeDisabled(filter)) {
								return { ...filter, disabled: true };
							}

							return filter;
						})
						.filter(isRecord)
				: [];

			const filters = parseViewFilterDrafts(childFilters);

			return filters.length > 0
				? {
						logic: viewFilterLogic(child.logic),
						filters,
						disabled: viewFilterNodeDisabled(child) ? true : undefined
					}
				: null;
		})
		.filter((group): group is ViewFilterGroupDraft => Boolean(group))
		.slice(0, 5);

	if (groups.length > 0) return groups;

	const filter = isRecord(expression.filter) ? expression.filter : expression;
	const filters = parseViewFilterDrafts([filter]);

	return filters.length > 0 ? [{ logic: 'all', filters }] : [];
}

export function viewDraftFilterRootLogicFromView(view: FormView): ViewFilterLogic {
	const expression = viewDraftExpressionFromView(view);

	return expression ? viewFilterLogic(expression.logic) : 'all';
}

export function viewDraftWipLimitsFromView(view: FormView): Record<string, string> {
	const raw = isRecord(view.config?.wip_limits) ? view.config.wip_limits : {};
	const limits: Record<string, string> = {};

	for (const [key, value] of Object.entries(raw)) {
		const limit = typeof value === 'number' ? value : Number(value);

		if (Number.isFinite(limit) && limit >= 0) {
			limits[key] = String(Math.floor(limit));
		}
	}

	return limits;
}

export function viewDraftCalendarExceptionDatesFromView(view: FormView): CalendarExceptionDateMap {
	return calendarExceptionDateMap(view.config?.calendar_exception_dates);
}

export function viewDraftFilterGroupsFromView(view: FormView): ViewFilterGroupDraft[] {
	const expression = viewDraftExpressionFromView(view);

	if (expression) {
		const groups = viewDraftFilterGroupsFromExpression(expression);

		if (groups.length > 0) return groups;
	}

	const rawGroups = Array.isArray(view.config?.filter_groups) ? view.config.filter_groups : [];

	const groups = rawGroups
		.map((group): ViewFilterGroupDraft | null => {
			if (!isRecord(group) || !Array.isArray(group.filters)) return null;

			const filters = parseViewFilterDrafts(group.filters);

			return filters.length > 0
				? {
						logic: viewFilterLogic(group.logic),
						filters,
						disabled: viewFilterNodeDisabled(group) ? true : undefined
					}
				: null;
		})
		.filter((group): group is ViewFilterGroupDraft => Boolean(group))
		.slice(0, 5);

	return groups;
}

export function filterLabelSuffix(groupIndex: number, filterIndex: number): string {
	return groupIndex === 0 ? String(filterIndex + 1) : `${groupIndex + 1}.${filterIndex + 1}`;
}
