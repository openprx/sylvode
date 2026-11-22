#![allow(clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::registry::{Collision, SessionRegistry};

const P1: &str = "11111111-1111-4111-8111-111111111111";
const P2: &str = "22222222-2222-4222-8222-222222222222";
const P3: &str = "33333333-3333-4333-8333-333333333333";

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-registry-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("sessions").join("registry.json")
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_renamed_projects_old_key_stays_reserved_until_it_is_released() {
    let dir = Dir::new();
    let registry = SessionRegistry::open(Some(dir.path())).expect("open");
    assert_eq!(registry.bind(P1, "FOO"), Ok(()));
    // The project is renamed: it also owns BAR, and FOO is not free for anyone else.
    assert_eq!(registry.bind(P1, "BAR"), Ok(()));
    assert_eq!(registry.check(P2, "FOO"), Err(Collision));
    assert_eq!(registry.bind(P2, "FOO"), Err(Collision));
    assert_eq!(registry.bind(P1, "FOO"), Ok(()), "the owner keeps using it");
    assert_eq!(registry.owner("FOO").as_deref(), Some(P1));
    // Only the explicit release (the sessions command) frees it.
    assert!(registry.release("FOO"));
    assert!(!registry.release("FOO"));
    assert_eq!(registry.bind(P2, "FOO"), Ok(()));
    assert_eq!(registry.check(P1, "FOO"), Err(Collision));
    // The state survives a restart.
    let again = SessionRegistry::open(Some(dir.path())).expect("reopen");
    assert_eq!(
        again.entries(),
        vec![("BAR".to_string(), P1.to_string()), ("FOO".to_string(), P2.to_string())]
    );
}

#[test]
fn checking_never_changes_anything() {
    let dir = Dir::new();
    let registry = SessionRegistry::open(Some(dir.path())).expect("open");
    assert_eq!(registry.check(P1, "FOO"), Ok(()));
    assert!(registry.entries().is_empty());
    assert!(!dir.file().exists(), "a check writes nothing");
}

#[test]
fn the_file_is_private_synced_and_versioned() {
    let dir = Dir::new();
    let registry = SessionRegistry::open(Some(dir.path())).expect("open");
    registry.bind(P1, "FOO").expect("bind");
    assert_eq!(
        fs::metadata(dir.file()).expect("meta").permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(dir.file().parent().expect("dir"))
            .expect("meta")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(dir.file()).expect("read")).expect("json");
    assert_eq!(value, serde_json::json!({"version": 1, "keys": {"FOO": P1}}));
    // No temporary file is left.
    assert_eq!(fs::read_dir(dir.file().parent().expect("dir")).expect("dir").count(), 1);
}

#[test]
fn a_corrupt_registry_is_an_error_and_is_left_alone() {
    for content in [
        "",
        "not json",
        "[]",
        "{\"version\":1}",
        "{\"version\":2,\"keys\":{}}",
        "{\"version\":1,\"keys\":{\"FOO\":5}}",
    ] {
        let dir = Dir::new();
        fs::create_dir_all(dir.file().parent().expect("dir")).expect("mkdir");
        fs::write(dir.file(), content).expect("write");
        let error = SessionRegistry::open(Some(dir.path())).expect_err(content);
        assert!(error.contains("registry"), "{error}");
        assert_eq!(
            fs::read_to_string(dir.file()).expect("read"),
            content,
            "the bad file is kept as it is"
        );
    }
}

#[test]
fn invalid_entries_are_dropped_and_the_rest_kept() {
    let dir = Dir::new();
    fs::create_dir_all(dir.file().parent().expect("dir")).expect("mkdir");
    let content =
        format!("{{\"version\":1,\"keys\":{{\"FOO\":\"{P1}\",\"bad key\":\"{P2}\",\"BAR\":\"not-a-uuid\"}}}}");
    fs::write(dir.file(), content).expect("write");
    let registry = SessionRegistry::open(Some(dir.path())).expect("open");
    assert_eq!(registry.entries(), vec![("FOO".to_string(), P1.to_string())]);
}

#[test]
fn concurrent_binds_leave_every_key_on_disk() {
    let dir = Dir::new();
    let registry = Arc::new(SessionRegistry::open(Some(dir.path())).expect("open"));
    let handles: Vec<_> = (0..32)
        .map(|n| {
            let registry = Arc::clone(&registry);
            std::thread::spawn(move || {
                let project = if n % 2 == 0 { P1 } else { P3 };
                registry.bind(project, &format!("K{n}")).expect("bind");
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("thread");
    }
    assert_eq!(registry.entries().len(), 32);
    let again = SessionRegistry::open(Some(dir.path())).expect("reopen");
    assert_eq!(again.entries().len(), 32, "the newest snapshot is what is on disk");
}

#[test]
fn without_a_state_directory_the_registry_lives_in_memory() {
    let registry = SessionRegistry::open(None).expect("open");
    assert_eq!(registry.bind(P1, "FOO"), Ok(()));
    assert_eq!(registry.bind(P2, "FOO"), Err(Collision));
}
