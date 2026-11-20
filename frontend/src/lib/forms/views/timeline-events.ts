import type { BusinessEvent } from '#lib/api/forms.js';
import type { TimelineEventLayout } from '#lib/forms/page-types.js';
import { translate } from '#lib/forms/i18n.js';
import { csvCell } from '#lib/forms/import-csv.js';
import { eventValue, isRecord } from '#lib/forms/record-format.js';

export function timelineEventAggregateKeys(events: BusinessEvent[]): string[] {
	return Array.from(
		new Set(events.map((event) => timelineEventAggregateKey(event)).filter(Boolean))
	);
}

export function timelineEventDateTimeLocal(date: Date): string {
	const localDate = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);

	return localDate.toISOString().slice(0, 16);
}

export function timelineEventLayout(value: unknown): TimelineEventLayout {
	return value === 'detailed' ? 'detailed' : 'summary';
}

export function timelineEventsCsv(events: BusinessEvent[]): string {
	const header = [
		'Event ID',
		'Type',
		'Title',
		'Subtitle',
		'Aggregate Type',
		'Aggregate ID',
		'Actor',
		'Created At',
		'Payload',
		'Metadata',
		'Source'
	]
		.map(csvCell)
		.join(',');

	const rows = events.map((event) =>
		[
			event.id,
			event.event_type,
			timelineEventTitle(event),
			timelineEventSubtitle(event),
			event.aggregate_type,
			event.aggregate_id,
			event.actor_id ?? '',
			event.created_at,
			timelineEventJson(event.payload),
			timelineEventJson(event.metadata),
			timelineEventJson(event.source)
		]
			.map(csvCell)
			.join(',')
	);

	return [header, ...rows].join('\n');
}

export function timelineEventAggregateKey(event: BusinessEvent): string {
	return `${event.aggregate_type}:${event.aggregate_id}`;
}

export function timelineEventTitle(event: BusinessEvent): string {
	return eventValue(event, ['title', 'record_title', 'form_name']) || event.event_type;
}

export function timelineEventSubtitle(event: BusinessEvent): string {
	const actor = event.actor_id ? `${translate('forms.eventActor')} ${event.actor_id}` : '';
	const aggregate = timelineEventAggregateKey(event);

	return actor ? `${aggregate} · ${actor}` : aggregate;
}

export function timelineEventJson(value: unknown): string {
	if (!value || (isRecord(value) && Object.keys(value).length === 0)) return '{}';

	return JSON.stringify(value, null, 2);
}

export function timelineEventDetailsPayload(event: BusinessEvent) {
	return {
		id: event.id,
		event_type: event.event_type,
		created_at: event.created_at,
		aggregate_type: event.aggregate_type,
		aggregate_id: event.aggregate_id,
		actor_id: event.actor_id,
		title: timelineEventTitle(event),
		subtitle: timelineEventSubtitle(event),
		payload: event.payload,
		metadata: event.metadata,
		source: event.source
	};
}

export function timelineEventDetailsJson(event: BusinessEvent): string {
	return JSON.stringify(timelineEventDetailsPayload(event), null, 2);
}

export function timelineEventsDetailsJson(events: BusinessEvent[]): string {
	return JSON.stringify(
		events.map((event) => timelineEventDetailsPayload(event)),
		null,
		2
	);
}

export function timelineEventSearchText(event: BusinessEvent): string {
	return [
		event.id,
		event.event_type,
		event.aggregate_type,
		event.aggregate_id,
		event.actor_id ?? '',
		timelineEventTitle(event),
		timelineEventSubtitle(event),
		timelineEventJson(event.payload),
		timelineEventJson(event.metadata),
		timelineEventJson(event.source)
	]
		.join('\n')
		.toLowerCase();
}

export function eventsByCreatedAt(sourceEvents: BusinessEvent[]): BusinessEvent[] {
	return [...sourceEvents].sort(
		(left, right) => new Date(right.created_at).getTime() - new Date(left.created_at).getTime()
	);
}

export function eventTypes(sourceEvents: BusinessEvent[]): string[] {
	return Array.from(new Set(sourceEvents.map((event) => event.event_type).filter(Boolean))).sort();
}

export function eventActors(sourceEvents: BusinessEvent[]): string[] {
	return Array.from(
		new Set(
			sourceEvents
				.map((event) => event.actor_id)
				.filter((actorId): actorId is string => typeof actorId === 'string' && actorId.length > 0)
		)
	).sort();
}

export function eventAggregates(sourceEvents: BusinessEvent[]): string[] {
	return Array.from(new Set(sourceEvents.map((event) => timelineEventAggregateKey(event)))).sort();
}

export function timelineEventBoundary(value: string): number | null {
	const trimmed = value.trim();

	if (!trimmed) return null;

	const parsed = new Date(trimmed).getTime();

	return Number.isFinite(parsed) ? parsed : null;
}
