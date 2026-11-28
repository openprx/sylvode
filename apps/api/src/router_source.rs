//! The router source files as text, for tests that check route registrations in the library crate.
//!
//! The binary crate has the same list as `router::ROUTER_SOURCES`; this reads the directory, so a
//! new area file is covered without an edit here.

/// Every file of `src/router` as `(file name, text)`, in file name order.
pub fn files() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/router");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("the router directory is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, std::fs::read_to_string(path).expect("a router file is readable"))
        })
        .collect()
}

/// The text of every file of `src/router`, in file name order.
pub fn text() -> String {
    files().into_iter().map(|(_, text)| text).collect::<Vec<_>>().join("\n")
}
