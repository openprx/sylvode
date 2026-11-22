//! The legacy callback endpoints are gone: nothing under `apps/` or `skills/` names them.

use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "target" || name == "node_modules" || name == ".git" {
            continue;
        }
        if path.is_dir() {
            files(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| matches!(e.to_str(), Some("rs" | "md" | "toml" | "ts" | "py" | "sh")))
        {
            out.push(path);
        }
    }
}

#[test]
fn nothing_names_the_legacy_callback_path() {
    // Built in two parts so this file does not name it itself.
    let needle = format!("/ai/callbacks/{}", "task");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut all = Vec::new();
    for dir in ["apps", "skills"] {
        files(&root.join(dir), &mut all);
    }
    assert!(!all.is_empty(), "the walk found no files");
    let hits: Vec<String> = all
        .iter()
        .filter(|path| std::fs::read_to_string(path).is_ok_and(|text| text.contains(&needle)))
        .map(|path| path.display().to_string())
        .collect();
    assert!(hits.is_empty(), "legacy callback path still named in: {hits:?}");
}
