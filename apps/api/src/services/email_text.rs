//! Subjects and bodies of notification e-mails, in English and Chinese.
//!
//! Plain text only. Text written by users (work item titles, names) passes through [`clean`]:
//! control characters are removed, so a title cannot add a header line or a fake paragraph, and
//! the length is capped. A notification's `content` follows the `key:argument` convention the
//! inbox already renders; each kind knows how to read its own argument.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// Longest user-written fragment placed into an e-mail, in characters.
pub const MAX_FRAGMENT_CHARS: usize = 200;

/// The language of an e-mail: the user's interface language saved with the preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Zh,
}

impl Language {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Zh => "zh",
        }
    }
}

/// A rendered message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailContent {
    pub subject: String,
    pub body: String,
}

/// One notification of a digest.
#[derive(Clone, Copy, Debug)]
pub struct DigestEntry<'a> {
    pub kind: &'a str,
    pub content: &'a str,
    /// Absolute link, when the notification has one.
    pub url: Option<&'a str>,
}

/// User-written text made safe for a subject line or a body: control characters removed, blanks
/// trimmed, at most [`MAX_FRAGMENT_CHARS`] characters.
pub fn clean(text: &str) -> String {
    let stripped: String = text.chars().filter(|c| !c.is_control()).collect();
    stripped.trim().chars().take(MAX_FRAGMENT_CHARS).collect()
}

/// `public_url` and a site-relative link, joined. `None` for a link that is not site-relative.
pub fn absolute_link(public_url: &str, link: &str) -> Option<String> {
    link.starts_with('/')
        .then(|| format!("{}{link}", public_url.trim_end_matches('/')))
}

const fn pick(lang: Language, en: &'static str, zh: &'static str) -> &'static str {
    match lang {
        Language::En => en,
        Language::Zh => zh,
    }
}

/// The one-line summary of a notification, used as the subject and in the digest.
pub fn headline(lang: Language, kind: &str, content: &str) -> String {
    let argument = clean(content.split_once(':').map_or("", |(_, argument)| argument));
    match kind {
        "assignment" => match lang {
            Language::En => format!("You were assigned: {argument}"),
            Language::Zh => format!("你被指派了工作项：{argument}"),
        },
        "mentioned" => match (lang, argument.is_empty()) {
            (Language::En, false) => format!("{argument} mentioned you"),
            (Language::Zh, false) => format!("{argument} 提到了你"),
            // The actor has no name to show: say so, never name an id.
            (Language::En, true) => "Someone mentioned you".to_string(),
            (Language::Zh, true) => "有人提到了你".to_string(),
        },
        "issue_state_changed" | "issue_priority_changed" => {
            let (title, old, new) = split_change(&argument);
            let state = kind == "issue_state_changed";
            match (lang, state) {
                (Language::En, true) => format!("\"{title}\": state changed from {old} to {new}"),
                (Language::En, false) => format!("\"{title}\": priority changed from {old} to {new}"),
                (Language::Zh, true) => format!("“{title}”：状态从 {old} 变为 {new}"),
                (Language::Zh, false) => format!("“{title}”：优先级从 {old} 变为 {new}"),
            }
        }
        "alert_opened" | "alert_escalated" => {
            let (severity, title) = argument.split_once(':').unwrap_or(("", argument.as_str()));
            match (lang, kind == "alert_opened") {
                (Language::En, true) => format!("Alert opened ({severity}): {title}"),
                (Language::En, false) => format!("Alert escalated ({severity}): {title}"),
                (Language::Zh, true) => format!("告警已创建（{severity}）：{title}"),
                (Language::Zh, false) => format!("告警已升级（{severity}）：{title}"),
            }
        }
        "alert_task_completed" => format!(
            "{} {argument}",
            pick(lang, "Alert task completed:", "告警处理任务已完成：")
        ),
        "alert_task_failed" => format!("{} {argument}", pick(lang, "Alert task failed:", "告警处理任务失败：")),
        "alert_suppressed" => match lang {
            Language::En => format!("Alert suppressed by rule {argument}"),
            Language::Zh => format!("告警被规则“{argument}”抑制"),
        },
        "alert_source_throttled" => match lang {
            Language::En => format!("Alert source {argument} is being throttled"),
            Language::Zh => format!("告警来源 {argument} 已被限流"),
        },
        _ => pick(lang, "New notification", "新通知").to_string(),
    }
}

/// `<title>|<old>|<new>`, split from the right because a title may contain `|`.
fn split_change(argument: &str) -> (&str, &str, &str) {
    let mut parts = argument.rsplitn(3, '|');
    let new = parts.next().unwrap_or("");
    let old = parts.next().unwrap_or("");
    let title = parts.next().unwrap_or("");
    (title, old, new)
}

const fn footer(lang: Language) -> &'static str {
    pick(
        lang,
        "You receive this e-mail because e-mail notifications are on in your Sylvode settings.",
        "你收到这封邮件，是因为你在 Sylvode 设置中开启了邮件通知。",
    )
}

/// The e-mail of one notification.
pub fn notification_email(lang: Language, kind: &str, content: &str, url: Option<&str>) -> EmailContent {
    let headline = headline(lang, kind, content);
    let mut body = format!("{headline}\n");
    if let Some(url) = url {
        let _ = write!(body, "\n{} {url}\n", pick(lang, "Open it:", "打开："));
    }
    let _ = write!(body, "\n-- \n{}\n", footer(lang));
    EmailContent {
        subject: format!("[Sylvode] {headline}"),
        body,
    }
}

/// The e-mail that replaces the immediate e-mails of a user on the daily digest: the first
/// entries, then how many more there are.
pub fn digest_email(lang: Language, entries: &[DigestEntry<'_>], more: usize) -> EmailContent {
    let total = entries.len() + more;
    let subject = match lang {
        Language::En => format!("[Sylvode] Your daily summary: {total} unread notifications"),
        Language::Zh => format!("[Sylvode] 每日摘要：{total} 条未读通知"),
    };
    let mut body = format!(
        "{}\n\n",
        pick(
            lang,
            "Unread notifications since your last summary:",
            "自上次摘要以来的未读通知："
        )
    );
    for entry in entries {
        let _ = writeln!(body, "- {}", headline(lang, entry.kind, entry.content));
        if let Some(url) = entry.url {
            let _ = writeln!(body, "  {url}");
        }
    }
    if more > 0 {
        let _ = match lang {
            Language::En => writeln!(body, "\n...and {more} more."),
            Language::Zh => writeln!(body, "\n……以及另外 {more} 条。"),
        };
    }
    let _ = write!(body, "\n-- \n{}\n", footer(lang));
    EmailContent { subject, body }
}

/// The message of the administrator's "send a test e-mail" action.
pub fn test_email(lang: Language) -> EmailContent {
    EmailContent {
        subject: pick(lang, "[Sylvode] Test e-mail", "[Sylvode] 测试邮件").to_string(),
        body: pick(
            lang,
            "This is a test e-mail from your Sylvode server. E-mail delivery works.\n",
            "这是来自你的 Sylvode 服务器的测试邮件。邮件发送正常。\n",
        )
        .to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_control_characters_and_caps_the_length() {
        assert_eq!(clean("a\r\nBcc: x@y\tz"), "aBcc: x@yz");
        assert_eq!(clean("  spaced  "), "spaced");
        assert_eq!(clean(&"x".repeat(500)).chars().count(), MAX_FRAGMENT_CHARS);
        assert_eq!(clean(&"好".repeat(300)).chars().count(), MAX_FRAGMENT_CHARS);
    }

    #[test]
    fn a_title_cannot_add_a_header_or_a_line_to_the_subject() {
        let mail = notification_email(
            Language::En,
            "assignment",
            "issue_assigned:Fix\r\nBcc: evil@example.com",
            None,
        );
        assert!(!mail.subject.contains('\n') && !mail.subject.contains('\r'));
        assert_eq!(mail.subject, "[Sylvode] You were assigned: FixBcc: evil@example.com");
    }

    #[test]
    fn every_kind_has_text_in_both_languages() {
        for (kind, content) in [
            ("assignment", "issue_assigned:Title"),
            ("mentioned", "mentioned_by:Alice"),
            ("issue_state_changed", "issue_state_changed:A | B|todo|done"),
            ("issue_priority_changed", "issue_priority_changed:Title|low|high"),
            ("alert_opened", "alert_opened:HIGH:Disk full"),
            ("alert_escalated", "alert_escalated:CRITICAL:Disk full"),
            ("alert_task_completed", "alert_task_completed:Disk full"),
            ("alert_task_failed", "alert_task_failed:Disk full"),
            ("alert_suppressed", "alert_suppressed:Noisy rule"),
            ("alert_source_throttled", "alert_source_throttled:Prometheus"),
            ("something_else", "x:y"),
        ] {
            let en = headline(Language::En, kind, content);
            let zh = headline(Language::Zh, kind, content);
            assert!(!en.is_empty() && !zh.is_empty());
            assert_ne!(en, zh, "{kind}");
            assert!(!zh.is_ascii(), "{kind}: {zh}");
        }
    }

    #[test]
    fn a_state_change_reads_old_and_new_even_when_the_title_has_a_bar() {
        assert_eq!(
            headline(Language::En, "issue_state_changed", "issue_state_changed:A|B|todo|done"),
            "\"A|B\": state changed from todo to done"
        );
        assert_eq!(
            headline(
                Language::Zh,
                "issue_priority_changed",
                "issue_priority_changed:T|low|high"
            ),
            "“T”：优先级从 low 变为 high"
        );
    }

    #[test]
    fn the_body_carries_the_link_and_the_footer() {
        let url = absolute_link("https://sv.example.com/", "/workspace/w/projects/p/issues/i");
        assert_eq!(
            url.as_deref(),
            Some("https://sv.example.com/workspace/w/projects/p/issues/i")
        );
        let mail = notification_email(Language::En, "mentioned", "mentioned_by:Alice", url.as_deref());
        assert_eq!(mail.subject, "[Sylvode] Alice mentioned you");
        assert!(
            mail.body
                .contains("Open it: https://sv.example.com/workspace/w/projects/p/issues/i")
        );
        assert!(mail.body.contains("e-mail notifications are on"));
        assert!(absolute_link("https://sv.example.com", "https://evil.example/x").is_none());
    }

    #[test]
    fn a_mention_without_an_actor_name_says_someone() {
        let en = notification_email(Language::En, "mentioned", "mentioned_by:", None);
        assert_eq!(en.subject, "[Sylvode] Someone mentioned you");
        let zh = notification_email(Language::Zh, "mentioned", "mentioned_by:", None);
        assert_eq!(zh.subject, "[Sylvode] 有人提到了你");
    }

    #[test]
    fn the_digest_lists_entries_and_counts_the_rest() {
        let entries = [
            DigestEntry {
                kind: "assignment",
                content: "issue_assigned:One",
                url: Some("https://sv.example.com/a"),
            },
            DigestEntry {
                kind: "mentioned",
                content: "mentioned_by:Bob",
                url: None,
            },
        ];
        let mail = digest_email(Language::En, &entries, 3);
        assert_eq!(mail.subject, "[Sylvode] Your daily summary: 5 unread notifications");
        assert!(
            mail.body
                .contains("- You were assigned: One\n  https://sv.example.com/a\n")
        );
        assert!(mail.body.contains("- Bob mentioned you\n"));
        assert!(mail.body.contains("...and 3 more."));
        let zh = digest_email(Language::Zh, &entries, 0);
        assert!(zh.subject.contains("每日摘要"));
        assert!(!zh.body.contains("以及另外"));
    }

    #[test]
    fn the_language_wire_values_are_en_and_zh() {
        assert_eq!(serde_json::to_string(&Language::Zh).unwrap(), "\"zh\"");
        assert_eq!(serde_json::from_str::<Language>("\"en\"").unwrap(), Language::En);
        assert!(serde_json::from_str::<Language>("\"fr\"").is_err());
        assert_eq!(Language::default(), Language::En);
    }
}
