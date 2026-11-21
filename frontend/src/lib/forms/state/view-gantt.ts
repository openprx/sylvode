import type { FormField, FormRecord } from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import type { GanttTask, GanttTimeline } from '#lib/forms/page-types.js';
import {
	addGanttBucket,
	addIsoDays,
	calendarDatePayloadValue,
	calendarRecordIsoDate,
	ganttBucketLimit,
	ganttBucketStartIso,
	isoDayDiff
} from '#lib/forms/calendar-dates.js';
import { translate } from '#lib/forms/i18n.js';
import { recordDisplay } from '#lib/forms/record-format.js';
import { recordGroupLabel } from '#lib/forms/record-grouping.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { viewConfigString, viewGanttScale } from '#lib/forms/view-config.js';

export class ViewGanttController extends FormsController {
	updateGanttRecordDate = async (record: FormRecord, startField: FormField, value: string) => {
		if (!value) return;

		const endField = this.viewGanttEndField();
		const currentStart = calendarRecordIsoDate(record, startField);
		const currentEnd = endField ? calendarRecordIsoDate(record, endField) : null;
		const duration =
			currentStart && currentEnd && currentEnd >= currentStart
				? isoDayDiff(currentEnd, currentStart)
				: 0;

		const nextValues = {
			...record.values,
			[startField.key]: calendarDatePayloadValue(startField, value)
		};

		if (endField && currentEnd) {
			nextValues[endField.key] = calendarDatePayloadValue(endField, addIsoDays(value, duration));
		}

		if (
			record.values[startField.key] === nextValues[startField.key] &&
			(!endField || record.values[endField.key] === nextValues[endField.key])
		) {
			return;
		}

		this.s.updatingGanttRecordId = record.id;

		const response = await this.fp.session.updateRecordWithVersion(record.id, {
			values: nextValues,
			source: { type: 'web', origin: 'gantt' }
		});

		this.s.updatingGanttRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		toast.success(translate('forms.ganttDateUpdated'));
		await this.fp.loaders.loadAggregates();
	};

	updateGanttRecordEndDate = async (record: FormRecord, endField: FormField, value: string) => {
		if (!value) return;

		const startField = this.viewGanttStartField();
		const currentStart = startField ? calendarRecordIsoDate(record, startField) : null;
		const nextEnd = currentStart && value < currentStart ? currentStart : value;

		if (calendarRecordIsoDate(record, endField) === nextEnd) return;

		this.s.updatingGanttRecordId = record.id;

		const response = await this.fp.session.updateRecordWithVersion(record.id, {
			values: {
				...record.values,
				[endField.key]: calendarDatePayloadValue(endField, nextEnd)
			},
			source: { type: 'web', origin: 'gantt_resize' }
		});

		this.s.updatingGanttRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		toast.success(translate('forms.ganttEndDateUpdated'));
		await this.fp.loaders.loadAggregates();
	};

	extendGanttTaskEnd = async (task: GanttTask) => {
		if (!this.s.ganttTimeline.endField) return;

		const nextEnd = addGanttBucket(task.end, this.s.ganttTimeline.scale, 1);

		await this.updateGanttRecordEndDate(task.record, this.s.ganttTimeline.endField, nextEnd);
	};

	handleGanttDragStart = (event: DragEvent, record: FormRecord) => {
		this.s.draggingGanttRecordId = record.id;
		event.dataTransfer?.setData('text/plain', record.id);

		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
		}
	};

	handleGanttDragEnd = () => {
		this.s.draggingGanttRecordId = null;
	};

	handleGanttDragOver = (event: DragEvent) => {
		if (!this.s.draggingGanttRecordId || !this.s.ganttTimeline.startField) return;

		event.preventDefault();

		if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
	};

	handleGanttDrop = async (event: DragEvent, isoDate: string) => {
		event.preventDefault();

		const recordId = event.dataTransfer?.getData('text/plain') || this.s.draggingGanttRecordId;

		this.s.draggingGanttRecordId = null;

		if (!recordId || !this.s.ganttTimeline.startField) return;

		const record = this.s.records.find((item) => item.id === recordId);

		if (!record) return;

		await this.updateGanttRecordDate(record, this.s.ganttTimeline.startField, isoDate);
	};

	viewGanttStartField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'gantt_start_field')
		);

		if (configured && ['date', 'datetime'].includes(configured.type ?? 'text')) return configured;

		return (
			this.s.visibleFields.find((field) => ['date', 'datetime'].includes(field.type ?? 'text')) ??
			null
		);
	};

	viewGanttEndField = (): FormField | null => {
		const startField = this.viewGanttStartField();
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'gantt_end_field')
		);

		if (
			configured &&
			configured.key !== startField?.key &&
			['date', 'datetime'].includes(configured.type ?? 'text')
		) {
			return configured;
		}

		return null;
	};

	viewGanttGroupField = (): FormField | null => {
		const startField = this.viewGanttStartField();
		const endField = this.viewGanttEndField();
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'gantt_group_by')
		);

		if (configured && configured.key !== startField?.key && configured.key !== endField?.key)
			return configured;

		return null;
	};

	viewGanttDependencyField = (): FormField | null => {
		const startField = this.viewGanttStartField();
		const endField = this.viewGanttEndField();
		const groupField = this.viewGanttGroupField();
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'gantt_dependency_field')
		);

		if (
			configured &&
			configured.key !== startField?.key &&
			configured.key !== endField?.key &&
			configured.key !== groupField?.key
		) {
			return configured;
		}

		return null;
	};

	buildGanttTimeline = (sourceRecords: FormRecord[]): GanttTimeline => {
		const startField = this.viewGanttStartField();
		const endField = this.viewGanttEndField();
		const groupField = this.viewGanttGroupField();
		const dependencyField = this.viewGanttDependencyField();
		const scale = viewGanttScale(this.s.selectedView?.config?.gantt_scale);

		if (!startField) {
			return {
				startField,
				endField,
				groupField,
				dependencyField,
				scale,
				dates: [],
				lanes: []
			};
		}

		const dated = sourceRecords
			.map((record) => {
				const start = calendarRecordIsoDate(record, startField);
				const rawEnd = endField ? calendarRecordIsoDate(record, endField) : null;

				if (!start) return null;

				const end = rawEnd && rawEnd >= start ? rawEnd : start;

				return { record, start, end };
			})
			.filter((item): item is { record: FormRecord; start: string; end: string } => Boolean(item));

		if (dated.length === 0) {
			return {
				startField,
				endField,
				groupField,
				dependencyField,
				scale,
				dates: [],
				lanes: []
			};
		}

		const firstStart = dated
			.map((item) => item.start)
			.sort((left, right) => left.localeCompare(right))[0];
		const maxEnd = dated
			.map((item) => item.end)
			.sort((left, right) => right.localeCompare(left))[0];
		const axisStart = ganttBucketStartIso(firstStart, scale);
		const lastBucket = ganttBucketStartIso(maxEnd, scale);

		const bucketCount = (() => {
			if (scale === 'day') {
				return Math.min(
					Math.max(isoDayDiff(lastBucket, axisStart) + 1, 7),
					ganttBucketLimit(scale)
				);
			}

			const candidateDates = Array.from({ length: ganttBucketLimit(scale) }, (_, index) =>
				addGanttBucket(axisStart, scale, index)
			);
			const lastIndex = candidateDates.findIndex((date) => date >= lastBucket);

			return Math.max(lastIndex >= 0 ? lastIndex + 1 : ganttBucketLimit(scale), 4);
		})();

		const axisEnd = addGanttBucket(axisStart, scale, bucketCount - 1);
		const dates = Array.from({ length: bucketCount }, (_, index) =>
			addGanttBucket(axisStart, scale, index)
		);

		const bucketIndex = (date: string) => {
			const bucket = ganttBucketStartIso(date, scale);
			const index = dates.indexOf(bucket);

			if (index >= 0) return index;

			return bucket < axisStart ? 0 : dates.length - 1;
		};

		const laneMap = new Map<string, { key: string; label: string; tasks: GanttTask[] }>();

		const ensureLane = (key: string, label: string) => {
			if (!laneMap.has(key)) laneMap.set(key, { key, label, tasks: [] });

			return laneMap.get(key);
		};

		if (groupField?.type === 'single_select' && Array.isArray(groupField.options)) {
			for (const option of groupField.options) {
				const label = option.trim();

				if (label) ensureLane(label.toLowerCase(), label);
			}
		}

		for (const item of dated) {
			const label = groupField
				? recordGroupLabel(item.record, groupField)
				: translate('forms.ganttUngrouped');

			const key = groupField ? label.toLowerCase() : '__all__';
			const lane = ensureLane(key, label);
			const start = item.start < axisStart ? axisStart : item.start;
			const end = item.end > axisEnd ? axisEnd : item.end;
			const startOffset = bucketIndex(start);
			const endOffset = bucketIndex(end);

			lane?.tasks.push({
				record: item.record,
				start: item.start,
				end: item.end,
				dependencyValue: dependencyField ? recordDisplay(item.record, dependencyField) : '',
				startOffset,
				span: Math.max(1, endOffset - startOffset + 1)
			});
		}

		return {
			startField,
			endField,
			groupField,
			dependencyField,
			scale,
			dates,
			lanes: Array.from(laneMap.values())
				.map((lane) => ({
					...lane,
					tasks: lane.tasks.sort(
						(left, right) =>
							left.start.localeCompare(right.start) ||
							left.record.title.localeCompare(right.record.title)
					)
				}))
				.sort((left, right) => left.label.localeCompare(right.label))
		};
	};
}
