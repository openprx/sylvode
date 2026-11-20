import type { FormField } from '#lib/api/forms.js';
import type { CalendarExceptionDateMap } from '#lib/forms/page-types.js';
import {
	addIsoDays,
	addIsoMonths,
	calendarRecordIsoDate,
	monthStartIso,
	weekStartIso
} from '#lib/forms/calendar-dates.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { calendarExceptionDateMap, isIsoDate, kanbanMoveOptions } from '#lib/forms/view-config.js';

export class ViewDraftFieldsController extends FormsController {
	viewDraftCalendarFocusBaseDate = (): string => {
		if (isIsoDate(this.s.viewDraft.calendarFocusDate)) return this.s.viewDraft.calendarFocusDate;

		const selectedFocusDate = this.fp.viewCalendar.viewCalendarFocusDate();

		if (selectedFocusDate) return selectedFocusDate;

		const field =
			this.fp.viewRecords.fieldByKey(this.s.viewDraft.dateField) ?? this.s.calendarDateField;
		const sourceDates = this.s.viewRecords
			.map((record) => calendarRecordIsoDate(record, field))
			.filter((date): date is string => Boolean(date))
			.sort((left, right) => left.localeCompare(right));

		return sourceDates[0] ?? new Date().toISOString().slice(0, 10);
	};

	shiftViewDraftCalendarFocus = (amount: number) => {
		if (this.s.viewDraft.view_type !== 'calendar') return;

		const range = this.s.viewDraft.calendarRange;
		const baseDate = this.viewDraftCalendarFocusBaseDate();

		const nextDate =
			range === 'month'
				? addIsoMonths(monthStartIso(baseDate), amount)
				: addIsoDays(weekStartIso(baseDate), amount * 7);

		this.s.viewDraft = { ...this.s.viewDraft, calendarFocusDate: nextDate };
	};

	validViewDraftCardFieldKeys = (): string[] => {
		const draftFields = this.fp.viewDraft.viewDraftCardFields();
		const defaultKeys = this.fp.viewDraft.viewDraftColumnFields().map((field) => field.key);
		const orderedKeys = draftFields.map((field) => field.key);

		return orderedKeys.some((key, index) => key !== defaultKeys[index]) ? orderedKeys : [];
	};

	viewDraftKanbanGroupField = (): FormField | null => {
		return this.fp.viewRecords.fieldByKey(this.s.viewDraft.groupBy);
	};

	viewDraftSwimlaneFieldCandidates = (): FormField[] => {
		if (this.s.viewDraft.view_type !== 'kanban') return [];

		return this.s.fields.filter(
			(field) => field.visibility?.list !== false && field.key !== this.s.viewDraft.groupBy
		);
	};

	validViewDraftSwimlaneBy = (): string => {
		if (this.s.viewDraft.view_type !== 'kanban' || !this.s.viewDraft.swimlaneBy) return '';
		if (this.s.viewDraft.swimlaneBy === this.s.viewDraft.groupBy) return '';

		return this.viewDraftSwimlaneFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.swimlaneBy
		)
			? this.s.viewDraft.swimlaneBy
			: '';
	};

	viewDraftCalendarResourceFieldCandidates = (): FormField[] => {
		if (this.s.viewDraft.view_type !== 'calendar') return [];

		return this.s.fields.filter(
			(field) => field.visibility?.list !== false && field.key !== this.s.viewDraft.dateField
		);
	};

	viewDraftCalendarEndFieldCandidates = (): FormField[] => {
		if (this.s.viewDraft.view_type !== 'calendar') return [];

		return this.s.fields.filter(
			(field) =>
				field.visibility?.list !== false &&
				field.key !== this.s.viewDraft.dateField &&
				['date', 'datetime'].includes(field.type ?? 'text')
		);
	};

	validViewDraftCalendarEndField = (): string => {
		if (this.s.viewDraft.view_type !== 'calendar' || !this.s.viewDraft.calendarEndField) return '';
		if (this.s.viewDraft.calendarEndField === this.s.viewDraft.dateField) return '';

		return this.viewDraftCalendarEndFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.calendarEndField
		)
			? this.s.viewDraft.calendarEndField
			: '';
	};

	validViewDraftCalendarResourceBy = (): string => {
		if (this.s.viewDraft.view_type !== 'calendar' || !this.s.viewDraft.calendarResourceBy)
			return '';
		if (this.s.viewDraft.calendarResourceBy === this.s.viewDraft.dateField) return '';

		return this.viewDraftCalendarResourceFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.calendarResourceBy
		)
			? this.s.viewDraft.calendarResourceBy
			: '';
	};

	viewDraftPivotDimensionFieldCandidates = (): FormField[] => {
		if (this.s.viewDraft.view_type !== 'pivot') return [];

		return this.s.fields.filter((field) => field.visibility?.list !== false);
	};

	viewDraftPivotValueFieldCandidates = (): FormField[] => {
		if (this.s.viewDraft.view_type !== 'pivot') return [];

		return this.s.fields.filter(
			(field) =>
				field.visibility?.list !== false &&
				['amount', 'number', 'integer'].includes(field.type ?? 'text')
		);
	};

	validViewDraftPivotRowField = (): string => {
		if (this.s.viewDraft.view_type !== 'pivot' || !this.s.viewDraft.pivotRowField) return '';

		return this.viewDraftPivotDimensionFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.pivotRowField
		)
			? this.s.viewDraft.pivotRowField
			: '';
	};

	validViewDraftPivotColumnField = (): string => {
		if (this.s.viewDraft.view_type !== 'pivot' || !this.s.viewDraft.pivotColumnField) return '';
		if (this.s.viewDraft.pivotColumnField === this.s.viewDraft.pivotRowField) return '';

		return this.viewDraftPivotDimensionFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.pivotColumnField
		)
			? this.s.viewDraft.pivotColumnField
			: '';
	};

	validViewDraftPivotValueField = (): string => {
		if (this.s.viewDraft.view_type !== 'pivot' || !this.s.viewDraft.pivotValueField) return '';

		return this.viewDraftPivotValueFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.pivotValueField
		)
			? this.s.viewDraft.pivotValueField
			: '';
	};

	swapViewDraftPivotAxes = () => {
		if (
			this.s.viewDraft.view_type !== 'pivot' ||
			!this.s.viewDraft.pivotRowField ||
			!this.s.viewDraft.pivotColumnField
		) {
			return;
		}

		this.s.viewDraft = {
			...this.s.viewDraft,
			pivotRowField: this.s.viewDraft.pivotColumnField,
			pivotColumnField: this.s.viewDraft.pivotRowField
		};
	};

	viewDraftGanttDateFieldCandidates = (): FormField[] => {
		if (this.s.viewDraft.view_type !== 'gantt') return [];

		return this.s.fields.filter(
			(field) =>
				field.visibility?.list !== false && ['date', 'datetime'].includes(field.type ?? 'text')
		);
	};

	viewDraftGanttGroupFieldCandidates = (): FormField[] => {
		if (this.s.viewDraft.view_type !== 'gantt') return [];

		return this.s.fields.filter(
			(field) =>
				field.visibility?.list !== false &&
				field.key !== this.s.viewDraft.ganttStartField &&
				field.key !== this.s.viewDraft.ganttEndField
		);
	};

	viewDraftGanttDependencyFieldCandidates = (): FormField[] => {
		if (this.s.viewDraft.view_type !== 'gantt') return [];

		return this.s.fields.filter(
			(field) =>
				field.visibility?.list !== false &&
				field.key !== this.s.viewDraft.ganttStartField &&
				field.key !== this.s.viewDraft.ganttEndField &&
				field.key !== this.s.viewDraft.ganttGroupBy
		);
	};

	validViewDraftGanttStartField = (): string => {
		if (this.s.viewDraft.view_type !== 'gantt' || !this.s.viewDraft.ganttStartField) return '';

		return this.viewDraftGanttDateFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.ganttStartField
		)
			? this.s.viewDraft.ganttStartField
			: '';
	};

	validViewDraftGanttEndField = (): string => {
		if (this.s.viewDraft.view_type !== 'gantt' || !this.s.viewDraft.ganttEndField) return '';
		if (this.s.viewDraft.ganttEndField === this.s.viewDraft.ganttStartField) return '';

		return this.viewDraftGanttDateFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.ganttEndField
		)
			? this.s.viewDraft.ganttEndField
			: '';
	};

	validViewDraftGanttGroupBy = (): string => {
		if (this.s.viewDraft.view_type !== 'gantt' || !this.s.viewDraft.ganttGroupBy) return '';

		return this.viewDraftGanttGroupFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.ganttGroupBy
		)
			? this.s.viewDraft.ganttGroupBy
			: '';
	};

	validViewDraftGanttDependencyField = (): string => {
		if (this.s.viewDraft.view_type !== 'gantt' || !this.s.viewDraft.ganttDependencyField) return '';

		return this.viewDraftGanttDependencyFieldCandidates().some(
			(field) => field.key === this.s.viewDraft.ganttDependencyField
		)
			? this.s.viewDraft.ganttDependencyField
			: '';
	};

	validViewDraftCalendarExceptionDates = (): CalendarExceptionDateMap => {
		const next = calendarExceptionDateMap(this.s.viewDraft.calendarExceptionDates);
		const recordId = this.s.viewDraft.calendarExceptionRecordId.trim();
		const date = this.s.viewDraft.calendarExceptionDate.trim();

		if (recordId && isIsoDate(date)) {
			next[recordId] = Array.from(new Set([...(next[recordId] ?? []), date])).sort();
		}

		return next;
	};

	viewDraftCalendarExceptionItems = (): Array<{
		recordId: string;
		date: string;
		label: string;
	}> => {
		return Object.entries(this.validViewDraftCalendarExceptionDates())
			.flatMap(([recordId, dates]) =>
				dates.map((date) => ({
					recordId,
					date,
					label: this.fp.viewCalendar.calendarExceptionRecordLabel(recordId)
				}))
			)
			.sort(
				(left, right) =>
					left.date.localeCompare(right.date) || left.label.localeCompare(right.label)
			);
	};

	addViewDraftCalendarException = () => {
		const recordId = this.s.viewDraft.calendarExceptionRecordId.trim();
		const date = this.s.viewDraft.calendarExceptionDate.trim();

		if (!recordId || !isIsoDate(date)) return;

		const nextDates = new Set(this.s.viewDraft.calendarExceptionDates[recordId] ?? []);

		nextDates.add(date);

		this.s.viewDraft = {
			...this.s.viewDraft,
			calendarExceptionDate: '',
			calendarExceptionDates: {
				...this.s.viewDraft.calendarExceptionDates,
				[recordId]: Array.from(nextDates).sort()
			}
		};
	};

	removeViewDraftCalendarException = (recordId: string, date: string) => {
		const next = { ...this.s.viewDraft.calendarExceptionDates };
		const dates = (next[recordId] ?? []).filter((item) => item !== date);

		if (dates.length > 0) {
			next[recordId] = dates;
		} else {
			delete next[recordId];
		}

		this.s.viewDraft = { ...this.s.viewDraft, calendarExceptionDates: next };
	};

	viewDraftKanbanWipOptions = (): string[] => {
		const field = this.viewDraftKanbanGroupField();

		if (this.s.viewDraft.view_type !== 'kanban' || field?.type !== 'single_select') return [];

		return kanbanMoveOptions(field);
	};

	setViewDraftWipLimit = (option: string, value: string) => {
		const nextLimits = { ...this.s.viewDraft.wipLimits };
		const trimmed = value.trim();

		if (!trimmed) {
			delete nextLimits[option];
		} else {
			nextLimits[option] = trimmed;
		}

		this.s.viewDraft = { ...this.s.viewDraft, wipLimits: nextLimits };
	};

	validViewDraftWipLimits = (): Record<string, number> => {
		const limits: Record<string, number> = {};

		for (const option of this.viewDraftKanbanWipOptions()) {
			const raw = this.s.viewDraft.wipLimits[option];

			if (!raw?.trim()) continue;

			const limit = Number(raw);

			if (Number.isFinite(limit) && limit >= 0) {
				limits[option] = Math.floor(limit);
			}
		}

		return limits;
	};
}
