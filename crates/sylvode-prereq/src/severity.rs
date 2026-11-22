//! The severity matrix of the design (section 7.11): which finding is how serious for which kind
//! of install. A pure function, so the whole table is testable without touching the machine.

use crate::component::Status;

/// What is being installed or checked; the rows of the severity table.
///
/// Running `doctor` later behaves like the install it belongs to ("either `doctor` at runtime:
/// same as its install row").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// A runner with at least one tmux-channel agent.
    RunnerTmux,
    /// A runner installed with `--headless-only`.
    RunnerHeadless,
    /// A runner whose agents only forward over HTTP.
    RunnerForwardOnly,
    /// The MCP server installed without a runner.
    McpServerAlone,
}

/// The kind of component a finding is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentKind {
    Tmux,
    /// The CLI selected for the install. A CLI nobody selected is never a finding.
    Cli,
    McpServer,
    McpRegistration,
    Runner,
    ApiProtocol,
}

/// What a probe found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finding {
    /// Present, usable and new enough.
    Healthy,
    /// The program was not found.
    Missing,
    /// Found, but older than the minimum.
    TooOld,
    /// Found, but its answer could not be understood (or, for the API, could not be obtained).
    Unparsable,
    /// An MCP registration that is not connected.
    NotConnected,
    /// The task protocol of two components differs.
    ProtocolMismatch,
    /// Found, but the integration with it has not been verified (opencode).
    Unverified,
}

/// The columns of the severity table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Tmux,
    Cli,
    McpServer,
    McpNotConnected,
    Runner,
    Protocol,
    Unverified,
}

const fn column(component: ComponentKind, finding: Finding) -> Option<Column> {
    use ComponentKind as C;
    use Finding as F;
    match (finding, component) {
        (F::ProtocolMismatch, _) | (F::Unparsable | F::Missing, C::ApiProtocol) => Some(Column::Protocol),
        (F::Unverified, _) => Some(Column::Unverified),
        (F::Missing | F::TooOld | F::Unparsable, C::Tmux) => Some(Column::Tmux),
        (F::Missing | F::TooOld | F::Unparsable, C::Cli) => Some(Column::Cli),
        (F::Missing | F::TooOld | F::Unparsable, C::McpServer) => Some(Column::McpServer),
        (F::Missing | F::TooOld | F::Unparsable, C::Runner) => Some(Column::Runner),
        (F::NotConnected | F::Missing | F::TooOld | F::Unparsable, C::McpRegistration) => Some(Column::McpNotConnected),
        // A combination no probe produces (for example "not connected" for tmux).
        (F::Healthy | F::NotConnected | F::TooOld, _) => None,
    }
}

/// The columns, in the order of the rows of [`row`].
const COLUMNS: [Column; 7] = [
    Column::Tmux,
    Column::Cli,
    Column::McpServer,
    Column::McpNotConnected,
    Column::Runner,
    Column::Protocol,
    Column::Unverified,
];

/// One row of the table of section 7.11, in the order of [`COLUMNS`]. "n/a" cells are `ok`.
const fn row(install: InstallKind) -> [Status; 7] {
    use Status::{Fail, Info, Ok, Warn};
    match install {
        InstallKind::RunnerTmux => [Fail, Fail, Fail, Fail, Ok, Fail, Warn],
        InstallKind::RunnerHeadless => [Warn, Fail, Fail, Fail, Ok, Fail, Warn],
        InstallKind::RunnerForwardOnly => [Info, Info, Info, Info, Ok, Warn, Ok],
        InstallKind::McpServerAlone => [Info, Info, Ok, Warn, Info, Fail, Warn],
    }
}

fn cell(install: InstallKind, column: Column) -> Status {
    COLUMNS
        .iter()
        .zip(row(install))
        .find_map(|(candidate, status)| (*candidate == column).then_some(status))
        .unwrap_or(Status::Ok)
}

/// The status of a finding for an install, as the severity table of the design defines it.
///
/// `info` is printed but counts as `ok` in the summary. A cell that does not apply ("n/a") is
/// `ok`. A finding that is `Healthy` is always `ok`.
///
/// A tmux whose version cannot be read ([`Finding::Unparsable`]) is as serious as a tmux that is
/// too old, because the minimum cannot be confirmed; an API whose protocol cannot be obtained is
/// as serious as a mismatch. For every other component an unreadable answer is at most a
/// warning: the program exists.
#[must_use]
pub fn severity(install: InstallKind, component: ComponentKind, finding: Finding) -> Status {
    let Some(column) = column(component, finding) else {
        return Status::Ok;
    };
    let status = cell(install, column);
    let capped = matches!(finding, Finding::Unparsable)
        && !matches!(component, ComponentKind::Tmux | ComponentKind::ApiProtocol);
    if capped && matches!(status, Status::Fail) {
        Status::Warn
    } else {
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ComponentKind as C;
    use Finding as F;
    use InstallKind as I;
    use Status::{Fail, Info, Ok as Good, Warn};

    /// The severity table of design section 7.11, written out cell by cell. Columns: tmux
    /// missing or old, selected CLI missing, MCP server missing, MCP not connected, runner
    /// missing, API protocol mismatch, opencode unverified.
    #[test]
    fn the_severity_table_is_applied_cell_by_cell() {
        let columns = [
            (C::Tmux, F::Missing),
            (C::Cli, F::Missing),
            (C::McpServer, F::Missing),
            (C::McpRegistration, F::NotConnected),
            (C::Runner, F::Missing),
            (C::ApiProtocol, F::ProtocolMismatch),
            (C::Cli, F::Unverified),
        ];
        let table = [
            (I::RunnerTmux, [Fail, Fail, Fail, Fail, Good, Fail, Warn]),
            (I::RunnerHeadless, [Warn, Fail, Fail, Fail, Good, Fail, Warn]),
            (I::RunnerForwardOnly, [Info, Info, Info, Info, Good, Warn, Good]),
            (I::McpServerAlone, [Info, Info, Good, Warn, Info, Fail, Warn]),
        ];
        for (install, expected) in table {
            for ((component, finding), want) in columns.iter().zip(expected) {
                assert_eq!(
                    severity(install, *component, *finding),
                    want,
                    "{install:?} {component:?} {finding:?}"
                );
            }
        }
    }

    /// A tmux that is too old or whose version cannot be read is the tmux column; every other
    /// MCP-registration state follows the not-connected column.
    #[test]
    fn related_findings_share_a_column() {
        for install in [
            I::RunnerTmux,
            I::RunnerHeadless,
            I::RunnerForwardOnly,
            I::McpServerAlone,
        ] {
            assert_eq!(
                severity(install, C::Tmux, F::TooOld),
                severity(install, C::Tmux, F::Missing)
            );
            assert_eq!(
                severity(install, C::Tmux, F::Unparsable),
                severity(install, C::Tmux, F::Missing)
            );
            assert_eq!(
                severity(install, C::McpRegistration, F::Missing),
                severity(install, C::McpRegistration, F::NotConnected)
            );
            assert_eq!(
                severity(install, C::McpServer, F::ProtocolMismatch),
                severity(install, C::ApiProtocol, F::ProtocolMismatch)
            );
        }
    }

    /// A healthy component is `ok` for every install, and an unreadable answer from a program that
    /// exists is at most a warning, except for tmux and the API.
    #[test]
    fn healthy_is_ok_and_unreadable_answers_are_capped() {
        let components = [
            C::Tmux,
            C::Cli,
            C::McpServer,
            C::McpRegistration,
            C::Runner,
            C::ApiProtocol,
        ];
        for install in [
            I::RunnerTmux,
            I::RunnerHeadless,
            I::RunnerForwardOnly,
            I::McpServerAlone,
        ] {
            for component in components {
                assert_eq!(
                    severity(install, component, F::Healthy),
                    Good,
                    "{install:?} {component:?}"
                );
            }
            for component in [C::Cli, C::McpServer, C::Runner, C::McpRegistration] {
                assert_ne!(
                    severity(install, component, F::Unparsable),
                    Fail,
                    "{install:?} {component:?}"
                );
            }
        }
        assert_eq!(severity(I::RunnerTmux, C::Cli, F::Unparsable), Warn);
        assert_eq!(severity(I::RunnerTmux, C::Tmux, F::Unparsable), Fail);
        assert_eq!(severity(I::RunnerTmux, C::ApiProtocol, F::Unparsable), Fail);
    }
}
