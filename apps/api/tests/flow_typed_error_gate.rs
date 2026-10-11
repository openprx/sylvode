//! Gate: every error the Flow surface constructs is a typed envelope.
//!
//! `error-mapping-v1.md` requires clients to branch on the stable `error_code` (plus its frozen
//! `details` discriminators), never on the English `message` or on the bare numeric `code`. The
//! legacy string-typed `ApiError` variants (`BadRequest(String)`, `Forbidden(String)`,
//! `NotFound(String)`, `Conflict(String)`, `Unauthorized(String)`, `Internal`, `Database(..)`)
//! serialize without an `error_code`, so every one of them that is reachable from a Flow route is
//! a response a contract-conforming client cannot classify.
//!
//! This test walks the Flow surface source (`src/flow/**`, `src/routes/flow.rs`,
//! `src/routes/collab.rs`), blanks comments, string/char literals and `#[cfg(test)]` items, and
//! lists every remaining mention of a legacy variant -- constructions *and* pattern matches, since
//! a `match err { ApiError::NotFound(_) => .. }` silently stops matching once the producer is
//! typed. A new non-typed construction point anywhere in that surface turns this test red.
//!
//! A source scan of construction points cannot see an implicit `From<sea_orm::DbErr>` conversion
//! through `?`. Those are covered at the response boundary instead: a database failure rendered
//! while a request is served by the Flow surface becomes `server_rejected` (deterministic) or
//! `server_draining{reason:"contention"}` (anything else), and that rendering is switched on in
//! exactly one place, `routes::flow_surface::seal`. The second test here proves the wiring from
//! source: every route that names a `routes::flow` / `routes::collab` handler, or whose path has a
//! `flow` / `collab` segment, is registered inside `fn flow_surface_routes` in `src/main.rs`, and
//! that function's router reaches the application only through `seal`. A Flow route registered
//! anywhere else turns it red. So does a route that is Flow by its path and names a handler of
//! another module (or the reverse), and so does a handler that is not written out as
//! `routes::<module>::<handler>`, because an alias would hide a Flow handler from every check
//! here. The binary's own test module drives every registered route through the real router and
//! checks the same thing on the responses.
//!
//! Not claimed by this gate: errors produced by shared helpers outside the scanned tree. The
//! authentication middleware is one: its user-session path propagates a database failure as
//! `Database`, and its bot-credential path reports one through
//! `ApiError::internal_database_failure`; both are rendered typed at the same boundary (the
//! binary's tests drive a bot credential through it). Its other refusals (a missing or invalid
//! credential) keep the legacy envelope.
#![allow(clippy::print_stderr, clippy::panic, clippy::indexing_slicing, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const LEGACY_VARIANTS: &[&str] = &[
    "BadRequest",
    "Unauthorized",
    "Forbidden",
    "NotFound",
    "Conflict",
    "Internal",
    "Database",
];

/// Replaces comments and the contents of string, byte-string, raw-string and char literals with
/// spaces, keeping every newline so line numbers survive.
fn blank_comments_and_literals(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        // Line comment.
        if c == '/' && next == Some('/') {
            let end = chars[i..]
                .iter()
                .position(|ch| *ch == '\n')
                .map_or(chars.len(), |p| i + p);
            out.extend(std::iter::repeat_n(' ', end - i));
            i = end;
            continue;
        }
        // Block comment (nested).
        if c == '/' && next == Some('*') {
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.extend(std::iter::repeat_n(' ', 2));
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.extend(std::iter::repeat_n(' ', 2));
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[i]));
                    i += 1;
                }
            }
            continue;
        }
        let prev_is_ident = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
        // Raw string: r"..", r#".."#, br#".."#.
        if !prev_is_ident && (c == 'r' || (c == 'b' && next == Some('r'))) {
            let start = if c == 'b' { i + 2 } else { i + 1 };
            let mut j = start;
            while chars.get(j) == Some(&'#') {
                j += 1;
            }
            if chars.get(j) == Some(&'"') {
                let hashes = j - start;
                out.extend(std::iter::repeat_n(' ', j + 1 - i));
                i = blank_raw_string_body(&chars, j + 1, hashes, &mut out);
                continue;
            }
        }
        // String / byte string.
        if c == '"' || (c == 'b' && next == Some('"') && !prev_is_ident) {
            if c == 'b' {
                out.push(' ');
                i += 1;
            }
            out.push(' ');
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    out.push(' ');
                    if let Some(&escaped) = chars.get(i + 1) {
                        out.push(blank(escaped));
                    }
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                }
                out.push(blank(chars[i]));
                i += 1;
            }
            continue;
        }
        // Char literal (but not a lifetime).
        if c == '\'' {
            if next == Some('\\') {
                let mut j = i + 2;
                while j < chars.len() && chars[j] != '\'' {
                    j += 1;
                }
                out.extend(std::iter::repeat_n(' ', j.min(chars.len() - 1) + 1 - i));
                i = j + 1;
                continue;
            }
            if chars.get(i + 2) == Some(&'\'') {
                out.extend(std::iter::repeat_n(' ', 3));
                i += 3;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out.into_iter().collect()
}

/// Blanks a raw string's body starting at `i` (just past the opening quote) through its closing
/// quote and `hashes` hash marks; returns the index just past the literal.
fn blank_raw_string_body(chars: &[char], mut i: usize, hashes: usize, out: &mut Vec<char>) -> usize {
    while i < chars.len() {
        if chars[i] == '"' && (0..hashes).all(|h| chars.get(i + 1 + h) == Some(&'#')) {
            out.extend(std::iter::repeat_n(' ', hashes + 1));
            return i + 1 + hashes;
        }
        out.push(if chars[i] == '\n' { '\n' } else { ' ' });
        i += 1;
    }
    i
}

/// Index of the `}` closing the `{` at `open`, or the last index when unbalanced.
fn matching_brace(chars: &[char], open: usize) -> usize {
    let mut depth = 0i32;
    for (k, ch) in chars.iter().enumerate().skip(open) {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return k;
                }
            }
            _ => {}
        }
    }
    chars.len() - 1
}

/// Blanks every item annotated `#[cfg(test)]` (a test module, a test-only function, a test-only
/// statement). Must run on text whose comments and literals are already blanked, so braces are
/// structural.
fn blank_cfg_test_items(code: &str) -> String {
    const MARKER: &str = "#[cfg(test)]";
    let mut chars: Vec<char> = code.chars().collect();
    let marker: Vec<char> = MARKER.chars().collect();
    let mut i = 0;
    while i + marker.len() <= chars.len() {
        if chars[i..i + marker.len()] != marker[..] {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + marker.len();
        let mut paren = 0i32;
        let mut end = None;
        while j < chars.len() {
            match chars[j] {
                '(' | '[' => paren += 1,
                ')' | ']' => paren -= 1,
                ';' if paren == 0 => {
                    end = Some(j);
                    break;
                }
                '{' if paren == 0 => {
                    end = Some(matching_brace(&chars, j));
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        let end = end.unwrap_or(chars.len() - 1);
        for ch in &mut chars[start..=end] {
            if *ch != '\n' {
                *ch = ' ';
            }
        }
        i = end + 1;
    }
    chars.into_iter().collect()
}

/// Every `ApiError::<legacy variant>` mention in production code, as `(line, variant)`.
fn legacy_mentions(src: &str) -> Vec<(usize, &'static str)> {
    let code = blank_cfg_test_items(&blank_comments_and_literals(src));
    let mut found = Vec::new();
    for (line_index, line) in code.lines().enumerate() {
        let mut rest = line;
        while let Some(pos) = rest.find("ApiError::") {
            let before_ok = rest[..pos]
                .chars()
                .next_back()
                .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'));
            let after = &rest[pos + "ApiError::".len()..];
            let ident: String = after
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                .collect();
            if before_ok && let Some(variant) = LEGACY_VARIANTS.iter().find(|v| **v == ident) {
                found.push((line_index + 1, *variant));
            }
            rest = &after[ident.len()..];
        }
    }
    found
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|err| panic!("cannot read {}: {err}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn flow_surface_files() -> Vec<PathBuf> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs(&src.join("flow"), &mut files);
    files.push(src.join("routes").join("flow.rs"));
    files.push(src.join("routes").join("collab.rs"));
    files.push(src.join("routes").join("flow_surface.rs"));
    files.sort();
    files
}

#[test]
fn the_scanner_sees_legacy_mentions_and_ignores_comments_literals_and_test_items() {
    let sample = r##"
fn prod() -> Result<(), ApiError> {
    // ApiError::NotFound("in a comment")
    let _s = "ApiError::Forbidden(in a string)";
    let _r = r#"ApiError::Conflict"#;
    let _c = '"';
    if x { return Err(ApiError::BadRequest("real".into())); }
    match e { ApiError::NotFound(_) => {}, _ => {} }
    Err(ApiError::Internal)
}
#[cfg(test)]
mod tests {
    fn t() { let _ = ApiError::Conflict("test only".into()); }
}
#[cfg(test)]
inject().await?;
fn after() { let _ = ApiError::Typed { kind, message, details: None }; let _ = MyApiError::Internal; }
fn last() { let _ = ApiError::Database(err); }
"##;
    let found = legacy_mentions(sample);
    let variants: Vec<&str> = found.iter().map(|(_, v)| *v).collect();
    assert_eq!(
        variants,
        vec!["BadRequest", "NotFound", "Internal", "Database"],
        "{found:?}"
    );
    assert_eq!(found[0].0, 7);
    assert_eq!(found[3].0, 18);
}

#[test]
fn the_flow_surface_constructs_no_legacy_untyped_api_error() {
    let files = flow_surface_files();
    assert!(
        files.len() >= 30,
        "scan saw only {} files; the walk is broken",
        files.len()
    );

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut report = String::new();
    let mut total = 0usize;
    let mut typed_mentions = 0usize;
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
        typed_mentions += src.matches("ApiError::Typed").count() + src.matches("ApiError::typed(").count();
        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
        for (line, variant) in legacy_mentions(&src) {
            total += 1;
            let _ = writeln!(report, "  {rel}:{line}  ApiError::{variant}");
        }
    }
    // A scan that blanks everything would trivially pass; the surface must visibly use typed errors.
    assert!(
        typed_mentions > 0,
        "no typed ApiError found in the Flow surface; the scan is not reading source"
    );
    eprintln!(
        "flow typed-error gate: {total} non-typed ApiError mention(s) in {} files",
        files.len()
    );
    assert!(
        total == 0,
        "{total} non-typed ApiError mention(s) on the Flow surface (use ApiError::typed / the typed constructors):\n{report}"
    );
}

/// Production code of `src`: comments, literals and `#[cfg(test)]` items blanked, one `char` per
/// `char` of the original so offsets into it address the original too.
fn production_code(src: &str) -> Vec<char> {
    blank_cfg_test_items(&blank_comments_and_literals(src))
        .chars()
        .collect()
}

/// Char offsets of every occurrence of `needle` in `code`.
fn occurrences(code: &[char], needle: &str) -> Vec<usize> {
    let needle: Vec<char> = needle.chars().collect();
    (0..code.len().saturating_sub(needle.len() - 1))
        .filter(|&i| code[i..i + needle.len()] == needle[..])
        .collect()
}

/// Index of the `)` closing the `(` at `open`, or the last index when unbalanced.
fn matching_paren(code: &[char], open: usize) -> usize {
    let mut depth = 0i32;
    for (k, ch) in code.iter().enumerate().skip(open) {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return k;
                }
            }
            _ => {}
        }
    }
    code.len() - 1
}

struct RouteCall {
    /// Char offset of the `.route(` call.
    at: usize,
    path: String,
    /// Whether the call names a handler of `routes::flow` or `routes::collab`.
    flow_handler: bool,
    /// The handler each method router of the call is given, as written (`routes::label::list`).
    handlers: Vec<String>,
}

/// The constructors and methods of `axum::routing::MethodRouter` that take a handler.
const METHOD_ROUTERS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace", "connect", "any", "fallback",
];

/// The `routes` module of a handler written as `routes::<module>::<handler>`, else `None`.
fn handler_module(handler: &str) -> Option<&str> {
    let mut parts = handler.split("::");
    let is_ident = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("routes"), Some(module), Some(name), None) if is_ident(module) && is_ident(name) => Some(module),
        _ => None,
    }
}

/// The handlers that are not written as `routes::<module>::<handler>`, as `(path, handler)`:
/// the liveness and readiness probes, which are functions of `main.rs` itself and so cannot be
/// handlers of `routes::flow` / `routes::collab`. Nothing else may be added without a reason of
/// that kind.
const LOCAL_HANDLERS: &[(&str, &str)] = &[("/health", "health"), ("/ready", "ready")];

fn is_flow_module(module: &str) -> bool {
    module == "flow" || module == "collab"
}

impl RouteCall {
    /// Whether the path has a `flow` / `collab` segment. Every path of the Flow surface has one
    /// and no other path does, so this needs no list of exceptions.
    fn flow_path(&self) -> bool {
        self.path
            .split('/')
            .any(|segment| segment == "flow" || segment == "collab")
    }

    /// A route of the Flow surface: a Flow handler, or a path with a `flow` / `collab` segment.
    fn is_flow(&self) -> bool {
        self.flow_handler || self.flow_path()
    }
}

/// The first argument of every method-router call (`get(..)`, `.post(..)`, ...) in `body`, the
/// production code of one `.route(..)` call.
fn method_router_handlers(body: &[char]) -> Vec<String> {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut handlers = Vec::new();
    for name in METHOD_ROUTERS {
        for at in occurrences(body, &format!("{name}(")) {
            // `get(` itself or `.get(`, not the tail of another identifier (`budget(`).
            if at > 0 && is_ident(body[at - 1]) {
                continue;
            }
            let open = at + name.len();
            let close = matching_paren(body, open);
            let mut depth = 0i32;
            let mut end = close;
            for (k, ch) in body.iter().enumerate().take(close).skip(open + 1) {
                match ch {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth -= 1,
                    ',' if depth == 0 => {
                        end = k;
                        break;
                    }
                    _ => {}
                }
            }
            let handler: String = body[open + 1..end].iter().filter(|c| !c.is_whitespace()).collect();
            handlers.push(handler);
        }
    }
    handlers
}

/// Every `.route("<path>", ..)` call in the production code of `raw`.
fn route_calls(raw: &str) -> Vec<RouteCall> {
    let original: Vec<char> = raw.chars().collect();
    let code = production_code(raw);
    occurrences(&code, ".route(")
        .into_iter()
        .map(|at| {
            let open = at + ".route".len();
            let close = matching_paren(&code, open);
            let call: String = original[open..=close].iter().collect();
            let body: String = code[open..=close].iter().collect();
            RouteCall {
                at,
                path: call.split('"').nth(1).unwrap_or_default().to_string(),
                flow_handler: body.contains("routes::flow::") || body.contains("routes::collab::"),
                handlers: method_router_handlers(&code[open..=close]),
            }
        })
        .collect()
}

/// Char span of the body of `fn <name>` in production code.
fn fn_body_span(code: &[char], name: &str) -> Option<(usize, usize)> {
    let start = *occurrences(code, &format!("fn {name}(")).first()?;
    let open = (start..code.len()).find(|&i| code[i] == '{')?;
    Some((open, matching_brace(code, open)))
}

fn main_rs() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("main.rs");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

/// What is wrong with the Flow route wiring of a `main.rs` source, one line per finding.
fn flow_wiring_findings(main: &str) -> Vec<String> {
    let code = production_code(main);
    let text: String = code.iter().collect();
    let mut findings = Vec::new();
    let Some((open, close)) = fn_body_span(&code, "flow_surface_routes") else {
        return vec!["main.rs has no `fn flow_surface_routes`".to_string()];
    };
    let routes = route_calls(main);
    let flow_routes = routes.iter().filter(|route| route.is_flow()).count();
    if flow_routes < 40 {
        findings.push(format!(
            "only {flow_routes} Flow routes found; the route parser no longer matches main.rs"
        ));
    }
    for route in &routes {
        let inside = route.at > open && route.at < close;
        if route.is_flow() && !inside {
            findings.push(format!(
                "Flow route {} is registered outside `fn flow_surface_routes`",
                route.path
            ));
        }
        if inside && !route.flow_handler {
            findings.push(format!(
                "{} inside `fn flow_surface_routes` names no routes::flow / routes::collab handler",
                route.path
            ));
        }
        // Handler by handler: written out in full, and Flow exactly when the path is.
        if route.handlers.is_empty() {
            findings.push(format!("{} has no method router with a handler", route.path));
        }
        for handler in &route.handlers {
            let Some(module) = handler_module(handler) else {
                if LOCAL_HANDLERS.contains(&(route.path.as_str(), handler.as_str())) && !route.flow_path() {
                    continue;
                }
                findings.push(format!(
                    "{} is served by `{handler}`, which is not written as `routes::<module>::<handler>`",
                    route.path
                ));
                continue;
            };
            if route.flow_path() && !is_flow_module(module) {
                findings.push(format!(
                    "Flow path {} is served by `{handler}`, which is not a routes::flow / routes::collab handler",
                    route.path
                ));
            }
            if !route.flow_path() && is_flow_module(module) {
                findings.push(format!(
                    "`{handler}` is served on {}, a path without a `flow` / `collab` segment",
                    route.path
                ));
            }
        }
    }
    // The handler modules are reached by their full path only: an import or an alias of
    // `routes`, `routes::flow` or `routes::collab` would let a handler be named in a way the
    // checks above cannot attribute.
    for module in ["routes::flow", "routes::collab"] {
        for at in occurrences(&code, module) {
            let next = code.get(at + module.len()).copied();
            let longer_name = next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if next != Some(':') && !longer_name {
                let rest: String = code[at..code.len().min(at + 40)].iter().collect();
                findings.push(format!(
                    "`{}`: {module} is imported or aliased instead of being named in full at each handler",
                    rest.trim()
                ));
            }
        }
    }
    for needle in ["routes::{", "routes::*", "routes as "] {
        if !occurrences(&code, needle).is_empty() {
            findings.push(format!(
                "`{needle}`: the routes modules are imported or aliased instead of being named in full"
            ));
        }
    }
    // The handlers themselves are mounted nowhere else (`routes::collab::trace_span` is the
    // tracing span maker, not a handler).
    for module in ["routes::flow::", "routes::collab::"] {
        for at in occurrences(&code, module) {
            let rest: String = code[at..code.len().min(at + 40)].iter().collect();
            let inside = at > open && at < close;
            if !inside && !rest.starts_with("routes::collab::trace_span") {
                findings.push(format!("`{}` is used outside `fn flow_surface_routes`", rest.trim()));
            }
        }
    }
    // One definition and one use, and that use is wrapped by the boundary.
    let uses = text.matches("flow_surface_routes(").count();
    let sealed = text.matches("routes::flow_surface::seal(flow_surface_routes(").count();
    let seals = text.matches("flow_surface::seal(").count();
    if seals != 1 {
        findings.push(format!(
            "`flow_surface::seal(` is called {seals} time(s); only the Flow router is sealed, once"
        ));
    }
    if uses != 2 || sealed != 1 {
        findings.push(format!(
            "`flow_surface_routes(` appears {uses} time(s) and is sealed {sealed} time(s); expected its \
             definition plus exactly one `routes::flow_surface::seal(flow_surface_routes(..))`"
        ));
    }
    findings
}

#[test]
fn the_wiring_check_sees_a_flow_route_registered_outside_the_boundary() {
    let template = |extra_app_route: &str, extra_flow_route: &str| {
        let mut flow_routes = String::new();
        for n in 0..40 {
            let _ = writeln!(
                flow_routes,
                "        .route(\"/api/v1/flow/things/{n}\", get(routes::flow::thing))"
            );
        }
        format!(
            "fn build_router(state: AppState) -> Router {{\n    Router::new()\n        \
             .route(\"/api/v1/workflows\", get(routes::workflow::list))\n{extra_app_route}        \
             .merge(routes::flow_surface::seal(flow_surface_routes(&state)))\n        \
             .layer(TraceLayer::new_for_http().make_span_with(routes::collab::trace_span))\n        \
             .with_state(state)\n}}\n\
             fn flow_surface_routes(auth_state: &AppState) -> Router<AppState> {{\n    Router::new()\n\
             {flow_routes}{extra_flow_route}}}\n\
             #[cfg(test)]\nmod tests {{ fn t() {{ Router::new().route(\"/api/v1/flow/x\", get(routes::flow::x)); }} }}\n"
        )
    };
    assert_eq!(flow_wiring_findings(&template("", "")), Vec::<String>::new());
    // By handler, by path alone, a non-Flow route put behind the boundary, a handler of another
    // module chained onto a Flow path, an aliased Flow handler on a path that does not look like
    // a Flow path, and a Flow handler on such a path behind the boundary.
    for (app_route, flow_route, expected) in [
        (
            "",
            "        .route(\"/api/v1/flow/mixed\", get(routes::flow::thing).post(routes::label::create))\n",
            "is served by `routes::label::create`, which is not a routes::flow",
        ),
        (
            "        .route(\"/api/v1/aliased\", get(flow_alias::thing))\n",
            "",
            "is served by `flow_alias::thing`, which is not written as",
        ),
        (
            "        .route(\"/api/v1/nested\", get(routes::flow::nested::thing))\n",
            "",
            "which is not written as",
        ),
        (
            "",
            "        .route(\"/api/v1/plain\", get(routes::flow::thing))\n",
            "a path without a `flow` / `collab` segment",
        ),
        (
            "        .route(\"/api/v1/other\", get(routes::flow::thing))\n",
            "",
            "outside",
        ),
        (
            "        .route(\"/api/v1/flow/other\", get(routes::elsewhere::thing))\n",
            "",
            "outside",
        ),
        (
            "        .route(\"/api/v1/collab/other\", get(routes::elsewhere::thing))\n",
            "",
            "outside",
        ),
        (
            "",
            "        .route(\"/api/v1/workflows/x\", get(routes::workflow::get))\n",
            "names no routes::flow",
        ),
    ] {
        let findings = flow_wiring_findings(&template(app_route, flow_route));
        assert!(
            findings.iter().any(|finding| finding.contains(expected)),
            "{app_route}{flow_route}: {findings:?}"
        );
    }
    for (import, expected) in [
        (
            "use api::routes::flow as flow_alias;\n",
            "routes::flow is imported or aliased",
        ),
        ("use api::routes::collab;\n", "routes::collab is imported or aliased"),
        ("use api::routes::{flow, label};\n", "`routes::{`"),
        ("use api::routes as r;\n", "`routes as `"),
    ] {
        let findings = flow_wiring_findings(&format!("{import}{}", template("", "")));
        assert!(
            findings.iter().any(|finding| finding.contains(expected)),
            "{import}: {findings:?}"
        );
    }
    let sealed_twice = template("", "").replace(
        "Router::new()\n        .route(\"/api/v1/workflows\"",
        "routes::flow_surface::seal(Router::new())\n        .route(\"/api/v1/workflows\"",
    );
    assert!(
        flow_wiring_findings(&sealed_twice)
            .iter()
            .any(|finding| finding.contains("called 2 time(s)")),
        "a second sealed router is reported"
    );
    let unsealed = template("", "").replace(
        "routes::flow_surface::seal(flow_surface_routes(&state))",
        "flow_surface_routes(&state)",
    );
    assert!(
        flow_wiring_findings(&unsealed)
            .iter()
            .any(|finding| finding.contains("sealed 0 time(s)")),
        "an unsealed merge is reported"
    );
}

#[test]
fn every_flow_route_is_registered_behind_the_typed_database_failure_boundary() {
    let findings = flow_wiring_findings(&main_rs());
    assert!(findings.is_empty(), "Flow route wiring:\n  {}", findings.join("\n  "));

    // The boundary is switched on in one place and applied from one place.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs(&src, &mut files);
    files.sort();
    let mentions = |needle: &str| -> Vec<String> {
        files
            .iter()
            .filter(|path| {
                let raw =
                    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
                !occurrences(&production_code(&raw), needle).is_empty()
            })
            .map(|path| path.strip_prefix(&src).unwrap_or(path).display().to_string())
            .collect()
    };
    assert_eq!(
        mentions("with_typed_database_failures"),
        vec!["error.rs", "routes/flow_surface.rs"],
        "typed database failures are switched on only by the Flow surface boundary"
    );
    assert_eq!(
        mentions("flow_surface::seal("),
        vec!["main.rs"],
        "only the application router seals a Flow router"
    );
    assert_eq!(
        mentions(".route("),
        vec!["main.rs"],
        "routes are registered in main.rs only, where the wiring check above reads them"
    );
}
