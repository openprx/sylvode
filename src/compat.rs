//! Legacy `openpr-webhook` names that keep working for the whole Sylvode 1.x line.
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
/// The legacy executable name, deprecated but kept for the whole Sylvode 1.x line.
pub const LEGACY_NAME: &str = "openpr-webhook";
/// The earliest Sylvode release in which a legacy name may be removed (ADR-0020 D1). The window
/// is the Sylvode product's, not this crate's version.
pub const EARLIEST_REMOVAL: &str = "Sylvode v2.0";
/// Environment variable that forces the tunnel, CLI and callback paths off.
pub const SAFE_MODE_ENV: &str = "SYLVODE_WEBHOOK_SAFE_MODE";
/// Legacy name of [`SAFE_MODE_ENV`].
pub const LEGACY_SAFE_MODE_ENV: &str = "OPENPR_WEBHOOK_SAFE_MODE";
/// The `tracing` target prefix of this service, as written in a `RUST_LOG` directive.
pub const LOG_TARGET: &str = "sylvode_webhook";
/// The target prefix `RUST_LOG` directives used before the crate was renamed.
pub const LEGACY_LOG_TARGET: &str = "openpr_webhook";

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
    /// The `x-openpr-signature` inbound signature header.
    SignatureHeader,
    /// The `OPENPR_WEBHOOK_SAFE_MODE` environment variable.
    SafeModeEnv,
    /// The `openpr_webhook` target in a `RUST_LOG` directive.
    LogTarget,
}

static EXECUTABLE_NAME_WARNED: AtomicBool = AtomicBool::new(false);
static SIGNATURE_HEADER_WARNED: AtomicBool = AtomicBool::new(false);
static SAFE_MODE_ENV_WARNED: AtomicBool = AtomicBool::new(false);
static LOG_TARGET_WARNED: AtomicBool = AtomicBool::new(false);

impl LegacyEntry {
    /// The legacy name and the name that replaces it.
    pub const fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::ExecutableName => (LEGACY_NAME, CANONICAL_NAME),
            Self::SignatureHeader => (
                crate::signature::LEGACY_SIGNATURE_HEADER,
                crate::signature::SYLVODE_SIGNATURE_HEADER,
            ),
            Self::SafeModeEnv => (LEGACY_SAFE_MODE_ENV, SAFE_MODE_ENV),
            Self::LogTarget => (LEGACY_LOG_TARGET, LOG_TARGET),
        }
    }

    const fn kind(self) -> &'static str {
        match self {
            Self::ExecutableName => "executable name",
            Self::SignatureHeader => "inbound signature header",
            Self::SafeModeEnv => "environment variable",
            Self::LogTarget => "RUST_LOG target",
        }
    }

    const fn flag(self) -> &'static AtomicBool {
        match self {
            Self::ExecutableName => &EXECUTABLE_NAME_WARNED,
            Self::SignatureHeader => &SIGNATURE_HEADER_WARNED,
            Self::SafeModeEnv => &SAFE_MODE_ENV_WARNED,
            Self::LogTarget => &LOG_TARGET_WARNED,
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

/// A setting resolved from its canonical and its legacy spelling.
#[derive(Debug, PartialEq, Eq)]
pub struct Resolved<T> {
    pub value: T,
    /// Whether the legacy spelling was supplied, which calls for the deprecation notice.
    pub used_legacy: bool,
}

fn safe_mode_flag(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Resolves safe mode from the values of [`SAFE_MODE_ENV`] and [`LEGACY_SAFE_MODE_ENV`].
///
/// Either spelling alone decides. When both are set they must mean the same thing (both on or
/// both off); otherwise the operator's intent is ambiguous and start-up is refused.
pub fn resolve_safe_mode(canonical: Option<&str>, legacy: Option<&str>) -> Result<Resolved<bool>, String> {
    match (canonical, legacy) {
        (None, None) => Ok(Resolved {
            value: false,
            used_legacy: false,
        }),
        (Some(c), None) => Ok(Resolved {
            value: safe_mode_flag(c),
            used_legacy: false,
        }),
        (None, Some(l)) => Ok(Resolved {
            value: safe_mode_flag(l),
            used_legacy: true,
        }),
        (Some(c), Some(l)) if safe_mode_flag(c) == safe_mode_flag(l) => Ok(Resolved {
            value: safe_mode_flag(c),
            used_legacy: true,
        }),
        (Some(c), Some(l)) => Err(format!(
            "{SAFE_MODE_ENV}={c:?} and the legacy {LEGACY_SAFE_MODE_ENV}={l:?} disagree; \
             set only {SAFE_MODE_ENV}"
        )),
    }
}

/// Rewrites a directive aimed at the legacy target to the canonical one, if it is one.
fn rewrite_legacy_directive(directive: &str) -> Option<String> {
    let rest = directive.strip_prefix(LEGACY_LOG_TARGET)?;
    (rest.is_empty() || rest.starts_with(['=', ':', '['])).then(|| format!("{LOG_TARGET}{rest}"))
}

/// The part of a directive that names what it applies to: everything before the level.
fn directive_selector(directive: &str) -> &str {
    directive.split_once('=').map_or(directive, |(selector, _)| selector)
}

/// Resolves a `RUST_LOG` value. Directives for the legacy `openpr_webhook` target are rewritten to
/// `sylvode_webhook`, so an existing filter keeps selecting this service's events after the crate
/// rename. A legacy directive that selects the same thing as a canonical one with a different
/// level is ambiguous and refused.
pub fn resolve_log_filter(raw: Option<&str>) -> Result<Resolved<Option<String>>, String> {
    let Some(raw) = raw else {
        return Ok(Resolved {
            value: None,
            used_legacy: false,
        });
    };
    let mut used_legacy = false;
    let mut canonical: Vec<&str> = Vec::new();
    let mut rewritten: Vec<(String, &str)> = Vec::new();
    let mut directives: Vec<String> = Vec::new();
    for directive in raw.split(',') {
        let trimmed = directive.trim();
        if let Some(new) = rewrite_legacy_directive(trimmed) {
            used_legacy = true;
            rewritten.push((new.clone(), trimmed));
            directives.push(new);
        } else {
            if trimmed.starts_with(LOG_TARGET) {
                canonical.push(trimmed);
            }
            directives.push(directive.to_string());
        }
    }
    for (new, original) in &rewritten {
        if let Some(clash) = canonical
            .iter()
            .find(|c| directive_selector(c) == directive_selector(new) && *c != new)
        {
            return Err(format!(
                "RUST_LOG directive {clash:?} and the legacy directive {original:?} disagree; \
                 use only {LOG_TARGET} directives"
            ));
        }
    }
    Ok(Resolved {
        value: Some(directives.join(",")),
        used_legacy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [LegacyEntry; 4] = [
        LegacyEntry::ExecutableName,
        LegacyEntry::SignatureHeader,
        LegacyEntry::SafeModeEnv,
        LegacyEntry::LogTarget,
    ];

    #[test]
    fn every_notice_names_legacy_replacement_and_earliest_removal() {
        for entry in ALL {
            let (legacy, replacement) = entry.names();
            assert_ne!(legacy, replacement, "{entry:?} must name a different replacement");
            let message = entry.message();
            assert!(message.contains(legacy), "{entry:?}: {message}");
            assert!(message.contains(replacement), "{entry:?}: {message}");
            assert!(
                message.contains("not removed before Sylvode v2.0"),
                "{entry:?}: {message}"
            );
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
    fn signature_header_notice_names_both_headers() {
        let message = LegacyEntry::SignatureHeader.message();
        assert!(message.contains("`x-openpr-signature`"), "{message}");
        assert!(message.contains("`x-sylvode-signature`"), "{message}");
    }

    #[test]
    fn a_notice_is_written_once_per_process() {
        // Only this test touches the executable-name flag inside the unit-test process.
        assert!(warn_once(LegacyEntry::ExecutableName));
        assert!(!warn_once(LegacyEntry::ExecutableName));
        assert!(!warn_once(LegacyEntry::ExecutableName));
    }

    #[test]
    fn safe_mode_canonical_variable_works_without_a_notice() {
        for (raw, expected) in [
            ("1", true),
            ("true", true),
            (" ON ", true),
            ("yes", true),
            ("0", false),
            ("", false),
        ] {
            assert_eq!(
                resolve_safe_mode(Some(raw), None),
                Ok(Resolved {
                    value: expected,
                    used_legacy: false
                }),
                "{raw:?}"
            );
        }
        assert_eq!(
            resolve_safe_mode(None, None),
            Ok(Resolved {
                value: false,
                used_legacy: false
            })
        );
    }

    #[test]
    fn safe_mode_legacy_variable_works_and_calls_for_the_notice() {
        assert_eq!(
            resolve_safe_mode(None, Some("1")),
            Ok(Resolved {
                value: true,
                used_legacy: true
            })
        );
        assert_eq!(
            resolve_safe_mode(None, Some("off")),
            Ok(Resolved {
                value: false,
                used_legacy: true
            })
        );
    }

    #[test]
    fn safe_mode_spellings_that_agree_are_accepted() {
        assert_eq!(
            resolve_safe_mode(Some("true"), Some("1")),
            Ok(Resolved {
                value: true,
                used_legacy: true
            })
        );
        assert_eq!(
            resolve_safe_mode(Some("0"), Some("no")),
            Ok(Resolved {
                value: false,
                used_legacy: true
            })
        );
    }

    #[test]
    fn safe_mode_spellings_that_disagree_are_refused() {
        for (c, l) in [("1", "0"), ("0", "1"), ("on", "")] {
            let err = resolve_safe_mode(Some(c), Some(l)).unwrap_err();
            assert!(
                err.contains(SAFE_MODE_ENV) && err.contains(LEGACY_SAFE_MODE_ENV),
                "{err}"
            );
        }
    }

    #[test]
    fn log_filter_without_legacy_target_is_kept_verbatim_without_a_notice() {
        for raw in [
            "sylvode_webhook=debug",
            "info",
            "sylvode_webhook::handler=trace,tower_http=warn",
            "",
        ] {
            assert_eq!(
                resolve_log_filter(Some(raw)),
                Ok(Resolved {
                    value: Some(raw.to_string()),
                    used_legacy: false
                }),
                "{raw:?}"
            );
        }
        assert_eq!(
            resolve_log_filter(None),
            Ok(Resolved {
                value: None,
                used_legacy: false
            })
        );
    }

    #[test]
    fn log_filter_legacy_target_is_rewritten_and_calls_for_the_notice() {
        for (raw, expected) in [
            ("openpr_webhook=debug", "sylvode_webhook=debug"),
            ("openpr_webhook", "sylvode_webhook"),
            ("openpr_webhook::handler=trace", "sylvode_webhook::handler=trace"),
            ("openpr_webhook[span]=info", "sylvode_webhook[span]=info"),
            ("warn,openpr_webhook=debug", "warn,sylvode_webhook=debug"),
            (
                "openpr_webhook=debug,sylvode_webhook=debug",
                "sylvode_webhook=debug,sylvode_webhook=debug",
            ),
        ] {
            assert_eq!(
                resolve_log_filter(Some(raw)),
                Ok(Resolved {
                    value: Some(expected.to_string()),
                    used_legacy: true
                }),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn log_filter_only_rewrites_the_exact_legacy_target() {
        assert_eq!(
            resolve_log_filter(Some("openpr_webhook_extra=debug")),
            Ok(Resolved {
                value: Some("openpr_webhook_extra=debug".to_string()),
                used_legacy: false
            })
        );
    }

    #[test]
    fn log_filter_legacy_and_canonical_directives_that_disagree_are_refused() {
        let err = resolve_log_filter(Some("openpr_webhook=debug,sylvode_webhook=error")).unwrap_err();
        assert!(
            err.contains("openpr_webhook=debug") && err.contains("sylvode_webhook=error"),
            "{err}"
        );
        assert!(resolve_log_filter(Some("sylvode_webhook::handler=info,openpr_webhook::handler=trace")).is_err());
        // Different selectors do not clash.
        assert!(resolve_log_filter(Some("sylvode_webhook=info,openpr_webhook::handler=trace")).is_ok());
    }
}
