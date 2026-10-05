/**
 * The webhook events a subscription can name, grouped for the webhook form.
 *
 * This is the frontend's single list. It mirrors `WEBHOOK_EVENTS` in
 * `apps/api/src/entities/webhook.rs`, which the API enforces when a webhook is created or
 * updated, in the same order; the API test `frontend_webhook_events_match_the_api_list` reads
 * this file and fails when the two differ. Each event has a label under
 * `webhook.eventLabel.<event>` and each group a heading under `webhook.eventGroup.<group>` in
 * every locale file (`tests/webhook-events.test.ts`).
 */
export const WEBHOOK_EVENT_GROUPS = [
	{
		group: 'issue',
		events: ['issue.created', 'issue.updated', 'issue.assigned', 'issue.deleted', 'issue.state_changed']
	},
	{ group: 'comment', events: ['comment.created', 'comment.updated', 'comment.deleted'] },
	{ group: 'label', events: ['label.added', 'label.removed'] },
	{ group: 'sprint', events: ['sprint.started', 'sprint.completed'] },
	{ group: 'ai', events: ['ai.task_completed', 'ai.task_failed'] }
] as const;

export type WebhookEventGroup = (typeof WEBHOOK_EVENT_GROUPS)[number]['group'];
export type WebhookEvent = (typeof WEBHOOK_EVENT_GROUPS)[number]['events'][number];

/** Every subscribable event, in the API's order. */
export const WEBHOOK_EVENTS: readonly WebhookEvent[] = WEBHOOK_EVENT_GROUPS.flatMap((entry) => entry.events);

export function isWebhookEvent(value: string): value is WebhookEvent {
	return (WEBHOOK_EVENTS as readonly string[]).includes(value);
}

export function webhookEventLabelKey(event: WebhookEvent): string {
	return `webhook.eventLabel.${event}`;
}

export function webhookEventGroupKey(group: WebhookEventGroup): string {
	return `webhook.eventGroup.${group}`;
}
