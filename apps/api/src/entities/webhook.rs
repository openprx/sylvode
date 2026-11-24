use platform::timestamp::Timestamp;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "webhooks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub url: String,
    #[serde(skip_serializing)]
    pub secret: String,
    pub events: Json,
    pub active: bool,
    pub bot_user_id: Option<Uuid>,
    pub created_by: Uuid,
    #[serde(with = "platform::timestamp::with_offset")]
    pub created_at: Timestamp,
    #[serde(with = "platform::timestamp::with_offset")]
    pub updated_at: Timestamp,
    #[serde(default, with = "platform::timestamp::with_offset::option")]
    pub last_triggered_at: Option<Timestamp>,
    pub metadata: Option<Json>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::workspace::Entity",
        from = "Column::WorkspaceId",
        to = "super::workspace::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Workspace,
}

impl Related<super::workspace::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Workspace.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

// Webhook delivery record (not using SeaORM entity for now)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebhookDelivery {
    pub id: Uuid,
    pub webhook_id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub request_headers: Option<serde_json::Value>,
    pub response_status: Option<i32>,
    pub response_body: Option<String>,
    pub error: Option<String>,
    #[serde(default, with = "platform::timestamp::with_offset::option")]
    pub delivered_at: Option<Timestamp>,
    #[serde(with = "platform::timestamp::with_offset")]
    pub created_at: Timestamp,
    pub retry_count: i32,
    #[serde(default, with = "platform::timestamp::with_offset::option")]
    pub next_retry_at: Option<Timestamp>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    pub name: Option<String>,
    pub url: String,
    pub secret: Option<String>,
    pub events: Vec<String>,
    pub active: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    pub bot_user_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWebhookRequest {
    pub name: Option<String>,
    pub url: Option<String>,
    pub secret: Option<String>,
    pub events: Option<Vec<String>>,
    pub active: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    pub bot_user_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebhookResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
    pub active: bool,
    pub bot_user_id: Option<Uuid>,
    pub created_by: Uuid,
    #[serde(with = "platform::timestamp::with_offset")]
    pub created_at: Timestamp,
    #[serde(with = "platform::timestamp::with_offset")]
    pub updated_at: Timestamp,
    #[serde(default, with = "platform::timestamp::with_offset::option")]
    pub last_triggered_at: Option<Timestamp>,
}

impl From<Model> for WebhookResponse {
    fn from(model: Model) -> Self {
        let events = serde_json::from_value(model.events).unwrap_or_default();
        Self {
            id: model.id,
            workspace_id: model.workspace_id,
            name: model.name,
            url: model.url,
            events,
            active: model.active,
            bot_user_id: model.bot_user_id,
            created_by: model.created_by,
            created_at: model.created_at,
            updated_at: model.updated_at,
            last_triggered_at: model.last_triggered_at,
        }
    }
}

// Available webhook events, in the order of the web UI's groups (`WEBHOOK_EVENT_GROUPS`).
//
// Issue, comment, label, sprint, milestone, AI, governance, project and member events are sent by
// `webhook_trigger`. The form record events and the Flow events are delivered by the event
// dispatcher (`events::dispatcher`), which retries and carries the `openpr.event.v1` envelope.
pub const WEBHOOK_EVENTS: &[&str] = &[
    // Work items
    "issue.created",
    "issue.updated",
    "issue.assigned",
    "issue.deleted",
    "issue.state_changed",
    // Comments
    "comment.created",
    "comment.updated",
    "comment.deleted",
    // Labels
    "label.added",
    "label.removed",
    // Sprints and milestones
    "sprint.started",
    "sprint.completed",
    "sprint.created",
    "sprint.updated",
    "sprint.cancelled",
    "sprint.deleted",
    "milestone.created",
    "milestone.updated",
    "milestone.deleted",
    // AI tasks
    "ai.task_completed",
    "ai.task_failed",
    // Governance
    "proposal.created",
    "proposal.updated",
    "proposal.deleted",
    "proposal.submitted",
    "proposal.voting_started",
    "proposal.archived",
    "proposal.vote_cast",
    "veto.exercised",
    "veto.withdrawn",
    "escalation.started",
    "appeal.created",
    "governance_config.updated",
    // Projects and members
    "project.created",
    "project.updated",
    "project.deleted",
    "member.added",
    "member.removed",
    // Forms (dispatcher; ids and changed field keys, never values)
    "form.record.created",
    "form.record.updated",
    "form.record.archived",
    "form.record.restored",
    // Flow (dispatcher; each has a public payload policy in `flow::event_policy`)
    "flow.object.created",
    "flow.object.archived",
    "flow.object.restored",
    "flow.object.moved",
    "flow.content.accepted",
    "flow.relation.linked",
    "flow.relation.unlinked",
    "flow.record.created",
    "flow.record.updated",
    "flow.record.archived",
    "flow.schema.changed",
    "flow.view.created",
    "flow.view.updated",
    "flow.view.reordered",
    "flow.reference.created",
    "flow.reference.removed",
    "flow.comment.created",
    "flow.comment.updated",
    "flow.comment.deleted",
    "flow.feature.enabled",
    "flow.feature.disabled",
];

#[cfg(test)]
mod tests {
    use super::WEBHOOK_EVENTS;

    #[test]
    fn the_list_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for event in WEBHOOK_EVENTS {
            assert!(seen.insert(*event), "{event} is listed twice");
        }
    }

    /// The form events in the list are exactly the ones the dispatcher delivers.
    #[test]
    fn the_form_events_are_the_delivered_form_events() {
        let listed: Vec<&str> = WEBHOOK_EVENTS
            .iter()
            .copied()
            .filter(|event| event.starts_with("form."))
            .collect();
        assert_eq!(listed, crate::forms::event_redaction::DELIVERED_FORM_EVENT_TYPES);
    }

    /// The web UI's webhook form offers exactly the events this list accepts, in this order.
    ///
    /// The form used to hard-code three of them, so eleven subscribable events could only be
    /// reached through the API. Its list now lives in one frontend constant, which this test
    /// reads: adding an event on either side without the other fails here.
    #[test]
    fn frontend_webhook_events_match_the_api_list() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../frontend/src/lib/webhooks/events.ts");
        let source = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
        let start = source
            .find("export const WEBHOOK_EVENT_GROUPS")
            .unwrap_or_else(|| panic!("{path} no longer defines WEBHOOK_EVENT_GROUPS"));
        let end = source[start..].find("] as const;").map_or_else(
            || panic!("{path}: WEBHOOK_EVENT_GROUPS is not closed by `] as const;`"),
            |offset| start + offset,
        );
        let frontend: Vec<&str> = source[start..end]
            .split('\'')
            .skip(1)
            .step_by(2)
            .filter(|literal| literal.contains('.'))
            .collect();
        assert_eq!(
            frontend, WEBHOOK_EVENTS,
            "frontend/src/lib/webhooks/events.ts and WEBHOOK_EVENTS disagree"
        );
    }
}
