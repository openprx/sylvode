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
		events: [
			'issue.created',
			'issue.updated',
			'issue.assigned',
			'issue.deleted',
			'issue.state_changed'
		]
	},
	{ group: 'comment', events: ['comment.created', 'comment.updated', 'comment.deleted'] },
	{ group: 'label', events: ['label.added', 'label.removed'] },
	{
		group: 'planning',
		events: [
			'sprint.started',
			'sprint.completed',
			'sprint.created',
			'sprint.updated',
			'sprint.cancelled',
			'sprint.deleted',
			'milestone.created',
			'milestone.updated',
			'milestone.deleted'
		]
	},
	{ group: 'ai', events: ['ai.task_completed', 'ai.task_failed'] },
	{
		group: 'governance',
		events: [
			'proposal.created',
			'proposal.updated',
			'proposal.deleted',
			'proposal.submitted',
			'proposal.voting_started',
			'proposal.archived',
			'proposal.vote_cast',
			'veto.exercised',
			'veto.withdrawn',
			'escalation.started',
			'appeal.created',
			'governance_config.updated'
		]
	},
	{
		group: 'projects',
		events: [
			'project.created',
			'project.updated',
			'project.deleted',
			'member.added',
			'member.removed'
		]
	},
	{
		group: 'forms',
		events: [
			'form.record.created',
			'form.record.updated',
			'form.record.archived',
			'form.record.restored'
		]
	},
	{
		group: 'flow',
		events: [
			'flow.object.created',
			'flow.object.archived',
			'flow.object.restored',
			'flow.object.moved',
			'flow.content.accepted',
			'flow.relation.linked',
			'flow.relation.unlinked',
			'flow.record.created',
			'flow.record.updated',
			'flow.record.archived',
			'flow.schema.changed',
			'flow.view.created',
			'flow.view.updated',
			'flow.view.reordered',
			'flow.reference.created',
			'flow.reference.removed',
			'flow.comment.created',
			'flow.comment.updated',
			'flow.comment.deleted',
			'flow.feature.enabled',
			'flow.feature.disabled'
		]
	}
] as const;

export type WebhookEventGroup = (typeof WEBHOOK_EVENT_GROUPS)[number]['group'];
export type WebhookEvent = (typeof WEBHOOK_EVENT_GROUPS)[number]['events'][number];

/** Every subscribable event, in the API's order. */
export const WEBHOOK_EVENTS: readonly WebhookEvent[] = WEBHOOK_EVENT_GROUPS.flatMap(
	(entry) => entry.events
);

export function isWebhookEvent(value: string): value is WebhookEvent {
	return (WEBHOOK_EVENTS as readonly string[]).includes(value);
}

export function webhookEventLabelKey(event: WebhookEvent): string {
	return `webhook.eventLabel.${event}`;
}

export function webhookEventGroupKey(group: WebhookEventGroup): string {
	return `webhook.eventGroup.${group}`;
}

/**
 * The selection after the user toggles `event`'s checkbox: added when absent, removed when
 * present, every other selected event kept in place. The webhook form's checkbox handler applies
 * exactly this (`tests/webhook-events.test.ts`).
 */
export function toggleWebhookEvent(selected: readonly string[], event: string): string[] {
	return selected.includes(event)
		? selected.filter((value) => value !== event)
		: [...selected, event];
}
