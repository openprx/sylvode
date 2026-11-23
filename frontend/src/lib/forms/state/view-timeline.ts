import { copyText } from '#lib/clipboard.js';
import type { BusinessEvent, FormField, FormRecord } from '#lib/api/forms.js';
import { csvDownloadBlob } from '#lib/utils/csv.js';
import * as precedence from '#lib/forms/view-precedence.js';
import { toast } from '#lib/stores/toast.js';
import type { TimelineEventLayout, ViewTimelineSource } from '#lib/forms/page-types.js';
import { translate } from '#lib/forms/i18n.js';
import { recordDisplay } from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { viewConfigString, viewTimelineSource } from '#lib/forms/view-config.js';
import {
	timelineEventAggregateKey,
	timelineEventAggregateKeys,
	timelineEventBoundary,
	timelineEventDateTimeLocal,
	timelineEventDetailsJson,
	timelineEventLayout,
	timelineEventsCsv,
	timelineEventsDetailsJson,
	timelineEventSearchText
} from '#lib/forms/views/timeline-events.js';

export class ViewTimelineController extends FormsController {
	copyTimelineEventValue = async (
		event: BusinessEvent,
		value: string,
		action: 'event-id' | 'aggregate' | 'details'
	) => {
		if (!(await copyText(value))) {
			toast.error(translate('forms.copyFailed'));
			return;
		}

		this.s.copiedTimelineEventAction = `${event.id}:${action}`;
		toast.success(translate('forms.timelineEventCopied'));
	};

	exportTimelineEventDetails = (event: BusinessEvent) => {
		if (!this.s.selectedForm) return;

		const blob = new Blob([timelineEventDetailsJson(event)], {
			type: 'application/json;charset=utf-8'
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-${event.id}-details.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedTimelineEventAction = `${event.id}:details`;
		toast.success(translate('forms.timelineEventExported'));
	};

	exportTimelineEventsCsv = (events: BusinessEvent[]) => {
		if (!this.s.selectedForm || events.length === 0) return;

		const blob = csvDownloadBlob(timelineEventsCsv(events));
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-timeline-events.csv`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedTimelineEventsCsvCount = events.length;
		toast.success(translate('forms.timelineEventsExported', { values: { count: events.length } }));
	};

	exportTimelineEventsJson = (events: BusinessEvent[]) => {
		if (!this.s.selectedForm || events.length === 0) return;

		const blob = new Blob([timelineEventsDetailsJson(events)], {
			type: 'application/json;charset=utf-8'
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-timeline-events.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedTimelineEventsJsonCount = events.length;
		toast.success(
			translate('forms.timelineEventsJsonExported', { values: { count: events.length } })
		);
	};

	copyTimelineEventIds = async (events: BusinessEvent[]) => {
		if (events.length === 0) return;

		if (!(await copyText(events.map((event) => event.id).join('\n')))) {
			toast.error(translate('forms.copyFailed'));
			return;
		}

		this.s.copiedTimelineEventIdsCount = events.length;
		toast.success(translate('forms.timelineEventsCopied', { values: { count: events.length } }));
	};

	copyTimelineAggregateIds = async (events: BusinessEvent[]) => {
		const aggregateKeys = timelineEventAggregateKeys(events);

		if (aggregateKeys.length === 0) return;

		if (!(await copyText(aggregateKeys.join('\n')))) {
			toast.error(translate('forms.copyFailed'));
			return;
		}

		this.s.copiedTimelineAggregateIdsCount = aggregateKeys.length;
		toast.success(
			translate('forms.timelineAggregatesCopied', { values: { count: aggregateKeys.length } })
		);
	};

	timelineEventPinned = (event: BusinessEvent): boolean => {
		return this.s.pinnedTimelineEventIds.includes(event.id);
	};

	orderedTimelineEvents = (events: BusinessEvent[]): BusinessEvent[] => {
		if (this.s.pinnedTimelineEventIds.length === 0) return events;

		const pinnedOrder = new Map(this.s.pinnedTimelineEventIds.map((id, index) => [id, index]));

		return [
			...events
				.filter((event) => pinnedOrder.has(event.id))
				.sort((a, b) => (pinnedOrder.get(a.id) ?? 0) - (pinnedOrder.get(b.id) ?? 0)),
			...events.filter((event) => !pinnedOrder.has(event.id))
		];
	};

	toggleTimelineEventPinned = (event: BusinessEvent) => {
		if (this.timelineEventPinned(event)) {
			this.s.pinnedTimelineEventIds = this.s.pinnedTimelineEventIds.filter((id) => id !== event.id);
			toast.success(translate('forms.timelineEventUnpinned'));

			return;
		}

		this.s.pinnedTimelineEventIds = [
			event.id,
			...this.s.pinnedTimelineEventIds.filter((id) => id !== event.id)
		];

		toast.success(translate('forms.timelineEventPinned'));
	};

	focusTimelineEventRecord = (event: BusinessEvent) => {
		const record = this.timelineEventRecord(event);

		if (!record) return;

		this.s.focusedRecordIds = [record.id];

		this.fp.views.showView(precedence.ALL_RECORDS_VIEW_ID, { remember: false });

		toast.success(translate('forms.timelineEventRecordFocused'));
	};

	timelineEventLinkedRecordIds = (events: BusinessEvent[]): string[] => {
		return Array.from(
			new Set(
				events
					.map((event) => this.timelineEventRecord(event)?.id)
					.filter((id): id is string => Boolean(id))
			)
		);
	};

	focusTimelineEventRecords = (events: BusinessEvent[]) => {
		const recordIds = this.timelineEventLinkedRecordIds(events);

		if (recordIds.length === 0) return;

		this.s.focusedRecordIds = recordIds;

		this.fp.views.showView(precedence.ALL_RECORDS_VIEW_ID, { remember: false });

		toast.success(translate('forms.focusedRecords', { values: { count: recordIds.length } }));
	};

	applyTimelineEventQuickFilter = (
		event: BusinessEvent,
		target: 'type' | 'actor' | 'aggregate'
	) => {
		if (target === 'actor' && !event.actor_id) return;

		this.s.viewDraft = {
			...this.s.viewDraft,
			view_type: 'timeline',
			timelineSource: 'events',
			timelineEventLayout: this.selectedTimelineEventLayout(),
			timelineEventType: target === 'type' ? event.event_type : this.s.viewDraft.timelineEventType,
			timelineEventActor:
				target === 'actor' ? (event.actor_id ?? '') : this.s.viewDraft.timelineEventActor,
			timelineEventAggregate:
				target === 'aggregate'
					? timelineEventAggregateKey(event)
					: this.s.viewDraft.timelineEventAggregate,
			timelineTitleField: '',
			timelineSubtitleField: ''
		};

		toast.success(translate('forms.timelineEventFilterApplied'));
	};

	applyTimelineEventDayWindow = (event: BusinessEvent) => {
		const eventDate = new Date(event.created_at);

		if (!Number.isFinite(eventDate.getTime())) return;

		const dayStart = new Date(eventDate);

		dayStart.setHours(0, 0, 0, 0);

		const dayEnd = new Date(eventDate);

		dayEnd.setHours(23, 59, 0, 0);

		this.s.viewDraft = {
			...this.s.viewDraft,
			view_type: 'timeline',
			timelineSource: 'events',
			timelineEventLayout: this.selectedTimelineEventLayout(),
			timelineEventFrom: timelineEventDateTimeLocal(dayStart),
			timelineEventTo: timelineEventDateTimeLocal(dayEnd),
			timelineTitleField: '',
			timelineSubtitleField: ''
		};

		toast.success(translate('forms.timelineEventDateWindowApplied'));
	};

	updateTimelineRecordSelectField = async (record: FormRecord, field: FormField, value: string) => {
		if (!value || record.values[field.key] === value) return;

		this.s.updatingTimelineRecordId = record.id;

		const response = await this.fp.session.updateRecordWithVersion(record.id, {
			values: { ...record.values, [field.key]: value },
			source: { type: 'web', origin: 'timeline' }
		});

		this.s.updatingTimelineRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		toast.success(translate('forms.timelineFieldUpdated'));
		await this.fp.loaders.loadAggregates();
	};

	viewTimelineTitleField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'timeline_title_field')
		);

		if (configured) return configured;

		return null;
	};

	viewTimelineSubtitleField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'timeline_subtitle_field')
		);

		if (configured && configured.key !== this.viewTimelineTitleField()?.key) return configured;

		return null;
	};

	selectedTimelineSource = (): ViewTimelineSource => {
		return viewTimelineSource(this.s.selectedView?.config?.timeline_source);
	};

	selectedTimelineEventLayout = (): TimelineEventLayout => {
		return timelineEventLayout(this.s.selectedView?.config?.timeline_event_layout);
	};

	selectedTimelineEventType = (): string => {
		return viewConfigString(this.s.selectedView, 'timeline_event_type');
	};

	selectedTimelineEventActor = (): string => {
		return viewConfigString(this.s.selectedView, 'timeline_event_actor');
	};

	selectedTimelineEventAggregate = (): string => {
		return viewConfigString(this.s.selectedView, 'timeline_event_aggregate');
	};

	selectedTimelineEventQuery = (): string => {
		return viewConfigString(this.s.selectedView, 'timeline_event_query').trim();
	};

	selectedTimelineEventFrom = (): string => {
		return viewConfigString(this.s.selectedView, 'timeline_event_from').trim();
	};

	selectedTimelineEventTo = (): string => {
		return viewConfigString(this.s.selectedView, 'timeline_event_to').trim();
	};

	timelineTitle = (record: FormRecord): string => {
		const value = recordDisplay(record, this.viewTimelineTitleField());

		return value && value !== '-' ? value : record.title;
	};

	timelineSubtitle = (record: FormRecord): string => {
		const value = recordDisplay(record, this.viewTimelineSubtitleField());

		return value && value !== '-'
			? value
			: this.s.cardFields[0]
				? recordDisplay(record, this.s.cardFields[0])
				: record.id.slice(0, 8);
	};

	timelineEventRecord = (event: BusinessEvent): FormRecord | null => {
		if (event.aggregate_type !== 'form_record') return null;

		return this.s.records.find((record) => record.id === event.aggregate_id) ?? null;
	};

	filteredTimelineEvents = (sourceEvents: BusinessEvent[]): BusinessEvent[] => {
		const eventType = this.selectedTimelineEventType();
		const eventActor = this.selectedTimelineEventActor();
		const eventAggregate = this.selectedTimelineEventAggregate();
		const eventQuery = this.selectedTimelineEventQuery().toLowerCase();
		const eventFrom = timelineEventBoundary(this.selectedTimelineEventFrom());
		const eventTo = timelineEventBoundary(this.selectedTimelineEventTo());

		return sourceEvents.filter((event) => {
			const eventTime = new Date(event.created_at).getTime();

			if (eventType && event.event_type !== eventType) return false;
			if (eventActor && event.actor_id !== eventActor) return false;
			if (eventAggregate && timelineEventAggregateKey(event) !== eventAggregate) return false;
			if (eventQuery && !timelineEventSearchText(event).includes(eventQuery)) return false;
			if (eventFrom !== null && Number.isFinite(eventTime) && eventTime < eventFrom) return false;
			if (eventTo !== null && Number.isFinite(eventTime) && eventTime > eventTo) return false;

			return true;
		});
	};
}
