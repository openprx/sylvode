//! The messages typed into a CLI session (design section 4.9, with delta C20). Every message is
//! English, self-contained, holds no secret, and starts with `[Sylvode`, so its first character
//! is never `/`, `!`, `#`, `@` or `$`. Every interpolated value is validated here or sanitised:
//! the task id is a [`Uuid`], the numbers are integers, the identifier and proposal id match their
//! patterns, the retry reason is one word of a closed enum, and the title goes through
//! [`sanitize_title`].

use uuid::Uuid;

use super::sanitize::sanitize_title;

/// Why a message could not be built.
#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, reason = "each variant names the value that was refused")]
pub enum MessageError {
    /// The identifier does not match `^[A-Z0-9]{1,32}-[0-9]{1,10}$`.
    InvalidIdentifier,
    /// The attempt is 0, or beyond the maximum.
    InvalidAttempt,
    /// The project key does not match `^[A-Z0-9]{1,32}$`.
    InvalidProjectKey,
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentifier => write!(f, "the work item identifier is not KEY-number"),
            Self::InvalidAttempt => write!(f, "the attempt number is outside 1..=max"),
            Self::InvalidProjectKey => write!(f, "the project key is not ^[A-Z0-9]{{1,32}}$"),
        }
    }
}

impl std::error::Error for MessageError {}

/// A work item identifier, `KEY-number`: `^[A-Z0-9]{1,32}-[0-9]{1,10}$`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identifier(String);

impl Identifier {
    /// # Errors
    ///
    /// [`MessageError::InvalidIdentifier`] for any other value.
    pub fn parse(value: &str) -> Result<Self, MessageError> {
        let valid = value.split_once('-').is_some_and(|(key, number)| {
            (1..=32).contains(&key.len())
                && key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && (1..=10).contains(&number.len())
                && number.chars().all(|c| c.is_ascii_digit())
        });
        if valid {
            Ok(Self(value.to_string()))
        } else {
            Err(MessageError::InvalidIdentifier)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A proposal id: `^(PROP-)?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalId(String);

impl ProposalId {
    /// `None` for any other value; the message then says `(id not shown)`.
    pub fn parse(value: &str) -> Option<Self> {
        let uuid = value.strip_prefix("PROP-").unwrap_or(value);
        let groups: Vec<&str> = uuid.split('-').collect();
        let lengths = [8, 4, 4, 4, 12];
        let valid = groups.len() == lengths.len()
            && groups.iter().zip(lengths).all(|(group, length)| {
                group.len() == length && group.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            });
        valid.then(|| Self(value.to_string()))
    }
}

/// The word that says why the earlier attempt ended; never free text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryReason {
    Timeout,
    AgentFailed,
    RunnerFailed,
    SessionEnded,
}

impl RetryReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::AgentFailed => "agent_failed",
            Self::RunnerFailed => "runner_failed",
            Self::SessionEnded => "session_ended",
        }
    }
}

/// What the task is about, with the line 2 and 3 text each kind has.
#[derive(Clone, Debug)]
pub enum Source {
    /// `assigned to you`, about an issue.
    Assignment(Identifier),
    /// `you were mentioned in a comment`, about an issue.
    CommentMention(Identifier),
    /// `review requested in a comment`, about an issue.
    ReviewRequest(Identifier),
    /// `your vote is requested`, about a proposal. `proposal_id` is the raw value; one that does
    /// not match the pattern is not shown.
    ProposalVote { proposal_id: String },
    /// `created by hand`, about an issue or about nothing (`Ref: none`).
    Manual(Option<Identifier>),
}

/// The task message of design section 4.9.
#[derive(Clone, Debug)]
pub struct TaskMessage<'a> {
    /// The key of the task's project: a bot's one session serves several projects, so every task
    /// says which one it belongs to. Validated here; no path and no project name goes in.
    pub project_key: &'a str,
    pub id: Uuid,
    pub attempt: u32,
    pub max_attempts: u32,
    pub source: Source,
    /// The raw title; it is sanitised here.
    pub title: &'a str,
    /// Why the earlier attempt ended. Used only when `attempt > 1`; without it a retry carries no
    /// suffix, because nothing is known to say.
    pub earlier_attempt: Option<RetryReason>,
}

/// What the title line says about the title: it is text a person typed, quoted, not an
/// instruction to the agent.
const TITLE_LABEL: &str = "title (untrusted, quoted from the work item)";

impl TaskMessage<'_> {
    /// The text to paste.
    ///
    /// # Errors
    ///
    /// [`MessageError::InvalidAttempt`] when `attempt` is 0 or above `max_attempts`;
    /// [`MessageError::InvalidProjectKey`] for a key outside `^[A-Z0-9]{1,32}$`.
    pub fn render(&self) -> Result<String, MessageError> {
        if self.attempt == 0 || self.attempt > self.max_attempts {
            return Err(MessageError::InvalidAttempt);
        }
        if !super::checks::valid_project_key(self.project_key) {
            return Err(MessageError::InvalidProjectKey);
        }
        let title = sanitize_title(self.title);
        let (subject, reason) = match &self.source {
            Source::Assignment(identifier) => (issue_line(identifier, &title), "assigned to you"),
            Source::CommentMention(identifier) => (issue_line(identifier, &title), "you were mentioned in a comment"),
            Source::ReviewRequest(identifier) => (issue_line(identifier, &title), "review requested in a comment"),
            Source::ProposalVote { proposal_id } => {
                let shown = ProposalId::parse(proposal_id).map_or("(id not shown)", |_| proposal_id.as_str());
                (
                    format!("Proposal: {shown}, {TITLE_LABEL}: \"{title}\""),
                    "your vote is requested",
                )
            }
            Source::Manual(Some(identifier)) => (issue_line(identifier, &title), "created by hand"),
            Source::Manual(None) => ("Ref: none".to_string(), "created by hand"),
        };
        let retry = match (self.attempt > 1, self.earlier_attempt) {
            (true, Some(kind)) => format!(" (retry; earlier attempt ended: {})", kind.as_str()),
            _ => String::new(),
        };
        Ok(format!(
            "[Sylvode task] id={} attempt={}/{}\nProject: {}\n{subject}\nReason: {reason}{retry}\nDo this:\n\
             1. Call sylvode_task_get with this id and attempt.\n\
             2. Call sylvode_task_ack with a short plan, then start work at once.\n\
             3. Call sylvode_task_progress at each milestone and at least every 10 minutes.\n\
             4. End with sylvode_task_complete, or sylvode_task_fail with the reason.",
            self.id.hyphenated(),
            self.attempt,
            self.max_attempts,
            self.project_key,
        ))
    }
}

fn issue_line(identifier: &Identifier, title: &str) -> String {
    format!("Issue: {}, {TITLE_LABEL}: \"{title}\"", identifier.as_str())
}

/// The reminder for a task the session has not acknowledged.
#[must_use]
pub fn reminder_not_acknowledged(id: &Uuid, attempt: u32) -> String {
    format!(
        "[Sylvode reminder] Task id={} attempt={attempt} is not acknowledged.\n\
         Call sylvode_task_ack now, or sylvode_task_fail if you cannot do it.",
        id.hyphenated()
    )
}

/// The reminder for a task whose turn ended while the task is still open.
#[must_use]
pub fn reminder_still_open(id: &Uuid, attempt: u32) -> String {
    format!(
        "[Sylvode reminder] Task id={} attempt={attempt} is still open.\n\
         Call sylvode_task_complete or sylvode_task_fail now.",
        id.hyphenated()
    )
}
