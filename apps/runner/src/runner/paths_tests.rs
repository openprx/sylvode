#![allow(clippy::indexing_slicing, clippy::type_complexity, clippy::needless_pass_by_value)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::paths::AllowedRoots;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        // Canonical, so comparisons do not depend on /tmp being a link (macOS).
        let dir = std::env::temp_dir().join(format!("sylvode-paths-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create dir");
        Self(dir.canonicalize().expect("canonical"))
    }

    fn path(&self) -> &Path {
        &self.0
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

/// A home directory that is not related to any test directory.
fn fake_home() -> PathBuf {
    let home = std::env::temp_dir().join("sylvode-paths-test-home");
    fs::create_dir_all(&home).expect("home");
    home
}

fn roots(entries: &[&Path], home: Option<&Path>) -> Result<AllowedRoots, String> {
    let entries: Vec<String> = entries.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let fallback = fake_home();
    AllowedRoots::from_entries(&entries, Some(home.unwrap_or(&fallback))).map(|report| report.roots)
}

fn code(roots: &AllowedRoots, raw: &str) -> &'static str {
    roots.check_directory(raw).expect_err("declined").code
}

#[test]
fn a_directory_inside_a_root_is_accepted_as_its_canonical_path() {
    let base = Dir::new();
    let root = base.sub("work");
    let project = root.join("svwl");
    fs::create_dir_all(&project).expect("mkdir");
    let allowed = roots(&[&root], None).expect("roots");
    assert_eq!(allowed.check_directory(&project.to_string_lossy()), Ok(project.clone()));
    // The root itself is inside the root.
    assert_eq!(allowed.check_directory(&root.to_string_lossy()), Ok(root.clone()));
    // A symlink to a directory inside the root is accepted as its target.
    let link = root.join("link");
    std::os::unix::fs::symlink(&project, &link).expect("symlink");
    assert_eq!(allowed.check_directory(&link.to_string_lossy()), Ok(project));
}

#[test]
fn containment_compares_components_not_bytes() {
    let base = Dir::new();
    let root = base.sub("work/svwl");
    let evil = base.sub("work/svwl-evil");
    let allowed = roots(&[&root], None).expect("roots");
    assert_eq!(code(&allowed, &evil.to_string_lossy()), "path_outside_allowlist");
    // The parent of a root is not inside it.
    assert_eq!(
        code(&allowed, &base.path().join("work").to_string_lossy()),
        "path_outside_allowlist"
    );
}

#[test]
fn syntactically_bad_paths_are_declined_without_touching_the_disk() {
    let base = Dir::new();
    let root = base.sub("work");
    let allowed = roots(&[&root], None).expect("roots");
    let r = root.to_string_lossy();
    for bad in [
        String::new(),
        "relative/path".to_string(),
        format!("{r}/../work"),
        format!("{r}/./x"),
        format!("{r}//x"),
        format!("{r}/"),
        format!("{r}/a\nb"),
        format!("{r}/a\u{1b}b"),
        format!("{r}/a\0b"),
        format!("{r}/#(touch x)"),
        format!("{r}/a#b"),
        format!("/{}", "a".repeat(2000)),
        "..".to_string(),
        ".".to_string(),
        "//".to_string(),
    ] {
        assert_eq!(code(&allowed, &bad), "path_outside_allowlist", "{bad:?}");
    }
}

#[test]
fn missing_files_unreadable_and_non_directories_have_their_own_codes() {
    let base = Dir::new();
    let root = base.sub("work");
    let allowed = roots(&[&root], None).expect("roots");
    assert_eq!(code(&allowed, &root.join("absent").to_string_lossy()), "path_missing");
    let file = root.join("file.txt");
    fs::write(&file, "x").expect("write");
    assert_eq!(code(&allowed, &file.to_string_lossy()), "path_not_directory");
    // A path through a file.
    assert_eq!(
        code(&allowed, &root.join("file.txt/sub").to_string_lossy()),
        "path_not_directory"
    );
    // A directory the runner user cannot enter (skipped when running as root).
    let locked = root.join("locked");
    fs::create_dir_all(locked.join("inner")).expect("mkdir");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");
    let result = allowed.check_directory(&locked.join("inner").to_string_lossy());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("chmod back");
    if fs::read_dir(&locked).is_ok() && result.is_ok() {
        return; // running with privileges that ignore permissions
    }
    assert_eq!(result.expect_err("declined").code, "path_unreadable");
}

#[test]
fn a_symlink_inside_a_root_that_points_outside_is_declined() {
    let base = Dir::new();
    let root = base.sub("work");
    let outside = base.sub("secret");
    std::os::unix::fs::symlink(&outside, root.join("escape")).expect("symlink");
    let allowed = roots(&[&root], None).expect("roots");
    assert_eq!(
        code(&allowed, &root.join("escape").to_string_lossy()),
        "path_outside_allowlist"
    );
    // And through a chain inside the root.
    std::os::unix::fs::symlink(root.join("escape"), root.join("chain")).expect("symlink");
    assert_eq!(
        code(&allowed, &root.join("chain").to_string_lossy()),
        "path_outside_allowlist"
    );
}

#[test]
fn a_symlink_to_a_directory_whose_real_path_has_a_hash_is_declined() {
    let base = Dir::new();
    let root = base.sub("work");
    let hashed = root.join("dir#1");
    fs::create_dir_all(&hashed).expect("mkdir");
    std::os::unix::fs::symlink(&hashed, root.join("plain")).expect("symlink");
    let allowed = roots(&[&root], None).expect("roots");
    assert_eq!(
        code(&allowed, &root.join("plain").to_string_lossy()),
        "path_outside_allowlist"
    );
}

#[test]
fn roots_are_validated_at_start_up() {
    let base = Dir::new();
    let ok = base.sub("ok");
    let home = base.sub("home");
    let under_home = base.sub("home/work");
    assert!(roots(&[&ok], Some(&home)).is_ok());
    assert!(roots(&[&under_home], Some(&home)).is_ok());
    let entries = |items: &[&str]| -> Result<AllowedRoots, String> {
        let entries: Vec<String> = items.iter().map(|s| (*s).to_string()).collect();
        AllowedRoots::from_entries(&entries, Some(&home)).map(|r| r.roots)
    };
    // `/`, the home directory itself and a directory that contains it.
    assert!(entries(&["/"]).is_err());
    assert!(entries(&[&home.to_string_lossy()]).is_err());
    assert!(entries(&[&base.path().to_string_lossy()]).is_err());
    // ~/ is expanded; other ~ forms and relative paths are refused.
    assert!(entries(&["~/work"]).is_ok());
    assert!(entries(&["~"]).is_err());
    assert!(entries(&["~other/work"]).is_err());
    assert!(entries(&["work"]).is_err());
    assert!(entries(&[""]).is_err());
    // Missing, a file, a hash in the path.
    assert!(entries(&[&base.path().join("absent").to_string_lossy()]).is_err());
    let file = base.path().join("f");
    fs::write(&file, "x").expect("write");
    assert!(entries(&[&file.to_string_lossy()]).is_err());
    let hashed = base.sub("with#hash");
    assert!(entries(&[&hashed.to_string_lossy()]).unwrap_err().contains('#'));
    // One bad entry refuses the lot, naming it.
    let error = entries(&[&ok.to_string_lossy(), "/"]).unwrap_err();
    assert!(error.contains("allowed_roots entry `/`"), "{error}");
    // `~/` without a home.
    assert!(AllowedRoots::from_entries(&["~/x".to_string()], None).is_err());
    // Without a known home directory no root is accepted at all.
    let error = AllowedRoots::from_entries(&[ok.to_string_lossy().into_owned()], None).expect_err("refused");
    assert!(error.contains("home directory"), "{error}");
}

#[test]
fn system_directories_are_never_roots() {
    let home = Dir::new();
    for system in [
        "/etc",
        "/usr",
        "/usr/local",
        "/bin",
        "/sbin",
        "/var",
        "/System",
        "/Library",
        "/private/etc",
        "/opt/homebrew",
        "/Users",
        "/home",
        "/tmp",
        "/private",
        "/dev",
        "/proc",
        "/sys",
        "/boot",
        "/lib",
        "/root",
        "/Volumes",
        "/Applications",
        "/run",
        "/srv",
        "/nix",
        "/snap",
        "/media",
        "/cores",
    ] {
        if !Path::new(system).is_dir() {
            continue; // not on this platform
        }
        let result = AllowedRoots::from_entries(&[system.to_string()], Some(home.path()));
        assert!(result.is_err(), "{system} was accepted");
    }
    // A directory below a parent-only entry is fine.
    let below = home.sub("below");
    assert!(AllowedRoots::from_entries(&[below.to_string_lossy().into_owned()], Some(&fake_home())).is_ok());
}

#[test]
fn a_world_writable_root_is_warned_about() {
    let base = Dir::new();
    let root = base.sub("open");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).expect("chmod");
    let report = AllowedRoots::from_entries(&[root.to_string_lossy().into_owned()], Some(&fake_home())).expect("roots");
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains("world-writable"));
    let closed = base.sub("closed");
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(
        AllowedRoots::from_entries(&[closed.to_string_lossy().into_owned()], Some(&fake_home()))
            .expect("roots")
            .warnings
            .is_empty()
    );
}

#[test]
fn duplicate_roots_are_kept_once() {
    let base = Dir::new();
    let root = base.sub("work");
    let link = base.path().join("alias");
    std::os::unix::fs::symlink(&root, &link).expect("symlink");
    let report = AllowedRoots::from_entries(
        &[root.to_string_lossy().into_owned(), link.to_string_lossy().into_owned()],
        Some(&fake_home()),
    )
    .expect("roots");
    let project = root.join("p");
    fs::create_dir_all(&project).expect("mkdir");
    assert!(report.roots.check_directory(&project.to_string_lossy()).is_ok());
}

/// What the answer says must not depend on what exists outside the roots.
#[test]
fn paths_outside_every_root_all_get_the_same_answer() {
    let base = Dir::new();
    let root = base.sub("work");
    let outside_dir = base.sub("elsewhere");
    let outside_file = base.0.join("elsewhere").join("secret.txt");
    fs::write(&outside_file, "x").expect("write");
    let allowed = roots(&[&root], None).expect("roots");
    for probe in [
        outside_dir.to_string_lossy().into_owned(),
        outside_file.to_string_lossy().into_owned(),
        base.0.join("missing").to_string_lossy().into_owned(),
        base.0
            .join("elsewhere/secret.txt/deeper")
            .to_string_lossy()
            .into_owned(),
        "/etc/passwd".to_string(),
        "/definitely/not/here".to_string(),
        "/proc/1/mem".to_string(),
    ] {
        assert_eq!(code(&allowed, &probe), "path_outside_allowlist", "{probe}");
    }
    // Inside a root the specific codes are reported.
    assert_eq!(code(&allowed, &root.join("absent").to_string_lossy()), "path_missing");
    fs::write(root.join("f"), "x").expect("write");
    assert_eq!(code(&allowed, &root.join("f").to_string_lossy()), "path_not_directory");
    // A link inside the root to a missing place outside does not reveal that it is missing.
    std::os::unix::fs::symlink(base.0.join("nowhere"), root.join("dangling")).expect("symlink");
    assert_eq!(
        code(&allowed, &root.join("dangling").to_string_lossy()),
        "path_outside_allowlist"
    );
    std::os::unix::fs::symlink(&outside_dir, root.join("to-outside")).expect("symlink");
    assert_eq!(
        code(&allowed, &root.join("to-outside/absent").to_string_lossy()),
        "path_outside_allowlist"
    );
    assert_eq!(
        code(&allowed, &root.join("to-outside/secret.txt").to_string_lossy()),
        "path_outside_allowlist"
    );
}

/// A root may be given in a form that differs from its canonical path (a link, `/tmp` on macOS).
#[test]
fn a_directory_may_name_the_root_as_written_or_canonical() {
    let base = Dir::new();
    let real = base.sub("real");
    let project = base.sub("real/p");
    let alias = base.0.join("alias");
    std::os::unix::fs::symlink(&real, &alias).expect("symlink");
    let allowed = roots(&[&alias], None).expect("roots");
    assert_eq!(
        allowed.check_directory(&alias.join("p").to_string_lossy()),
        Ok(project.clone())
    );
    assert_eq!(allowed.check_directory(&real.join("p").to_string_lossy()), Ok(project));
}

#[cfg(target_os = "linux")]
#[test]
fn a_canonical_path_that_is_not_utf8_is_refused() {
    use std::os::unix::ffi::OsStrExt;
    let base = Dir::new();
    let root = base.sub("work");
    let odd = root.join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
    fs::create_dir_all(&odd).expect("mkdir");
    std::os::unix::fs::symlink(&odd, root.join("plain")).expect("symlink");
    let allowed = roots(&[&root], None).expect("roots");
    assert_eq!(
        code(&allowed, &root.join("plain").to_string_lossy()),
        "path_outside_allowlist"
    );
}

/// macOS volumes ignore case: the system and home rules must too.
#[cfg(target_os = "macos")]
#[test]
fn case_variants_of_system_trees_and_home_are_refused() {
    let home = Dir::new();
    for variant in ["/USR/local", "/system/Library", "/LIBRARY", "/applications", "/VOLUMES"] {
        if Path::new(variant).exists() {
            assert!(
                AllowedRoots::from_entries(&[variant.to_string()], Some(home.path())).is_err(),
                "{variant}"
            );
        }
    }
    let upper = PathBuf::from(home.path().to_string_lossy().to_uppercase());
    if upper.exists() {
        assert!(AllowedRoots::from_entries(&[upper.to_string_lossy().into_owned()], Some(home.path())).is_err());
    }
}
