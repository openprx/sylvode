use std::fs;
use std::path::{Path, PathBuf};

use super::RunnerRuntime;
use crate::config::Config;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-runtime-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create dir");
        Self(dir.canonicalize().expect("canonical"))
    }

    fn sub(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).expect("mkdir");
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const BOT_A: &str = "0b7e2d4c-2f0a-4b8e-8f6c-3a1d9e5c7b20";
const BOT_B: &str = "5989a9a2-c7f1-4a74-ba05-cffe01e33ad4";

fn config(state: &Path, runner_roots: &[&Path], agent_a: &[&Path], agent_b: &[&Path]) -> Config {
    let list = |paths: &[&Path]| {
        paths
            .iter()
            .map(|p| format!("\"{}\"", p.display()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let agent_roots = |paths: &[&Path]| {
        if paths.is_empty() {
            String::new()
        } else {
            format!("allowed_roots = [{}]\n", list(paths))
        }
    };
    Config::from_toml(&format!(
        "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n\
         [ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:2\"\n\
         [[ai_task_callback.bots]]\nbot_id = \"{BOT_A}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"opr_{t}\"\nsurface = \"mcp_stdio\"\nwebhook_secret = \"secret-for-bot-a-1\"\n\
         [[ai_task_callback.bots]]\nbot_id = \"{BOT_B}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"opr_{t}\"\nsurface = \"mcp_stdio\"\nwebhook_secret = \"secret-for-bot-b-1\"\n\
         [runner]\nstate_dir = \"{state}\"\nallowed_roots = [{runner_roots}]\n\
         [[agents]]\nid = \"{BOT_A}\"\nname = \"A\"\nagent_type = \"runner\"\n{a}[agents.route]\nbot_ids = [\"{BOT_A}\"]\n\
         [[agents]]\nid = \"{BOT_B}\"\nname = \"B\"\nagent_type = \"runner\"\n{b}[agents.route]\nbot_ids = [\"{BOT_B}\"]\n",
        t = "k9".repeat(14),
        state = state.display(),
        runner_roots = list(runner_roots),
        a = agent_roots(agent_a),
        b = agent_roots(agent_b),
    ))
    .expect("config")
}

/// The state directory holds every bot's token: inside, equal to or above an allowed root it would
/// make the tokens a project directory, so the roots are refused.
#[test]
fn the_state_directory_must_be_outside_every_root() {
    let base = Dir::new();
    let work = base.sub("work");
    let (a, b) = (base.sub("a"), base.sub("b"));
    // Outside: fine.
    let state = base.0.join("state");
    let ok = RunnerRuntime::new(&config(&state, &[], &[&a], &[&b]));
    assert!(ok.agent_roots.values().all(Result::is_ok));
    // Inside a root, equal to it, and above it.
    for (state, root) in [
        (work.join("state"), work.clone()),
        (work.clone(), work.clone()),
        (base.0.clone(), work.clone()),
    ] {
        let runtime = RunnerRuntime::new(&config(&state, &[], &[&root], &[&b]));
        assert!(
            matches!(runtime.agent_roots.get(BOT_A), Some(Err(e)) if e.contains("state_dir")),
            "{state:?} vs {root:?}"
        );
    }
}

/// Two runner agents with their own roots must not overlap, so one agent's task cannot read
/// another agent's tree; each agent checks directories against its own roots only.
#[test]
fn agents_have_their_own_disjoint_roots() {
    let base = Dir::new();
    let (a, b) = (base.sub("a"), base.sub("b"));
    let state = base.0.join("state");
    let runtime = RunnerRuntime::new(&config(&state, &[], &[&a], &[&b]));
    let (agent_a, agent_b) = (
        runtime.agent_roots.get(BOT_A).and_then(|r| r.as_ref().ok()).expect("a"),
        runtime.agent_roots.get(BOT_B).and_then(|r| r.as_ref().ok()).expect("b"),
    );
    let in_a = a.to_string_lossy().into_owned();
    let in_b = b.to_string_lossy().into_owned();
    assert!(agent_a.check_directory(&in_a).is_ok());
    assert_eq!(
        agent_a.check_directory(&in_b).expect_err("b is outside a").code,
        "path_outside_allowlist"
    );
    assert!(agent_b.check_directory(&in_b).is_ok());
    // Overlap (one root inside the other) refuses both.
    let nested = a.join("inner");
    fs::create_dir_all(&nested).expect("mkdir");
    let runtime = RunnerRuntime::new(&config(&state, &[], &[&a], &[&nested]));
    assert!(matches!(runtime.agent_roots.get(BOT_A), Some(Err(e)) if e.contains("overlap")));
    assert!(matches!(runtime.agent_roots.get(BOT_B), Some(Err(e)) if e.contains("overlap")));
}

/// With more than one runner agent each needs its own `allowed_roots`.
#[test]
fn more_than_one_runner_agent_needs_a_set_of_roots_each() {
    let base = Dir::new();
    let a = base.sub("a");
    let state = base.0.join("state");
    let error = Config::from_toml(&format!(
        "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n\
         [ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:2\"\n\
         [[ai_task_callback.bots]]\nbot_id = \"{BOT_A}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"opr_{t}\"\nsurface = \"mcp_stdio\"\nwebhook_secret = \"secret-for-bot-a-1\"\n\
         [[ai_task_callback.bots]]\nbot_id = \"{BOT_B}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"opr_{t}\"\nsurface = \"mcp_stdio\"\nwebhook_secret = \"secret-for-bot-b-1\"\n\
         [runner]\nstate_dir = \"{s}\"\nallowed_roots = [\"{a}\"]\n\
         [[agents]]\nid = \"{BOT_A}\"\nname = \"A\"\nagent_type = \"runner\"\n[agents.route]\nbot_ids = [\"{BOT_A}\"]\n\
         [[agents]]\nid = \"{BOT_B}\"\nname = \"B\"\nagent_type = \"runner\"\n[agents.route]\nbot_ids = [\"{BOT_B}\"]\n",
        t = "k9".repeat(14),
        s = state.display(),
        a = a.display(),
    ))
    .expect_err("refused");
    assert!(
        error.contains("allowed_roots") && error.contains("more than one runner agent"),
        "{error}"
    );
}

/// The configuration file holds every token: a root that contains it is refused, like one that
/// contains the state directory.
#[test]
fn the_configuration_file_must_be_outside_every_root() {
    let base = Dir::new();
    let (a, b) = (base.sub("a"), base.sub("b"));
    let state = base.0.join("state");
    let mut inside = config(&state, &[], &[&a], &[&b]);
    inside.config_path = Some(a.join("sub").join("config.toml"));
    let runtime = RunnerRuntime::new(&inside);
    assert!(matches!(runtime.agent_roots.get(BOT_A), Some(Err(e)) if e.contains("configuration file")));
    assert!(matches!(runtime.agent_roots.get(BOT_B), Some(Ok(_))));
    let mut outside = config(&state, &[], &[&a], &[&b]);
    outside.config_path = Some(base.0.join("config.toml"));
    let runtime = RunnerRuntime::new(&outside);
    assert!(runtime.agent_roots.values().all(Result::is_ok));
}
