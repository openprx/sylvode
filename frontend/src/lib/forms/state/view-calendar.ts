import type { FormField, FormRecord, FormView } from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import type {
	CalendarExceptionDateMap,
	CalendarGridGroup,
	CalendarRecordOccurrence,
	CalendarResourceRow,
	ViewFilterDraft,
	ViewFilterLogic
} from '#lib/forms/page-types.js';
import {
	addIsoDays,
	calendarDatePayloadValue,
	calendarGridLabel,
	calendarOccurrenceDates,
	calendarRecordIsoDate,
	calendarScheduleKey,
	isIsoWorkday,
	isoDayDiff,
	monthDayCount,
	monthStartIso
} from '#lib/forms/calendar-dates.js';
import { translate } from '#lib/forms/i18n.js';
import { groupRecordsByField } from '#lib/forms/record-grouping.js';
import { FormsController } from '#lib/forms/state/controller.js';
import {
	calendarExceptionDateMap,
	isIsoDate,
	viewCalendarDayScope,
	viewCalendarRange,
	viewCalendarRecurrence,
	viewConfigString
} from '#lib/forms/view-config.js';

export class ViewCalendarController extends FormsController {
	copyCalendarSchedule = async (occurrence: CalendarRecordOccurrence) => {
		const value = JSON.stringify(
			{
				record_id: occurrence.record.id,
				title: occurrence.record.title,
				start: occurrence.startDate,
				end: occurrence.endDate,
				span_days: occurrence.spanDays,
				recurring: occurrence.recurring
			},
			null,
			2
		);

		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;

		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(value);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep calendar operations non-blocking.
		}

		this.s.copiedCalendarScheduleKey = calendarScheduleKey(occurrence);
		toast.success(translate('forms.calendarScheduleCopied'));
	};

	canShiftCalendarOccurrence = (occurrence: CalendarRecordOccurrence): boolean => {
		if (
			!this.s.calendarDateField ||
			!['date', 'datetime'].includes(this.s.calendarDateField.type ?? 'text')
		)
			return false;

		return (
			occurrence.spanStart &&
			!occurrence.recurring &&
			occurrence.startDate === calendarRecordIsoDate(occurrence.record, this.s.calendarDateField)
		);
	};

	shiftCalendarOccurrence = async (occurrence: CalendarRecordOccurrence, days: number) => {
		if (!this.canShiftCalendarOccurrence(occurrence) || !this.s.calendarDateField) return;

		const currentStart = calendarRecordIsoDate(occurrence.record, this.s.calendarDateField);

		if (!currentStart) return;

		const nextStart = addIsoDays(currentStart, days);
		const endField = this.viewCalendarEndField();

		const currentEnd = endField ? calendarRecordIsoDate(occurrence.record, endField) : null;

		const duration =
			currentEnd && currentEnd >= currentStart ? isoDayDiff(currentEnd, currentStart) : 0;

		const nextValues = {
			...occurrence.record.values,
			[this.s.calendarDateField.key]: calendarDatePayloadValue(this.s.calendarDateField, nextStart)
		};

		if (endField && currentEnd) {
			nextValues[endField.key] = calendarDatePayloadValue(
				endField,
				addIsoDays(nextStart, duration)
			);
		}

		this.s.updatingCalendarRecordId = occurrence.record.id;

		const response = await this.fp.session.updateRecordWithVersion(occurrence.record.id, {
			values: nextValues,
			source: { type: 'web', origin: 'calendar_shift' }
		});

		this.s.updatingCalendarRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		this.s.shiftedCalendarScheduleKey = `${occurrence.record.id}:${nextStart}`;
		toast.success(translate('forms.calendarShifted'));
		await this.fp.loaders.loadAggregates();
	};

	updateCalendarRecordDate = async (record: FormRecord, field: FormField, value: string) => {
		if (!value) return;

		const endField = this.viewCalendarEndField();
		const currentStart = calendarRecordIsoDate(record, field);
		const currentEnd = endField ? calendarRecordIsoDate(record, endField) : null;
		const duration =
			currentStart && currentEnd && currentEnd >= currentStart
				? isoDayDiff(currentEnd, currentStart)
				: 0;
		const nextValue = calendarDatePayloadValue(field, value);
		const nextValues = { ...record.values, [field.key]: nextValue };

		if (endField && currentEnd) {
			nextValues[endField.key] = calendarDatePayloadValue(endField, addIsoDays(value, duration));
		}

		if (
			record.values[field.key] === nextValues[field.key] &&
			(!endField || record.values[endField.key] === nextValues[endField.key])
		) {
			return;
		}

		this.s.updatingCalendarRecordId = record.id;

		const response = await this.fp.session.updateRecordWithVersion(record.id, {
			values: nextValues,
			source: { type: 'web', origin: 'calendar' }
		});

		this.s.updatingCalendarRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		toast.success(translate('forms.calendarDateUpdated'));
		await this.fp.loaders.loadAggregates();
	};

	updateCalendarSeriesDate = async (record: FormRecord, field: FormField, value: string) => {
		if (!value) return;

		const endField = this.viewCalendarEndField();
		const currentStart = calendarRecordIsoDate(record, field);
		const currentEnd = endField ? calendarRecordIsoDate(record, endField) : null;
		const duration =
			currentStart && currentEnd && currentEnd >= currentStart
				? isoDayDiff(currentEnd, currentStart)
				: 0;
		const nextValue = calendarDatePayloadValue(field, value);
		const nextValues = { ...record.values, [field.key]: nextValue };

		if (endField && currentEnd) {
			nextValues[endField.key] = calendarDatePayloadValue(endField, addIsoDays(value, duration));
		}

		if (
			calendarRecordIsoDate(record, field) === value &&
			(!endField || record.values[endField.key] === nextValues[endField.key])
		) {
			return;
		}

		this.s.updatingCalendarRecordId = record.id;

		const response = await this.fp.session.updateRecordWithVersion(record.id, {
			values: nextValues,
			source: { type: 'web', origin: 'calendar_series' }
		});

		this.s.updatingCalendarRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		toast.success(translate('forms.calendarSeriesUpdated'));
		await this.fp.loaders.loadAggregates();
	};

	updateCalendarRecordEndDate = async (record: FormRecord, endField: FormField, value: string) => {
		if (!value) return;

		const startField = this.viewDateField();
		const currentStart = startField ? calendarRecordIsoDate(record, startField) : null;
		const nextEnd = currentStart && value < currentStart ? currentStart : value;

		if (calendarRecordIsoDate(record, endField) === nextEnd) return;

		this.s.updatingCalendarRecordId = record.id;

		const response = await this.fp.session.updateRecordWithVersion(record.id, {
			values: {
				...record.values,
				[endField.key]: calendarDatePayloadValue(endField, nextEnd)
			},
			source: { type: 'web', origin: 'calendar_end_resize' }
		});

		this.s.updatingCalendarRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		toast.success(translate('forms.calendarEndDateUpdated'));
		await this.fp.loaders.loadAggregates();
	};

	handleCalendarDragStart = (event: DragEvent, record: FormRecord) => {
		this.s.draggingCalendarRecordId = record.id;
		event.dataTransfer?.setData('text/plain', record.id);

		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
		}
	};

	handleCalendarDragEnd = () => {
		this.s.draggingCalendarRecordId = null;
	};

	handleCalendarDragOver = (event: DragEvent) => {
		if (!this.s.draggingCalendarRecordId || !this.s.calendarDateField) return;

		event.preventDefault();

		if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
	};

	handleCalendarDrop = async (event: DragEvent, isoDate: string) => {
		event.preventDefault();

		const recordId = event.dataTransfer?.getData('text/plain') || this.s.draggingCalendarRecordId;

		this.s.draggingCalendarRecordId = null;

		if (!recordId || !this.s.calendarDateField) return;

		const record = this.s.records.find((item) => item.id === recordId);

		if (!record) return;

		await this.updateCalendarRecordDate(record, this.s.calendarDateField, isoDate);
	};

	viewDateField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'date_field')
		);

		if (configured) return configured;

		return (
			this.s.visibleFields.find((field) => ['date', 'datetime'].includes(field.type ?? 'text')) ??
			null
		);
	};

	viewCalendarResourceField = (): FormField | null => {
		const configuredKey = viewConfigString(this.s.selectedView, 'calendar_resource_by');

		if (!configuredKey || configuredKey === this.s.calendarDateField?.key) return null;

		return this.fp.viewRecords.fieldByKey(configuredKey);
	};

	viewCalendarEndField = (): FormField | null => {
		const configuredKey = viewConfigString(this.s.selectedView, 'calendar_end_field');

		if (!configuredKey || configuredKey === this.s.calendarDateField?.key) return null;

		const configured = this.fp.viewRecords.fieldByKey(configuredKey);

		if (!configured || !['date', 'datetime'].includes(configured.type ?? 'text')) return null;

		return configured;
	};

	viewCalendarFocusDate = (view: FormView | null = this.s.selectedView): string => {
		const focusDate = viewConfigString(view, 'calendar_focus_date');

		return isIsoDate(focusDate) ? focusDate : '';
	};

	selectedCalendarExceptionDates = (): CalendarExceptionDateMap => {
		return calendarExceptionDateMap(this.s.selectedView?.config?.calendar_exception_dates);
	};

	calendarExceptionDateSet = (recordId: string): Set<string> => {
		return new Set(this.selectedCalendarExceptionDates()[recordId] ?? []);
	};

	calendarDateInScope = (isoDate: string): boolean => {
		const dayScope = viewCalendarDayScope(this.s.selectedView?.config?.calendar_day_scope);

		return dayScope === 'all' || isIsoWorkday(isoDate);
	};

	groupRecordsByCalendarGrid = (
		sourceRecords: FormRecord[],
		field: FormField | null,
		scopeRecords: FormRecord[] = sourceRecords
	): CalendarGridGroup[] => {
		const dated = sourceRecords
			.map((record) => ({ record, date: calendarRecordIsoDate(record, field) }))
			.filter((item): item is { record: FormRecord; date: string } => Boolean(item.date));
		const scopedDates = scopeRecords
			.map((record) => calendarRecordIsoDate(record, field))
			.filter((date): date is string => Boolean(date));
		const focusDate = this.viewCalendarFocusDate();
		const startDate =
			focusDate ||
			(scopedDates.sort((left, right) => left.localeCompare(right))[0] ??
				dated.map((item) => item.date).sort((left, right) => left.localeCompare(right))[0] ??
				new Date().toISOString().slice(0, 10));
		const range = viewCalendarRange(this.s.selectedView?.config?.calendar_range);
		const gridStartDate = range === 'month' ? monthStartIso(startDate) : startDate;
		const gridDayCount = range === 'month' ? monthDayCount(startDate) : 7;
		const gridEndDate = addIsoDays(gridStartDate, gridDayCount - 1);
		const recurrence = viewCalendarRecurrence(this.s.selectedView?.config?.calendar_recurrence);
		const groups = new Map<string, CalendarGridGroup>();

		for (let index = 0; index < gridDayCount; index += 1) {
			const date = addIsoDays(gridStartDate, index);

			if (this.calendarDateInScope(date)) {
				groups.set(date, { key: date, label: calendarGridLabel(date), date, records: [] });
			}
		}

		for (const { record, date } of dated) {
			const rawEnd = this.s.calendarEndField
				? calendarRecordIsoDate(record, this.s.calendarEndField)
				: null;

			const endDate = rawEnd && rawEnd >= date ? rawEnd : date;
			const spanDays = isoDayDiff(endDate, date) + 1;
			const exceptionDates = this.calendarExceptionDateSet(record.id);

			for (const occurrenceDate of calendarOccurrenceDates(
				date,
				gridStartDate,
				gridEndDate,
				recurrence
			)) {
				if (exceptionDates.has(occurrenceDate)) continue;

				const occurrenceEndDate = addIsoDays(occurrenceDate, spanDays - 1);
				let bucketDate = occurrenceDate < gridStartDate ? gridStartDate : occurrenceDate;

				while (bucketDate <= occurrenceEndDate && bucketDate <= gridEndDate) {
					if (!this.calendarDateInScope(bucketDate)) {
						bucketDate = addIsoDays(bucketDate, 1);

						continue;
					}

					if (!groups.has(bucketDate)) {
						groups.set(bucketDate, {
							key: bucketDate,
							label: calendarGridLabel(bucketDate),
							date: bucketDate,
							records: []
						});
					}

					groups.get(bucketDate)?.records.push({
						key: `${record.id}:${occurrenceDate}:${bucketDate}`,
						record,
						date: bucketDate,
						startDate: occurrenceDate,
						endDate: occurrenceEndDate,
						recurring: occurrenceDate !== date,
						spanStart: bucketDate === occurrenceDate,
						spanEnd: bucketDate === occurrenceEndDate,
						spanDays
					});

					bucketDate = addIsoDays(bucketDate, 1);
				}
			}
		}

		const orderedGroups = Array.from(groups.values()).sort((left, right) =>
			left.date.localeCompare(right.date)
		);

		return this.s.selectedCalendarHideEmptyDays
			? orderedGroups.filter((group) => group.records.length > 0)
			: orderedGroups;
	};

	groupCalendarResourceLanes = (sourceRecords: FormRecord[]): CalendarResourceRow[] => {
		if (!this.s.calendarResourceField) return [];

		return groupRecordsByField(sourceRecords, this.s.calendarResourceField).map((resource) => ({
			...resource,
			groups: this.groupRecordsByCalendarGrid(
				resource.records,
				this.s.calendarDateField,
				sourceRecords
			)
		}));
	};

	calendarResourceRows = (): CalendarResourceRow[] => {
		if (this.s.calendarResourceField) return this.s.calendarResourceLanes;

		return [
			{
				key: '__all__',
				label: '',
				records: this.s.viewRecords,
				groups: this.s.calendarGridGroups
			}
		];
	};

	calendarBucketFilterKey = (isoDate: string, resource: CalendarResourceRow): string => {
		return `${isoDate}:${this.s.calendarResourceField ? resource.key : '__all__'}`;
	};

	calendarBucketFilterDrafts = (
		isoDate: string,
		resource: CalendarResourceRow
	): ViewFilterDraft[] => {
		if (!this.s.calendarDateField) return [];

		const filters: ViewFilterDraft[] = [
			{
				field: this.s.calendarDateField.key,
				operator: (this.s.calendarDateField.type ?? 'date') === 'datetime' ? 'contains' : 'equals',
				value: isoDate
			}
		];

		if (this.s.calendarResourceField && resource.key !== '__all__') {
			filters.push({
				field: this.s.calendarResourceField.key,
				operator: 'equals',
				value: resource.label
			});
		}

		return filters;
	};

	canApplyCalendarBucketFilters = (isoDate: string, resource: CalendarResourceRow): boolean => {
		const filters = this.calendarBucketFilterDrafts(isoDate, resource);

		if (filters.length === 0) return false;

		const currentGroups = this.fp.viewDraftFilters
			.visibleViewDraftFilterGroups()
			.filter((group) => group.filters.length > 0);
		const currentFilterCount = currentGroups.reduce(
			(count, group) => count + group.filters.length,
			0
		);

		if (currentFilterCount + filters.length > 10) return false;
		if (currentGroups.length < 5) return true;

		const lastGroup = currentGroups[currentGroups.length - 1];

		return Boolean(lastGroup && lastGroup.filters.length + filters.length <= 5);
	};

	applyCalendarBucketFilters = (isoDate: string, resource: CalendarResourceRow) => {
		if (!this.canApplyCalendarBucketFilters(isoDate, resource)) return;

		const filters = this.calendarBucketFilterDrafts(isoDate, resource);

		const currentGroups = this.fp.viewDraftFilters
			.visibleViewDraftFilterGroups()
			.filter((group) => group.filters.length > 0)
			.map((group) => ({
				logic: group.logic,
				disabled: group.disabled || undefined,
				filters: group.filters.map((filter) => ({ ...filter }))
			}));

		const nextGroups =
			currentGroups.length === 0
				? [{ logic: 'all' as ViewFilterLogic, filters }]
				: currentGroups.length < 5
					? [...currentGroups, { logic: 'all' as ViewFilterLogic, filters }]
					: currentGroups.map((group, index) =>
							index === currentGroups.length - 1
								? { ...group, filters: [...group.filters, ...filters] }
								: group
						);

		this.fp.viewDraftFilters.setViewDraftFilterGroups(nextGroups);
		this.fp.viewDraftFilters.updateViewDraftFilterRootLogic('all');
		this.s.appliedCalendarFilterKey = this.calendarBucketFilterKey(isoDate, resource);
		toast.success(translate('forms.calendarBucketFiltersApplied'));
	};

	calendarExceptionRecordLabel = (recordId: string): string => {
		const record = this.s.records.find((item) => item.id === recordId);

		return record ? record.title : recordId;
	};
}
