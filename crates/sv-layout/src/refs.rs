//! Reference grammar: `^\$(params|data|slot|viewer|actions)(\.[a-z_][a-z0-9_]*|\[[0-9]{1,4}\])*$`.

/// Root of a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefRoot {
    /// `$params`
    Params,
    /// `$data`
    Data,
    /// `$slot`
    Slot,
    /// `$viewer`
    Viewer,
    /// `$actions`
    Actions,
}

/// A parsed reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The root.
    pub root: RefRoot,
    /// First `.name` segment after the root, when present.
    pub first: Option<String>,
}

/// True for `[a-z_][a-z0-9_]*`.
pub fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Parses a reference string (including the leading `$`).
pub fn parse_ref(s: &str) -> Option<Reference> {
    let rest = s.strip_prefix('$')?;
    let root_end = rest.find(['.', '[']).unwrap_or(rest.len());
    let (root_name, mut tail) = rest.split_at(root_end);
    let root = match root_name {
        "params" => RefRoot::Params,
        "data" => RefRoot::Data,
        "slot" => RefRoot::Slot,
        "viewer" => RefRoot::Viewer,
        "actions" => RefRoot::Actions,
        _ => return None,
    };
    let mut first = None;
    while !tail.is_empty() {
        if let Some(after) = tail.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            let (name, next) = after.split_at(end);
            if !is_ident(name) {
                return None;
            }
            if first.is_none() {
                first = Some(name.to_owned());
            }
            tail = next;
        } else if let Some(after) = tail.strip_prefix('[') {
            let end = after.find(']')?;
            let (digits, next) = after.split_at(end);
            if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            tail = next.strip_prefix(']')?;
        } else {
            return None;
        }
    }
    Some(Reference { root, first })
}

/// A form reference: `$bind.<name>` or `form:<key>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormRef {
    /// `$bind.<name>`
    Bind(String),
    /// `form:<key>`
    Key(String),
}

/// Parses a form reference; raw UUIDs and anything else are refused.
pub fn parse_form_ref(s: &str) -> Option<FormRef> {
    if let Some(name) = s.strip_prefix("$bind.") {
        return is_ident(name).then(|| FormRef::Bind(name.to_owned()));
    }
    let key = s.strip_prefix("form:")?;
    let ok = !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    ok.then(|| FormRef::Key(key.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_references() {
        assert!(parse_ref("$data.req.values.status").is_some());
        assert!(parse_ref("$data.notes.items[0].body").is_some());
        assert!(parse_ref("$bind.x").is_none());
        assert!(parse_ref("$data.Req").is_none());
        assert!(parse_ref("$data.a[12345]").is_none());
        assert!(parse_ref("$data..a").is_none());
    }

    #[test]
    fn parses_form_refs() {
        assert_eq!(parse_form_ref("$bind.req"), Some(FormRef::Bind("req".into())));
        assert_eq!(parse_form_ref("form:svc.req"), Some(FormRef::Key("svc.req".into())));
        assert!(parse_form_ref("0b2f0c3e-1111-2222-3333-444455556666").is_none());
    }
}
