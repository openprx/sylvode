//! Legacy `openpr-webhook` names that keep working for the whole 1.x line.
//!
//! Every legacy entry point behaves exactly like its canonical replacement and, the first time it
//! is used in a process, writes one deprecation line to stderr. The notice goes to stderr directly
//! rather than through `tracing`, because the log subscriber writes to stdout and stdout must stay
//! byte-for-byte identical between the legacy and the canonical entry point. A failed write is
//! ignored: a deprecation notice must never change the exit code or fail the process.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// The canonical executable name.
pub const CANONICAL_NAME: &str = "sylvode-webhook";
/// The legacy executable name, deprecated but kept for the whole 1.x line.
pub const LEGACY_NAME: &str = "openpr-webhook";
/// The earliest release in which a legacy name may be removed (ADR-0020 D1).
pub const EARLIEST_REMOVAL: &str = "v2.0";

/// Which executable name the process was started under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invocation {
    /// Started as `sylvode-webhook`.
    Canonical,
    /// Started as the legacy `openpr-webhook`.
    Legacy,
}

/// A legacy entry point that produces a deprecation notice when used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyEntry {
    /// The `openpr-webhook` executable name.
    ExecutableName,
}

static EXECUTABLE_NAME_WARNED: AtomicBool = AtomicBool::new(false);

impl LegacyEntry {
    /// The legacy name and the name that replaces it.
    pub const fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::ExecutableName => (LEGACY_NAME, CANONICAL_NAME),
        }
    }

    const fn kind(self) -> &'static str {
        match self {
            Self::ExecutableName => "executable name",
        }
    }

    const fn flag(self) -> &'static AtomicBool {
        match self {
            Self::ExecutableName => &EXECUTABLE_NAME_WARNED,
        }
    }

    /// The deprecation line. It names the legacy name, its replacement and the earliest removal.
    pub fn message(self) -> String {
        let (legacy, replacement) = self.names();
        format!(
            "warning: the {kind} `{legacy}` is deprecated, use `{replacement}` instead; \
             `{legacy}` is not removed before {EARLIEST_REMOVAL}",
            kind = self.kind(),
        )
    }
}

/// Writes the deprecation line for `entry` to stderr, at most once per process.
///
/// Returns whether this call wrote the notice.
pub fn warn_once(entry: LegacyEntry) -> bool {
    if entry.flag().swap(true, Ordering::SeqCst) {
        return false;
    }
    let mut stderr = std::io::stderr().lock();
    // A notice that cannot be written is dropped: it must not fail the process.
    let _ = writeln!(stderr, "{}", entry.message());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [LegacyEntry; 1] = [LegacyEntry::ExecutableName];

    #[test]
    fn every_notice_names_legacy_replacement_and_earliest_removal() {
        for entry in ALL {
            let (legacy, replacement) = entry.names();
            assert_ne!(legacy, replacement, "{entry:?} must name a different replacement");
            let message = entry.message();
            assert!(message.contains(legacy), "{entry:?}: {message}");
            assert!(message.contains(replacement), "{entry:?}: {message}");
            assert!(message.contains("not removed before v2.0"), "{entry:?}: {message}");
            assert!(!message.contains('\n'), "{entry:?} must be a single line: {message}");
        }
    }

    #[test]
    fn executable_name_notice_names_both_executables() {
        let message = LegacyEntry::ExecutableName.message();
        assert!(message.contains("`openpr-webhook`"), "{message}");
        assert!(message.contains("`sylvode-webhook`"), "{message}");
    }

    #[test]
    fn a_notice_is_written_once_per_process() {
        // Only this test touches the executable-name flag inside the unit-test process.
        assert!(warn_once(LegacyEntry::ExecutableName));
        assert!(!warn_once(LegacyEntry::ExecutableName));
        assert!(!warn_once(LegacyEntry::ExecutableName));
    }
}
