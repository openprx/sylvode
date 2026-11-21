#![allow(clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::checks::{Accepted, BotCreds, Channel};
use super::files::StateDir;
use super::headless::prepare;
use super::sv_client::SvClient;
use super::tmux::allowed_environment;
use crate::config::Config;

const BOT: &str = "00000000-0000-4000-8000-000000000001";

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-headless-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        Self(dir.canonicalize().expect("canonical"))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config(extra: &str) -> Config {
    Config::from_toml(&format!(
        "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n\
         [ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:2\"\n\
         [[ai_task_callback.bots]]\nbot_id = \"{BOT}\"\ntoken = \"opr_{}\"\nsurface = \"mcp_stdio\"\n\
         [runner]\nallowed_roots = [\"/\"]\n{extra}\n",
        "k9".repeat(14)
    ))
    .expect("config")
}

fn accepted(cli: &str, dir: &Path, key: &str, program: &Path) -> Accepted {
    Accepted {
        client: SvClient::new(reqwest::Client::new(), "http://127.0.0.1:2", "opr_x", "mcp_stdio"),
        bot: BotCreds {
            bot_id: BOT.to_string(),
            api_url: "http://127.0.0.1:2".to_string(),
            token: format!("opr_{}", "k9".repeat(14)),
        },
        task_id: "11111111-1111-4111-8111-111111111111".to_string(),
        attempt: 1,
        max_attempts: 3,
        project_key: key.to_string(),
        project_id: "22222222-2222-4222-8222-222222222222".to_string(),
        message: "[Sylvode task] id=x".to_string(),
        dir: dir.to_path_buf(),
        channel: Channel::Headless,
        cli: cli.to_string(),
        program: program.to_path_buf(),
        mcp_server: PathBuf::from("/opt/mcp-server"),
    }
}

#[test]
fn a_directory_with_cli_configuration_that_cannot_be_switched_off_is_refused_unless_trusted() {
    let base = Dir::new();
    let state = StateDir::open(&base.0.join("state")).expect("state");
    let project = base.0.join("project");
    fs::create_dir_all(project.join(".codex")).expect("mkdir");
    fs::write(
        project.join(".codex/config.toml"),
        "[mcp_servers.evil]\ncommand = \"/bin/false\"\n",
    )
    .expect("write");
    let plain = base.0.join("plain");
    fs::create_dir_all(&plain).expect("mkdir");
    let bin = Path::new("/usr/bin/codex");
    let cfg = config("[runner.cli.codex]\nenabled = true\n");
    let refused = prepare(
        &cfg,
        &state,
        &accepted("codex", &project, "SVWL", bin),
        Path::new("/opt/mcp-server"),
    )
    .expect_err("refused");
    assert_eq!(refused.code, "workspace_untrusted");
    assert!(
        !refused.reason.contains(&*project.to_string_lossy()),
        "{}",
        refused.reason
    );
    // A directory without such files runs; so does the project once the operator trusts it.
    assert!(
        prepare(
            &cfg,
            &state,
            &accepted("codex", &plain, "SVWL", bin),
            Path::new("/opt/mcp-server")
        )
        .is_ok()
    );
    let trusted = config("trust_project_files = [\"SVWL\"]\n[runner.cli.codex]\nenabled = true\n");
    assert!(
        prepare(
            &trusted,
            &state,
            &accepted("codex", &project, "SVWL", bin),
            Path::new("/opt/mcp-server")
        )
        .is_ok()
    );
    assert!(
        prepare(
            &trusted,
            &state,
            &accepted("codex", &project, "OTHER", bin),
            Path::new("/opt/mcp-server")
        )
        .is_err()
    );
    // opencode likewise.
    fs::write(plain.join("opencode.json"), "{}").expect("write");
    assert!(
        prepare(
            &cfg,
            &state,
            &accepted("opencode", &plain, "SVWL", Path::new("/usr/bin/opencode")),
            Path::new("/m")
        )
        .is_err()
    );
    // claude is controlled by its flags and needs no such refusal.
    fs::create_dir_all(project.join(".claude")).expect("mkdir");
    assert!(
        prepare(
            &cfg,
            &state,
            &accepted("claude", &project, "SVWL", Path::new("/usr/bin/claude")),
            Path::new("/m")
        )
        .is_ok()
    );
}

#[test]
fn prepare_writes_private_files_and_an_argv_without_the_token() {
    let base = Dir::new();
    let state = StateDir::open(&base.0.join("state")).expect("state");
    let project = base.0.join("project");
    fs::create_dir_all(&project).expect("mkdir");
    let cfg = config("");
    let prepared = prepare(
        &cfg,
        &state,
        &accepted("claude", &project, "SVWL", Path::new("/usr/bin/claude")),
        Path::new("/opt/mcp-server"),
    )
    .expect("prepared");
    assert!(prepared.args.iter().all(|a| !a.contains("opr_")), "{:?}", prepared.args);
    assert!(prepared.args.contains(&"--strict-mcp-config".to_string()));
    let toml = state.mcp_toml(BOT);
    assert_eq!(fs::metadata(&toml).expect("meta").permissions().mode() & 0o777, 0o600);
    assert!(fs::read_to_string(&toml).expect("toml").contains("opr_"));
    assert!(
        !fs::read_to_string(state.claude_mcp_json(BOT))
            .expect("json")
            .contains("opr_")
    );
}

#[test]
fn the_environment_is_the_allow_list_plus_the_named_extras() {
    let names = |extra: &[String]| -> Vec<String> {
        allowed_environment(extra, false)
            .into_iter()
            .map(|(n, _)| n.to_string_lossy().into_owned())
            .collect()
    };
    let plain = names(&[]);
    assert!(plain.iter().any(|n| n == "PATH"));
    // The test runs under cargo, which sets many variables; none passes unless named.
    assert!(
        plain.iter().all(|n| !n.starts_with("CARGO") && !n.starts_with("RUST")),
        "{plain:?}"
    );
    let with = names(&["CARGO_MANIFEST_DIR".to_string()]);
    assert!(with.iter().any(|n| n == "CARGO_MANIFEST_DIR"));
    // The agent variables are never passed, even when named.
    assert!(
        names(&["TMUX".to_string(), "CLAUDECODE".to_string()])
            .iter()
            .all(|n| n != "TMUX" && n != "CLAUDECODE")
    );
}

#[test]
fn the_start_up_posture_has_one_line_per_cli_and_warns_about_opencode() {
    let lines = super::posture(&config(""));
    assert_eq!(lines.len(), 3);
    assert!(
        lines[0].text.starts_with("claude: permission_mode = dontAsk"),
        "{:?}",
        lines[0]
    );
    assert!(lines[0].text.contains("8 search-mode Sylvode tools") && !lines[0].warn);
    assert_eq!(lines[1].text, "codex: disabled, sandbox = read-only, approvals never");
    assert_eq!(lines[2].text, "opencode: disabled");
    let wide = super::posture(&config(
        "[runner.cli.opencode]\nenabled = true\n[runner.cli.claude]\nallow_write_call = true\n",
    ));
    assert!(wide[0].text.contains("sylvode_write"), "{:?}", wide[0]);
    assert!(
        wide[2].warn
            && wide[2]
                .text
                .contains("opencode permissions are not controlled by sylvode-runner")
    );
}
