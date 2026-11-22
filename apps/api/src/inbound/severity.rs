//! The five severities of an inbound event and their ranks.

/// Severity of an event, lowest first. The rank is what the database stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub const ALL: [Self; 5] = [Self::Info, Self::Low, Self::Medium, Self::High, Self::Critical];

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|severity| severity.as_str() == text)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    /// Upper case form used in work item titles and notification text.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Low => "LOW",
            Self::Medium => "MEDIUM",
            Self::High => "HIGH",
            Self::Critical => "CRITICAL",
        }
    }

    pub const fn rank(self) -> i16 {
        match self {
            Self::Info => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::Critical => 4,
        }
    }

    pub fn from_rank(rank: i16) -> Option<Self> {
        Self::ALL.into_iter().find(|severity| severity.rank() == rank)
    }

    /// The priority of the task a worker picks up first: alerts go ahead of routine work, whose
    /// assignment tasks use 10.
    pub const fn task_priority(self) -> i32 {
        match self {
            Self::Critical => 40,
            Self::High => 30,
            Self::Medium => 20,
            Self::Low | Self::Info => 10,
        }
    }

    /// The work item priority an alert gets when a rule does not map the severity.
    pub const fn default_work_item_priority(self) -> &'static str {
        match self {
            Self::Critical => "urgent",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low | Self::Info => "low",
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn ranks_order_the_severities_and_round_trip() {
        for (index, severity) in Severity::ALL.into_iter().enumerate() {
            assert_eq!(usize::try_from(severity.rank()).unwrap(), index);
            assert_eq!(Severity::from_rank(severity.rank()), Some(severity));
            assert_eq!(Severity::parse(severity.as_str()), Some(severity));
        }
        assert!(Severity::Critical > Severity::High);
        assert_eq!(Severity::parse("HIGH"), None, "the wire form is lower case");
        assert_eq!(Severity::from_rank(5), None);
    }

    #[test]
    fn task_priority_follows_severity() {
        assert_eq!(Severity::Critical.task_priority(), 40);
        assert_eq!(Severity::High.task_priority(), 30);
        assert_eq!(Severity::Medium.task_priority(), 20);
        assert_eq!(Severity::Low.task_priority(), 10);
        assert_eq!(Severity::Info.task_priority(), 10);
    }
}
