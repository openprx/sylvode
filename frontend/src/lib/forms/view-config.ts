import type { FormField, FormView, UniversalForm } from '#lib/api/forms.js';
import { isIsoDate as calendarIsIsoDate } from '#lib/ui/date/calendar.js';
import * as precedence from '#lib/forms/view-precedence.js';
import type {
	CalendarExceptionDateMap,
	CardCoverAspect,
	CardCoverFit,
	CardCoverPosition,
	CardFieldLimit,
	CardLayoutMode,
	GanttScale,
	PivotAggregate,
	PivotColumnLimit,
	PivotRowLimit,
	PivotSortMode,
	PivotTotalsMode,
	PivotValueDisplay,
	ViewCalendarDayScope,
	ViewCalendarLayout,
	ViewCalendarRange,
	ViewCalendarRecurrence,
	ViewFilterLogic,
	ViewFilterOperator,
	ViewSortDirection,
	ViewTimelineSource,
	ViewVisibility
} from '#lib/forms/page-types.js';
import { parseFields } from '#lib/forms/designer-schema.js';
import { optionArchived } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { isRecord } from '#lib/forms/record-format.js';

export function viewColumns(view: FormView | null): string[] {
	const columns = view?.config?.columns;

	return Array.isArray(columns)
		? columns.filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
		: [];
}

export function viewConfigStringArray(view: FormView | null, key: string): string[] {
	const value = view?.config?.[key];

	return Array.isArray(value)
		? value.filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
		: [];
}

export function orderedFieldsByKeys(baseFields: FormField[], fieldKeys: string[]): FormField[] {
	if (fieldKeys.length === 0) return baseFields;

	const byKey = new Map(baseFields.map((field) => [field.key, field]));
	const ordered = fieldKeys
		.map((key) => byKey.get(key))
		.filter((field): field is FormField => Boolean(field));
	const orderedKeys = new Set(ordered.map((field) => field.key));

	return [...ordered, ...baseFields.filter((field) => !orderedKeys.has(field.key))];
}

export function viewCardFieldKeys(view: FormView | null): string[] {
	return viewConfigStringArray(view, 'card_fields');
}

export function viewConfigString(view: FormView | null, key: string): string {
	const value = view?.config?.[key];

	return typeof value === 'string' ? value.trim() : '';
}

export function viewConfigBoolean(view: FormView | null, key: string): boolean {
	return view?.config?.[key] === true;
}

export function viewConfigRecord(
	view: FormView | null,
	key: string
): Record<string, unknown> | null {
	const value = view?.config?.[key];

	return isRecord(value) ? value : null;
}

export function isIsoDate(value: string): boolean {
	return calendarIsIsoDate(value);
}

export function calendarExceptionDateMap(value: unknown): CalendarExceptionDateMap {
	if (!isRecord(value)) return {};

	const next: CalendarExceptionDateMap = {};

	for (const [recordId, dates] of Object.entries(value)) {
		if (!recordId.trim() || !Array.isArray(dates)) continue;

		const uniqueDates = Array.from(
			new Set(
				dates
					.filter((date): date is string => typeof date === 'string' && isIsoDate(date.trim()))
					.map((date) => date.trim())
			)
		).sort();

		if (uniqueDates.length > 0) next[recordId] = uniqueDates;
	}

	return next;
}

export function viewIsDefault(view: FormView | null): boolean {
	return view?.config?.is_default === true;
}

export function viewVisibility(view: FormView | null): ViewVisibility {
	return view?.config?.visibility === 'private' ? 'private' : 'shared';
}

export function sortViewsByDefault(views: FormView[]): FormView[] {
	return [...views].sort((left, right) => {
		if (viewIsDefault(left) === viewIsDefault(right)) {
			return new Date(left.created_at).getTime() - new Date(right.created_at).getTime();
		}

		return viewIsDefault(left) ? -1 : 1;
	});
}

export function normalizeLocalDefaultViews(
	nextViews: FormView[],
	defaultViewId: string
): FormView[] {
	return sortViewsByDefault(
		nextViews.map((view) =>
			view.id === defaultViewId ? view : { ...view, config: { ...view.config, is_default: false } }
		)
	);
}

export function viewFilterOperator(value: unknown): ViewFilterOperator {
	return value === 'equals' || value === 'not_equals' || value === 'not_empty' ? value : 'contains';
}

export function viewFilterLogic(value: unknown): ViewFilterLogic {
	return value === 'any' || value === 'or' ? 'any' : 'all';
}

export function viewSortDirection(value: unknown): ViewSortDirection {
	return value === 'desc' ? 'desc' : 'asc';
}

export function viewCalendarRange(value: unknown): ViewCalendarRange {
	return value === 'month' ? 'month' : 'week';
}

export function viewCalendarDayScope(value: unknown): ViewCalendarDayScope {
	return value === 'workweek' ? 'workweek' : 'all';
}

export function viewCalendarLayout(value: unknown): ViewCalendarLayout {
	return value === 'agenda' ? 'agenda' : 'grid';
}

export function viewGanttScale(value: unknown): GanttScale {
	return value === 'week' || value === 'month' ? value : 'day';
}

export function viewPivotAggregate(value: unknown): PivotAggregate {
	if (value === 'count' || value === 'avg') return value;

	return 'sum';
}

export function viewPivotTotalsMode(value: unknown): PivotTotalsMode {
	if (value === 'rows' || value === 'columns' || value === 'none') return value;

	return 'all';
}

export function viewPivotValueDisplay(value: unknown): PivotValueDisplay {
	if (value === 'percent_row' || value === 'percent_column' || value === 'percent_total') {
		return value;
	}

	return 'value';
}

export function viewPivotSortMode(value: unknown): PivotSortMode {
	if (
		value === 'row_total_desc' ||
		value === 'row_total_asc' ||
		value === 'column_total_desc' ||
		value === 'column_total_asc'
	) {
		return value;
	}

	return 'label';
}

export function viewPivotRowLimit(value: unknown): PivotRowLimit {
	const numeric = typeof value === 'number' ? value : Number(value);

	if (numeric === 2 || numeric === 5 || numeric === 10) return numeric;

	return 0;
}

export function viewPivotColumnLimit(value: unknown): PivotColumnLimit {
	const numeric = typeof value === 'number' ? value : Number(value);

	if (numeric === 2 || numeric === 5 || numeric === 10) return numeric;

	return 0;
}

export function viewCalendarRecurrence(value: unknown): ViewCalendarRecurrence {
	if (value === 'daily' || value === 'weekly' || value === 'monthly') return value;

	return 'none';
}

export function viewCardLayout(value: unknown): CardLayoutMode {
	if (value === 'list') return 'list';
	if (value === 'compact') return 'compact';

	return value === 'gallery' ? 'gallery' : 'standard';
}

export function viewCardCoverAspect(value: unknown): CardCoverAspect {
	if (value === 'wide' || value === 'square' || value === 'portrait') return value;

	return 'auto';
}

export function viewCardCoverFit(value: unknown): CardCoverFit {
	return value === 'contain' ? 'contain' : 'cover';
}

export function viewCardCoverPosition(value: unknown): CardCoverPosition {
	if (value === 'left' || value === 'hidden') return value;

	return 'top';
}

export function cardCoverObjectClass(fit: CardCoverFit): string {
	return fit === 'contain' ? 'bg-surface object-contain' : 'object-cover';
}

export function cardCoverImageClass(
	layout: CardLayoutMode,
	aspect: CardCoverAspect,
	fit: CardCoverFit
): string {
	const resolvedAspect =
		aspect === 'auto'
			? layout === 'gallery'
				? 'aspect-[4/3]'
				: 'aspect-[16/9]'
			: aspect === 'square'
				? 'aspect-square'
				: aspect === 'portrait'
					? 'aspect-[3/4]'
					: 'aspect-[16/9]';

	return `${resolvedAspect} w-full ${cardCoverObjectClass(fit)}`;
}

export function viewCardFieldLimit(value: unknown): CardFieldLimit {
	const numeric = typeof value === 'number' ? value : Number(value);

	if (numeric === 3 || numeric === 9 || numeric === 99) return numeric;

	return 6;
}

export function viewTimelineSource(value: unknown): ViewTimelineSource {
	return value === 'events' ? 'events' : 'records';
}

export function viewFilterNodeDisabled(value: Record<string, unknown> | null): boolean {
	return value?.disabled === true || value?.enabled === false;
}

export function kanbanMoveOptions(field: FormField | null): string[] {
	if (!field || field.type !== 'single_select') return [];

	return Array.isArray(field.options)
		? field.options.filter((option) => option.trim() && !optionArchived(field, option))
		: [];
}

/** The built-in "All records" table: every list field, no filters, server order. */
export function builtInView(form: UniversalForm): FormView {
	const { id, workspace_id, project_id, created_at, updated_at } = form;
	const name = translate('forms.views.allRecords');
	const columns = parseFields(form.schema)
		.filter((field) => field.visibility?.list !== false)
		.map((field) => field.key);
	return {
		id: precedence.ALL_RECORDS_VIEW_ID,
		workspace_id,
		project_id,
		form_id: id,
		key: 'all',
		name,
		view_type: 'grid',
		config: { columns },
		created_at,
		updated_at
	};
}
