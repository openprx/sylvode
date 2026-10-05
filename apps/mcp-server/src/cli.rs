//! The workspace command groups shared by the `mcp-server` and `sylvode` executables.
//!
//! `projects`, `work-items`, `comments`, `labels`, `sprints`, `search`, `files`,
//! `operation-logs` and `tools` are defined exactly once, here, as [`BusinessCommands`], and run
//! by exactly one handler, [`run_business`]. `mcp-server` mounts them beside `serve` through
//! [`Cli`]; `sylvode` mounts the very same enum through [`BusinessCli`] (ADR-0020 D5). Both
//! executables therefore parse the same arguments, resolve configuration the same way, print
//! the same bytes and exit with the same codes for every one of these commands; only the
//! program name in `--help` differs.

// CLI output functions necessarily use print macros and indexing — allow these for this module.
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::indexing_slicing)]

use std::io::Write as _;
use std::path::PathBuf;

use base64::Engine as _;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use platform::config::{MCP_BOT_TOKEN_REQUIRED, McpConfig, McpRuntime, McpTransport, OpenPrConfig, Secret};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_CLI, transport_label};
use crate::protocol::{CallToolResult, ToolContent};
use crate::server::McpServer;

/// Tracing target of the MCP server and of the workspace commands, and the scope of the
/// default `[logging]` filter.
///
/// The module path `tracing` stamps on every event is `mcp_server`, so a filter written
/// against the binary's hyphenated name would silence the whole process.
pub const SERVICE_NAME: &str = "mcp_server";

/// Configuration key carrying the identity `stdio` and the CLI subcommands act as.
const BOT_TOKEN_KEY: &str = "mcp.bot_token";

#[derive(Debug, Clone, ValueEnum, Default)]
pub enum OutputFormat {
    #[default]
    Json,
    Table,
}

/// Sylvode MCP server and CLI tool. The `mcp-server` executable remains a compatibility shim.
#[derive(Debug, Parser)]
#[command(name = "mcp-server", about = "Sylvode MCP server and CLI tool")]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[command(flatten)]
    pub global: GlobalArgs,
}

/// The workspace command groups as `sylvode` mounts them: the same [`BusinessCommands`] and
/// [`GlobalArgs`] as [`Cli`], without `serve`.
///
/// Build the parser with [`business_cli_command`], which only adjusts help prose that names
/// the program.
#[derive(Debug, Parser)]
#[command(name = "sylvode", about = "Sylvode workspace commands")]
#[command(arg_required_else_help = true)]
pub struct BusinessCli {
    #[command(subcommand)]
    pub command: BusinessCommands,

    #[command(flatten)]
    pub global: GlobalArgs,
}

/// The options every workspace command and `serve` accept, wherever they appear on the line.
#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Path to the configuration file \[default: config/sylvode.toml; legacy config/openpr.toml fallback\]
    ///
    /// Global because every subcommand needs it: the settings it carries are read before
    /// the subcommand is dispatched, so `mcp-server projects list --config <path>` has to
    /// parse exactly like `mcp-server serve --config <path>`.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Output format
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Json)]
    pub format: OutputFormat,

    /// API URL (overrides `mcp.api_url`)
    #[arg(long, global = true)]
    pub api_url: Option<String>,

    /// Bot authentication token (overrides `mcp.bot_token`)
    ///
    /// Used by the CLI subcommands and by `serve --transport stdio`. The `http` and `sse`
    /// transports ignore it: they act as whoever calls them, never as a configured bot.
    #[arg(long, global = true)]
    pub bot_token: Option<String>,

    /// Workspace ID (overrides `mcp.workspace_id`)
    #[arg(long, global = true)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the MCP server (default mode)
    Serve(ServeArgs),
    #[command(flatten)]
    Business(BusinessCommands),
}

/// The nine workspace command groups, shared verbatim by `mcp-server` and `sylvode`.
#[derive(Debug, Subcommand)]
pub enum BusinessCommands {
    /// Manage projects
    Projects(ProjectsCmd),
    /// Manage work items
    #[command(name = "work-items")]
    WorkItems(WorkItemsCmd),
    /// Manage comments
    Comments(CommentsCmd),
    /// Manage labels
    Labels(LabelsCmd),
    /// Manage sprints
    Sprints(SprintsCmd),
    /// Global workspace search
    Search(SearchArgs),
    /// Upload files
    Files(FilesCmd),
    /// Inspect metadata-only bot operation records
    #[command(name = "operation-logs")]
    OperationLogs(OperationLogsCmd),
    /// Call any MCP tool by name
    Tools(ToolsCmd),
}

/// The top-level names of [`BusinessCommands`], in declaration order.
pub const BUSINESS_GROUPS: [&str; 9] = [
    "projects",
    "work-items",
    "comments",
    "labels",
    "sprints",
    "search",
    "files",
    "operation-logs",
    "tools",
];

/// The `sylvode` parser for the workspace command groups.
///
/// Identical to [`BusinessCli::command`] except that help prose naming `mcp-server` names
/// `sylvode` instead, so the only difference between the two executables' help is the program
/// name (ADR-0020 D5).
pub fn business_cli_command() -> clap::Command {
    BusinessCli::command().mut_arg("config", |arg| {
        let renamed = arg
            .get_long_help()
            .map(|help| help.to_string().replace("mcp-server ", "sylvode "));
        match renamed {
            Some(help) => arg.long_help(help),
            None => arg,
        }
    })
}

/// Parses `args` with [`business_cli_command`], exiting the way [`Parser::parse_from`] does on
/// a usage error or a help request.
pub fn parse_business_cli<I, T>(args: I) -> BusinessCli
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let mut command = business_cli_command();
    let mut matches = command.clone().get_matches_from(args);
    BusinessCli::from_arg_matches_mut(&mut matches)
        .map_err(|error| error.format(&mut command))
        .unwrap_or_else(|error| error.exit())
}

// ---- Serve ----

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Transport {
    Http,
    Sse,
    Stdio,
}

impl From<Transport> for McpTransport {
    fn from(transport: Transport) -> Self {
        match transport {
            Transport::Http => Self::Http,
            Transport::Sse => Self::Sse,
            Transport::Stdio => Self::Stdio,
        }
    }
}

// Both fields are optional rather than defaulted by `clap`: a clap default is
// indistinguishable from a value the operator typed, so defaulting here would silently
// override whatever `[mcp]` says. `None` means "the configuration file decides", and the
// file's own fallbacks are `mcp.transport = stdio` and `DEFAULT_MCP_BIND_ADDR`.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Transport protocol (overrides `mcp.transport`) \[default: stdio\]
    #[arg(long, value_enum)]
    pub transport: Option<Transport>,
    /// Bind address for HTTP/SSE transports (overrides `mcp.bind_addr`) \[default: 127.0.0.1:8090\]
    #[arg(long)]
    pub bind_addr: Option<String>,
}

// ---- Projects ----

#[derive(Debug, Args)]
pub struct ProjectsCmd {
    #[command(subcommand)]
    pub action: ProjectsAction,
}

#[derive(Debug, Subcommand)]
pub enum ProjectsAction {
    /// List all projects in the workspace
    List,
    /// Get a project by UUID
    Get {
        /// Project UUID
        id: String,
    },
    /// Create a new project
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
}

// ---- Work Items ----

#[derive(Debug, Args)]
pub struct WorkItemsCmd {
    #[command(subcommand)]
    pub action: WorkItemsAction,
}

#[derive(Debug, Subcommand)]
pub enum WorkItemsAction {
    /// List work items in a project
    List {
        #[arg(long)]
        project: String,
        /// Filter by state (`backlog|todo|in_progress|done`)
        #[arg(long)]
        state: Option<String>,
    },
    /// Get a work item by UUID or identifier (e.g. PRX-42)
    Get { id: String },
    /// Create a work item
    Create {
        #[arg(long)]
        project: String,
        #[arg(long)]
        title: String,
        /// Initial state (`backlog|todo|in_progress|done`)
        #[arg(long, default_value = "backlog")]
        state: String,
        /// Priority (`none|low|medium|high|urgent`)
        #[arg(long, default_value = "medium")]
        priority: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Search work items by query
    Search {
        #[arg(long)]
        query: String,
    },
    /// Update a work item
    Update {
        /// Work item UUID
        id: String,
        /// New state (`backlog|todo|in_progress|done`)
        #[arg(long)]
        state: Option<String>,
        /// New priority (`none|low|medium|high|urgent`)
        #[arg(long)]
        priority: Option<String>,
        #[arg(long)]
        title: Option<String>,
    },
}

// ---- Comments ----

#[derive(Debug, Args)]
pub struct CommentsCmd {
    #[command(subcommand)]
    pub action: CommentsAction,
}

#[derive(Debug, Subcommand)]
pub enum CommentsAction {
    /// List comments on a work item
    List {
        #[arg(long)]
        work_item: String,
    },
    /// Create a comment on a work item
    Create {
        #[arg(long)]
        work_item: String,
        #[arg(long)]
        content: String,
    },
}

// ---- Labels ----

#[derive(Debug, Args)]
pub struct LabelsCmd {
    #[command(subcommand)]
    pub action: LabelsAction,
}

#[derive(Debug, Subcommand)]
pub enum LabelsAction {
    /// List labels (workspace-wide, or project-specific with --project)
    List {
        #[arg(long)]
        project: Option<String>,
    },
}

// ---- Sprints ----

#[derive(Debug, Args)]
pub struct SprintsCmd {
    #[command(subcommand)]
    pub action: SprintsAction,
}

#[derive(Debug, Subcommand)]
pub enum SprintsAction {
    /// List sprints for a project
    List {
        #[arg(long)]
        project: String,
    },
}

// ---- Search ----

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Search query
    pub query: String,
}

// ---- Files ----

#[derive(Debug, Args)]
pub struct FilesCmd {
    #[command(subcommand)]
    pub action: FilesAction,
}

#[derive(Debug, Subcommand)]
pub enum FilesAction {
    /// Upload a file from disk (reads file, encodes to base64, posts to API)
    Upload {
        /// Path to the file to upload
        #[arg(long)]
        file: String,
    },
}

// ---- Bot Operation Logs ----

#[derive(Debug, Args)]
pub struct OperationLogsCmd {
    #[command(subcommand)]
    pub action: OperationLogsAction,
}

#[derive(Debug, Subcommand)]
pub enum OperationLogsAction {
    /// List operation records in reverse chronological order
    List {
        #[arg(long)]
        bot_id: Option<String>,
        #[arg(long)]
        tool_name: Option<String>,
        #[arg(long)]
        outcome: Option<String>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u64,
    },
}

// ---- Generic Tools ----

#[derive(Debug, Args)]
pub struct ToolsCmd {
    #[command(subcommand)]
    pub action: ToolsAction,
}

#[derive(Debug, Subcommand)]
pub enum ToolsAction {
    /// Call any MCP tool with a JSON object argument payload
    Call {
        /// Tool name, for example forms.list or plugins.invoke
        #[arg(long)]
        name: String,
        /// JSON object passed as MCP tool arguments
        #[arg(long, default_value = "{}")]
        args_json: String,
    },
}

// ---- Output formatting ----

pub fn print_result(format: &OutputFormat, result: &CallToolResult) {
    let text = result
        .content
        .iter()
        .find_map(|c| match c {
            ToolContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("");

    if result.is_error == Some(true) {
        eprintln!("{text}");
        std::process::exit(1);
    }

    match format {
        OutputFormat::Json => println!("{text}"),
        OutputFormat::Table => {
            if let Ok(value) = serde_json::from_str::<Value>(text) {
                print_table(&value);
            } else {
                println!("{text}");
            }
        }
    }
}

fn print_table(value: &Value) {
    match value {
        Value::Array(arr) if !arr.is_empty() => {
            if let Some(Value::Object(first)) = arr.first() {
                let keys: Vec<String> = first.keys().cloned().collect();
                let mut widths: Vec<usize> = keys.iter().map(String::len).collect();
                for item in arr {
                    if let Value::Object(obj) = item {
                        for (width, key) in widths.iter_mut().zip(keys.iter()) {
                            let s = fmt_val(obj.get(key).unwrap_or(&Value::Null));
                            *width = (*width).max(s.len().min(60));
                        }
                    }
                }
                // header
                for (key, width) in keys.iter().zip(widths.iter()) {
                    print!("{key:<width$}  ");
                }
                println!();
                // separator
                for w in &widths {
                    print!("{:-<w$}  ", "");
                }
                println!();
                // rows
                for item in arr {
                    if let Value::Object(obj) = item {
                        for (key, width) in keys.iter().zip(widths.iter()) {
                            let s = fmt_val(obj.get(key).unwrap_or(&Value::Null));
                            let truncated = truncate_display(s, 59);
                            print!("{truncated:<width$}  ");
                        }
                        println!();
                    }
                }
            } else {
                for item in arr {
                    println!("{}", fmt_val(item));
                }
            }
        }
        Value::Array(_) => println!("(empty)"),
        Value::Object(obj) => {
            let max_key = obj.keys().map(String::len).max().unwrap_or(0);
            for (key, val) in obj {
                println!("{key:<max_key$}  {}", fmt_val(val));
            }
        }
        _ => println!("{}", fmt_val(value)),
    }
}

/// Truncate a string to at most `max_bytes` bytes on a char boundary, appending `…` if truncated.
fn truncate_display(s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    // Find char boundary just before max_bytes
    let end = s
        .char_indices()
        .take_while(|(i, _)| *i < max_bytes)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    let prefix = s.get(..end).unwrap_or(&s);
    format!("{prefix}…")
}

fn fmt_val(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

// ---- Dispatch ----

/// The subcommand path a command line selected, as typed: `projects list`, `search`.
///
/// Names only, never argument values, so it is safe to print: a value may be a token.
pub fn subcommand_path(matches: &clap::ArgMatches) -> String {
    let mut path = Vec::new();
    let mut current = matches;
    while let Some((name, next)) = current.subcommand() {
        path.push(name);
        current = next;
    }
    path.join(" ")
}

/// Tells the user, on stderr and once per process, that `mcp-server <subcommand>` is deprecated
/// in favour of `sylvode <subcommand>` (ADR-0020 D2).
///
/// Never stdout: a workspace command's stdout is a machine contract, and nothing about it may
/// change. A failed write is dropped rather than reported, because stderr is the only channel
/// it could be reported on and D2 forbids the notice from failing the command; `writeln!` is
/// used instead of `eprintln!` because the latter panics when stderr cannot be written.
pub fn report_legacy_invocation(subcommand: &str) {
    static REPORTED: std::sync::Once = std::sync::Once::new();
    REPORTED.call_once(|| {
        let notice = platform::deprecation::legacy_cli_invocation(subcommand);
        let mut stderr = std::io::stderr().lock();
        let written = writeln!(stderr, "{notice}").and_then(|()| stderr.flush());
        drop(written);
    });
}

/// Runs one workspace command end to end: configuration, identity, the tool call and its
/// output. The single handler behind every [`BusinessCommands`] variant in both executables.
pub async fn run_business(global: &GlobalArgs, command: &BusinessCommands) -> anyhow::Result<()> {
    let mcp = prepare_runtime(global, None)?;
    // A CLI subcommand is a local process with no caller to act on behalf of, so it speaks to
    // the API as the configured identity and cannot run without one.
    let client = build_client(&mcp, Some(configured_bot_token(&mcp)?))?.with_transport_label(TRANSPORT_LABEL_CLI);
    run_cli_command(command, &global.format, client).await
}

/// Loads the configuration, installs the logger and layers the command-line overrides onto
/// `[mcp]`, returning the validated runtime settings.
///
/// Shared by `serve` and the workspace commands so that both resolve configuration in exactly
/// one way.
pub fn prepare_runtime(global: &GlobalArgs, serve: Option<&ServeArgs>) -> anyhow::Result<McpRuntime> {
    // The file is the only source of configuration; nothing below reads the environment.
    let mut config = OpenPrConfig::load(global.config.as_deref())?;
    // stdio frames JSON-RPC on stdout, so the log stream is reserved to stderr no
    // matter what the file asks for.
    platform::logging::init_reserving_stdout(&config.logging, SERVICE_NAME)?;
    // Emitted only now: the logger it goes through did not exist while the file was loading.
    if let Some(notice) = platform::config::take_legacy_discovery_notice() {
        tracing::warn!("{notice}");
    }

    apply_cli_overrides(&mut config.mcp, global, serve)?;

    // Lazy validation: `[mcp]` is optional for the other binaries, so the fields this one
    // cannot run without are reported here, all of them in one pass.
    Ok(config.mcp_runtime()?)
}

/// The configured identity, or the refusal that names the key which supplies it.
///
/// Reported here rather than at load time because an `http`/`sse` deployment is *expected*
/// to have no `mcp.bot_token`: it never speaks to the API as itself.
pub fn configured_bot_token(mcp: &McpRuntime) -> anyhow::Result<Secret> {
    mcp.bot_token
        .clone()
        .ok_or_else(|| anyhow::anyhow!("{MCP_BOT_TOKEN_REQUIRED} (missing {BOT_TOKEN_KEY})"))
}

/// Builds the API client every request of one transport starts from.
///
/// `credential` is `None` for the networked transports. That is the structural half of the
/// invariant this server rests on: there is no server-side identity in the process for a
/// networked request to fall back to, so a request that somehow reached a tool without a
/// caller credential fails closed instead of quietly acting as a workspace bot.
pub fn build_client(mcp: &McpRuntime, credential: Option<Secret>) -> anyhow::Result<OpenPrClient> {
    OpenPrClient::new(ClientConfig {
        base_url: mcp.api_url.clone(),
        credential,
        workspace_id: mcp.workspace_id.to_string(),
        transport_label: transport_label(mcp.transport),
    })
    .map_err(|e| anyhow::anyhow!(e))
}

/// Layers the CLI overrides onto the file's `[mcp]` section.
///
/// A flag wins over the file: the file is the deployment's configuration, a flag is a
/// deliberate one-off. A value that arrives through a flag has not passed the file's
/// validation, so it is checked here — against the same rules, reported against the flag
/// that carried it. Values that came from the file are left alone: they are already
/// validated, and checking them twice is how one bad value grows two different messages.
fn apply_cli_overrides(mcp: &mut McpConfig, global: &GlobalArgs, serve: Option<&ServeArgs>) -> anyhow::Result<()> {
    if let Some(api_url) = global.api_url.as_deref() {
        mcp.api_url = Some(checked_api_url(api_url)?);
    }
    if let Some(bot_token) = global.bot_token.as_deref() {
        mcp.bot_token = Some(checked_bot_token(bot_token)?);
    }
    if let Some(workspace_id) = global.workspace_id.as_deref() {
        mcp.workspace_id = Some(checked_workspace_id(workspace_id)?);
    }
    if let Some(serve) = serve {
        if let Some(transport) = serve.transport {
            mcp.transport = transport.into();
        }
        if let Some(bind_addr) = serve.bind_addr.as_deref() {
            mcp.bind_addr = Some(checked_bind_addr(bind_addr)?);
        }
    }
    Ok(())
}

/// Validates an `--api-url`, mirroring the rules `mcp.api_url` is held to.
fn checked_api_url(value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") {
        anyhow::bail!("--api-url must be a concrete URL, not a placeholder");
    }
    let parsed =
        reqwest::Url::parse(trimmed).map_err(|error| anyhow::anyhow!("--api-url is not a valid URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("--api-url must start with http:// or https://");
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        anyhow::bail!("--api-url names no host");
    }
    Ok(trimmed.to_string())
}

/// Validates a `--bot-token`, mirroring the rules `mcp.bot_token` is held to.
///
/// The token itself is never echoed, not even its prefix.
fn checked_bot_token(value: &str) -> anyhow::Result<Secret> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") || trimmed.contains("replace_with") {
        anyhow::bail!("--bot-token must be a concrete bot token");
    }
    if !trimmed.starts_with("opr_") {
        anyhow::bail!("--bot-token must use the opr_ token prefix");
    }
    Ok(Secret::new(trimmed))
}

/// Validates a `--workspace-id`, mirroring the rules `mcp.workspace_id` is held to.
fn checked_workspace_id(value: &str) -> anyhow::Result<Uuid> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") || trimmed.contains("replace_with") {
        anyhow::bail!("--workspace-id must be a concrete UUID, not a placeholder");
    }
    let parsed =
        Uuid::parse_str(trimmed).map_err(|error| anyhow::anyhow!("--workspace-id is not a valid UUID: {error}"))?;
    if parsed.is_nil() {
        anyhow::bail!("--workspace-id must not be the nil UUID placeholder");
    }
    Ok(parsed)
}

/// Validates a `--bind-addr`, mirroring the rules `mcp.bind_addr` is held to.
///
/// A host without a port is refused rather than completed with one: an invented port would
/// put the listener somewhere the operator did not ask for and did not publish.
fn checked_bind_addr(value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") {
        anyhow::bail!("--bind-addr must be a concrete host:port, not a placeholder");
    }
    if trimmed.chars().any(char::is_whitespace) {
        anyhow::bail!("--bind-addr must not contain whitespace");
    }
    let (host, port) = split_host_port(trimmed)
        .ok_or_else(|| anyhow::anyhow!("--bind-addr must be host:port, e.g. 127.0.0.1:8090"))?;
    if host.is_empty() {
        anyhow::bail!("--bind-addr names no host");
    }
    match port.parse::<u16>() {
        Ok(0) | Err(_) => anyhow::bail!("--bind-addr has an invalid port {port}, expected 1-65535"),
        Ok(_) => Ok(trimmed.to_string()),
    }
}

/// Splits an authority into host and port, tolerating a bracketed IPv6 literal.
fn split_host_port(authority: &str) -> Option<(&str, &str)> {
    if let Some(end) = authority.rfind(']') {
        let port = authority.get(end + 1..)?.strip_prefix(':')?;
        return Some((authority.get(..=end)?, port));
    }
    authority.rsplit_once(':')
}

async fn run_cli_command(
    command: &BusinessCommands,
    format: &OutputFormat,
    client: OpenPrClient,
) -> anyhow::Result<()> {
    let server = McpServer::new(client);

    // Files upload requires async disk I/O before calling execute_tool, handle it separately
    if let BusinessCommands::Files(files_cmd) = command {
        let result = run_file_upload(files_cmd, &server).await?;
        print_result(format, &result);
        return Ok(());
    }

    let (tool_name, args): (&str, Value) = match command {
        BusinessCommands::Projects(cmd) => match &cmd.action {
            ProjectsAction::List => ("projects.list", json!({})),
            ProjectsAction::Get { id } => ("projects.get", json!({ "project_id": id })),
            ProjectsAction::Create { name, description } => {
                let mut body = json!({ "name": name });
                if let Some(desc) = description {
                    body["description"] = json!(desc);
                }
                ("projects.create", body)
            }
        },

        BusinessCommands::WorkItems(cmd) => match &cmd.action {
            WorkItemsAction::List { project, state } => {
                let mut args = json!({ "project_id": project });
                if let Some(s) = state {
                    args["state"] = json!(s);
                }
                ("work_items.list", args)
            }
            WorkItemsAction::Get { id } => {
                // Heuristic: 36-char hex with dashes is UUID, else treat as identifier
                if id.len() == 36 && id.chars().filter(|&c| c == '-').count() == 4 {
                    ("work_items.get", json!({ "work_item_id": id }))
                } else {
                    ("work_items.get_by_identifier", json!({ "identifier": id }))
                }
            }
            WorkItemsAction::Create {
                project,
                title,
                state,
                priority,
                description,
            } => {
                let mut args = json!({
                    "project_id": project,
                    "title": title,
                    "state": state,
                    "priority": priority,
                });
                if let Some(desc) = description {
                    args["description"] = json!(desc);
                }
                ("work_items.create", args)
            }
            WorkItemsAction::Search { query } => ("work_items.search", json!({ "query": query })),
            WorkItemsAction::Update {
                id,
                state,
                priority,
                title,
            } => {
                let mut args = json!({ "work_item_id": id });
                if let Some(s) = state {
                    args["state"] = json!(s);
                }
                if let Some(p) = priority {
                    args["priority"] = json!(p);
                }
                if let Some(t) = title {
                    args["title"] = json!(t);
                }
                ("work_items.update", args)
            }
        },

        BusinessCommands::Comments(cmd) => match &cmd.action {
            CommentsAction::List { work_item } => ("comments.list", json!({ "work_item_id": work_item })),
            CommentsAction::Create { work_item, content } => (
                "comments.create",
                json!({ "work_item_id": work_item, "content": content }),
            ),
        },

        BusinessCommands::Labels(cmd) => match &cmd.action {
            LabelsAction::List { project } => project.as_ref().map_or_else(
                || ("labels.list", json!({})),
                |pid| ("labels.list_by_project", json!({ "project_id": pid })),
            ),
        },

        BusinessCommands::Sprints(cmd) => match &cmd.action {
            SprintsAction::List { project } => ("sprints.list", json!({ "project_id": project })),
        },

        BusinessCommands::Search(search_args) => ("search.all", json!({ "query": search_args.query })),

        // Files is handled above via `run_file_upload`, which returns before this match.
        BusinessCommands::Files(_) => anyhow::bail!("files is handled by run_file_upload before this match"),

        BusinessCommands::OperationLogs(cmd) => match &cmd.action {
            OperationLogsAction::List {
                bot_id,
                tool_name,
                outcome,
                cursor,
                limit,
            } => {
                let mut args = json!({ "limit": limit });
                if let Some(value) = bot_id {
                    args["bot_id"] = json!(value);
                }
                if let Some(value) = tool_name {
                    args["tool_name"] = json!(value);
                }
                if let Some(value) = outcome {
                    args["outcome"] = json!(value);
                }
                if let Some(value) = cursor {
                    args["cursor"] = json!(value);
                }
                ("bot_operation_logs.list", args)
            }
        },

        BusinessCommands::Tools(cmd) => match &cmd.action {
            ToolsAction::Call { name, args_json } => (name.as_str(), parse_tool_args_json(args_json)?),
        },
    };

    let result = server.call_tool(tool_name, args).await;
    print_result(format, &result);
    Ok(())
}

fn parse_tool_args_json(args_json: &str) -> anyhow::Result<Value> {
    let value = serde_json::from_str::<Value>(args_json)
        .map_err(|error| anyhow::anyhow!("Invalid --args-json payload: {error}"))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(anyhow::anyhow!("--args-json must be a JSON object"))
    }
}

/// Handle file upload: read file from disk, base64-encode, then call files.upload tool.
async fn run_file_upload(cmd: &FilesCmd, server: &McpServer) -> anyhow::Result<CallToolResult> {
    match &cmd.action {
        FilesAction::Upload { file } => {
            let path = std::path::Path::new(file.as_str());
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file.as_str())
                .to_string();
            let content = tokio::fs::read(path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to read file {file}: {e}"))?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&content);
            // Must go through `call_tool`: `execute_tool` skips the project policy gate
            // and the tool-call audit, so calling it here made the CLI a bypass.
            Ok(server
                .call_tool(
                    "files.upload",
                    json!({ "filename": filename, "content_base64": encoded }),
                )
                .await)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BUSINESS_GROUPS, BusinessCli, BusinessCommands, Cli, Commands, McpTransport, OperationLogsAction, ToolsAction,
        Transport, business_cli_command, checked_api_url, checked_bind_addr, checked_bot_token, checked_workspace_id,
        parse_tool_args_json,
    };
    use clap::{CommandFactory, Parser};
    use platform::config::DEFAULT_MCP_API_URL;
    use serde_json::json;

    #[test]
    fn parses_generic_tool_call_command() {
        let cli = Cli::try_parse_from([
            "mcp-server",
            "tools",
            "call",
            "--name",
            "forms.list",
            "--args-json",
            r#"{"project_id":"project-1"}"#,
        ])
        .expect("generic tools call command should parse");

        match cli.command {
            Commands::Business(BusinessCommands::Tools(cmd)) => match cmd.action {
                ToolsAction::Call { name, args_json } => {
                    assert_eq!(name, "forms.list");
                    assert_eq!(
                        parse_tool_args_json(&args_json).unwrap(),
                        json!({ "project_id": "project-1" })
                    );
                }
            },
            _ => panic!("expected tools command"),
        }
    }

    #[test]
    fn parses_operation_log_filters() {
        let cli = Cli::try_parse_from([
            "mcp-server",
            "operation-logs",
            "list",
            "--tool-name",
            "forms.list",
            "--outcome",
            "error",
            "--limit",
            "25",
        ])
        .expect("operation log command should parse");

        match cli.command {
            Commands::Business(BusinessCommands::OperationLogs(command)) => match command.action {
                OperationLogsAction::List {
                    tool_name,
                    outcome,
                    limit,
                    ..
                } => {
                    assert_eq!(tool_name.as_deref(), Some("forms.list"));
                    assert_eq!(outcome.as_deref(), Some("error"));
                    assert_eq!(limit, 25);
                }
            },
            _ => panic!("expected operation-logs command"),
        }
    }

    #[test]
    fn generic_tool_args_must_be_json_object() {
        assert!(parse_tool_args_json(r#"{"project_id":"project-1"}"#).is_ok());
        assert!(parse_tool_args_json("[]").is_err());
        assert!(parse_tool_args_json("{not-json}").is_err());
    }

    /// An unspecified `--bind-addr` must stay `None` so `[mcp] bind_addr` still decides.
    /// A clap default here would look exactly like an operator supplied value and would
    /// override the file on every single run.
    #[test]
    fn an_unspecified_transport_and_bind_address_defer_to_the_configuration_file() {
        let cli = Cli::try_parse_from(["mcp-server", "serve", "--transport", "http"])
            .expect("serve http command should parse");

        match cli.command {
            Commands::Serve(args) => {
                assert!(matches!(args.transport, Some(Transport::Http)));
                assert_eq!(args.bind_addr, None);
            }
            Commands::Business(_) => panic!("expected serve command"),
        }

        let bare = Cli::try_parse_from(["mcp-server", "serve"]).expect("bare serve should parse");
        match bare.command {
            Commands::Serve(args) => {
                assert!(args.transport.is_none());
                assert!(args.bind_addr.is_none());
            }
            Commands::Business(_) => panic!("expected serve command"),
        }
    }

    /// `--config` is global, so it has to parse after any subcommand, not just after
    /// `serve`. It used to be possible to declare it on `serve` alone, which made
    /// `mcp-server projects list --config <path>` fail to parse.
    #[test]
    fn the_config_flag_parses_on_every_subcommand() {
        for args in [
            vec!["mcp-server", "serve", "--config", "/etc/openpr.toml"],
            vec!["mcp-server", "projects", "list", "--config", "/etc/openpr.toml"],
            vec![
                "mcp-server",
                "work-items",
                "get",
                "PRX-1",
                "--config",
                "/etc/openpr.toml",
            ],
            vec!["mcp-server", "search", "anything", "--config", "/etc/openpr.toml"],
            vec![
                "mcp-server",
                "tools",
                "call",
                "--name",
                "forms.list",
                "--config",
                "/etc/openpr.toml",
            ],
            vec!["mcp-server", "--config", "/etc/openpr.toml", "projects", "list"],
        ] {
            let cli = Cli::try_parse_from(&args).unwrap_or_else(|error| panic!("{args:?} should parse: {error}"));
            assert_eq!(
                cli.global.config.as_deref(),
                Some(std::path::Path::new("/etc/openpr.toml")),
                "{args:?} did not carry the config path"
            );
        }
    }

    #[test]
    fn transport_flags_map_onto_the_configuration_enum() {
        assert_eq!(McpTransport::from(Transport::Stdio), McpTransport::Stdio);
        assert_eq!(McpTransport::from(Transport::Http), McpTransport::Http);
        assert_eq!(McpTransport::from(Transport::Sse), McpTransport::Sse);
    }

    /// `BUSINESS_GROUPS` is what `sylvode` routes on, so it has to name exactly the groups the
    /// shared enum defines, in both parsers, and `serve` must stay `mcp-server`'s alone.
    #[test]
    fn the_business_group_list_matches_both_parsers() {
        let legacy: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|command| command.get_name().to_string())
            .filter(|name| name != "serve")
            .collect();
        let sylvode: Vec<String> = BusinessCli::command()
            .get_subcommands()
            .map(|command| command.get_name().to_string())
            .collect();
        assert_eq!(legacy, BUSINESS_GROUPS);
        assert_eq!(sylvode, BUSINESS_GROUPS);
        assert!(Cli::command().find_subcommand("serve").is_some());
        assert!(BusinessCli::command().find_subcommand("serve").is_none());
    }

    /// The `sylvode` parser renames the program in help prose and nothing else.
    #[test]
    fn the_sylvode_parser_only_renames_the_program_in_the_config_help() {
        let mut command = business_cli_command();
        let help = command.render_long_help().to_string();
        assert!(help.contains("`sylvode projects list --config <path>`"), "{help}");
        assert!(!help.contains("mcp-server projects list"), "{help}");
    }

    #[test]
    fn accepts_concrete_cli_overrides() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(checked_api_url(" http://api:8080 ")?, "http://api:8080");
        assert_eq!(
            checked_bot_token("opr_forms_mcp_test_token")?.expose(),
            "opr_forms_mcp_test_token"
        );
        assert_eq!(
            checked_workspace_id("550e8400-e29b-41d4-a716-446655440000")?.to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(checked_bind_addr("0.0.0.0:8090")?, "0.0.0.0:8090");
        assert_eq!(checked_bind_addr("[::1]:8090")?, "[::1]:8090");
        Ok(())
    }

    #[test]
    fn the_api_url_default_still_targets_the_compose_api_port() {
        assert_eq!(DEFAULT_MCP_API_URL, "http://localhost:8081");
    }

    /// The shell templates a compose file used to interpolate are values, not configuration:
    /// reaching the process unexpanded means the deployment is broken, so they are refused
    /// rather than used as a hostname or a token.
    #[test]
    fn rejects_unexpanded_shell_templates_on_the_command_line() {
        assert!(checked_api_url("${OPENPR_API_URL:-http://api:8080}").is_err());
        assert!(checked_bot_token("${OPENPR_BOT_TOKEN:?set OPENPR_BOT_TOKEN}").is_err());
        assert!(checked_workspace_id("${OPENPR_WORKSPACE_ID:?set OPENPR_WORKSPACE_ID}").is_err());
        assert!(checked_bind_addr("${OPENPR_MCP_BIND_ADDR}").is_err());
    }

    #[test]
    fn rejects_placeholder_token_and_nil_workspace_on_the_command_line() {
        assert!(checked_bot_token("opr_replace_with_workspace_bot_token").is_err());
        assert!(checked_bot_token("some_other_prefix_token").is_err());
        assert!(checked_bot_token("").is_err());
        assert!(checked_workspace_id("00000000-0000-0000-0000-000000000000").is_err());
        assert!(checked_workspace_id("not-a-uuid").is_err());
    }

    /// A `--bot-token` failure must not print the token it rejected.
    #[test]
    fn a_rejected_bot_token_is_never_echoed() {
        let Err(error) = checked_bot_token("nope_secret_material_here") else {
            panic!("a token without the opr_ prefix must be refused");
        };
        assert!(!error.to_string().contains("secret_material"), "{error}");
    }

    #[test]
    fn rejects_api_urls_that_are_not_absolute_http_urls() {
        assert!(checked_api_url("ftp://api:8080").is_err());
        assert!(checked_api_url("api:8080").is_err());
        assert!(checked_api_url("").is_err());
    }

    #[test]
    fn rejects_bind_addresses_without_a_usable_port() {
        assert!(checked_bind_addr("0.0.0.0").is_err());
        assert!(checked_bind_addr("0.0.0.0:0").is_err());
        assert!(checked_bind_addr("0.0.0.0:not-a-port").is_err());
        assert!(checked_bind_addr(":8090").is_err());
        assert!(checked_bind_addr("0.0.0.0 :8090").is_err());
    }
}
