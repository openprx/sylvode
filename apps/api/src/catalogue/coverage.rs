//! The source-level scan of the router files: every `.route(path, ...)` registration, its methods
//! and the authentication layer on it.
//!
//! The router is assembled from method chains, so the scan reads the text of the router files (the
//! binary embeds them with `include_str!`). It is the input of the boot check in [`super::boot`].

use super::types::Method;

/// The authentication layer on a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthKind {
    /// `bot_or_user_auth_middleware`: bot tokens, delegated tokens and user sessions.
    BotOrUser,
    /// `jwt_auth_middleware`: user sessions.
    Jwt,
    /// `admin_middleware`.
    Admin,
    /// No authentication layer.
    None,
}

/// One method of one registered route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteFact {
    pub method: Method,
    pub path: String,
    pub auth: AuthKind,
}

/// The text of a source without its trailing `#[cfg(test)] mod tests` block.
fn without_tests(source: &str) -> &str {
    source
        .find("\nmod tests {")
        .and_then(|module| source.get(..module))
        .and_then(|head| head.rfind("#[cfg(test)]").and_then(|at| head.get(..at)))
        .unwrap_or(source)
}

/// The paths of every `.route(...)` registration of non-test code, with a path given as a constant
/// resolved from the `const NAME: &str = "..."` items of the same sources. A path that cannot be
/// resolved is returned as `unresolved:NAME`, so the caller's comparison fails on it.
#[must_use]
pub fn scan_paths(sources: &[(&str, &str)]) -> std::collections::BTreeSet<String> {
    const CALL: &str = ".route(";
    let sources: Vec<&str> = sources.iter().map(|(_, source)| without_tests(source)).collect();
    let mut constants = std::collections::BTreeMap::new();
    for line in sources.iter().flat_map(|source| source.lines()) {
        let Some(rest) = line
            .trim_start()
            .strip_prefix("pub const ")
            .or_else(|| line.trim_start().strip_prefix("const "))
        else {
            continue;
        };
        let Some((name, value)) = rest.split_once(": &str = \"") else {
            continue;
        };
        if let Some(value) = value.split('"').next() {
            constants.insert(name.trim(), value);
        }
    }
    let mut paths = std::collections::BTreeSet::new();
    for source in sources {
        let mut from = 0;
        while let Some(at) = source.get(from..).and_then(|rest| rest.find(CALL)) {
            let open = from + at + CALL.len();
            from = open;
            let Some(rest) = source.get(open..).map(str::trim_start) else {
                continue;
            };
            if let Some(literal) = rest.strip_prefix('"') {
                if let Some(path) = literal.find('"').and_then(|end| literal.get(..end)) {
                    paths.insert(path.to_string());
                }
                continue;
            }
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            match constants.get(name.as_str()) {
                Some(path) => paths.insert((*path).to_string()),
                None => paths.insert(format!("unresolved:{name}")),
            };
        }
    }
    paths
}

const METHOD_CALLS: [(&str, Method); 5] = [
    ("get(", Method::Get),
    ("post(", Method::Post),
    ("put(", Method::Put),
    ("patch(", Method::Patch),
    ("delete(", Method::Delete),
];

/// Every `(method, path)` the router files register, with the authentication kind of its route.
#[must_use]
pub fn scan(sources: &[(&str, &str)]) -> Vec<RouteFact> {
    let mut facts = Vec::new();
    for (_, source) in sources {
        scan_source(source, &mut facts);
    }
    facts
}

fn scan_source(source: &str, facts: &mut Vec<RouteFact>) {
    const CALL: &str = ".route(";
    let mut from = 0;
    while let Some(at) = source.get(from..).and_then(|rest| rest.find(CALL)) {
        let open = from + at + CALL.len();
        from = open;
        let Some(literal) = source
            .get(open..)
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('"'))
        else {
            continue;
        };
        let Some(path) = literal.find('"').and_then(|end| literal.get(..end)) else {
            continue;
        };
        let Some(body) = source.get(open..matching_paren(source, open)) else {
            continue;
        };
        let auth = auth_kind(body);
        for (needle, method) in METHOD_CALLS {
            if has_call(body, needle) {
                facts.push(RouteFact {
                    method,
                    path: path.to_string(),
                    auth,
                });
            }
        }
    }
}

/// The index just past the parenthesis that closes the call whose argument list starts at `open`.
fn matching_paren(source: &str, open: usize) -> usize {
    let mut depth = 1_i32;
    for (index, byte) in source.bytes().enumerate().skip(open) {
        match byte {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return index + 1;
        }
    }
    source.len()
}

fn auth_kind(body: &str) -> AuthKind {
    if body.contains("bot_or_user_auth_middleware") {
        AuthKind::BotOrUser
    } else if body.contains("jwt_auth_middleware") {
        AuthKind::Jwt
    } else if body.contains("admin_middleware") {
        AuthKind::Admin
    } else {
        AuthKind::None
    }
}

/// Whether `body` calls the method function `needle` (`get(`), not a longer name ending in it.
fn has_call(body: &str, needle: &str) -> bool {
    let mut search = 0;
    while let Some(position) = body.get(search..).and_then(|rest| rest.find(needle)) {
        let absolute = search + position;
        search = absolute + needle.len();
        let before = body.get(..absolute).and_then(|head| head.chars().next_back());
        if !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{AuthKind, scan};
    use crate::catalogue::types::Method;

    const SOURCE: &str = r#"
        Router::new()
            .route("/a/{id}", get(h::one).patch(h::two).route_layer(from_fn_with_state(s, middleware::bot_or_user_auth_middleware)))
            .route("/b", post(h::three).route_layer(from_fn(jwt_auth_middleware)))
            .route("/c", get(h::forget_it).delete(h::drop))
            .route("/d", put(h::fourth).route_layer(from_fn(middleware::admin::admin_middleware)))
            .route(SOME_CONST, get(h::five))
    "#;

    /// The scan reads the path, each method once, and the layer; a longer name ending in a method name is not a method.
    #[test]
    fn the_scan_reads_paths_methods_and_layers() {
        let facts = scan(&[("a.rs", SOURCE)]);
        let got: Vec<(Method, &str, AuthKind)> = facts
            .iter()
            .map(|fact| (fact.method, fact.path.as_str(), fact.auth))
            .collect();
        assert_eq!(
            got,
            vec![
                (Method::Get, "/a/{id}", AuthKind::BotOrUser),
                (Method::Patch, "/a/{id}", AuthKind::BotOrUser),
                (Method::Post, "/b", AuthKind::Jwt),
                (Method::Get, "/c", AuthKind::None),
                (Method::Delete, "/c", AuthKind::None),
                (Method::Put, "/d", AuthKind::Admin),
            ]
        );
    }
}
