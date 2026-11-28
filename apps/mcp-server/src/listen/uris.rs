//! The URIs a listen request may subscribe to, and the keys a change hint is matched on.
//!
//! Three shapes are subscribable: `sylvode://tasks/{task_id}`, `sylvode://issues/{identifier}` and
//! `sylvode://agents/{agent_id}/inbox`. Anything else (every other template, a query string, a
//! trailing slash) is not, and is left out of the acknowledgment. Parsing never alters the string
//! the client sent: the acknowledgment and every update echo it byte for byte.

use crate::server::{
    canonical_uuid, identifier_argument, parse_agent_inbox_resource_uri, parse_issue_identifier_uri,
    parse_task_resource_uri,
};
use serde_json::json;
use uuid::Uuid;

/// Most URIs one listen request may ask for; the rest are left out of the acknowledgment.
pub const MAX_URIS_PER_REQUEST: usize = 100;
/// Longest URI accepted, in bytes; a longer one is left out of the acknowledgment.
pub const MAX_URI_BYTES: usize = 2_048;

/// A subscribable URI, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subscribable {
    /// `sylvode://tasks/{task_id}`: one task of the calling bot.
    Task(Uuid),
    /// `sylvode://issues/{identifier}`: one work item, by its human-readable identifier.
    Issue(String),
    /// `sylvode://agents/{agent_id}/inbox`: the open tasks of the bot `agent_id`.
    Inbox(Uuid),
}

/// What a change hint is matched against. It names the object, never the URI, so two spellings of
/// the same task share a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubKey {
    Task(Uuid),
    Issue(Uuid),
    Inbox,
}

/// The kind of object a hint without a valid id may concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Task,
    Issue,
}

impl SubKey {
    pub const fn kind(self) -> Option<KeyKind> {
        match self {
            Self::Task(_) => Some(KeyKind::Task),
            Self::Issue(_) => Some(KeyKind::Issue),
            Self::Inbox => None,
        }
    }
}

/// What deciding one subscription came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The caller may read it now; the key to match hints on.
    Granted(SubKey),
    /// Refused, unknown or not the caller's: indistinguishable on purpose.
    Denied,
    /// The API could not be asked; nothing is known.
    Unavailable,
}

/// The subscribable shape of `uri`, or `None` for anything else. A URI over [`MAX_URI_BYTES`] is
/// never subscribable.
pub fn parse_subscribable(uri: &str) -> Option<Subscribable> {
    if uri.len() > MAX_URI_BYTES {
        return None;
    }
    if let Some(id) = parse_task_resource_uri(uri) {
        return strict_uuid(id).map(Subscribable::Task);
    }
    if let Some(id) = parse_agent_inbox_resource_uri(uri) {
        return strict_uuid(id).map(Subscribable::Inbox);
    }
    let identifier = parse_issue_identifier_uri(uri)?;
    // The allowlist the tool path uses for identifiers; the string that passes is the one used.
    let accepted = identifier_argument(&json!({ "identifier": identifier.as_str() }))
        .ok()
        .flatten()
        .is_some_and(|checked| checked == identifier);
    accepted.then_some(Subscribable::Issue(identifier))
}

/// A UUID written exactly as a canonical UUID (no surrounding space), upper or lower case.
fn strict_uuid(raw: &str) -> Option<Uuid> {
    if raw.len() != 36 || canonical_uuid(raw).is_none() {
        return None;
    }
    Uuid::parse_str(raw).ok()
}

/// The requested URIs that are worth deciding: at most [`MAX_URIS_PER_REQUEST`] of the request's
/// array are looked at, in request order; a repeated string and a string that is not subscribable
/// are dropped.
pub fn candidates(requested: &[String]) -> Vec<(String, Subscribable)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for uri in requested.iter().take(MAX_URIS_PER_REQUEST) {
        if let Some(parsed) = parse_subscribable(uri)
            && seen.insert(uri.as_str())
        {
            out.push((uri.clone(), parsed));
        }
    }
    out
}
