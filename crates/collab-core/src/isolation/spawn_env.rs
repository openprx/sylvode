//! The environment of the isolated-apply worker.
//!
//! Compiled on every platform so that its test runs everywhere; the worker itself runs only on
//! Linux (`host`).

use std::process::Command;

use super::wire;

/// The environment variables the worker inherits from the API process: none. The worker reads two
/// variables of its own (`wire::OPERATION_ENV`, always set below, and `wire::WINDOW_ENV`, set below
/// only when the host asks for the full window) and, in a debug build, the test-injection
/// variable, which only a test sets through `extra_env`. It needs nothing else from its parent:
/// it is spawned by absolute path, links no library through the environment and reads no
/// configuration. Everything the API process has in its environment, including a stray
/// `COLLAB_ISOLATION_WINDOW` or `COLLAB_ISOLATION_TEST_INJECT`, stays out of it.
const INHERITED_ENV: &[&str] = &[];

/// The command that starts the worker: a cleared environment, the allow-list of
/// [`INHERITED_ENV`] copied from this process, then the variables the host sets.
pub(super) fn worker_command(worker_path: &std::path::Path, operation: &str, extra_env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(worker_path);
    command.env_clear();
    for name in INHERITED_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env(wire::OPERATION_ENV, operation);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use parking_lot::Mutex;

    use super::*;

    const TEST_INJECT_ENV_VAR: &str = "COLLAB_ISOLATION_TEST_INJECT";

    /// Serializes the tests of this module that set process environment variables.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// What a child started by [`worker_command`] sees of the environment: the command is run with
    /// `env` as the program (it prints the child's environment and exits), so the test sees the
    /// variables that really reach a child, on every platform, without the worker binary.
    fn environment_seen_by_a_child(extra_env: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        let output = worker_command(std::path::Path::new("/usr/bin/env"), wire::OPERATION_APPLY, extra_env)
            .stdin(Stdio::null())
            .output()
            .expect("env runs");
        String::from_utf8(output.stdout)
            .expect("env prints text")
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect()
    }

    #[test]
    fn the_worker_does_not_inherit_the_api_processs_environment() {
        let guard = env_lock().lock();
        // SAFETY: held under `env_lock()`; the variables set here are read by no other test of this
        // module, and are removed before the lock is released.
        unsafe {
            std::env::set_var(wire::WINDOW_ENV, "inherited-window");
            std::env::set_var(TEST_INJECT_ENV_VAR, "cpu_burn_micros=1");
            std::env::set_var("SYLVODE_SECRET_PROBE", "must-not-reach-the-worker");
        }
        let seen = environment_seen_by_a_child(&[]);
        let seen_full = environment_seen_by_a_child(&[(wire::WINDOW_ENV, wire::WINDOW_FULL)]);
        // SAFETY: as above.
        unsafe {
            std::env::remove_var(wire::WINDOW_ENV);
            std::env::remove_var(TEST_INJECT_ENV_VAR);
            std::env::remove_var("SYLVODE_SECRET_PROBE");
        }
        drop(guard);
        assert_eq!(
            seen.get(wire::OPERATION_ENV).map(String::as_str),
            Some(wire::OPERATION_APPLY)
        );
        assert_eq!(
            seen.get(wire::WINDOW_ENV),
            None,
            "an inherited window variable reaches the worker"
        );
        assert_eq!(
            seen.get(TEST_INJECT_ENV_VAR),
            None,
            "an inherited injection variable reaches the worker"
        );
        assert_eq!(seen.get("SYLVODE_SECRET_PROBE"), None);
        assert_eq!(
            seen.len(),
            1,
            "the worker's environment is exactly the operation: {seen:?}"
        );
        // The variable the host sets itself still reaches it.
        assert_eq!(
            seen_full.get(wire::WINDOW_ENV).map(String::as_str),
            Some(wire::WINDOW_FULL)
        );
        assert_eq!(seen_full.len(), 2, "{seen_full:?}");
    }
}
