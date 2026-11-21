//! Legacy `sylvode-webhook` and `openpr-webhook` names that keep working for the whole Sylvode 1.x
//! line.
//!
//! Every legacy entry point behaves exactly like its canonical replacement and, the first time it
//! is used in a process, writes one deprecation line to stderr. The notice goes to stderr directly
//! rather than through `tracing`, because the log subscriber writes to stdout and stdout must stay
//! byte-for-byte identical between the legacy and the canonical entry point. A failed write is
//! ignored: a deprecation notice must never change the exit code or fail the process.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// The canonical executable name.
pub const CANONICAL_NAME: &str = "sylvode-runner";
/// The executable name used while this service was called Sylvode Webhook.
pub const WEBHOOK_ALIAS_NAME: &str = "sylvode-webhook";
/// The oldest executable name, deprecated but kept for the whole Sylvode 1.x line.
pub const LEGACY_NAME: &str = "openpr-webhook";
/// The earliest Sylvode release in which a legacy name may be removed (ADR-0020 D1). The window
/// is the Sylvode product's, not this crate's version.
pub const EARLIEST_REMOVAL: &str = "Sylvode v2.0";
/// Environment variable that forces the tunnel, CLI and callback paths off.
pub const SAFE_MODE_ENV: &str = "SYLVODE_RUNNER_SAFE_MODE";
/// Name of [`SAFE_MODE_ENV`] while this service was called Sylvode Webhook.
pub const WEBHOOK_SAFE_MODE_ENV: &str = "SYLVODE_WEBHOOK_SAFE_MODE";
/// Oldest name of [`SAFE_MODE_ENV`].
pub const LEGACY_SAFE_MODE_ENV: &str = "OPENPR_WEBHOOK_SAFE_MODE";
/// The `tracing` target prefix of this service, as written in a `RUST_LOG` directive.
pub const LOG_TARGET: &str = "sylvode_runner";
/// The target prefix `RUST_LOG` directives used while this service was called Sylvode Webhook.
pub const WEBHOOK_LOG_TARGET: &str = "sylvode_webhook";
/// The oldest target prefix `RUST_LOG` directives used.
pub const LEGACY_LOG_TARGET: &str = "openpr_webhook";

/// Which executable name the process was started under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invocation {
    /// Started as `sylvode-runner`.
    Canonical,
    /// Started as `sylvode-webhook`.
    WebhookAlias,
    /// Started as the legacy `openpr-webhook`.
    Legacy,
}

/// A legacy entry point that produces a deprecation notice when used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyEntry {
    /// The `sylvode-webhook` executable name.
    WebhookExecutableName,
    /// The `openpr-webhook` executable name.
    ExecutableName,
    /// The `x-openpr-signature` inbound signature header.
    SignatureHeader,
    /// The `SYLVODE_WEBHOOK_SAFE_MODE` environment variable.
    WebhookSafeModeEnv,
    /// The `OPENPR_WEBHOOK_SAFE_MODE` environment variable.
    SafeModeEnv,
    /// The `sylvode_webhook` target in a `RUST_LOG` directive.
    WebhookLogTarget,
    /// The `openpr_webhook` target in a `RUST_LOG` directive.
    LogTarget,
}

static WEBHOOK_EXECUTABLE_NAME_WARNED: AtomicBool = AtomicBool::new(false);
static EXECUTABLE_NAME_WARNED: AtomicBool = AtomicBool::new(false);
static SIGNATURE_HEADER_WARNED: AtomicBool = AtomicBool::new(false);
static WEBHOOK_SAFE_MODE_ENV_WARNED: AtomicBool = AtomicBool::new(false);
static SAFE_MODE_ENV_WARNED: AtomicBool = AtomicBool::new(false);
static WEBHOOK_LOG_TARGET_WARNED: AtomicBool = AtomicBool::new(false);
static LOG_TARGET_WARNED: AtomicBool = AtomicBool::new(false);

impl LegacyEntry {
    /// The legacy name and the name that replaces it.
    pub const fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::WebhookExecutableName => (WEBHOOK_ALIAS_NAME, CANONICAL_NAME),
            Self::ExecutableName => (LEGACY_NAME, CANONICAL_NAME),
            Self::SignatureHeader => (
                crate::signature::LEGACY_SIGNATURE_HEADER,
                crate::signature::SYLVODE_SIGNATURE_HEADER,
            ),
            Self::WebhookSafeModeEnv => (WEBHOOK_SAFE_MODE_ENV, SAFE_MODE_ENV),
            Self::SafeModeEnv => (LEGACY_SAFE_MODE_ENV, SAFE_MODE_ENV),
            Self::WebhookLogTarget => (WEBHOOK_LOG_TARGET, LOG_TARGET),
            Self::LogTarget => (LEGACY_LOG_TARGET, LOG_TARGET),
        }
    }

    const fn kind(self) -> &'static str {
        match self {
            Self::WebhookExecutableName | Self::ExecutableName => "executable name",
            Self::SignatureHeader => "inbound signature header",
            Self::WebhookSafeModeEnv | Self::SafeModeEnv => "environment variable",
            Self::WebhookLogTarget | Self::LogTarget => "RUST_LOG target",
        }
    }

    const fn flag(self) -> &'static AtomicBool {
        match self {
            Self::WebhookExecutableName => &WEBHOOK_EXECUTABLE_NAME_WARNED,
            Self::ExecutableName => &EXECUTABLE_NAME_WARNED,
            Self::SignatureHeader => &SIGNATURE_HEADER_WARNED,
            Self::WebhookSafeModeEnv => &WEBHOOK_SAFE_MODE_ENV_WARNED,
            Self::SafeModeEnv => &SAFE_MODE_ENV_WARNED,
            Self::WebhookLogTarget => &WEBHOOK_LOG_TARGET_WARNED,
            Self::LogTarget => &LOG_TARGET_WARNED,
        }
    }

    /// The deprecation line. It names the legacy name, its replacement and the earliest removal.
    pub fn message(self) -> String {
        let (legacy, replacement) = self.names();
        if self == Self::WebhookExecutableName {
            return format!("{legacy} is now {replacement}; the old name is kept until {EARLIEST_REMOVAL}");
        }
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

/// A setting resolved from its canonical and its legacy spellings.
#[derive(Debug, PartialEq, Eq)]
pub struct Resolved<T> {
    pub value: T,
    /// The legacy entries whose spelling was supplied, each of which calls for its notice.
    pub used_legacy: Vec<LegacyEntry>,
}

fn safe_mode_flag(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Resolves safe mode from the values of [`SAFE_MODE_ENV`], [`WEBHOOK_SAFE_MODE_ENV`] and
/// [`LEGACY_SAFE_MODE_ENV`].
///
/// Any spelling alone decides. When several are set they must all mean the same thing (all on or
/// all off); otherwise the operator's intent is ambiguous and start-up is refused.
pub fn resolve_safe_mode(
    canonical: Option<&str>,
    webhook: Option<&str>,
    legacy: Option<&str>,
) -> Result<Resolved<bool>, String> {
    let spellings = [
        (SAFE_MODE_ENV, canonical, None),
        (WEBHOOK_SAFE_MODE_ENV, webhook, Some(LegacyEntry::WebhookSafeModeEnv)),
        (LEGACY_SAFE_MODE_ENV, legacy, Some(LegacyEntry::SafeModeEnv)),
    ];
    let set: Vec<(&str, &str, Option<LegacyEntry>)> = spellings
        .iter()
        .filter_map(|(name, value, entry)| value.map(|v| (*name, v, *entry)))
        .collect();
    let Some(&(first_name, first_value, _)) = set.first() else {
        return Ok(Resolved {
            value: false,
            used_legacy: Vec::new(),
        });
    };
    let value = safe_mode_flag(first_value);
    if let Some(&(name, other, _)) = set.iter().find(|(_, v, _)| safe_mode_flag(v) != value) {
        return Err(format!(
            "{first_name}={first_value:?} and {name}={other:?} disagree; set only {SAFE_MODE_ENV}"
        ));
    }
    Ok(Resolved {
        value,
        used_legacy: set.iter().filter_map(|(_, _, entry)| *entry).collect(),
    })
}

/// The legacy target a directive is aimed at, with the directive rewritten to the canonical one.
fn rewrite_legacy_directive(directive: &str) -> Option<(LegacyEntry, String)> {
    [
        (WEBHOOK_LOG_TARGET, LegacyEntry::WebhookLogTarget),
        (LEGACY_LOG_TARGET, LegacyEntry::LogTarget),
    ]
    .into_iter()
    .find_map(|(target, entry)| {
        let rest = directive.strip_prefix(target)?;
        (rest.is_empty() || rest.starts_with(['=', ':', '['])).then(|| (entry, format!("{LOG_TARGET}{rest}")))
    })
}

/// The part of a directive that names what it applies to: everything before the level.
fn directive_selector(directive: &str) -> &str {
    directive.split_once('=').map_or(directive, |(selector, _)| selector)
}

/// Resolves a `RUST_LOG` value. Directives for the legacy `sylvode_webhook` and `openpr_webhook`
/// targets are rewritten to `sylvode_runner`, so an existing filter keeps selecting this service's
/// events after the crate rename. A legacy directive that selects the same thing as a canonical
/// one with a different level is ambiguous and refused.
pub fn resolve_log_filter(raw: Option<&str>) -> Result<Resolved<Option<String>>, String> {
    let Some(raw) = raw else {
        return Ok(Resolved {
            value: None,
            used_legacy: Vec::new(),
        });
    };
    let mut used_legacy: Vec<LegacyEntry> = Vec::new();
    let mut canonical: Vec<&str> = Vec::new();
    let mut rewritten: Vec<(String, &str)> = Vec::new();
    let mut directives: Vec<String> = Vec::new();
    for directive in raw.split(',') {
        let trimmed = directive.trim();
        if let Some((entry, new)) = rewrite_legacy_directive(trimmed) {
            if !used_legacy.contains(&entry) {
                used_legacy.push(entry);
            }
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

    const ALL: [LegacyEntry; 7] = [
        LegacyEntry::WebhookExecutableName,
        LegacyEntry::ExecutableName,
        LegacyEntry::SignatureHeader,
        LegacyEntry::WebhookSafeModeEnv,
        LegacyEntry::SafeModeEnv,
        LegacyEntry::WebhookLogTarget,
        LegacyEntry::LogTarget,
    ];

    fn resolved<T>(value: T, used_legacy: &[LegacyEntry]) -> Resolved<T> {
        Resolved {
            value,
            used_legacy: used_legacy.to_vec(),
        }
    }

    #[test]
    fn every_notice_names_legacy_replacement_and_earliest_removal() {
        for entry in ALL {
            let (legacy, replacement) = entry.names();
            assert_ne!(legacy, replacement, "{entry:?} must name a different replacement");
            let message = entry.message();
            assert!(message.contains(legacy), "{entry:?}: {message}");
            assert!(message.contains(replacement), "{entry:?}: {message}");
            assert!(message.contains("Sylvode v2.0"), "{entry:?}: {message}");
            assert!(!message.contains('\n'), "{entry:?} must be a single line: {message}");
        }
    }

    #[test]
    fn executable_name_notice_names_both_executables() {
        let message = LegacyEntry::ExecutableName.message();
        assert!(message.contains("`openpr-webhook`"), "{message}");
        assert!(message.contains("`sylvode-runner`"), "{message}");
        assert!(message.contains("not removed before Sylvode v2.0"), "{message}");
    }

    #[test]
    fn webhook_alias_notice_is_the_fixed_line() {
        assert_eq!(
            LegacyEntry::WebhookExecutableName.message(),
            "sylvode-webhook is now sylvode-runner; the old name is kept until Sylvode v2.0"
        );
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
                resolve_safe_mode(Some(raw), None, None),
                Ok(resolved(expected, &[])),
                "{raw:?}"
            );
        }
        assert_eq!(resolve_safe_mode(None, None, None), Ok(resolved(false, &[])));
    }

    #[test]
    fn safe_mode_legacy_variables_work_and_each_calls_for_its_notice() {
        assert_eq!(
            resolve_safe_mode(None, Some("1"), None),
            Ok(resolved(true, &[LegacyEntry::WebhookSafeModeEnv]))
        );
        assert_eq!(
            resolve_safe_mode(None, None, Some("1")),
            Ok(resolved(true, &[LegacyEntry::SafeModeEnv]))
        );
        assert_eq!(
            resolve_safe_mode(None, None, Some("off")),
            Ok(resolved(false, &[LegacyEntry::SafeModeEnv]))
        );
    }

    #[test]
    fn safe_mode_spellings_that_agree_are_accepted() {
        assert_eq!(
            resolve_safe_mode(Some("true"), None, Some("1")),
            Ok(resolved(true, &[LegacyEntry::SafeModeEnv]))
        );
        assert_eq!(
            resolve_safe_mode(Some("0"), Some("no"), Some("off")),
            Ok(resolved(
                false,
                &[LegacyEntry::WebhookSafeModeEnv, LegacyEntry::SafeModeEnv]
            ))
        );
    }

    #[test]
    fn safe_mode_spellings_that_disagree_are_refused() {
        for (c, w, l) in [
            (Some("1"), None, Some("0")),
            (Some("0"), Some("1"), None),
            (Some("on"), None, Some("")),
            (None, Some("1"), Some("0")),
        ] {
            let err = resolve_safe_mode(c, w, l).unwrap_err();
            assert!(err.contains("disagree") && err.contains(SAFE_MODE_ENV), "{err}");
        }
        let err = resolve_safe_mode(None, Some("1"), Some("0")).unwrap_err();
        assert!(
            err.contains(WEBHOOK_SAFE_MODE_ENV) && err.contains(LEGACY_SAFE_MODE_ENV),
            "{err}"
        );
    }

    #[test]
    fn log_filter_without_legacy_target_is_kept_verbatim_without_a_notice() {
        for raw in [
            "sylvode_runner=debug",
            "info",
            "sylvode_runner::handler=trace,tower_http=warn",
            "",
        ] {
            assert_eq!(
                resolve_log_filter(Some(raw)),
                Ok(resolved(Some(raw.to_string()), &[])),
                "{raw:?}"
            );
        }
        assert_eq!(resolve_log_filter(None), Ok(resolved(None, &[])));
    }

    #[test]
    fn log_filter_legacy_targets_are_rewritten_and_each_calls_for_one_notice() {
        for (raw, expected, entry) in [
            (
                "sylvode_webhook=debug",
                "sylvode_runner=debug",
                LegacyEntry::WebhookLogTarget,
            ),
            ("sylvode_webhook", "sylvode_runner", LegacyEntry::WebhookLogTarget),
            ("openpr_webhook=debug", "sylvode_runner=debug", LegacyEntry::LogTarget),
            ("openpr_webhook", "sylvode_runner", LegacyEntry::LogTarget),
            (
                "openpr_webhook::handler=trace",
                "sylvode_runner::handler=trace",
                LegacyEntry::LogTarget,
            ),
            (
                "sylvode_webhook::handler=trace",
                "sylvode_runner::handler=trace",
                LegacyEntry::WebhookLogTarget,
            ),
            (
                "openpr_webhook[span]=info",
                "sylvode_runner[span]=info",
                LegacyEntry::LogTarget,
            ),
            (
                "warn,openpr_webhook=debug",
                "warn,sylvode_runner=debug",
                LegacyEntry::LogTarget,
            ),
            (
                "openpr_webhook=debug,sylvode_runner=debug",
                "sylvode_runner=debug,sylvode_runner=debug",
                LegacyEntry::LogTarget,
            ),
        ] {
            assert_eq!(
                resolve_log_filter(Some(raw)),
                Ok(resolved(Some(expected.to_string()), &[entry])),
                "{raw:?}"
            );
        }
        assert_eq!(
            resolve_log_filter(Some(
                "sylvode_webhook=info,sylvode_webhook::app=debug,openpr_webhook::app=warn"
            )),
            Ok(resolved(
                Some("sylvode_runner=info,sylvode_runner::app=debug,sylvode_runner::app=warn".to_string()),
                &[LegacyEntry::WebhookLogTarget, LegacyEntry::LogTarget]
            ))
        );
    }

    #[test]
    fn log_filter_only_rewrites_the_exact_legacy_targets() {
        for raw in ["openpr_webhook_extra=debug", "sylvode_webhook_extra=debug"] {
            assert_eq!(
                resolve_log_filter(Some(raw)),
                Ok(resolved(Some(raw.to_string()), &[])),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn log_filter_legacy_and_canonical_directives_that_disagree_are_refused() {
        let err = resolve_log_filter(Some("openpr_webhook=debug,sylvode_runner=error")).unwrap_err();
        assert!(
            err.contains("openpr_webhook=debug") && err.contains("sylvode_runner=error"),
            "{err}"
        );
        assert!(resolve_log_filter(Some("sylvode_webhook=debug,sylvode_runner=error")).is_err());
        assert!(resolve_log_filter(Some("sylvode_runner::handler=info,openpr_webhook::handler=trace")).is_err());
        // Different selectors do not clash.
        assert!(resolve_log_filter(Some("sylvode_runner=info,openpr_webhook::handler=trace")).is_ok());
    }
}
