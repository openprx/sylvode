//! Project bot connections: connect a bot to a project and run its agent tasks on a runner.
//!
//! A connection is derived from rows that already exist (the bot user and `workspace_bots` token,
//! the `webhooks` row, the `ai_participants` registration, `project_execution_settings`) plus two
//! linking tables: `bot_runners` (what is shared by all of a bot's projects) and
//! `project_bot_connections` (the project-to-bot link and its version).
//!
//! Every write here is one transaction: it either leaves every row in place or none. The token and
//! the webhook's signing secret exist only in memory, are returned once inside the configuration
//! block, and appear in no event, log line or error. Business events carry ids, names, the runner
//! address and the token prefix.
//!
//! Lock order, everywhere: the project row (connect only), then the `bot_runners` row, then the
//! `project_bot_connections` row, then the rows that hang off them. A write that changes shared
//! state increments the version of every connection of the bot.

use platform::{app::AppState, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    services::{
        bots::{self, ManagedPolicy, ManagedProject, NewBot},
        execution_settings::{self, ExecutionSettings, ExecutionSettingsInput, ProjectRef, SaveOutcome, ValidSettings},
        runner_config_block::{self, BlockInput, ConfigBlock},
        trust_score_service::normalize_domain_key,
        webhooks::{self, NewWebhook},
    },
};

/// The decision levels an AI participant can be capped at.
pub const LEVELS: [&str; 5] = ["observer", "advisor", "voter", "vetoer", "autonomous"];
const MAX_BOT_NAME_CHARS: usize = 100;
const MAX_RUNNER_URL_BYTES: usize = 2048;
const MAX_DOMAIN_OVERRIDES: usize = 50;
/// The hint added to a refusal of a blocked runner address.
const ALLOWED_HOSTS_HINT: &str = "an administrator can allow this host with [outbound] allowed_hosts";

pub const NO_CONNECTION: &str = "project has no bot connection";
pub const ALREADY_CONNECTED: &str = "project already has a bot connection; reload";
pub const VERSION_CONFLICT: &str = "connection changed; reload";

fn invalid(field: &str, why: &str) -> ApiError {
    ApiError::BadRequest(format!("{field}: {why}"))
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

/// Which bot to connect: exactly one of a new name or the id of a bot that is already connected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotRef {
    pub name: Option<String>,
    pub id: Option<Uuid>,
}

/// Governance fields; every key is optional.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernanceInput {
    pub max_domain_level: Option<String>,
    pub can_veto_human_consensus: Option<bool>,
    pub reason_min_length: Option<i64>,
    pub domain_overrides: Option<Map<String, Value>>,
}

/// `POST /bot-connection`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectRequest {
    pub bot: BotRef,
    pub runner_url: String,
    pub cli: Option<String>,
    pub working_directory: Option<String>,
    pub channel: Option<String>,
    pub on_complete_state: Option<String>,
    pub on_fail_label_id: Option<Uuid>,
    pub governance: Option<GovernanceInput>,
}

/// A key that is present with `null` clears the value; an absent key leaves it alone.
#[allow(
    clippy::option_option,
    reason = "absent, null and a value are three different requests"
)]
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// `PATCH /bot-connection`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRequest {
    pub expected_version: i32,
    pub runner_url: Option<String>,
    pub cli: Option<String>,
    pub working_directory: Option<String>,
    pub channel: Option<String>,
    pub on_complete_state: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[allow(
        clippy::option_option,
        reason = "absent, null and a value are three different requests"
    )]
    pub on_fail_label_id: Option<Option<Uuid>>,
    pub governance: Option<GovernanceInput>,
}

impl UpdateRequest {
    const fn is_empty(&self) -> bool {
        self.runner_url.is_none()
            && self.cli.is_none()
            && self.working_directory.is_none()
            && self.channel.is_none()
            && self.on_complete_state.is_none()
            && self.on_fail_label_id.is_none()
            && self.governance.is_none()
    }
}

// ---------------------------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Governance {
    pub max_domain_level: String,
    pub can_veto_human_consensus: bool,
    pub reason_min_length: i32,
    pub domain_overrides: Map<String, Value>,
}

impl Governance {
    /// The conservative defaults of a new connection.
    fn defaults() -> Self {
        Self {
            max_domain_level: "voter".to_string(),
            can_veto_human_consensus: false,
            reason_min_length: 50,
            domain_overrides: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BotView {
    pub id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub token_active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunnerView {
    pub url: String,
    pub webhook_id: Uuid,
    pub credentials_issued_at: Timestamp,
    /// Every project the bot is connected to, this one included.
    pub shared_with_projects: Vec<ManagedProject>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutionView {
    pub channel: String,
    pub cli: String,
    pub working_directory: String,
    pub on_complete_state: String,
    pub on_fail_label_id: Option<Uuid>,
    pub approval_gate: String,
}

/// The connection as owners and admins see it. Holds no secret: the token and the signing secret
/// are never readable after the answer that issued them.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionView {
    pub project_id: Uuid,
    pub version: i32,
    pub enabled: bool,
    pub bot: BotView,
    pub runner: RunnerView,
    pub execution: ExecutionView,
    pub governance: Governance,
    pub created_by: Option<Uuid>,
    pub created_at: Timestamp,
    pub updated_by: Option<Uuid>,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberBot {
    pub id: Uuid,
    pub name: String,
}

/// The connection as project members see it.
#[derive(Debug, Clone, Serialize)]
pub struct MemberView {
    pub bot: MemberBot,
    pub enabled: bool,
    pub cli: String,
    pub channel: String,
}

/// A connect answer. `config_block` is `None` when an already connected bot was added.
#[derive(Debug)]
pub struct Connected {
    pub connection: ConnectionView,
    pub config_block: Option<ConfigBlock>,
}

#[derive(Debug)]
pub struct Rotated {
    pub connection: ConnectionView,
    pub config_block: ConfigBlock,
}

#[derive(Debug, Serialize)]
pub struct Removed {
    pub removed: bool,
    pub bot_still_connected_to: Vec<ManagedProject>,
}

/// A write that needs the caller's version: it either happened or the version was stale.
#[derive(Debug)]
pub enum Outcome<T> {
    Done(T),
    /// Nothing was written. `current` is the connection as it now stands.
    Conflict {
        current: Box<ConnectionView>,
    },
}

// ---------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------

/// Checks a runner address and returns it as stored: trimmed, one trailing `/` stripped, no query
/// or fragment, at most 2048 bytes. `webhook_url` is what the webhook will be given.
pub fn normalize_runner_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let stored = trimmed.strip_suffix('/').unwrap_or(trimmed);
    if !(stored.starts_with("http://") || stored.starts_with("https://")) {
        return Err("must start with http:// or https://".to_string());
    }
    if stored.len() > MAX_RUNNER_URL_BYTES {
        return Err("must be at most 2048 bytes".to_string());
    }
    if stored.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("must not contain spaces or control characters".to_string());
    }
    if stored.contains('?') || stored.contains('#') {
        return Err("must not contain a query or a fragment".to_string());
    }
    if stored.ends_with('/') {
        return Err("must not end in more than one slash".to_string());
    }
    let after_scheme = stored.split_once("://").map_or("", |(_, rest)| rest);
    let authority = after_scheme.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return Err("must name a host".to_string());
    }
    if authority.contains('@') {
        return Err("must not embed credentials".to_string());
    }
    Ok(stored.to_string())
}

/// Validates a runner address with the outbound policy (`[outbound] allowed_hosts` and the SSRF
/// rules, DNS included) against an explicit allowlist, and returns it as stored.
pub async fn validate_runner_url_with(raw: &str, allowlist: &str, allow_private: bool) -> Result<String, ApiError> {
    let stored = normalize_runner_url(raw).map_err(|why| invalid("runner_url", &why))?;
    crate::outbound::validate_outbound_url_with(&format!("{stored}/webhook"), allowlist, allow_private)
        .await
        .map_err(|why| {
            if why.contains("blocked address") {
                invalid("runner_url", &format!("{why}; {ALLOWED_HOSTS_HINT}"))
            } else {
                invalid("runner_url", &why)
            }
        })?;
    Ok(stored)
}

/// [`validate_runner_url_with`] against the installed configuration.
pub async fn validate_runner_url(raw: &str) -> Result<String, ApiError> {
    let runtime = crate::config::runtime();
    validate_runner_url_with(raw, &runtime.outbound.allowlist_csv(), runtime.outbound.allow_private).await
}

fn validate_bot_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(invalid("bot.name", "required"));
    }
    if name.chars().count() > MAX_BOT_NAME_CHARS {
        return Err(invalid("bot.name", "at most 100 characters"));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("bot.name", "must not contain control characters"));
    }
    Ok(name.to_string())
}

/// Applies `input` over `base` and checks the result.
pub fn validate_governance(base: &Governance, input: &GovernanceInput) -> Result<Governance, ApiError> {
    let mut governance = base.clone();
    if let Some(level) = &input.max_domain_level {
        governance.max_domain_level.clone_from(level);
    }
    if let Some(veto) = input.can_veto_human_consensus {
        governance.can_veto_human_consensus = veto;
    }
    if let Some(length) = input.reason_min_length {
        governance.reason_min_length = i32::try_from(length)
            .ok()
            .filter(|length| (0..=2000).contains(length))
            .ok_or_else(|| invalid("governance.reason_min_length", "must be between 0 and 2000"))?;
    }
    if let Some(overrides) = &input.domain_overrides {
        governance.domain_overrides = validate_domain_overrides(overrides)?;
    }
    if !LEVELS.contains(&governance.max_domain_level.as_str()) {
        return Err(invalid(
            "governance.max_domain_level",
            "must be observer, advisor, voter, vetoer or autonomous",
        ));
    }
    if governance.can_veto_human_consensus && !matches!(governance.max_domain_level.as_str(), "vetoer" | "autonomous") {
        return Err(invalid(
            "governance.can_veto_human_consensus",
            "requires max_domain_level vetoer or autonomous",
        ));
    }
    Ok(governance)
}

fn validate_domain_overrides(overrides: &Map<String, Value>) -> Result<Map<String, Value>, ApiError> {
    let field = "governance.domain_overrides";
    if overrides.len() > MAX_DOMAIN_OVERRIDES {
        return Err(invalid(field, "at most 50 keys"));
    }
    let mut normalized = Map::new();
    for (key, value) in overrides {
        let domain = normalize_domain_key(key);
        let well_formed =
            !domain.is_empty() && domain.len() <= 50 && domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !well_formed {
            return Err(invalid(
                field,
                &format!("{key}: must be 1 to 50 characters of a-z, 0-9 and _"),
            ));
        }
        if domain == "governance" {
            return Err(invalid(
                field,
                &format!("{key}: an AI is always an observer in governance"),
            ));
        }
        let level = value.as_str().filter(|level| LEVELS.contains(level));
        let Some(level) = level else {
            return Err(invalid(
                field,
                &format!("{key}: must be observer, advisor, voter, vetoer or autonomous"),
            ));
        };
        if normalized.insert(domain, json!(level)).is_some() {
            return Err(invalid(field, &format!("{key}: each domain can appear once")));
        }
    }
    Ok(normalized)
}

// ---------------------------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
struct LinkRow {
    project_id: Uuid,
    bot_id: Uuid,
    version: i32,
    created_by: Option<Uuid>,
    updated_by: Option<Uuid>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct RunnerRow {
    bot_id: Uuid,
    workspace_id: Uuid,
    webhook_id: Uuid,
    runner_url: String,
    credentials_issued_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct BotRow {
    name: String,
    token_prefix: String,
    is_active: bool,
}

#[derive(Debug, FromQueryResult)]
struct ParticipantRow {
    max_domain_level: String,
    can_veto_human_consensus: bool,
    reason_min_length: i32,
    domain_overrides: Option<Value>,
}

#[derive(FromQueryResult)]
struct Count {
    n: i64,
}

const RUNNER_COLUMNS: &str = "bot_id, workspace_id, webhook_id, runner_url, credentials_issued_at";
const LINK_COLUMNS: &str = "project_id, bot_id, version, created_by, updated_by, created_at, updated_at";

async fn one<T: FromQueryResult>(
    db: &impl ConnectionTrait,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Option<T>, ApiError> {
    Ok(
        T::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .one(db)
            .await?,
    )
}

async fn run(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) -> Result<u64, ApiError> {
    Ok(db
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?
        .rows_affected())
}

async fn connected_projects(db: &impl ConnectionTrait, bot_id: Uuid) -> Result<Vec<ManagedProject>, ApiError> {
    Ok(bots::managed_projects(db, bot_id).await?.projects)
}

fn governance_of(row: &ParticipantRow) -> Governance {
    Governance {
        max_domain_level: row.max_domain_level.clone(),
        can_veto_human_consensus: row.can_veto_human_consensus,
        reason_min_length: row.reason_min_length,
        domain_overrides: row
            .domain_overrides
            .as_ref()
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
    }
}

async fn load_participant(
    db: &impl ConnectionTrait,
    project_id: Uuid,
    bot_id: Uuid,
) -> Result<ParticipantRow, ApiError> {
    one(
        db,
        "SELECT max_domain_level, can_veto_human_consensus, reason_min_length, domain_overrides \
         FROM ai_participants WHERE project_id = $1 AND id = $2",
        vec![project_id.into(), bot_id.to_string().into()],
    )
    .await?
    .ok_or(ApiError::Internal)
}

async fn load_link(db: &impl ConnectionTrait, project_id: Uuid) -> Result<Option<LinkRow>, ApiError> {
    one(
        db,
        &format!("SELECT {LINK_COLUMNS} FROM project_bot_connections WHERE project_id = $1"),
        vec![project_id.into()],
    )
    .await
}

/// The connection of a project as owners and admins see it, `None` when it has none.
pub async fn load_view(db: &impl ConnectionTrait, project_id: Uuid) -> Result<Option<ConnectionView>, ApiError> {
    let Some(link) = load_link(db, project_id).await? else {
        return Ok(None);
    };
    let runner: RunnerRow = one(
        db,
        &format!("SELECT {RUNNER_COLUMNS} FROM bot_runners WHERE bot_id = $1"),
        vec![link.bot_id.into()],
    )
    .await?
    .ok_or(ApiError::Internal)?;
    let bot: BotRow = one(
        db,
        "SELECT name, token_prefix, is_active FROM workspace_bots WHERE id = $1",
        vec![link.bot_id.into()],
    )
    .await?
    .ok_or(ApiError::Internal)?;
    let settings = execution_settings::load(db, project_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let participant = load_participant(db, project_id, link.bot_id).await?;
    Ok(Some(ConnectionView {
        project_id,
        version: link.version,
        enabled: settings.enabled,
        bot: BotView {
            id: link.bot_id,
            name: bot.name,
            token_prefix: bot.token_prefix,
            token_active: bot.is_active,
        },
        runner: RunnerView {
            url: runner.runner_url,
            webhook_id: runner.webhook_id,
            credentials_issued_at: runner.credentials_issued_at,
            shared_with_projects: connected_projects(db, link.bot_id).await?,
        },
        execution: ExecutionView {
            channel: settings.channel,
            cli: settings.cli,
            working_directory: settings.working_directory,
            on_complete_state: settings.on_complete_state,
            on_fail_label_id: settings.on_fail_label_id,
            approval_gate: settings.approval_gate,
        },
        governance: governance_of(&participant),
        created_by: link.created_by,
        created_at: link.created_at,
        updated_by: link.updated_by,
        updated_at: link.updated_at,
    }))
}

/// The connection of a project as members see it, `None` when it has none.
pub async fn load_member_view(db: &impl ConnectionTrait, project_id: Uuid) -> Result<Option<MemberView>, ApiError> {
    let Some(link) = load_link(db, project_id).await? else {
        return Ok(None);
    };
    let bot: BotRow = one(
        db,
        "SELECT name, token_prefix, is_active FROM workspace_bots WHERE id = $1",
        vec![link.bot_id.into()],
    )
    .await?
    .ok_or(ApiError::Internal)?;
    let settings = execution_settings::load(db, project_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok(Some(MemberView {
        bot: MemberBot {
            id: link.bot_id,
            name: bot.name,
        },
        enabled: settings.enabled,
        cli: settings.cli,
        channel: settings.channel,
    }))
}

// ---------------------------------------------------------------------------------------------
// Shared helpers of the writes
// ---------------------------------------------------------------------------------------------

struct Locked {
    link: LinkRow,
    runner: RunnerRow,
}

/// Locks the `bot_runners` row and then the connection row of `project_id`, in the lock order of
/// this module. 404 when the project has no connection.
async fn lock(tx: &impl ConnectionTrait, project_id: Uuid) -> Result<Locked, ApiError> {
    let missing = || ApiError::NotFound(NO_CONNECTION.to_string());
    let unlocked = load_link(tx, project_id).await?.ok_or_else(missing)?;
    let runner: RunnerRow = one(
        tx,
        &format!("SELECT {RUNNER_COLUMNS} FROM bot_runners WHERE bot_id = $1 FOR UPDATE"),
        vec![unlocked.bot_id.into()],
    )
    .await?
    .ok_or_else(missing)?;
    let link: LinkRow = one::<LinkRow>(
        tx,
        &format!("SELECT {LINK_COLUMNS} FROM project_bot_connections WHERE project_id = $1 FOR UPDATE"),
        vec![project_id.into()],
    )
    .await?
    .filter(|link| link.bot_id == runner.bot_id)
    .ok_or_else(missing)?;
    Ok(Locked { link, runner })
}

/// Increments the version of this connection, or of every connection of its bot when the write
/// changed shared state.
async fn bump(tx: &impl ConnectionTrait, locked: &Locked, actor: Uuid, shared: bool) -> Result<(), ApiError> {
    if shared {
        run(
            tx,
            "UPDATE project_bot_connections SET version = version + 1, updated_by = $2, updated_at = now() \
             WHERE bot_id = $1",
            vec![locked.runner.bot_id.into(), actor.into()],
        )
        .await?;
    } else {
        run(
            tx,
            "UPDATE project_bot_connections SET version = version + 1, updated_by = $2, updated_at = now() \
             WHERE project_id = $1",
            vec![locked.link.project_id.into(), actor.into()],
        )
        .await?;
    }
    Ok(())
}

/// Business event of a connection write, on the same transaction. The payload is ids, names, the
/// runner address and the token prefix, never a secret.
#[allow(clippy::too_many_arguments)]
async fn emit(
    tx: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    event_type: &str,
    aggregate_type: &str,
    aggregate_id: Uuid,
    actor: Option<Uuid>,
    payload: Value,
) -> Result<(), ApiError> {
    insert_business_event(
        tx,
        BusinessEventInput {
            workspace_id,
            project_id,
            event_type: event_type.to_string(),
            aggregate_type: aggregate_type.to_string(),
            aggregate_id: aggregate_id.to_string(),
            actor_id: actor,
            source: json!({ "type": "user", "actor_id": actor }),
            payload,
            metadata: json!({ "managed_by_connection": true }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

async fn webhook_active(tx: &impl ConnectionTrait, webhook_id: Uuid) -> Result<bool, ApiError> {
    Ok(one::<Count>(
        tx,
        "SELECT (CASE WHEN active THEN 1 ELSE 0 END)::bigint AS n FROM webhooks WHERE id = $1",
        vec![webhook_id.into()],
    )
    .await?
    .is_some_and(|row| row.n == 1))
}

async fn token_active(tx: &impl ConnectionTrait, bot_id: Uuid) -> Result<bool, ApiError> {
    Ok(one::<Count>(
        tx,
        "SELECT (CASE WHEN is_active THEN 1 ELSE 0 END)::bigint AS n FROM workspace_bots WHERE id = $1",
        vec![bot_id.into()],
    )
    .await?
    .is_some_and(|row| row.n == 1))
}

/// Whether both shared parts (token and webhook) are on.
async fn shared_is_on(tx: &impl ConnectionTrait, runner: &RunnerRow) -> Result<bool, ApiError> {
    Ok(token_active(tx, runner.bot_id).await? && webhook_active(tx, runner.webhook_id).await?)
}

/// Switches the shared parts of a bot: the token and the managed webhook.
async fn switch_shared(tx: &impl ConnectionTrait, runner: &RunnerRow, actor: Uuid, on: bool) -> Result<(), ApiError> {
    webhooks::update(
        tx,
        runner.workspace_id,
        runner.webhook_id,
        actor,
        serde_json::from_value(json!({ "active": on })).map_err(|_| ApiError::Internal)?,
        ManagedPolicy::Allow,
    )
    .await?;
    bots::set_token_active(tx, runner.bot_id, on).await
}

/// How many connections of the bot other than `project_id` are enabled.
async fn other_enabled(tx: &impl ConnectionTrait, bot_id: Uuid, project_id: Uuid) -> Result<i64, ApiError> {
    Ok(one::<Count>(
        tx,
        "SELECT COUNT(*)::bigint AS n FROM project_bot_connections c \
         JOIN project_execution_settings s ON s.project_id = c.project_id \
         WHERE c.bot_id = $1 AND c.project_id <> $2 AND s.enabled",
        vec![bot_id.into(), project_id.into()],
    )
    .await?
    .map_or(0, |row| row.n))
}

async fn project_key(db: &impl ConnectionTrait, project_id: Uuid) -> Result<String, ApiError> {
    #[derive(FromQueryResult)]
    struct Key {
        key: String,
    }
    Ok(
        one::<Key>(db, "SELECT key FROM projects WHERE id = $1", vec![project_id.into()])
            .await?
            .map_or_else(String::new, |row| row.key),
    )
}

fn settings_input(settings: &ExecutionSettings) -> ExecutionSettingsInput {
    ExecutionSettingsInput {
        enabled: settings.enabled,
        channel: settings.channel.clone(),
        cli: settings.cli.clone(),
        working_directory: Some(settings.working_directory.clone()),
        on_complete_state: settings.on_complete_state.clone(),
        on_fail_label_id: settings.on_fail_label_id,
        approval_gate: settings.approval_gate.clone(),
    }
}

/// Stores `valid` as the project's settings. The caller holds the connection lock, so the settings
/// row cannot change underneath: the version it reads is the one it writes against.
async fn store_settings(
    tx: &impl ConnectionTrait,
    project: &ProjectRef,
    valid: &ValidSettings,
    actor: Uuid,
) -> Result<ExecutionSettings, ApiError> {
    let expected = execution_settings::load(tx, project.id)
        .await?
        .map(|settings| settings.version);
    match execution_settings::upsert(tx, project, valid, expected, actor).await? {
        SaveOutcome::Saved { after, .. } => Ok(after),
        SaveOutcome::Conflict { .. } => Err(ApiError::Internal),
    }
}

/// The version check shared by every write after connect.
async fn check_version(
    tx: &impl ConnectionTrait,
    locked: &Locked,
    expected_version: i32,
) -> Result<Option<Box<ConnectionView>>, ApiError> {
    if locked.link.version == expected_version {
        return Ok(None);
    }
    let current = load_view(tx, locked.link.project_id).await?.ok_or(ApiError::Internal)?;
    Ok(Some(Box::new(current)))
}

// ---------------------------------------------------------------------------------------------
// Connect
// ---------------------------------------------------------------------------------------------

/// Everything `connect` validates before it writes.
struct ValidConnect {
    bot: BotChoice,
    runner_url: String,
    settings: ValidSettings,
    governance: Governance,
}

enum BotChoice {
    New { name: String },
    Existing { id: Uuid },
}

async fn validate_connect(
    state: &AppState,
    project: &ProjectRef,
    req: &ConnectRequest,
) -> Result<ValidConnect, ApiError> {
    let bot = match (&req.bot.name, req.bot.id) {
        (Some(name), None) => BotChoice::New {
            name: validate_bot_name(name)?,
        },
        (None, Some(id)) => BotChoice::Existing { id },
        _ => return Err(invalid("bot", "give either name or id")),
    };
    let runner_url = validate_runner_url(&req.runner_url).await?;
    let cli = req.cli.clone().ok_or_else(|| invalid("cli", "is required"))?;
    let input = ExecutionSettingsInput {
        enabled: true,
        channel: req.channel.clone().unwrap_or_else(|| "tmux".to_string()),
        cli,
        working_directory: req.working_directory.clone(),
        on_complete_state: req.on_complete_state.clone().unwrap_or_else(|| "auto".to_string()),
        on_fail_label_id: req.on_fail_label_id,
        approval_gate: "none".to_string(),
    };
    let settings = execution_settings::validate(state, project, &input).await?;
    let governance = validate_governance(&Governance::defaults(), &req.governance.clone().unwrap_or_default())?;
    Ok(ValidConnect {
        bot,
        runner_url,
        settings,
        governance,
    })
}

/// Connects a bot to a project: all of it, or nothing.
///
/// A new bot gets a user, a workspace seat, a token (surface `mcp_stdio`, read and write) and a
/// managed webhook; an already connected bot is added to the project and keeps its token and
/// webhook. The configuration block is returned for a new bot only.
pub async fn connect(
    state: &AppState,
    project: &ProjectRef,
    actor: Uuid,
    req: ConnectRequest,
    origin: Option<&str>,
) -> Result<Connected, ApiError> {
    let valid = validate_connect(state, project, &req).await?;
    let tx = state.db.begin().await?;

    // Serialises connects of the same project: the second waits here and then sees the first.
    one::<ProjectRef>(
        &tx,
        "SELECT id, workspace_id FROM projects WHERE id = $1 FOR UPDATE",
        vec![project.id.into()],
    )
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;
    if load_link(&tx, project.id).await?.is_some() {
        return Err(ApiError::Conflict(ALREADY_CONNECTED.to_string()));
    }

    let mut fresh: Option<(String, String, String, Timestamp)> = None;
    let (runner, bot_name, new_bot) = match valid.bot {
        BotChoice::New { name } => {
            // Two connects with the same name in different projects must not both pass the check.
            one::<Count>(
                &tx,
                "SELECT 1::bigint AS n FROM (SELECT pg_advisory_xact_lock(hashtextextended($1, 0))) AS locked",
                vec![format!("bot-name:{}:{}", project.workspace_id, name.to_lowercase()).into()],
            )
            .await?;
            let taken = one::<Count>(
                &tx,
                "SELECT COUNT(*)::bigint AS n FROM workspace_bots \
                 WHERE workspace_id = $1 AND is_active AND lower(name) = lower($2)",
                vec![project.workspace_id.into(), name.clone().into()],
            )
            .await?
            .is_some_and(|row| row.n > 0);
            if taken {
                return Err(invalid(
                    "bot.name",
                    "a bot with this name already exists in this workspace",
                ));
            }
            let created = bots::create(
                &tx,
                NewBot {
                    workspace_id: project.workspace_id,
                    created_by: actor,
                    name: &name,
                    permissions: vec!["read".to_string(), "write".to_string()],
                    transport_surface: "mcp_stdio".to_string(),
                    expires_at: None,
                },
            )
            .await?;
            let hook = webhooks::create(
                &tx,
                NewWebhook {
                    workspace_id: project.workspace_id,
                    actor_id: actor,
                    name: format!("Runner for {name}"),
                    url: format!("{}/webhook", valid.runner_url),
                    secret: None,
                    events: Vec::new(),
                    active: true,
                    bot_user_id: Some(created.id),
                },
                ManagedPolicy::Allow,
            )
            .await?;
            let runner: RunnerRow = one(
                &tx,
                &format!(
                    "INSERT INTO bot_runners (bot_id, workspace_id, webhook_id, runner_url, created_by) \
                     VALUES ($1, $2, $3, $4, $5) RETURNING {RUNNER_COLUMNS}"
                ),
                vec![
                    created.id.into(),
                    project.workspace_id.into(),
                    hook.webhook.id.into(),
                    valid.runner_url.clone().into(),
                    actor.into(),
                ],
            )
            .await?
            .ok_or(ApiError::Internal)?;
            fresh = Some((
                created.token.clone(),
                created.token_prefix.clone(),
                hook.secret.clone(),
                runner.credentials_issued_at,
            ));
            (runner, name, true)
        }
        BotChoice::Existing { id } => {
            let runner: RunnerRow = one(
                &tx,
                &format!("SELECT {RUNNER_COLUMNS} FROM bot_runners WHERE bot_id = $1 AND workspace_id = $2 FOR UPDATE"),
                vec![id.into(), project.workspace_id.into()],
            )
            .await?
            .ok_or_else(|| invalid("bot.id", "not a connected bot of this workspace"))?;
            if runner.runner_url != valid.runner_url {
                return Err(ApiError::Conflict(format!(
                    "runner_url: this bot already delivers to {}; change the address on its connection",
                    runner.runner_url
                )));
            }
            let bot: BotRow = one(
                &tx,
                "SELECT name, token_prefix, is_active FROM workspace_bots WHERE id = $1",
                vec![id.into()],
            )
            .await?
            .ok_or(ApiError::Internal)?;
            (runner, bot.name, false)
        }
    };

    let bot_id = runner.bot_id;
    one::<Count>(
        &tx,
        "INSERT INTO ai_participants \
         (id, project_id, name, domain_overrides, max_domain_level, can_veto_human_consensus, \
          reason_min_length, is_active, registered_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, true, $8) RETURNING 1::bigint AS n",
        vec![
            bot_id.to_string().into(),
            project.id.into(),
            bot_name.clone().into(),
            Value::Object(valid.governance.domain_overrides.clone()).into(),
            valid.governance.max_domain_level.clone().into(),
            valid.governance.can_veto_human_consensus.into(),
            valid.governance.reason_min_length.into(),
            actor.into(),
        ],
    )
    .await?;
    let settings = store_settings(&tx, project, &valid.settings, actor).await?;
    run(
        &tx,
        "INSERT INTO project_bot_connections (project_id, bot_id, created_by, updated_by) VALUES ($1, $2, $3, $3)",
        vec![project.id.into(), bot_id.into(), actor.into()],
    )
    .await?;

    // Shared parts follow the per-project state: this connection is enabled, so they are on.
    let switched = if !new_bot && !shared_is_on(&tx, &runner).await? {
        switch_shared(&tx, &runner, actor, true).await?;
        run(
            &tx,
            "UPDATE project_bot_connections SET version = version + 1, updated_by = $2, updated_at = now() \
             WHERE bot_id = $1 AND project_id <> $3",
            vec![bot_id.into(), actor.into(), project.id.into()],
        )
        .await?;
        true
    } else {
        false
    };

    emit(
        &tx,
        project.workspace_id,
        Some(project.id),
        "project.bot_connected",
        "project",
        project.id,
        Some(actor),
        json!({
            "bot_id": bot_id,
            "bot_name": bot_name,
            "runner_url": runner.runner_url,
            "new_bot": new_bot,
            "shared_parts_switched": switched,
            "execution": {
                "channel": settings.channel,
                "cli": settings.cli,
                "working_directory": settings.working_directory,
                "on_complete_state": settings.on_complete_state,
                "on_fail_label_id": settings.on_fail_label_id,
            },
            "governance": valid.governance,
        }),
    )
    .await?;

    let connection = load_view(&tx, project.id).await?.ok_or(ApiError::Internal)?;
    let config_block = match fresh {
        Some((token, token_prefix, secret, issued_at)) => {
            let key = project_key(&tx, project.id).await?;
            let issuer = crate::config::runtime()
                .oauth
                .as_ref()
                .map(|oauth| oauth.issuer.clone());
            Some(runner_config_block::build(&BlockInput {
                bot_name: &bot_name,
                bot_id,
                project_key: &key,
                issued_at,
                runner_url: &runner.runner_url,
                working_directory: &settings.working_directory,
                cli: &settings.cli,
                token: &token,
                token_prefix: &token_prefix,
                secret: &secret,
                issuer: issuer.as_deref(),
                origin,
            }))
        }
        None => None,
    };
    tx.commit().await?;
    if new_bot {
        crate::flow::collab::permission_cache::invalidate_workspace_after_commit(state, project.workspace_id);
    }
    Ok(Connected {
        connection,
        config_block,
    })
}

// ---------------------------------------------------------------------------------------------
// Update, disable, enable
// ---------------------------------------------------------------------------------------------

/// Changes the address, execution settings or governance of a connection.
pub async fn update(
    state: &AppState,
    project: &ProjectRef,
    actor: Uuid,
    req: UpdateRequest,
) -> Result<Outcome<ConnectionView>, ApiError> {
    if req.is_empty() {
        return Err(ApiError::BadRequest("no changes provided".to_string()));
    }
    let runner_url = match &req.runner_url {
        Some(raw) => Some(validate_runner_url(raw).await?),
        None => None,
    };
    let tx = state.db.begin().await?;
    let locked = lock(&tx, project.id).await?;
    if let Some(current) = check_version(&tx, &locked, req.expected_version).await? {
        return Ok(Outcome::Conflict { current });
    }

    let settings = execution_settings::load(&tx, project.id)
        .await?
        .ok_or(ApiError::Internal)?;
    let participant = load_participant(&tx, project.id, locked.runner.bot_id).await?;
    let before = governance_of(&participant);

    let mut input = settings_input(&settings);
    if let Some(cli) = req.cli {
        input.cli = cli;
    }
    if let Some(directory) = req.working_directory {
        input.working_directory = Some(directory);
    }
    if let Some(channel) = req.channel {
        input.channel = channel;
    }
    if let Some(state_value) = req.on_complete_state {
        input.on_complete_state = state_value;
    }
    if let Some(label) = req.on_fail_label_id {
        input.on_fail_label_id = label;
    }
    let valid = execution_settings::validate(state, project, &input).await?;
    let governance = match &req.governance {
        Some(change) => validate_governance(&before, change)?,
        None => before.clone(),
    };

    let mut changed_before = Map::new();
    let mut changed_after = Map::new();
    let mut track = |key: &str, old: Value, new: Value| {
        if old != new {
            changed_before.insert(key.to_string(), old);
            changed_after.insert(key.to_string(), new);
        }
    };

    let shared = if let Some(url) = runner_url.as_ref().filter(|url| **url != locked.runner.runner_url) {
        track("runner_url", json!(locked.runner.runner_url), json!(url));
        run(
            &tx,
            "UPDATE bot_runners SET runner_url = $2, updated_at = now() WHERE bot_id = $1",
            vec![locked.runner.bot_id.into(), url.clone().into()],
        )
        .await?;
        webhooks::update(
            &tx,
            locked.runner.workspace_id,
            locked.runner.webhook_id,
            actor,
            serde_json::from_value(json!({ "url": format!("{url}/webhook") })).map_err(|_| ApiError::Internal)?,
            ManagedPolicy::Allow,
        )
        .await?;
        true
    } else {
        false
    };
    track("cli", json!(settings.cli), json!(valid.cli));
    track("channel", json!(settings.channel), json!(valid.channel));
    track(
        "working_directory",
        json!(settings.working_directory),
        json!(valid.working_directory),
    );
    track(
        "on_complete_state",
        json!(settings.on_complete_state),
        json!(valid.on_complete_state),
    );
    track(
        "on_fail_label_id",
        json!(settings.on_fail_label_id),
        json!(valid.on_fail_label_id),
    );
    track("governance", json!(before), json!(governance));
    store_settings(&tx, project, &valid, actor).await?;
    if governance != before {
        run(
            &tx,
            "UPDATE ai_participants SET max_domain_level = $3, can_veto_human_consensus = $4, \
             reason_min_length = $5, domain_overrides = $6 WHERE project_id = $1 AND id = $2",
            vec![
                project.id.into(),
                locked.runner.bot_id.to_string().into(),
                governance.max_domain_level.clone().into(),
                governance.can_veto_human_consensus.into(),
                governance.reason_min_length.into(),
                Value::Object(governance.domain_overrides.clone()).into(),
            ],
        )
        .await?;
    }
    bump(&tx, &locked, actor, shared).await?;
    emit(
        &tx,
        project.workspace_id,
        Some(project.id),
        "project.bot_connection_updated",
        "project",
        project.id,
        Some(actor),
        json!({
            "bot_id": locked.runner.bot_id,
            "before": changed_before,
            "after": changed_after,
        }),
    )
    .await?;
    let view = load_view(&tx, project.id).await?.ok_or(ApiError::Internal)?;
    tx.commit().await?;
    Ok(Outcome::Done(view))
}

/// Disables or enables a connection for its project.
///
/// Switching the last enabled connection of a bot off (or the first one back on) also switches the
/// bot's token and webhook. Repeating the state it already has answers the current connection and
/// changes nothing.
pub async fn set_enabled(
    state: &AppState,
    project: &ProjectRef,
    actor: Uuid,
    expected_version: i32,
    enabled: bool,
) -> Result<Outcome<ConnectionView>, ApiError> {
    let tx = state.db.begin().await?;
    let locked = lock(&tx, project.id).await?;
    if let Some(current) = check_version(&tx, &locked, expected_version).await? {
        return Ok(Outcome::Conflict { current });
    }
    let settings = execution_settings::load(&tx, project.id)
        .await?
        .ok_or(ApiError::Internal)?;
    if settings.enabled == enabled {
        let view = load_view(&tx, project.id).await?.ok_or(ApiError::Internal)?;
        return Ok(Outcome::Done(view));
    }

    let valid = ValidSettings {
        enabled,
        channel: settings.channel.clone(),
        cli: settings.cli.clone(),
        working_directory: settings.working_directory.clone(),
        on_complete_state: settings.on_complete_state.clone(),
        on_fail_label_id: settings.on_fail_label_id,
        approval_gate: settings.approval_gate.clone(),
    };
    store_settings(&tx, project, &valid, actor).await?;
    run(
        &tx,
        "UPDATE ai_participants SET is_active = $3 WHERE project_id = $1 AND id = $2",
        vec![
            project.id.into(),
            locked.runner.bot_id.to_string().into(),
            enabled.into(),
        ],
    )
    .await?;

    let others = other_enabled(&tx, locked.runner.bot_id, project.id).await?;
    let shared_on = shared_is_on(&tx, &locked.runner).await?;
    let switch = if enabled { !shared_on } else { others == 0 && shared_on };
    if switch {
        switch_shared(&tx, &locked.runner, actor, enabled).await?;
    }
    bump(&tx, &locked, actor, switch).await?;
    emit(
        &tx,
        project.workspace_id,
        Some(project.id),
        if enabled {
            "project.bot_connection_enabled"
        } else {
            "project.bot_connection_disabled"
        },
        "project",
        project.id,
        Some(actor),
        json!({ "bot_id": locked.runner.bot_id, "shared_parts_switched": switch }),
    )
    .await?;
    let view = load_view(&tx, project.id).await?.ok_or(ApiError::Internal)?;
    tx.commit().await?;
    Ok(Outcome::Done(view))
}

// ---------------------------------------------------------------------------------------------
// Rotate
// ---------------------------------------------------------------------------------------------

/// Issues a new token and a new signing secret for the bot, for every project it serves. The old
/// ones stop working with the commit. Returns the configuration block.
pub async fn rotate(
    state: &AppState,
    project: &ProjectRef,
    actor: Uuid,
    expected_version: i32,
    origin: Option<&str>,
) -> Result<Outcome<Rotated>, ApiError> {
    let tx = state.db.begin().await?;
    let locked = lock(&tx, project.id).await?;
    if let Some(current) = check_version(&tx, &locked, expected_version).await? {
        return Ok(Outcome::Conflict { current });
    }
    let bot = one::<BotRow>(
        &tx,
        "SELECT name, token_prefix, is_active FROM workspace_bots WHERE id = $1",
        vec![locked.runner.bot_id.into()],
    )
    .await?
    .ok_or(ApiError::Internal)?;
    let projects = connected_projects(&tx, locked.runner.bot_id).await?;

    let token = bots::rotate_token(&tx, locked.runner.bot_id).await?;
    let hook = webhooks::rotate_secret(
        &tx,
        locked.runner.workspace_id,
        locked.runner.webhook_id,
        actor,
        ManagedPolicy::Allow,
    )
    .await?;
    let issued_at = one::<RunnerRow>(
        &tx,
        &format!(
            "UPDATE bot_runners SET credentials_issued_at = now(), updated_at = now() \
             WHERE bot_id = $1 RETURNING {RUNNER_COLUMNS}"
        ),
        vec![locked.runner.bot_id.into()],
    )
    .await?
    .ok_or(ApiError::Internal)?
    .credentials_issued_at;
    bump(&tx, &locked, actor, true).await?;
    emit(
        &tx,
        project.workspace_id,
        None,
        "bot.credentials_rotated",
        "bot",
        locked.runner.bot_id,
        Some(actor),
        json!({
            "bot_id": locked.runner.bot_id,
            "previous_token_prefix": token.previous_prefix,
            "token_prefix": token.token_prefix,
            "projects": projects.iter().map(|project| project.id).collect::<Vec<_>>(),
        }),
    )
    .await?;

    let settings = execution_settings::load(&tx, project.id)
        .await?
        .ok_or(ApiError::Internal)?;
    let key = project_key(&tx, project.id).await?;
    let issuer = crate::config::runtime()
        .oauth
        .as_ref()
        .map(|oauth| oauth.issuer.clone());
    let config_block = runner_config_block::build(&BlockInput {
        bot_name: &bot.name,
        bot_id: locked.runner.bot_id,
        project_key: &key,
        issued_at,
        runner_url: &locked.runner.runner_url,
        working_directory: &settings.working_directory,
        cli: &settings.cli,
        token: &token.token,
        token_prefix: &token.token_prefix,
        secret: &hook.secret,
        issuer: issuer.as_deref(),
        origin,
    });
    let connection = load_view(&tx, project.id).await?.ok_or(ApiError::Internal)?;
    tx.commit().await?;
    Ok(Outcome::Done(Rotated {
        connection,
        config_block,
    }))
}

// ---------------------------------------------------------------------------------------------
// Remove
// ---------------------------------------------------------------------------------------------

/// What a removal did, for the caller's after-commit work.
pub struct Removal {
    pub workspace_id: Uuid,
    pub bot_id: Uuid,
    pub remaining: Vec<ManagedProject>,
    /// The bot's token, webhook and seat were removed: this was its last connection.
    pub released: bool,
    /// The Flow authorization epoch advanced, because the bot's seat left the workspace.
    pub epoch: Option<i64>,
}

/// Removes the connection of `project_id` inside `tx`.
///
/// Always: the link, the participant registration and the project's settings. When it was the bot's
/// last connection: also the shared parts (the `bot_runners` row, the webhook, the token, the
/// workspace seat). The bot's user row stays: its task history, comments and votes name it. The
/// caller holds the locks of [`lock`].
async fn remove_locked(
    tx: &impl ConnectionTrait,
    project_id: Uuid,
    locked: &Locked,
    actor: Option<Uuid>,
) -> Result<Removal, ApiError> {
    let runner = &locked.runner;
    run(
        tx,
        "DELETE FROM project_bot_connections WHERE project_id = $1",
        vec![project_id.into()],
    )
    .await?;
    run(
        tx,
        "DELETE FROM ai_participants WHERE project_id = $1 AND id = $2",
        vec![project_id.into(), runner.bot_id.to_string().into()],
    )
    .await?;
    run(
        tx,
        "DELETE FROM project_execution_settings WHERE project_id = $1",
        vec![project_id.into()],
    )
    .await?;
    let remaining = connected_projects(tx, runner.bot_id).await?;
    let mut epoch = None;
    let released = remaining.is_empty();
    if released {
        run(
            tx,
            "DELETE FROM bot_runners WHERE bot_id = $1",
            vec![runner.bot_id.into()],
        )
        .await?;
        if let Some(actor) = actor {
            webhooks::delete(tx, runner.workspace_id, runner.webhook_id, actor, ManagedPolicy::Allow).await?;
        } else {
            run(tx, "DELETE FROM webhooks WHERE id = $1", vec![runner.webhook_id.into()]).await?;
        }
        bots::set_token_active(tx, runner.bot_id, false).await?;
        epoch = crate::flow::collab::authz::advance_epoch_if_present(tx, runner.workspace_id).await?;
        run(
            tx,
            "DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![runner.workspace_id.into(), runner.bot_id.into()],
        )
        .await?;
        run(
            tx,
            "UPDATE users SET is_active = false, updated_at = now() WHERE id = $1",
            vec![runner.bot_id.into()],
        )
        .await?;
    }
    emit(
        tx,
        runner.workspace_id,
        Some(project_id),
        "project.bot_disconnected",
        "project",
        project_id,
        actor,
        json!({ "bot_id": runner.bot_id, "bot_released": released }),
    )
    .await?;
    Ok(Removal {
        workspace_id: runner.workspace_id,
        bot_id: runner.bot_id,
        remaining,
        released,
        epoch,
    })
}

/// Removes the connection of a project.
pub async fn remove(
    state: &AppState,
    project: &ProjectRef,
    actor: Uuid,
    expected_version: i32,
) -> Result<Outcome<Removed>, ApiError> {
    let tx = state.db.begin().await?;
    let locked = lock(&tx, project.id).await?;
    if let Some(current) = check_version(&tx, &locked, expected_version).await? {
        return Ok(Outcome::Conflict { current });
    }
    let removal = remove_locked(&tx, project.id, &locked, Some(actor)).await?;
    tx.commit().await?;
    if let Some(epoch) = removal.epoch {
        crate::flow::collab::permission_cache::invalidate_workspace_after_commit(state, removal.workspace_id);
        let revalidated =
            crate::flow::collab::revocation::revalidate_workspace_after_commit(state, removal.workspace_id, epoch)
                .await;
        tracing::debug!(workspace_id = %removal.workspace_id, ?revalidated, "workspace sessions re-evaluated after a bot left");
    }
    Ok(Outcome::Done(Removed {
        removed: true,
        bot_still_connected_to: removal.remaining,
    }))
}

/// Releases the connection of a project that is about to be deleted.
///
/// A deleted project never leaves an orphaned managed webhook or a live token. A project without a
/// connection is left alone. The caller owns the transaction.
pub async fn release_for_project(tx: &impl ConnectionTrait, project_id: Uuid) -> Result<(), ApiError> {
    if load_link(tx, project_id).await?.is_none() {
        return Ok(());
    }
    let locked = lock(tx, project_id).await?;
    remove_locked(tx, project_id, &locked, None).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Governance, GovernanceInput, normalize_runner_url, validate_governance, validate_runner_url_with};
    use crate::error::ApiError;
    use serde_json::json;

    fn message(result: Result<impl std::fmt::Debug, ApiError>) -> String {
        match result {
            Err(ApiError::BadRequest(message)) => message,
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    fn input(value: serde_json::Value) -> GovernanceInput {
        serde_json::from_value(value).expect("a governance input")
    }

    #[test]
    fn runner_addresses_are_normalised_or_refused_by_shape() {
        assert_eq!(
            normalize_runner_url(" https://r.example.net:9090/ ").as_deref(),
            Ok("https://r.example.net:9090")
        );
        assert_eq!(normalize_runner_url("http://r/p").as_deref(), Ok("http://r/p"));
        for bad in [
            "",
            "r.example.net",
            "ftp://r",
            "HTTP://r",
            "http://",
            "http:///x",
            "http://u:p@r",
            "http://r?x=1",
            "http://r#f",
            "http://r//",
            "http://r a",
        ] {
            assert!(normalize_runner_url(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(normalize_runner_url(&format!("http://{}", "a".repeat(2050))).is_err());
    }

    #[tokio::test]
    async fn the_outbound_policy_decides_blocked_hosts_and_the_allowlist_lifts_them() {
        // A literal blocked address.
        let refused = message(validate_runner_url_with("http://127.0.0.1:9", "", false).await);
        assert!(refused.starts_with("runner_url: "), "{refused}");
        assert!(refused.contains("blocked address"), "{refused}");
        assert!(
            refused.contains("[outbound] allowed_hosts"),
            "the hint names the setting: {refused}"
        );
        // A name that resolves to one.
        let resolved = message(validate_runner_url_with("http://localhost:9", "", false).await);
        assert!(
            resolved.contains("blocked address") && resolved.contains("allowed_hosts"),
            "{resolved}"
        );
        // The allowlist lifts both, by host and port.
        assert_eq!(
            validate_runner_url_with("http://127.0.0.1:9", "127.0.0.1:9", false)
                .await
                .expect("allowed"),
            "http://127.0.0.1:9"
        );
        assert!(
            validate_runner_url_with("http://localhost:9", "localhost:9", false)
                .await
                .is_ok()
        );
        // Another port of an allowed host is still refused.
        assert!(
            validate_runner_url_with("http://127.0.0.1:10", "127.0.0.1:9", false)
                .await
                .is_err()
        );
        // The private switch lifts everything.
        assert!(validate_runner_url_with("http://10.0.0.5:9", "", true).await.is_ok());
        // A public literal needs no resolution.
        assert!(
            validate_runner_url_with("https://93.184.216.34:9090", "", false)
                .await
                .is_ok()
        );
    }

    #[test]
    fn governance_is_checked_field_by_field() {
        let base = Governance::defaults();
        let defaults = validate_governance(&base, &GovernanceInput::default()).expect("defaults");
        assert_eq!(defaults, base);
        assert_eq!(base.max_domain_level, "voter");
        assert_eq!(base.reason_min_length, 50);
        assert!(!base.can_veto_human_consensus);

        let level = message(validate_governance(&base, &input(json!({"max_domain_level": "boss"}))));
        assert_eq!(
            level,
            "governance.max_domain_level: must be observer, advisor, voter, vetoer or autonomous"
        );
        let veto = message(validate_governance(
            &base,
            &input(json!({"can_veto_human_consensus": true})),
        ));
        assert_eq!(
            veto,
            "governance.can_veto_human_consensus: requires max_domain_level vetoer or autonomous"
        );
        assert!(
            validate_governance(
                &base,
                &input(json!({"max_domain_level": "vetoer", "can_veto_human_consensus": true}))
            )
            .is_ok()
        );
        for length in [-1, 2001] {
            let why = message(validate_governance(&base, &input(json!({"reason_min_length": length}))));
            assert_eq!(why, "governance.reason_min_length: must be between 0 and 2000");
        }
        assert!(validate_governance(&base, &input(json!({"reason_min_length": 0}))).is_ok());
        assert!(validate_governance(&base, &input(json!({"reason_min_length": 2000}))).is_ok());
    }

    #[test]
    fn domain_overrides_are_normalised_and_bounded() {
        let base = Governance::defaults();
        let ok = validate_governance(&base, &input(json!({"domain_overrides": {"Code Review": "advisor"}})))
            .expect("normalised");
        assert_eq!(ok.domain_overrides.get("code_review"), Some(&json!("advisor")));
        for (bad, why) in [
            (json!({"governance": "voter"}), "an AI is always an observer"),
            (json!({"code": "boss"}), "must be observer"),
            (json!({"code": 3}), "must be observer"),
            (json!({"a-b": "voter"}), "must be 1 to 50 characters"),
            (
                json!({"code review": "voter", "Code_Review": "voter"}),
                "each domain can appear once",
            ),
            (json!({"x".repeat(51): "voter"}), "must be 1 to 50 characters"),
        ] {
            let message = message(validate_governance(&base, &input(json!({"domain_overrides": bad}))));
            assert!(message.starts_with("governance.domain_overrides: "), "{message}");
            assert!(message.contains(why), "{message} should contain {why}");
        }
        let many: serde_json::Map<String, serde_json::Value> =
            (0..51).map(|i| (format!("d{i}"), json!("voter"))).collect();
        let message = message(validate_governance(
            &base,
            &GovernanceInput {
                domain_overrides: Some(many),
                ..GovernanceInput::default()
            },
        ));
        assert_eq!(message, "governance.domain_overrides: at most 50 keys");
    }
}
