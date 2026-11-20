//! What goes on the wire: the event kinds, the envelope, the control messages, and the decision
//! whether a grant may see an event at all.
//!
//! The decision ([`candidacy`]) is a pure function of the grant, the requested types and the
//! columns the feed keeps for routing, so the whole filter matrix is unit-tested without a
//! database. The one question that needs the database (does the user still see the project?) is
//! answered by [`super::visible_projects`] and applied by the caller.

use std::collections::HashMap;

use axum::body::Bytes;
use platform::timestamp::Timestamp;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::oauth::scopes::{ProjectScope, Scope, ScopeSet};

/// The schema name carried by every event.
pub const SPEC: &str = "sylvode.delegated_event.v1";
/// Longest `content` of a notification passed on, in characters.
const MAX_CONTENT_CHARS: usize = 4000;
/// Longest task summary, in characters (contract section 9.4).
const MAX_SUMMARY_CHARS: usize = 2000;
/// Longest task error, in characters (contract section 9.4).
const MAX_ERROR_CHARS: usize = 500;

/// The four event types of the first release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    NotificationCreated,
    NotificationRead,
    TaskCreated,
    TaskStatusChanged,
}

impl EventKind {
    pub const ALL: [Self; 4] = [
        Self::NotificationCreated,
        Self::NotificationRead,
        Self::TaskCreated,
        Self::TaskStatusChanged,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotificationCreated => "notification.created",
            Self::NotificationRead => "notification.read",
            Self::TaskCreated => "task.created",
            Self::TaskStatusChanged => "task.status_changed",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// The scope a grant needs to receive this type.
    pub const fn required_scope(self) -> Scope {
        match self {
            Self::NotificationCreated | Self::NotificationRead => Scope::NotificationsRead,
            Self::TaskCreated | Self::TaskStatusChanged => Scope::TasksRead,
        }
    }

    pub const fn is_notification(self) -> bool {
        matches!(self, Self::NotificationCreated | Self::NotificationRead)
    }

    const fn bit(self) -> u8 {
        match self {
            Self::NotificationCreated => 1,
            Self::NotificationRead => 2,
            Self::TaskCreated => 4,
            Self::TaskStatusChanged => 8,
        }
    }
}

/// The types a stream asked for (`types=`), already narrowed to what the token's scopes allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeFilter(u8);

impl TypeFilter {
    /// Every type `scopes` allows.
    pub fn permitted_by(scopes: ScopeSet) -> Self {
        Self::from_kinds(
            EventKind::ALL
                .into_iter()
                .filter(|kind| scopes.allows(kind.required_scope())),
        )
    }

    pub fn from_kinds(kinds: impl IntoIterator<Item = EventKind>) -> Self {
        Self(kinds.into_iter().fold(0, |bits, kind| bits | kind.bit()))
    }

    pub const fn contains(self, kind: EventKind) -> bool {
        self.0 & kind.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// What the delivery decision needs to know about a grant. Rebuilt from the database at most
/// every few seconds, so a change of the project set or of scope reaches a stream quickly.
#[derive(Clone, Debug)]
pub struct GrantView {
    pub grant_id: Uuid,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub scopes: ScopeSet,
    pub projects: ProjectScope,
}

/// The columns of a feed row that decide who may see it.
#[derive(Clone, Copy, Debug)]
pub struct Routing<'a> {
    pub event_type: &'a str,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
}

/// The cheap part of the delivery decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Candidacy {
    /// The grant may not see the event, whatever the database says.
    Refused,
    /// The event concerns no project and the grant reaches all of them: deliverable.
    Unscoped,
    /// Deliverable only if the user still sees this project (checked against the database).
    Project(Uuid),
}

/// Steps 2 to 5 of the delivery filter (contract section 9.7), minus the live visibility lookup:
///
/// 2. the event belongs to the grant's workspace;
/// 3. the type is wanted and the token's scopes allow it;
/// 4. a notification belongs to the grant's user, and its project (resolved at production time
///    from the notification itself or its work item) is in the grant's project set; when it
///    resolves to no project only a grant over all projects sees it;
/// 5. a task needs a project in the project set.
pub fn candidacy(grant: &GrantView, types: TypeFilter, routing: Routing<'_>) -> Candidacy {
    let Some(kind) = EventKind::from_name(routing.event_type) else {
        return Candidacy::Refused;
    };
    if routing.workspace_id != grant.workspace_id
        || !types.contains(kind)
        || !grant.scopes.allows(kind.required_scope())
    {
        return Candidacy::Refused;
    }
    if kind.is_notification() && routing.user_id != Some(grant.user_id) {
        return Candidacy::Refused;
    }
    match routing.project_id {
        Some(project) if grant.projects.contains(project) => Candidacy::Project(project),
        None if kind.is_notification() && matches!(grant.projects, ProjectScope::All) => Candidacy::Unscoped,
        Some(_) | None => Candidacy::Refused,
    }
}

/// A project as a stream shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectInfo {
    pub key: String,
    pub name: String,
}

/// One row of the feed.
#[derive(Clone, Debug)]
pub struct FeedRow {
    pub seq: i64,
    pub event_type: String,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub subject: Value,
    pub occurred_at: Timestamp,
    pub created_at: Timestamp,
}

impl FeedRow {
    pub fn routing(&self) -> Routing<'_> {
        Routing {
            event_type: &self.event_type,
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            user_id: self.user_id,
        }
    }
}

fn text(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

fn limited(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn copy(into: &mut Map<String, Value>, from: &Value, key: &str) {
    if let Some(value) = from.get(key) {
        into.insert(key.to_owned(), value.clone());
    }
}

fn pick(from: &Value, keys: &[&str]) -> Value {
    let mut out = Map::new();
    for key in keys {
        copy(&mut out, from, key);
    }
    Value::Object(out)
}

/// The subject of an event, reduced to the fields the frozen v1 shape names. Whatever else the
/// feed row holds does not leave the process.
pub fn frozen_subject(kind: EventKind, stored: &Value) -> Value {
    match kind {
        EventKind::NotificationCreated => {
            let mut out = Map::new();
            out.insert("kind".to_owned(), json!("notification"));
            for key in ["notification_id", "notification_type", "title_key"] {
                copy(&mut out, stored, key);
            }
            if let Some(content) = text(stored.get("content")) {
                out.insert("content".to_owned(), json!(limited(content, MAX_CONTENT_CHARS)));
            }
            out.insert(
                "related".to_owned(),
                stored.get("related").map_or_else(
                    || json!({"issue_id": null, "comment_id": null, "project_id": null}),
                    |related| {
                        let mut picked = pick(related, &["issue_id", "comment_id", "project_id"]);
                        if let Value::Object(map) = &mut picked {
                            for key in ["issue_id", "comment_id", "project_id"] {
                                map.entry(key.to_owned()).or_insert(Value::Null);
                            }
                        }
                        picked
                    },
                ),
            );
            copy(&mut out, stored, "is_read");
            Value::Object(out)
        }
        EventKind::NotificationRead => {
            let mut out = Map::new();
            out.insert("kind".to_owned(), json!("notification"));
            copy(&mut out, stored, "notification_id");
            out.insert("is_read".to_owned(), json!(true));
            Value::Object(out)
        }
        EventKind::TaskCreated | EventKind::TaskStatusChanged => {
            let mut out = Map::new();
            out.insert("kind".to_owned(), json!("ai_task"));
            for key in ["task_id", "task_type", "status"] {
                copy(&mut out, stored, key);
            }
            if kind == EventKind::TaskStatusChanged {
                copy(&mut out, stored, "previous_status");
            }
            for key in ["attempts", "max_attempts"] {
                copy(&mut out, stored, key);
            }
            if let Some(agent) = stored.get("agent") {
                out.insert("agent".to_owned(), pick(agent, &["id", "name"]));
            }
            if let Some(reference) = stored.get("reference") {
                let keys: &[&str] = if text(reference.get("type")) == Some("work_item") {
                    &["type", "id", "identifier", "title"]
                } else {
                    &["type", "id"]
                };
                out.insert("reference".to_owned(), pick(reference, keys));
            }
            if let Some(summary) = text(stored.get("summary")) {
                out.insert("summary".to_owned(), json!(limited(summary, MAX_SUMMARY_CHARS)));
            }
            if text(stored.get("status")) == Some("failed")
                && let Some(error) = text(stored.get("error"))
            {
                out.insert("error".to_owned(), json!(limited(error, MAX_ERROR_CHARS)));
            }
            Value::Object(out)
        }
    }
}

/// The web link of an event, from the issuer and the UI routes that already exist.
fn web_link(issuer: &str, kind: EventKind, row: &FeedRow, subject: &Value) -> String {
    let base = format!("{issuer}/workspace/{}", row.workspace_id);
    let Some(project) = row.project_id else {
        return base;
    };
    let issue = if kind.is_notification() {
        text(subject.get("related").and_then(|related| related.get("issue_id")))
    } else {
        subject
            .get("reference")
            .filter(|reference| text(reference.get("type")) == Some("work_item"))
            .and_then(|reference| text(reference.get("id")))
    };
    issue.map_or_else(
        || format!("{base}/projects/{project}"),
        |issue| format!("{base}/projects/{project}/issues/{issue}"),
    )
}

pub fn millis(at: Timestamp) -> String {
    Timestamp::from_unix_timestamp_millis(i64::try_from(at.unix_timestamp_millis()).unwrap_or(0))
        .unwrap_or(at)
        .to_string()
}

/// The JSON of one `sv.event` message.
pub fn event_json<S: std::hash::BuildHasher>(
    id: &str,
    kind: EventKind,
    row: &FeedRow,
    projects: &HashMap<Uuid, ProjectInfo, S>,
    issuer: &str,
) -> Value {
    let subject = frozen_subject(kind, &row.subject);
    let project = row
        .project_id
        .and_then(|id| projects.get(&id).map(|info| (id, info)))
        .map_or(
            Value::Null,
            |(id, info)| json!({"id": id, "key": info.key, "name": info.name}),
        );
    let link = web_link(issuer, kind, row, &subject);
    json!({
        "spec": SPEC,
        "id": id,
        "type": kind.as_str(),
        "occurred_at": millis(row.occurred_at),
        "workspace_id": row.workspace_id,
        "project": project,
        "subject": subject,
        "links": {"web": link},
    })
}

/// Frames a stored event: `id`, `event` and a single `data` line.
pub fn event_frame(id: &str, json: &Value) -> Bytes {
    Bytes::from(format!("id: {id}\nevent: sv.event\ndata: {json}\n\n"))
}

/// Frames a control message: no `id`, so it never moves a client's resume position.
pub fn control_frame(json: &Value) -> Bytes {
    Bytes::from(format!("event: sv.control\ndata: {json}\n\n"))
}

/// The idle heartbeat, a comment line every client ignores.
pub const fn keep_alive_frame() -> Bytes {
    Bytes::from_static(b": keep-alive\n\n")
}

/// The control messages of the stream (contract section 9.4).
pub mod control {
    use serde_json::{Value, json};
    use uuid::Uuid;

    pub fn hello(grant_id: Uuid, workspace_id: Uuid, cursor: &str, server_time: &str) -> Value {
        json!({
            "type": "hello",
            "grant_id": grant_id,
            "workspace_id": workspace_id,
            "cursor": cursor,
            "server_time": server_time,
        })
    }

    pub fn resync_required(reason: &str, oldest_available: &str) -> Value {
        json!({"type": "resync_required", "reason": reason, "oldest_available": oldest_available})
    }

    pub fn lagged(last_sent: &str) -> Value {
        json!({"type": "lagged", "last_sent": last_sent})
    }

    pub fn reauth(reason: &str) -> Value {
        json!({"type": "reauth", "reason": reason})
    }

    pub fn revoked(reason: &str) -> Value {
        json!({"type": "revoked", "reason": reason})
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn grant(projects: ProjectScope, scopes: &[Scope]) -> GrantView {
        GrantView {
            grant_id: Uuid::new_v4(),
            user_id: Uuid::from_u128(1),
            workspace_id: Uuid::from_u128(100),
            scopes: ScopeSet::from_scopes(scopes),
            projects,
        }
    }

    fn selected(ids: &[u128]) -> ProjectScope {
        ProjectScope::Selected(ids.iter().map(|id| Uuid::from_u128(*id)).collect())
    }

    const BOTH: [Scope; 2] = [Scope::NotificationsRead, Scope::TasksRead];

    fn route(event_type: &str, workspace: u128, project: Option<u128>, user: Option<u128>) -> Routing<'_> {
        Routing {
            event_type,
            workspace_id: Uuid::from_u128(workspace),
            project_id: project.map(Uuid::from_u128),
            user_id: user.map(Uuid::from_u128),
        }
    }

    fn all_types() -> TypeFilter {
        TypeFilter::from_kinds(EventKind::ALL)
    }

    #[test]
    fn the_filter_matrix() {
        let p1 = Uuid::from_u128(11);
        let both_selected = grant(selected(&[11]), &BOTH);
        let both_all = grant(ProjectScope::All, &BOTH);
        let only_tasks = grant(ProjectScope::All, &[Scope::TasksRead]);
        let only_notifications = grant(ProjectScope::All, &[Scope::NotificationsRead]);

        // Wrong workspace.
        assert_eq!(
            candidacy(&both_all, all_types(), route("task.created", 999, Some(11), None)),
            Candidacy::Refused
        );
        // Wrong user for a notification; the user of a task does not matter.
        assert_eq!(
            candidacy(
                &both_all,
                all_types(),
                route("notification.created", 100, Some(11), Some(2))
            ),
            Candidacy::Refused
        );
        assert_eq!(
            candidacy(
                &both_all,
                all_types(),
                route("notification.created", 100, Some(11), None)
            ),
            Candidacy::Refused
        );
        assert_eq!(
            candidacy(
                &both_all,
                all_types(),
                route("notification.created", 100, Some(11), Some(1))
            ),
            Candidacy::Project(p1)
        );
        assert_eq!(
            candidacy(
                &both_all,
                all_types(),
                route("task.status_changed", 100, Some(11), Some(2))
            ),
            Candidacy::Project(p1)
        );
        // A project outside a selected grant, for both families.
        assert_eq!(
            candidacy(
                &both_selected,
                all_types(),
                route("notification.read", 100, Some(12), Some(1))
            ),
            Candidacy::Refused
        );
        assert_eq!(
            candidacy(&both_selected, all_types(), route("task.created", 100, Some(12), None)),
            Candidacy::Refused
        );
        assert_eq!(
            candidacy(&both_selected, all_types(), route("task.created", 100, Some(11), None)),
            Candidacy::Project(p1)
        );
        // Missing scope.
        assert_eq!(
            candidacy(
                &only_tasks,
                all_types(),
                route("notification.created", 100, Some(11), Some(1))
            ),
            Candidacy::Refused
        );
        assert_eq!(
            candidacy(
                &only_notifications,
                all_types(),
                route("task.created", 100, Some(11), None)
            ),
            Candidacy::Refused
        );
        // An unresolvable project: only an `all` grant sees a notification, and no grant sees a task.
        assert_eq!(
            candidacy(
                &both_all,
                all_types(),
                route("notification.created", 100, None, Some(1))
            ),
            Candidacy::Unscoped
        );
        assert_eq!(
            candidacy(
                &both_selected,
                all_types(),
                route("notification.created", 100, None, Some(1))
            ),
            Candidacy::Refused
        );
        assert_eq!(
            candidacy(&both_all, all_types(), route("task.created", 100, None, None)),
            Candidacy::Refused
        );
        // A type the stream did not ask for, and one this release does not know.
        let tasks_only = TypeFilter::from_kinds([EventKind::TaskCreated]);
        assert_eq!(
            candidacy(
                &both_all,
                tasks_only,
                route("notification.created", 100, Some(11), Some(1))
            ),
            Candidacy::Refused
        );
        assert_eq!(
            candidacy(&both_all, all_types(), route("task.deleted", 100, Some(11), None)),
            Candidacy::Refused
        );
        // An empty selected set reaches nothing.
        let nothing = grant(selected(&[]), &BOTH);
        assert_eq!(
            candidacy(&nothing, all_types(), route("task.created", 100, Some(11), None)),
            Candidacy::Refused
        );
    }

    #[test]
    fn only_the_read_scopes_open_a_type() {
        // `notifications:write` does not imply `notifications:read` in the scope hierarchy.
        let write_only = TypeFilter::permitted_by(ScopeSet::from_scopes(&[Scope::NotificationsWrite]));
        assert!(write_only.is_empty());
        let scopes = ScopeSet::from_scopes(&[Scope::NotificationsRead, Scope::NotificationsWrite]);
        let permitted = TypeFilter::permitted_by(scopes);
        assert!(permitted.contains(EventKind::NotificationCreated));
        assert!(permitted.contains(EventKind::NotificationRead));
        assert!(!permitted.contains(EventKind::TaskCreated));
        assert!(TypeFilter::permitted_by(ScopeSet::from_scopes(&[Scope::ProjectsRead])).is_empty());
        let asked = TypeFilter::from_kinds([EventKind::TaskCreated, EventKind::NotificationRead]);
        assert_eq!(
            permitted.intersect(asked),
            TypeFilter::from_kinds([EventKind::NotificationRead])
        );
    }

    #[test]
    fn kind_names_round_trip() {
        for kind in EventKind::ALL {
            assert_eq!(EventKind::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(EventKind::from_name("notification"), None);
    }

    fn keys(value: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value.as_object().expect("object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn notification_subjects_carry_only_the_frozen_fields() {
        let stored = json!({
            "kind": "notification", "notification_id": "n1", "notification_type": "assignment",
            "title_key": "notification.issueAssignedTitle", "content": "issue_assigned:Investigate",
            "related": {"issue_id": "i1", "comment_id": null, "project_id": "p1", "secret": "x"},
            "is_read": false, "metadata": {"token": "must not leak"}, "link": "/x",
        });
        let created = frozen_subject(EventKind::NotificationCreated, &stored);
        assert_eq!(
            keys(&created),
            [
                "content",
                "is_read",
                "kind",
                "notification_id",
                "notification_type",
                "related",
                "title_key"
            ]
        );
        assert_eq!(keys(&created["related"]), ["comment_id", "issue_id", "project_id"]);
        assert!(!created.to_string().contains("leak"));
        let read = frozen_subject(EventKind::NotificationRead, &stored);
        assert_eq!(keys(&read), ["is_read", "kind", "notification_id"]);
        assert_eq!(read["is_read"], json!(true));
    }

    #[test]
    fn task_subjects_carry_only_the_frozen_fields() {
        let stored = json!({
            "kind": "ai_task", "task_id": "t1", "task_type": "issue_assigned", "status": "completed",
            "previous_status": "acknowledged", "attempts": 1, "max_attempts": 3,
            "agent": {"id": "a1", "name": "Triage agent", "email": "x@y"},
            "reference": {"type": "work_item", "id": "w1", "identifier": "SEC-42", "title": "Investigate", "body": "x"},
            "summary": "done", "error": "ignored unless failed", "payload": {"secret": "x"},
        });
        let status = frozen_subject(EventKind::TaskStatusChanged, &stored);
        assert_eq!(
            keys(&status),
            [
                "agent",
                "attempts",
                "kind",
                "max_attempts",
                "previous_status",
                "reference",
                "status",
                "summary",
                "task_id",
                "task_type"
            ]
        );
        assert_eq!(keys(&status["agent"]), ["id", "name"]);
        assert_eq!(keys(&status["reference"]), ["id", "identifier", "title", "type"]);
        // `previous_status` is absent on a created task, and `error` appears only when failed.
        let created = frozen_subject(EventKind::TaskCreated, &stored);
        assert!(created.get("previous_status").is_none());
        assert!(created.get("error").is_none());
        let failed = frozen_subject(
            EventKind::TaskStatusChanged,
            &json!({"status": "failed", "error": "e".repeat(900), "summary": "s".repeat(3000)}),
        );
        assert_eq!(failed["error"].as_str().map(|e| e.chars().count()), Some(500));
        assert_eq!(failed["summary"].as_str().map(|e| e.chars().count()), Some(2000));
        // Other reference types carry a type and an id only.
        let other = frozen_subject(
            EventKind::TaskCreated,
            &json!({"reference": {"type": "proposal", "id": "p1", "title": "no"}}),
        );
        assert_eq!(keys(&other["reference"]), ["id", "type"]);
    }

    #[test]
    fn the_envelope_has_the_frozen_shape_and_links() {
        let project = Uuid::from_u128(11);
        let row = FeedRow {
            seq: 5,
            event_type: "task.status_changed".to_owned(),
            workspace_id: Uuid::from_u128(100),
            project_id: Some(project),
            user_id: None,
            subject: json!({"reference": {"type": "work_item", "id": "w1", "identifier": "SEC-42", "title": "t"},
                            "task_id": "t1"}),
            occurred_at: Timestamp::from_unix_timestamp_millis(1_795_000_007_120).expect("time"),
            created_at: Timestamp::now(),
        };
        let projects = HashMap::from([(
            project,
            ProjectInfo {
                key: "SEC".to_owned(),
                name: "Security".to_owned(),
            },
        )]);
        let value = event_json(
            "opaque",
            EventKind::TaskStatusChanged,
            &row,
            &projects,
            "https://sv.example.test",
        );
        assert_eq!(
            keys(&value),
            [
                "id",
                "links",
                "occurred_at",
                "project",
                "spec",
                "subject",
                "type",
                "workspace_id"
            ]
        );
        assert_eq!(value["spec"], SPEC);
        assert_eq!(value["id"], "opaque");
        assert_eq!(value["occurred_at"], "2026-11-18T11:06:47.120Z");
        assert_eq!(value["project"]["key"], "SEC");
        assert_eq!(
            value["links"]["web"],
            format!(
                "https://sv.example.test/workspace/{}/projects/{project}/issues/w1",
                row.workspace_id
            )
        );
        // No visible project: null, and the link falls back to the workspace.
        let bare = FeedRow {
            project_id: None,
            ..row.clone()
        };
        let value = event_json(
            "x",
            EventKind::NotificationCreated,
            &bare,
            &HashMap::new(),
            "https://sv.example.test",
        );
        assert!(value["project"].is_null());
        assert_eq!(
            value["links"]["web"],
            format!("https://sv.example.test/workspace/{}", row.workspace_id)
        );
    }

    #[test]
    fn frames_are_single_data_lines() {
        let value = json!({"a": "line one\nline two\r\nline three"});
        let frame = event_frame("abc", &value);
        let text = std::str::from_utf8(&frame).expect("utf8");
        assert!(text.starts_with("id: abc\nevent: sv.event\ndata: {"), "{text}");
        assert!(text.ends_with("\n\n"));
        assert_eq!(text.matches("data:").count(), 1);
        assert_eq!(text.lines().filter(|line| !line.is_empty()).count(), 3, "{text}");
        let control = control_frame(&control::reauth("token_expired"));
        let text = std::str::from_utf8(&control).expect("utf8");
        assert!(!text.contains("\nid:") && !text.starts_with("id:"), "{text}");
        assert!(text.starts_with("event: sv.control\ndata: "));
        assert_eq!(&keep_alive_frame()[..], b": keep-alive\n\n");
    }
}
