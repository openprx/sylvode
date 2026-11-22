//! Project execution settings: where and how a runner executes a project's agent tasks.
//!
//! One optional row per project (`project_execution_settings`). The database mirrors every rule
//! checked here with a constraint, so a path that bypasses this module still cannot store a
//! relative working directory or an unknown channel. This module adds what the database cannot
//! see: the workflow states of the project and the labels of its workspace.

use platform::{app::AppState, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    services::workflow_service::resolve_effective_workflow_for_project,
};

/// The channels a runner can use to run a task.
pub const CHANNELS: [&str; 2] = ["tmux", "headless"];
/// The command line agents a runner can start.
pub const CLIS: [&str; 3] = ["claude", "codex", "opencode"];
/// Longest accepted working directory, in bytes.
const MAX_WORKING_DIRECTORY_BYTES: usize = 1024;
/// Longest workflow state key a `state:<key>` value can name.
const MAX_STATE_KEY_CHARS: usize = 64;

const SETTINGS_COLUMNS: &str = "enabled, channel, cli, working_directory, on_complete_state, \
                                on_fail_label_id, approval_gate, version, updated_by, updated_at";

/// The stored settings of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromQueryResult)]
pub struct ExecutionSettings {
    pub enabled: bool,
    pub channel: String,
    pub cli: String,
    pub working_directory: String,
    pub on_complete_state: String,
    pub on_fail_label_id: Option<Uuid>,
    pub approval_gate: String,
    pub version: i32,
    pub updated_by: Option<Uuid>,
    pub updated_at: Timestamp,
}

/// The settings a connection asks for, before [`validate`]. The working directory is required.
#[derive(Debug)]
pub struct ExecutionSettingsInput {
    pub enabled: bool,
    pub channel: String,
    pub cli: String,
    pub working_directory: Option<String>,
    pub on_complete_state: String,
    pub on_fail_label_id: Option<Uuid>,
    pub approval_gate: String,
}

/// Settings that passed [`validate`], ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidSettings {
    pub enabled: bool,
    pub channel: String,
    pub cli: String,
    pub working_directory: String,
    pub on_complete_state: String,
    pub on_fail_label_id: Option<Uuid>,
    pub approval_gate: String,
}

/// The project a settings call is about.
#[derive(Debug, Clone, Copy, FromQueryResult)]
pub struct ProjectRef {
    pub id: Uuid,
    pub workspace_id: Uuid,
}

/// What a save did.
#[derive(Debug)]
pub enum SaveOutcome {
    Saved {
        before: Option<ExecutionSettings>,
        after: ExecutionSettings,
    },
    /// The caller's `expected_version` does not match the stored row (or the existence of one).
    Conflict { current: Option<ExecutionSettings> },
}

fn invalid(field: &str, why: &str) -> ApiError {
    ApiError::BadRequest(format!("{field}: {why}"))
}

/// Looks the project up, or answers 404.
pub async fn project_ref<C: ConnectionTrait>(db: &C, project_id: Uuid) -> Result<ProjectRef, ApiError> {
    ProjectRef::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))
}

/// The settings of a project, `None` when it has none.
pub async fn load<C: ConnectionTrait>(db: &C, project_id: Uuid) -> Result<Option<ExecutionSettings>, ApiError> {
    Ok(ExecutionSettings::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {SETTINGS_COLUMNS} FROM project_execution_settings WHERE project_id = $1"),
        vec![project_id.into()],
    ))
    .one(db)
    .await?)
}

/// What a name or a path is refused with when it holds a character that hides or reorders text.
pub const HIDDEN_CHARS_MESSAGE: &str = "must not contain control, bidirectional or zero-width characters";

/// A control character, or one that reorders or hides text: zero-width and invisible format
/// characters, the bidirectional marks, embeddings, overrides and isolates, and the byte-order mark.
#[must_use]
pub const fn is_hidden_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00ad}'
                | '\u{061c}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{feff}'
        )
}

/// Normalises and checks a working directory on the runner machine.
///
/// One trailing `/` is stripped. The result is absolute, 2 to 1024 bytes, free of control
/// characters, `.` and `..` segments and `//`, and does not end in `/`. The API cannot see the
/// runner's file system, so whether the directory exists is the runner's question.
pub fn normalize_working_directory(raw: &str) -> Result<String, &'static str> {
    let path = raw.strip_suffix('/').unwrap_or(raw);
    if !path.starts_with('/') {
        return Err("must be an absolute path");
    }
    if path.len() < 2 || path.len() > MAX_WORKING_DIRECTORY_BYTES {
        return Err("must be 2 to 1024 bytes long");
    }
    if path.contains('#') {
        return Err("must not contain #");
    }
    if path.chars().any(is_hidden_char) {
        return Err(HIDDEN_CHARS_MESSAGE);
    }
    if path.contains("//") || path.ends_with('/') {
        return Err("must not contain empty segments or end in a second slash");
    }
    if path.split('/').any(|segment| segment == "." || segment == "..") {
        return Err("must not contain . or .. segments");
    }
    Ok(path.to_string())
}

/// The workflow state key of a `state:<key>` value, `None` for `auto` and `none`.
fn complete_state_key(value: &str) -> Result<Option<&str>, &'static str> {
    if matches!(value, "auto" | "none") {
        return Ok(None);
    }
    let key = value
        .strip_prefix("state:")
        .ok_or("must be auto, none or state:<key>")?;
    let well_formed = !key.is_empty()
        && key.len() <= MAX_STATE_KEY_CHARS
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if well_formed {
        Ok(Some(key))
    } else {
        Err("state key must be 1 to 64 characters of a-z, 0-9 and _")
    }
}

/// Checks the settings of a connection against the rules of the settings and the project's workflow and
/// workspace. The first refusal names its field.
pub async fn validate(
    state: &AppState,
    project: &ProjectRef,
    input: &ExecutionSettingsInput,
) -> Result<ValidSettings, ApiError> {
    let working_directory = input
        .working_directory
        .as_deref()
        .ok_or_else(|| invalid("working_directory", "is required"))?;
    let working_directory =
        normalize_working_directory(working_directory).map_err(|why| invalid("working_directory", why))?;
    if !CHANNELS.contains(&input.channel.as_str()) {
        return Err(invalid("channel", "must be tmux or headless"));
    }
    if !CLIS.contains(&input.cli.as_str()) {
        return Err(invalid("cli", "must be claude, codex or opencode"));
    }
    if input.approval_gate != "none" {
        return Err(invalid("approval_gate", "must be none"));
    }
    if let Some(key) = complete_state_key(&input.on_complete_state).map_err(|why| invalid("on_complete_state", why))? {
        let workflow = resolve_effective_workflow_for_project(state, project.id).await?;
        if !workflow.states.iter().any(|candidate| candidate.key == key) {
            return Err(invalid(
                "on_complete_state",
                &format!("state {key} is not in the project's workflow"),
            ));
        }
    }
    if let Some(label_id) = input.on_fail_label_id {
        let found = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM labels WHERE id = $1 AND workspace_id = $2",
                vec![label_id.into(), project.workspace_id.into()],
            ))
            .await?
            .is_some();
        if !found {
            return Err(invalid("on_fail_label_id", "is not a label of the project's workspace"));
        }
    }
    Ok(ValidSettings {
        enabled: input.enabled,
        channel: input.channel.clone(),
        cli: input.cli.clone(),
        working_directory,
        on_complete_state: input.on_complete_state.clone(),
        on_fail_label_id: input.on_fail_label_id,
        approval_gate: input.approval_gate.clone(),
    })
}

/// Creates or updates the settings as a compare-and-set on `version`, and records
/// `project.execution_settings_updated` on `db`.
///
/// The project connection service is the only caller: it passes the transaction that holds the
/// connection, so the settings change commits or rolls back with it. Without a stored row the
/// caller must send no `expected_version`; with one it must send the stored version. Any other
/// combination is a [`SaveOutcome::Conflict`] and nothing is written.
pub async fn upsert(
    db: &impl ConnectionTrait,
    project: &ProjectRef,
    valid: &ValidSettings,
    expected_version: Option<i32>,
    actor_id: Uuid,
) -> Result<SaveOutcome, ApiError> {
    let before = ExecutionSettings::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {SETTINGS_COLUMNS} FROM project_execution_settings WHERE project_id = $1 FOR UPDATE"),
        vec![project.id.into()],
    ))
    .one(db)
    .await?;

    let saved = match (&before, expected_version) {
        (None, None) => {
            ExecutionSettings::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "INSERT INTO project_execution_settings \
                     (project_id, enabled, channel, cli, working_directory, on_complete_state, \
                      on_fail_label_id, approval_gate, updated_by) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                     ON CONFLICT (project_id) DO NOTHING \
                     RETURNING {SETTINGS_COLUMNS}"
                ),
                vec![
                    project.id.into(),
                    valid.enabled.into(),
                    valid.channel.clone().into(),
                    valid.cli.clone().into(),
                    valid.working_directory.clone().into(),
                    valid.on_complete_state.clone().into(),
                    valid.on_fail_label_id.into(),
                    valid.approval_gate.clone().into(),
                    actor_id.into(),
                ],
            ))
            .one(db)
            .await?
        }
        (Some(current), Some(expected)) if current.version == expected => {
            ExecutionSettings::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "UPDATE project_execution_settings \
                     SET enabled = $2, channel = $3, cli = $4, working_directory = $5, \
                         on_complete_state = $6, on_fail_label_id = $7, approval_gate = $8, \
                         version = version + 1, updated_by = $9, updated_at = now() \
                     WHERE project_id = $1 AND version = $10 \
                     RETURNING {SETTINGS_COLUMNS}"
                ),
                vec![
                    project.id.into(),
                    valid.enabled.into(),
                    valid.channel.clone().into(),
                    valid.cli.clone().into(),
                    valid.working_directory.clone().into(),
                    valid.on_complete_state.clone().into(),
                    valid.on_fail_label_id.into(),
                    valid.approval_gate.clone().into(),
                    actor_id.into(),
                    expected.into(),
                ],
            ))
            .one(db)
            .await?
        }
        _ => None,
    };

    let Some(after) = saved else {
        let current = load(db, project.id).await?;
        return Ok(SaveOutcome::Conflict { current });
    };

    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: project.workspace_id,
            project_id: Some(project.id),
            event_type: "project.execution_settings_updated".to_string(),
            aggregate_type: "project".to_string(),
            aggregate_id: project.id.to_string(),
            actor_id: Some(actor_id),
            source: json!({ "type": "api", "actor_id": actor_id }),
            payload: json!({ "before": before, "after": after, "actor_id": actor_id }),
            metadata: json!({
                "project_id": project.id,
                "version": after.version,
                "managed_by_connection": true
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(SaveOutcome::Saved { before, after })
}

#[cfg(test)]
mod tests {
    use super::{complete_state_key, normalize_working_directory};

    #[test]
    fn working_directories_are_checked_and_normalised() {
        let long = format!("/{}", "a".repeat(1024));
        for bad in [
            "relative",
            "",
            "/",
            "//",
            "//a",
            "/a//b",
            "/a/../b",
            "/a/./b",
            "/a/..",
            "/a/.",
            "/a\u{1}b",
            "/a\nb",
            "/a#b",
            "/a\u{202e}b",
            "/a\u{200b}b",
            "/a\u{feff}b",
            "/a\u{2066}b",
            "/a//",
            long.as_str(),
        ] {
            assert!(normalize_working_directory(bad).is_err(), "{bad:?} must be refused");
        }
        for (raw, stored) in [
            ("/a", "/a"),
            ("/a/b_c", "/a/b_c"),
            ("/a/b/", "/a/b"),
            ("/a/.hidden", "/a/.hidden"),
            ("/a/b..c", "/a/b..c"),
        ] {
            assert_eq!(normalize_working_directory(raw).as_deref(), Ok(stored), "{raw:?}");
        }
        let longest = format!("/{}", "a".repeat(1023));
        assert!(normalize_working_directory(&longest).is_ok(), "1024 bytes is allowed");
    }

    #[test]
    fn complete_states_are_auto_none_or_a_state_key() {
        assert_eq!(complete_state_key("auto"), Ok(None));
        assert_eq!(complete_state_key("none"), Ok(None));
        assert_eq!(complete_state_key("state:done"), Ok(Some("done")));
        assert_eq!(complete_state_key("state:in_review_2"), Ok(Some("in_review_2")));
        for bad in ["state:", "state:Bad", "state:a-b", "done", "AUTO", "state:a b"] {
            assert!(complete_state_key(bad).is_err(), "{bad:?} must be refused");
        }
        let too_long = format!("state:{}", "a".repeat(65));
        assert!(complete_state_key(&too_long).is_err());
        let longest = format!("state:{}", "a".repeat(64));
        assert!(complete_state_key(&longest).is_ok());
    }
}
