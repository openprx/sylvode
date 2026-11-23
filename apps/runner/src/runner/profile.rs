//! One static profile per CLI: the facts about its screen that `detect.rs` reads (prompt glyph,
//! placeholder texts, the marker of a pasted message, the dialogs it shows). Everything that
//! differs between the CLIs and is a fact of the CLI itself lives here; what the operator may
//! change lives in `[runner.cli.<name>]` (see `config.rs`).

use super::detect::Dialog;

/// What the runner knows about one CLI.
pub struct CliProfile {
    /// `claude`, `codex` or `opencode`.
    pub name: &'static str,
    /// Whether the CLI may run on the tmux channel (opencode is not verified for it).
    pub tmux: bool,
    /// The glyphs that start the input line.
    pub prompt_glyphs: &'static [char],
    /// Whether a numbered option of a dialog (`› 1. Yes`) is drawn with a prompt glyph and must
    /// not be taken for the input line.
    pub options_use_prompt_glyph: bool,
    /// Texts that count as an empty input line when they are not drawn faint.
    pub placeholders: &'static [&'static str],
    /// How a message pasted into the input line starts when the CLI shows it (the CLI may
    /// collapse a large paste into a marker of its own).
    pub pasted_markers: &'static [&'static str],
    /// A text of the footer or banner that says permission checks are off (compared lower case).
    pub footer_bypass: Option<&'static str>,
    /// How the CLI is asked which MCP server it has.
    pub mcp_probe: Option<McpProbe>,
    /// The blocking dialog on a screen of plain text (no escape sequences), by the texts
    /// verified for this CLI.
    pub dialogs: fn(&str) -> Option<Dialog>,
}

/// How a CLI reports its MCP servers and which file names the bot's MCP setup in its arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpProbe {
    /// `claude mcp get sylvode`; the arguments carry `--mcp-config <bot>.claude.json`.
    ClaudeMcpGet,
    /// `codex mcp get sylvode --json`; the arguments carry `<bot>.toml`.
    CodexMcpGetJson,
}

impl McpProbe {
    /// The file of the bot that the CLI's arguments name when the runner started it.
    pub fn args_file(self, bot_id: &str) -> String {
        match self {
            Self::ClaudeMcpGet => format!("{bot_id}.claude.json"),
            Self::CodexMcpGetJson => format!("{bot_id}.toml"),
        }
    }

    /// The arguments of the probe call, after the program.
    pub const fn command(self) -> &'static [&'static str] {
        match self {
            Self::ClaudeMcpGet => &["mcp", "get", "sylvode"],
            Self::CodexMcpGetJson => &["mcp", "get", "sylvode", "--json"],
        }
    }
}

impl CliProfile {
    /// Whether the text of the input line starts like a message the runner pasted.
    pub fn has_pasted_marker(&self, input: &str) -> bool {
        self.pasted_markers.iter().any(|marker| input.starts_with(marker))
    }
}

/// claude.
pub static CLAUDE: CliProfile = CliProfile {
    name: "claude",
    tmux: true,
    prompt_glyphs: &['>', '\u{276f}'],
    options_use_prompt_glyph: false,
    placeholders: &[],
    pasted_markers: &["[Sylvode"],
    footer_bypass: Some("bypass permissions on"),
    mcp_probe: Some(McpProbe::ClaudeMcpGet),
    dialogs: claude_dialogs,
};

/// codex (texts from the live captures of codex 0.171.0).
pub static CODEX: CliProfile = CliProfile {
    name: "codex",
    tmux: true,
    prompt_glyphs: &['\u{203a}'],
    options_use_prompt_glyph: true,
    // The placeholder is drawn faint; this is the non-faint fallback of the same composer.
    placeholders: &["Ask a follow-up question"],
    pasted_markers: &["[Sylvode", "[Pasted Content "],
    footer_bypass: Some("permissions: yolo mode"),
    mcp_probe: Some(McpProbe::CodexMcpGetJson),
    dialogs: codex_dialogs,
};

/// opencode: headless only.
pub static OPENCODE: CliProfile = CliProfile {
    name: "opencode",
    tmux: false,
    prompt_glyphs: &['>'],
    options_use_prompt_glyph: false,
    placeholders: &[],
    pasted_markers: &["[Sylvode"],
    footer_bypass: None,
    mcp_probe: None,
    dialogs: no_dialogs,
};

/// The profile of the CLI called `name`.
pub fn profile(name: &str) -> Option<&'static CliProfile> {
    [&CLAUDE, &CODEX, &OPENCODE].into_iter().find(|p| p.name == name)
}

const fn no_dialogs(_screen: &str) -> Option<Dialog> {
    None
}

fn claude_dialogs(screen: &str) -> Option<Dialog> {
    if screen.contains("Is this a project you created or one you trust") || screen.contains("Yes, I trust this folder")
    {
        Some(Dialog::Trust)
    } else if screen.contains("Bypass Permissions mode") && screen.contains("Yes, I accept") {
        Some(Dialog::BypassConfirm)
    } else if screen.contains("Select login method") || screen.contains("Please run /login") {
        Some(Dialog::NotLoggedIn)
    } else {
        None
    }
}

fn codex_dialogs(screen: &str) -> Option<Dialog> {
    if screen.contains("usage limit") {
        Some(Dialog::UsageLimit)
    } else if screen.contains("Welcome to Codex, OpenAI's command-line coding agent") {
        Some(Dialog::NotLoggedIn)
    } else if screen.contains("Trust this folder?") && screen.contains("1. Trust and continue") {
        Some(Dialog::Trust)
    } else if screen.contains("Hooks need review") {
        Some(Dialog::HooksReview)
    } else if screen.contains("Would you like to run the following command?") {
        Some(Dialog::CommandPermission)
    } else if asks_mcp_tool_approval(screen) {
        Some(Dialog::McpApproval)
    } else if screen.contains("enter select \u{b7} esc back") {
        Some(Dialog::Chooser)
    } else {
        None
    }
}

/// `Allow the <server> MCP server to run tool` on one line.
fn asks_mcp_tool_approval(screen: &str) -> bool {
    screen.lines().any(|line| {
        line.find("Allow the ")
            .is_some_and(|at| line[at + "Allow the ".len()..].contains(" MCP server to run tool"))
    })
}
