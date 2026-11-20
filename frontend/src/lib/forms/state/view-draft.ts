import type { FormField, FormView } from '#lib/api/forms.js';
import { FormsController } from '#lib/forms/state/controller.js';
import {
	isIsoDate,
	orderedFieldsByKeys,
	viewCalendarDayScope,
	viewCalendarLayout,
	viewCalendarRange,
	viewCalendarRecurrence,
	viewCardCoverAspect,
	viewCardCoverFit,
	viewCardCoverPosition,
	viewCardFieldKeys,
	viewCardFieldLimit,
	viewCardLayout,
	viewColumns,
	viewConfigBoolean,
	viewConfigRecord,
	viewConfigString,
	viewGanttScale,
	viewIsDefault,
	viewPivotAggregate,
	viewPivotColumnLimit,
	viewPivotRowLimit,
	viewPivotSortMode,
	viewPivotTotalsMode,
	viewPivotValueDisplay,
	viewSortDirection,
	viewTimelineSource,
	viewVisibility
} from '#lib/forms/view-config.js';
import {
	syncLegacyViewFilterFields,
	viewDraftCalendarExceptionDatesFromView,
	viewDraftFilterGroupsFromView,
	viewDraftFilterRootLogicFromView,
	viewDraftWipLimitsFromView
} from '#lib/forms/view-draft-parse.js';
import { timelineEventLayout } from '#lib/forms/views/timeline-events.js';

export class ViewDraftController extends FormsController {
	defaultViewColumns = (): string[] => {
		return this.s.fields
			.filter((field) => field.visibility?.list !== false)
			.slice(0, 8)
			.map((field) => field.key);
	};

	resetViewDraft = (view: FormView | null = this.s.selectedView) => {
		if (view) {
			const filterGroups = viewDraftFilterGroupsFromView(view);
			const sort = viewConfigRecord(view, 'sort');

			this.s.viewDraft = {
				key: view.key,
				name: view.name,
				view_type: view.view_type,
				columns: viewColumns(view),
				filterGroups,
				filterRootLogic: viewDraftFilterRootLogicFromView(view),
				...syncLegacyViewFilterFields(filterGroups),
				sortField: typeof sort?.field === 'string' ? sort.field : '',
				sortDirection: viewSortDirection(sort?.direction),
				groupBy: viewConfigString(view, 'group_by'),
				pivotRowField: viewConfigString(view, 'pivot_row_field'),
				pivotColumnField: viewConfigString(view, 'pivot_column_field'),
				pivotValueField: viewConfigString(view, 'pivot_value_field'),
				pivotAggregate: viewPivotAggregate(view.config?.pivot_aggregate),
				pivotTotals: viewPivotTotalsMode(view.config?.pivot_totals),
				pivotValueDisplay: viewPivotValueDisplay(view.config?.pivot_value_display),
				pivotSort: viewPivotSortMode(view.config?.pivot_sort),
				pivotEmptyBuckets: viewConfigBoolean(view, 'pivot_empty_buckets'),
				pivotHideZeroBuckets: viewConfigBoolean(view, 'pivot_hide_zero_buckets'),
				pivotShowCounts: viewConfigBoolean(view, 'pivot_show_counts'),
				pivotHeatmap: viewConfigBoolean(view, 'pivot_heatmap'),
				pivotRowLimit: viewPivotRowLimit(view.config?.pivot_row_limit),
				pivotColumnLimit: viewPivotColumnLimit(view.config?.pivot_column_limit),
				ganttStartField: viewConfigString(view, 'gantt_start_field'),
				ganttEndField: viewConfigString(view, 'gantt_end_field'),
				ganttGroupBy: viewConfigString(view, 'gantt_group_by'),
				ganttDependencyField: viewConfigString(view, 'gantt_dependency_field'),
				ganttScale: viewGanttScale(view.config?.gantt_scale),
				swimlaneBy: viewConfigString(view, 'swimlane_by'),
				wipLimits: viewDraftWipLimitsFromView(view),
				dateField: viewConfigString(view, 'date_field'),
				calendarRange: viewCalendarRange(view.config?.calendar_range),
				calendarFocusDate: this.fp.viewCalendar.viewCalendarFocusDate(view),
				calendarRecurrence: viewCalendarRecurrence(view.config?.calendar_recurrence),
				calendarDayScope: viewCalendarDayScope(view.config?.calendar_day_scope),
				calendarLayout: viewCalendarLayout(view.config?.calendar_layout),
				calendarHideEmptyDays: viewConfigBoolean(view, 'calendar_hide_empty_days'),
				calendarEndField: viewConfigString(view, 'calendar_end_field'),
				calendarResourceBy: viewConfigString(view, 'calendar_resource_by'),
				calendarExceptionRecordId: '',
				calendarExceptionDate: '',
				calendarExceptionDates: viewDraftCalendarExceptionDatesFromView(view),
				cardTitleField: viewConfigString(view, 'card_title_field'),
				cardSubtitleField: viewConfigString(view, 'card_subtitle_field'),
				cardBadgeField: viewConfigString(view, 'card_badge_field'),
				cardCoverField: viewConfigString(view, 'card_cover_field'),
				cardLayout: viewCardLayout(view.config?.card_layout),
				cardCoverAspect: viewCardCoverAspect(view.config?.card_cover_aspect),
				cardCoverFit: viewCardCoverFit(view.config?.card_cover_fit),
				cardCoverPosition: viewCardCoverPosition(view.config?.card_cover_position),
				cardFieldLimit: viewCardFieldLimit(view.config?.card_field_limit),
				cardHideEmptyFields: viewConfigBoolean(view, 'card_hide_empty_fields'),
				cardShowTimestamps: viewConfigBoolean(view, 'card_show_timestamps'),
				cardShowRecordId: viewConfigBoolean(view, 'card_show_record_id'),
				cardFieldKeys: viewCardFieldKeys(view),
				timelineSource: viewTimelineSource(view.config?.timeline_source),
				timelineEventLayout: timelineEventLayout(view.config?.timeline_event_layout),
				timelineEventType: viewConfigString(view, 'timeline_event_type'),
				timelineEventActor: viewConfigString(view, 'timeline_event_actor'),
				timelineEventAggregate: viewConfigString(view, 'timeline_event_aggregate'),
				timelineEventQuery: viewConfigString(view, 'timeline_event_query'),
				timelineEventFrom: viewConfigString(view, 'timeline_event_from'),
				timelineEventTo: viewConfigString(view, 'timeline_event_to'),
				timelineTitleField: viewConfigString(view, 'timeline_title_field'),
				timelineSubtitleField: viewConfigString(view, 'timeline_subtitle_field'),
				visibility: viewVisibility(view),
				isDefault: viewIsDefault(view)
			};

			return;
		}

		this.s.viewDraft = {
			key: '',
			name: '',
			view_type: 'grid',
			columns: this.defaultViewColumns(),
			filters: [],
			filterRootLogic: 'all',
			filterGroups: [],
			filterField: '',
			filterOperator: 'contains',
			filterValue: '',
			sortField: '',
			sortDirection: 'asc',
			groupBy: '',
			pivotRowField: '',
			pivotColumnField: '',
			pivotValueField: '',
			pivotAggregate: 'sum',
			pivotTotals: 'all',
			pivotValueDisplay: 'value',
			pivotSort: 'label',
			pivotEmptyBuckets: false,
			pivotHideZeroBuckets: false,
			pivotShowCounts: false,
			pivotHeatmap: false,
			pivotRowLimit: 0,
			pivotColumnLimit: 0,
			ganttStartField: '',
			ganttEndField: '',
			ganttGroupBy: '',
			ganttDependencyField: '',
			ganttScale: 'day',
			swimlaneBy: '',
			wipLimits: {},
			dateField: '',
			calendarRange: 'week',
			calendarFocusDate: '',
			calendarRecurrence: 'none',
			calendarDayScope: 'all',
			calendarLayout: 'grid',
			calendarHideEmptyDays: false,
			calendarEndField: '',
			calendarResourceBy: '',
			calendarExceptionRecordId: '',
			calendarExceptionDate: '',
			calendarExceptionDates: {},
			cardTitleField: '',
			cardSubtitleField: '',
			cardBadgeField: '',
			cardCoverField: '',
			cardLayout: 'standard',
			cardCoverAspect: 'auto',
			cardCoverFit: 'cover',
			cardCoverPosition: 'top',
			cardFieldLimit: 6,
			cardHideEmptyFields: false,
			cardShowTimestamps: false,
			cardShowRecordId: false,
			cardFieldKeys: [],
			timelineSource: 'records',
			timelineEventLayout: 'summary',
			timelineEventType: '',
			timelineEventActor: '',
			timelineEventAggregate: '',
			timelineEventQuery: '',
			timelineEventFrom: '',
			timelineEventTo: '',
			timelineTitleField: '',
			timelineSubtitleField: '',
			visibility: 'shared',
			isDefault: false
		};
		this.s.formsSessionViewVersion = this.s.selectedView?.updated_at ?? null;
		this.fp.session.cleanFormsGroup('view');
	};

	startNewViewDraft = (type: FormView['view_type']) => {
		// A new view starts from what is on screen: same filters and sort, the chosen type.
		this.s.viewDraft = { ...this.s.viewDraft, view_type: type };
		if (type !== 'grid') this.fp.views.openLayoutSettings();
	};

	setViewDraftColumn = (fieldKey: string, checked: boolean) => {
		this.s.viewDraft = {
			...this.s.viewDraft,
			columns: checked
				? Array.from(new Set([...this.s.viewDraft.columns, fieldKey]))
				: this.s.viewDraft.columns.filter((key) => key !== fieldKey)
		};
	};

	viewDraftHasColumn = (fieldKey: string): boolean => {
		return this.s.viewDraft.columns.includes(fieldKey);
	};

	viewDraftColumnFields = (): FormField[] => {
		const listable = this.s.fields.filter((field) => field.visibility?.list !== false);
		const draftColumns =
			this.s.viewDraft.columns.length > 0 ? this.s.viewDraft.columns : this.defaultViewColumns();

		return orderedFieldsByKeys(
			listable.filter((field) => draftColumns.includes(field.key)),
			draftColumns
		);
	};

	viewDraftCardFields = (): FormField[] => {
		return orderedFieldsByKeys(this.viewDraftColumnFields(), this.s.viewDraft.cardFieldKeys);
	};

	moveViewDraftCardField = (fieldIndex: number, direction: -1 | 1) => {
		const fieldsInOrder = this.viewDraftCardFields();
		const targetIndex = fieldIndex + direction;

		if (targetIndex < 0 || targetIndex >= fieldsInOrder.length) return;

		[fieldsInOrder[fieldIndex], fieldsInOrder[targetIndex]] = [
			fieldsInOrder[targetIndex],
			fieldsInOrder[fieldIndex]
		];

		this.s.viewDraft = {
			...this.s.viewDraft,
			cardFieldKeys: fieldsInOrder.map((field) => field.key)
		};
	};

	viewConfigFromDraft = (): Record<string, unknown> => {
		const config: Record<string, unknown> = {
			columns:
				this.s.viewDraft.columns.length > 0 ? this.s.viewDraft.columns : this.defaultViewColumns(),
			density: 'compact',
			visibility: this.s.viewDraft.visibility
		};

		const filterGroups = this.fp.viewDraftFilters.validViewFilterGroups();
		const filters = filterGroups[0]?.filters ?? [];

		if (filters.length > 0) {
			const filterExpression =
				this.fp.viewDraftFilters.viewFilterExpressionConfigFromGroups(filterGroups);

			if (filterExpression) config.filter_expression = filterExpression;

			config.filter_groups = filterGroups;
			config.filters = filters;
			config.filter = filters[0];
		}

		if (this.s.viewDraft.sortField) {
			config.sort = {
				field: this.s.viewDraft.sortField,
				direction: this.s.viewDraft.sortDirection
			};
		}

		if (this.s.viewDraft.groupBy) config.group_by = this.s.viewDraft.groupBy;

		const swimlaneBy = this.fp.viewDraftFields.validViewDraftSwimlaneBy();

		if (swimlaneBy) config.swimlane_by = swimlaneBy;

		const wipLimits = this.fp.viewDraftFields.validViewDraftWipLimits();

		if (Object.keys(wipLimits).length > 0) config.wip_limits = wipLimits;
		if (this.s.viewDraft.dateField) config.date_field = this.s.viewDraft.dateField;

		if (this.s.viewDraft.view_type === 'pivot') {
			const pivotRowField = this.fp.viewDraftFields.validViewDraftPivotRowField();
			const pivotColumnField = this.fp.viewDraftFields.validViewDraftPivotColumnField();
			const pivotValueField = this.fp.viewDraftFields.validViewDraftPivotValueField();

			if (pivotRowField) config.pivot_row_field = pivotRowField;
			if (pivotColumnField) config.pivot_column_field = pivotColumnField;
			if (pivotValueField) config.pivot_value_field = pivotValueField;

			config.pivot_aggregate = this.s.viewDraft.pivotAggregate;
			config.pivot_totals = this.s.viewDraft.pivotTotals;
			config.pivot_value_display = this.s.viewDraft.pivotValueDisplay;
			config.pivot_sort = this.s.viewDraft.pivotSort;

			if (this.s.viewDraft.pivotEmptyBuckets) config.pivot_empty_buckets = true;
			if (this.s.viewDraft.pivotHideZeroBuckets) config.pivot_hide_zero_buckets = true;
			if (this.s.viewDraft.pivotShowCounts) config.pivot_show_counts = true;
			if (this.s.viewDraft.pivotHeatmap) config.pivot_heatmap = true;
			if (this.s.viewDraft.pivotRowLimit > 0)
				config.pivot_row_limit = this.s.viewDraft.pivotRowLimit;
			if (this.s.viewDraft.pivotColumnLimit > 0)
				config.pivot_column_limit = this.s.viewDraft.pivotColumnLimit;
		}

		if (this.s.viewDraft.view_type === 'gantt') {
			const ganttStartField = this.fp.viewDraftFields.validViewDraftGanttStartField();
			const ganttEndField = this.fp.viewDraftFields.validViewDraftGanttEndField();
			const ganttGroupBy = this.fp.viewDraftFields.validViewDraftGanttGroupBy();
			const ganttDependencyField = this.fp.viewDraftFields.validViewDraftGanttDependencyField();

			config.gantt_scale = this.s.viewDraft.ganttScale;

			if (ganttStartField) config.gantt_start_field = ganttStartField;
			if (ganttEndField) config.gantt_end_field = ganttEndField;
			if (ganttGroupBy) config.gantt_group_by = ganttGroupBy;
			if (ganttDependencyField) config.gantt_dependency_field = ganttDependencyField;
		}

		if (this.s.viewDraft.view_type === 'calendar') {
			config.calendar_range = this.s.viewDraft.calendarRange;

			if (isIsoDate(this.s.viewDraft.calendarFocusDate))
				config.calendar_focus_date = this.s.viewDraft.calendarFocusDate;

			config.calendar_day_scope = this.s.viewDraft.calendarDayScope;
			config.calendar_layout = this.s.viewDraft.calendarLayout;

			if (this.s.viewDraft.calendarHideEmptyDays) config.calendar_hide_empty_days = true;

			const calendarEndField = this.fp.viewDraftFields.validViewDraftCalendarEndField();

			if (calendarEndField) config.calendar_end_field = calendarEndField;

			if (this.s.viewDraft.calendarRecurrence !== 'none') {
				config.calendar_recurrence = this.s.viewDraft.calendarRecurrence;
			}

			const calendarResourceBy = this.fp.viewDraftFields.validViewDraftCalendarResourceBy();

			if (calendarResourceBy) config.calendar_resource_by = calendarResourceBy;

			const calendarExceptionDates = this.fp.viewDraftFields.validViewDraftCalendarExceptionDates();

			if (Object.keys(calendarExceptionDates).length > 0) {
				config.calendar_exception_dates = calendarExceptionDates;
			}
		}

		if (this.s.viewDraft.view_type === 'card') {
			if (this.s.viewDraft.cardTitleField)
				config.card_title_field = this.s.viewDraft.cardTitleField;

			if (
				this.s.viewDraft.cardSubtitleField &&
				this.s.viewDraft.cardSubtitleField !== this.s.viewDraft.cardTitleField
			) {
				config.card_subtitle_field = this.s.viewDraft.cardSubtitleField;
			}

			if (
				this.s.viewDraft.cardBadgeField &&
				this.s.viewDraft.cardBadgeField !== this.s.viewDraft.cardTitleField &&
				this.s.viewDraft.cardBadgeField !== this.s.viewDraft.cardSubtitleField
			) {
				config.card_badge_field = this.s.viewDraft.cardBadgeField;
			}

			if (this.s.viewDraft.cardCoverField)
				config.card_cover_field = this.s.viewDraft.cardCoverField;
			if (this.s.viewDraft.cardLayout !== 'standard')
				config.card_layout = this.s.viewDraft.cardLayout;
			if (this.s.viewDraft.cardCoverAspect !== 'auto')
				config.card_cover_aspect = this.s.viewDraft.cardCoverAspect;
			if (this.s.viewDraft.cardCoverFit !== 'cover')
				config.card_cover_fit = this.s.viewDraft.cardCoverFit;
			if (this.s.viewDraft.cardCoverPosition !== 'top')
				config.card_cover_position = this.s.viewDraft.cardCoverPosition;
			if (this.s.viewDraft.cardFieldLimit !== 6)
				config.card_field_limit = this.s.viewDraft.cardFieldLimit;
			if (this.s.viewDraft.cardHideEmptyFields) config.card_hide_empty_fields = true;
			if (this.s.viewDraft.cardShowTimestamps) config.card_show_timestamps = true;
			if (this.s.viewDraft.cardShowRecordId) config.card_show_record_id = true;

			const cardFieldKeys = this.fp.viewDraftFields.validViewDraftCardFieldKeys();

			if (cardFieldKeys.length > 0) config.card_fields = cardFieldKeys;
		}

		if (this.s.viewDraft.view_type === 'timeline') {
			config.timeline_source = this.s.viewDraft.timelineSource;

			if (this.s.viewDraft.timelineSource === 'records' && this.s.viewDraft.timelineTitleField)
				config.timeline_title_field = this.s.viewDraft.timelineTitleField;

			if (
				this.s.viewDraft.timelineSource === 'records' &&
				this.s.viewDraft.timelineSubtitleField &&
				this.s.viewDraft.timelineSubtitleField !== this.s.viewDraft.timelineTitleField
			) {
				config.timeline_subtitle_field = this.s.viewDraft.timelineSubtitleField;
			}

			if (this.s.viewDraft.timelineSource === 'events') {
				config.timeline_event_layout = this.s.viewDraft.timelineEventLayout;

				if (this.s.viewDraft.timelineEventType)
					config.timeline_event_type = this.s.viewDraft.timelineEventType;
				if (this.s.viewDraft.timelineEventActor)
					config.timeline_event_actor = this.s.viewDraft.timelineEventActor;
				if (this.s.viewDraft.timelineEventAggregate)
					config.timeline_event_aggregate = this.s.viewDraft.timelineEventAggregate;
				if (this.s.viewDraft.timelineEventQuery.trim())
					config.timeline_event_query = this.s.viewDraft.timelineEventQuery.trim();
				if (this.s.viewDraft.timelineEventFrom.trim())
					config.timeline_event_from = this.s.viewDraft.timelineEventFrom.trim();
				if (this.s.viewDraft.timelineEventTo.trim())
					config.timeline_event_to = this.s.viewDraft.timelineEventTo.trim();
			}
		}

		config.is_default = this.s.viewDraft.isDefault;

		return config;
	};
}
