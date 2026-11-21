//! Request/response wire codec between `isolation::host` (parent) and
//! `src/bin/isolated_apply_worker.rs` (child). Pure, `unsafe`-free encode/decode -- no I/O and no
//! process/signal handling live here; those belong to `isolation::host` (parent side) and
//! `src/bin/isolated_apply_worker.rs` (child side).

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use crate::error::CollabError;
use crate::setup_guard::Fired;
use crate::window_check::{FallbackReason, WindowReport};
use crate::{SemanticDiff, SemanticSnapshot};

/// Child-only operation selector set explicitly by the trusted host for every spawn.
pub const OPERATION_ENV: &str = "COLLAB_ISOLATION_OPERATION";
pub const OPERATION_APPLY: &str = "apply";
pub const OPERATION_DIFF: &str = "diff";

/// Child-only window selector, set by the trusted host for a spawn that must run the full
/// computation in the metered window. The only value the worker acts on is [`WINDOW_FULL`]; the
/// variable can only ask for more checking (the worker then computes no base facts, so its window
/// is today's full projection and `check_snapshot`), never for less: no value turns a check off.
pub const WINDOW_ENV: &str = "COLLAB_ISOLATION_WINDOW";
pub const WINDOW_FULL: &str = "full";

/// Response payload cap. Sized well above `bootstrap_response_bytes_max` (12 MiB,
/// `contracts/limits-v1.md`) with headroom for a real exported document snapshot -- unlike
/// `spikes/collab-shared`'s calibration frame (1 MiB, sized for a tiny synthetic report), this
/// frame's success case carries the whole isolated candidate document.
pub const MAX_RESPONSE_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

/// Request field cap (each of `base_snapshot`/`update`), generous relative to
/// `bootstrap_decoded_bytes_max` (8 MiB) and `update_bytes_max` (64 KiB) -- exists only so a
/// corrupt length prefix cannot make either side attempt an unbounded allocation.
pub const MAX_REQUEST_FIELD_BYTES: usize = 32 * 1024 * 1024;

fn write_u32<W: Write>(writer: &mut W, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn read_u32<R: Read>(reader: &mut R) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn write_u64<W: Write>(writer: &mut W, value: u64) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn read_u64<R: Read>(reader: &mut R) -> io::Result<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn write_field<W: Write>(writer: &mut W, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "field too large"))?;
    write_u32(writer, len)?;
    writer.write_all(bytes)
}

fn read_field<R: Read>(reader: &mut R, max_len: usize) -> io::Result<Vec<u8>> {
    let len_u32 = read_u32(reader)?;
    let len = usize::try_from(len_u32).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "field length"))?;
    if len > max_len {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "field exceeds cap"));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_str<W: Write>(writer: &mut W, value: &str) -> io::Result<()> {
    write_field(writer, value.as_bytes())
}

fn read_str<R: Read>(reader: &mut R, max_len: usize) -> io::Result<String> {
    let bytes = read_field(reader, max_len)?;
    String::from_utf8(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 string field"))
}

/// Writes the harness-envelope request: `[base_len][base][update_len][update]`. Reading this back
/// is excluded from the child's metered window (`ADR-0014` section 1.3: "harness envelope 的反序列化"
/// is explicitly not counted toward the CPU/wall ceilings).
///
/// # Errors
/// Any I/O failure writing to `writer`, or [`io::ErrorKind::InvalidInput`] if either field is
/// larger than `u32::MAX` bytes (`update`/`base_snapshot` are already bounded far below that by
/// their own callers' limits, so this never fires in practice).
pub fn write_request<W: Write>(writer: &mut W, base_snapshot: &[u8], update: &[u8]) -> io::Result<()> {
    write_field(writer, base_snapshot)?;
    write_field(writer, update)?;
    writer.flush()
}

/// Reads back a request written by [`write_request`].
///
/// # Errors
/// Any I/O failure, or [`io::ErrorKind::InvalidData`] if a declared field length exceeds
/// `max_field_bytes`.
pub fn read_request<R: Read>(reader: &mut R, max_field_bytes: usize) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let base = read_field(reader, max_field_bytes)?;
    let update = read_field(reader, max_field_bytes)?;
    Ok((base, update))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDiffUpdate {
    pub bytes: Vec<u8>,
    pub before_frontier: Vec<u8>,
    pub after_frontier: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDiffRequest {
    pub snapshot_frontier: Vec<u8>,
    pub updates: Vec<ReplayDiffUpdate>,
    pub from_frontier: Vec<u8>,
    pub to_frontier: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_updates_base64: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDiffResult {
    pub semantic_diff: SemanticDiff,
    pub to_snapshot: SemanticSnapshot,
    pub from_title: String,
    pub to_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_package_semantic_hash_v1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_package_semantic_hash_v2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_package_semantic_hash_v3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_package_semantic_hash_v4: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_reference_remap_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_head_snapshot_base64: Option<String>,
}

/// Known Flow reference containers need a typed identity-remapping adapter, never a UUID scan.
pub fn package_reference_remap_required(engine: &crate::LoroCollabEngine) -> Result<bool, crate::CollabError> {
    let semantic = engine.semantic_snapshot()?;
    Ok(engine.minimum_editor_model()? >= 5
        || !engine.navigator_order()?.is_empty()
        || semantic.nodes.values().any(|node| {
            node.kind == crate::NodeKind::NavigatorNode
                || ["embed_object_id", "child_page_id", "reference_id", "asset_id"]
                    .iter()
                    .any(|key| node.properties.contains_key(*key))
        }))
}

fn json_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

pub fn encode_replay_diff_request(request: &ReplayDiffRequest) -> io::Result<Vec<u8>> {
    serde_json::to_vec(request).map_err(json_error)
}

pub fn decode_replay_diff_request(bytes: &[u8]) -> io::Result<ReplayDiffRequest> {
    serde_json::from_slice(bytes).map_err(json_error)
}

pub fn encode_replay_diff_result(result: &ReplayDiffResult) -> io::Result<Vec<u8>> {
    serde_json::to_vec(result).map_err(json_error)
}

pub fn decode_replay_diff_result(bytes: &[u8]) -> io::Result<ReplayDiffResult> {
    serde_json::from_slice(bytes).map_err(json_error)
}

/// What the child reports back on a run that completes without being killed by a ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `window_cpu_us` is the process CPU time (`CLOCK_PROCESS_CPUTIME_ID`, microseconds) the
    /// worker spent between opening and closing its metered window. Telemetry only: it never
    /// influences a verdict. `None` when the worker did not measure it (the diff operation, an
    /// unreadable clock) or when an older worker produced the outcome.
    ///
    /// `window_report` is what the worker's incremental window check established about the
    /// accepted document (`window_check::WindowReport`); `None` when the full computation decided.
    /// The host compares it with its own projection of the accepted document.
    ///
    /// `setup_guard` says which of the worker's setup guards (`crate::setup_guard`) fired, so that the
    /// window had no base facts and the full computation decided. Telemetry only.
    Success {
        snapshot: Vec<u8>,
        window_cpu_us: Option<u64>,
        window_report: Option<WindowReport>,
        setup_guard: Option<Fired>,
    },
    Rejected(CollabError),
}

/// The encoding of a window CPU time that was not measured, when a report follows it.
const NOT_MEASURED: u64 = u64::MAX;
/// Flags of a success outcome: a window report follows, and which setup guard fired.
const FLAG_REPORT: u8 = 1;
const FLAG_ALLOCATION_GUARD: u8 = 2;
const FLAG_CPU_GUARD: u8 = 4;
const TAG_SUCCESS: u8 = 0;
const TAG_EMPTY_INPUT: u8 = 1;
const TAG_INPUT_TOO_LARGE: u8 = 2;
const TAG_DECODE_FAILED: u8 = 3;
const TAG_UNKNOWN_NODE: u8 = 4;
const TAG_DUPLICATE_NODE: u8 = 5;
const TAG_CYCLE_REJECTED: u8 = 6;
const TAG_OPERATION_FAILED: u8 = 7;
const TAG_LIMIT_EXCEEDED: u8 = 8;
const TAG_CAUSAL_DEPENDENCY_MISSING: u8 = 9;

/// `CollabError::EmptyInput`/`InputTooLarge`/`DecodeFailed`'s `input` field, and
/// `LimitExceeded`'s `limit_kind` field, are `&'static str` -- not `String` -- because every
/// producer in this crate only ever constructs them from a small, fixed vocabulary
/// (`InputLimits::validate_snapshot`/`validate_update` only ever pass `"snapshot"`/`"update"`;
/// `check_snapshot` only ever passes one of five frozen `limit_kind` values from
/// `contracts/limits-v1.md`). The wire only ever carries their UTF-8 bytes, so decoding maps back
/// to the matching static string from that same fixed set -- an unrecognized value (which a
/// well-behaved worker process never sends) falls back to a clearly-labeled placeholder rather
/// than panicking or leaking memory to fabricate an arbitrary `&'static str`.
fn static_str_from(value: &str) -> &'static str {
    match value {
        "snapshot" => "snapshot",
        "update" => "update",
        "tree_depth" => "tree_depth",
        "container_count" => "container_count",
        "document_block_count" => "document_block_count",
        "text_block_chars" => "text_block_chars",
        "document_text_chars" => "document_text_chars",
        _ => "unknown",
    }
}

/// Encodes an [`Outcome`] into a raw payload (not yet length/CRC32-framed -- see
/// `isolation::host`'s response frame helpers, which wrap this payload for transport).
///
/// # Errors
/// Any I/O failure writing into the in-memory buffer (only possible if a field exceeds
/// `u32::MAX` bytes, which none of `CollabError`'s fields ever do in practice).
pub fn encode_outcome(outcome: &Outcome) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    match outcome {
        Outcome::Success {
            snapshot,
            window_cpu_us,
            window_report,
            setup_guard,
        } => {
            buf.push(TAG_SUCCESS);
            write_field(&mut buf, snapshot)?;
            // The CPU time (`u64::MAX` standing for "not measured"), the flags, and the report when
            // the flags say there is one.
            write_u64(&mut buf, window_cpu_us.unwrap_or(NOT_MEASURED))?;
            let mut flags = 0;
            if window_report.is_some() {
                flags |= FLAG_REPORT;
            }
            match setup_guard {
                Some(Fired::Allocation) => flags |= FLAG_ALLOCATION_GUARD,
                Some(Fired::Cpu) => flags |= FLAG_CPU_GUARD,
                None => {}
            }
            buf.push(flags);
            if let Some(report) = window_report {
                for value in [
                    report.blocks,
                    report.navigators,
                    report.chars,
                    report.depth_bound,
                    report.longest_bound,
                ] {
                    write_u64(&mut buf, value)?;
                }
            }
        }
        Outcome::Rejected(CollabError::EmptyInput { input }) => {
            buf.push(TAG_EMPTY_INPUT);
            write_str(&mut buf, input)?;
        }
        Outcome::Rejected(CollabError::InputTooLarge {
            input,
            actual_bytes,
            max_bytes,
        }) => {
            buf.push(TAG_INPUT_TOO_LARGE);
            write_str(&mut buf, input)?;
            write_u64(&mut buf, u64::try_from(*actual_bytes).unwrap_or(u64::MAX))?;
            write_u64(&mut buf, u64::try_from(*max_bytes).unwrap_or(u64::MAX))?;
        }
        Outcome::Rejected(CollabError::DecodeFailed { input, reason }) => {
            buf.push(TAG_DECODE_FAILED);
            write_str(&mut buf, input)?;
            write_str(&mut buf, reason)?;
        }
        Outcome::Rejected(CollabError::UnknownNode { id }) => {
            buf.push(TAG_UNKNOWN_NODE);
            write_str(&mut buf, id)?;
        }
        Outcome::Rejected(CollabError::DuplicateNode { id }) => {
            buf.push(TAG_DUPLICATE_NODE);
            write_str(&mut buf, id)?;
        }
        Outcome::Rejected(CollabError::CycleRejected { id }) => {
            buf.push(TAG_CYCLE_REJECTED);
            write_str(&mut buf, id)?;
        }
        Outcome::Rejected(CollabError::OperationFailed { reason }) => {
            buf.push(TAG_OPERATION_FAILED);
            write_str(&mut buf, reason)?;
        }
        Outcome::Rejected(CollabError::LimitExceeded {
            limit_kind,
            limit,
            observed,
        }) => {
            buf.push(TAG_LIMIT_EXCEEDED);
            write_str(&mut buf, limit_kind)?;
            write_u64(&mut buf, *limit)?;
            write_u64(&mut buf, *observed)?;
        }
        Outcome::Rejected(CollabError::CausalDependencyMissing) => buf.push(TAG_CAUSAL_DEPENDENCY_MISSING),
    }
    Ok(buf)
}

/// Decodes a payload produced by [`encode_outcome`].
///
/// # Errors
/// [`io::ErrorKind::UnexpectedEof`]/`InvalidData` for a truncated or malformed payload (an empty
/// payload, an unrecognized tag byte, a declared field longer than its cap, or non-UTF-8 bytes in
/// a string field) -- the caller (`isolation::host::isolated_apply`) treats any of these as
/// [`super::IsolatedApplyError::HostFailure`], never as a business rejection.
pub fn decode_outcome(payload: &[u8]) -> io::Result<Outcome> {
    let mut cursor = payload;
    let mut tag = [0u8; 1];
    cursor.read_exact(&mut tag)?;
    let Some(&tag_byte) = tag.first() else {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "missing tag byte"));
    };
    match tag_byte {
        TAG_SUCCESS => {
            let snapshot = read_field(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            let window_cpu_us = Some(read_u64(&mut cursor)?).filter(|micros| *micros != NOT_MEASURED);
            let mut flag_byte = [0u8; 1];
            cursor.read_exact(&mut flag_byte)?;
            let [flags] = flag_byte;
            if flags & !(FLAG_REPORT | FLAG_ALLOCATION_GUARD | FLAG_CPU_GUARD) != 0
                || flags & FLAG_ALLOCATION_GUARD != 0 && flags & FLAG_CPU_GUARD != 0
            {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown outcome flags"));
            }
            let window_report = if flags & FLAG_REPORT == 0 {
                None
            } else {
                Some(WindowReport {
                    blocks: read_u64(&mut cursor)?,
                    navigators: read_u64(&mut cursor)?,
                    chars: read_u64(&mut cursor)?,
                    depth_bound: read_u64(&mut cursor)?,
                    longest_bound: read_u64(&mut cursor)?,
                })
            };
            Ok(Outcome::Success {
                snapshot,
                window_cpu_us,
                window_report,
                setup_guard: if flags & FLAG_ALLOCATION_GUARD != 0 {
                    Some(Fired::Allocation)
                } else if flags & FLAG_CPU_GUARD != 0 {
                    Some(Fired::Cpu)
                } else {
                    None
                },
            })
        }
        TAG_EMPTY_INPUT => {
            let input = read_str(&mut cursor, 64)?;
            Ok(Outcome::Rejected(CollabError::EmptyInput {
                input: static_str_from(&input),
            }))
        }
        TAG_INPUT_TOO_LARGE => {
            let input = read_str(&mut cursor, 64)?;
            let actual_bytes = read_u64(&mut cursor)?;
            let max_bytes = read_u64(&mut cursor)?;
            Ok(Outcome::Rejected(CollabError::InputTooLarge {
                input: static_str_from(&input),
                actual_bytes: usize::try_from(actual_bytes).unwrap_or(usize::MAX),
                max_bytes: usize::try_from(max_bytes).unwrap_or(usize::MAX),
            }))
        }
        TAG_DECODE_FAILED => {
            let input = read_str(&mut cursor, 64)?;
            let reason = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::DecodeFailed {
                input: static_str_from(&input),
                reason,
            }))
        }
        TAG_UNKNOWN_NODE => {
            let id = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::UnknownNode { id }))
        }
        TAG_DUPLICATE_NODE => {
            let id = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::DuplicateNode { id }))
        }
        TAG_CYCLE_REJECTED => {
            let id = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::CycleRejected { id }))
        }
        TAG_OPERATION_FAILED => {
            let reason = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::OperationFailed { reason }))
        }
        TAG_LIMIT_EXCEEDED => {
            let limit_kind = read_str(&mut cursor, 64)?;
            let limit = read_u64(&mut cursor)?;
            let observed = read_u64(&mut cursor)?;
            Ok(Outcome::Rejected(CollabError::LimitExceeded {
                limit_kind: static_str_from(&limit_kind),
                limit,
                observed,
            }))
        }
        TAG_CAUSAL_DEPENDENCY_MISSING => Ok(Outcome::Rejected(CollabError::CausalDependencyMissing)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unrecognized outcome tag")),
    }
}

/// The first byte of the decision record the worker writes right after its marker byte, before it
/// does the full computation (`WindowDecision`). Every response the worker completes is
/// `marker, record, frame`; a worker killed by a ceiling after the record has still told the host
/// what its window check decided.
pub const DECISION_RECORD_TAG: u8 = 0xD5;

/// What the worker's window check decided for one attempt, as the host learns it.
///
/// It is written to the wire as one stable byte ([`Self::to_wire`], an explicit table that does not
/// depend on the order of the variants), so that a new `FallbackReason` cannot silently change the
/// meaning of an existing code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowDecision {
    /// The worker did not reach a decision: the import itself was refused or failed, the base did
    /// not load, or this was not an apply. The host treats it as "not a sequential-gate fallback".
    Undecided,
    /// The incremental check accepted the update.
    Incremental,
    /// The incremental check refused the update with the full computation's own refusal (a total
    /// over its limit, or the write of an existing node's id).
    Refused,
    /// The incremental check did not decide, for this reason, and the full computation did.
    Fallback(FallbackReason),
}

const DECISION_UNDECIDED: u8 = 0;
const DECISION_INCREMENTAL: u8 = 1;
const DECISION_REFUSED: u8 = 2;
const DECISION_FALLBACK_BASE: u8 = 16;

const fn fallback_code(reason: FallbackReason) -> u8 {
    DECISION_FALLBACK_BASE
        + match reason {
            FallbackReason::FactsAbsent => 0,
            FallbackReason::StaleFacts => 1,
            FallbackReason::GlobalStructure => 2,
            FallbackReason::UnreadableInsert => 3,
            FallbackReason::TreeStructure => 4,
            FallbackReason::DuplicateLogicalId => 5,
            FallbackReason::ResolvedNodeId => 6,
            FallbackReason::IdRewritten => 7,
            FallbackReason::NotSequential => 8,
            FallbackReason::WorkBudget => 9,
            FallbackReason::Unclassified => 10,
            FallbackReason::KeyCollision => 11,
            FallbackReason::InvalidUnit => 12,
            FallbackReason::AliasLimit => 13,
            FallbackReason::PresenceChange => 14,
            FallbackReason::Inconsistent => 15,
            FallbackReason::LimitReached => 16,
        }
}

impl WindowDecision {
    /// The stable wire code of this decision.
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Undecided => DECISION_UNDECIDED,
            Self::Incremental => DECISION_INCREMENTAL,
            Self::Refused => DECISION_REFUSED,
            Self::Fallback(reason) => fallback_code(reason),
        }
    }

    /// The decision a wire code names, or `None` for a code no worker writes.
    #[must_use]
    pub fn from_wire(code: u8) -> Option<Self> {
        match code {
            DECISION_UNDECIDED => Some(Self::Undecided),
            DECISION_INCREMENTAL => Some(Self::Incremental),
            DECISION_REFUSED => Some(Self::Refused),
            _ => FallbackReason::ALL
                .iter()
                .copied()
                .find(|reason| fallback_code(*reason) == code)
                .map(Self::Fallback),
        }
    }

    /// A short label for the host's log (`fallback:not_sequential`).
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Undecided => "undecided".to_owned(),
            Self::Incremental => "incremental".to_owned(),
            Self::Refused => "refused".to_owned(),
            Self::Fallback(reason) => format!("fallback:{}", fallback_label(reason)),
        }
    }
}

fn fallback_label(reason: FallbackReason) -> String {
    // The variant names are the stable vocabulary; `Debug` prints exactly them.
    let name = format!("{reason:?}");
    let mut label = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                label.push('_');
            }
            label.push(ch.to_ascii_lowercase());
        } else {
            label.push(ch);
        }
    }
    label
}

/// Splits the bytes a worker wrote after its marker byte into its decision record and the rest
/// (the response frame, or nothing for a killed worker).
///
/// The record is present when the first byte is [`DECISION_RECORD_TAG`] and a second byte follows;
/// the second byte must then be a code [`WindowDecision::from_wire`] knows, else the record is
/// invalid (`Err`). Without a record the whole input is returned as the rest.
///
/// # Errors
/// A tag byte followed by a code that names no decision.
pub fn split_decision_record(bytes: &[u8]) -> Result<(Option<WindowDecision>, &[u8]), &'static str> {
    match bytes {
        [DECISION_RECORD_TAG, code, rest @ ..] => WindowDecision::from_wire(*code)
            .map(|decision| (Some(decision), rest))
            .ok_or("decision record carries an unknown code"),
        _ => Ok((None, bytes)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips() {
        let base = b"a base snapshot".to_vec();
        let update = b"an update".to_vec();
        let mut buf = Vec::new();
        write_request(&mut buf, &base, &update).expect("write succeeds");
        let mut cursor = buf.as_slice();
        let (read_base, read_update) = read_request(&mut cursor, MAX_REQUEST_FIELD_BYTES).expect("read succeeds");
        assert_eq!(read_base, base);
        assert_eq!(read_update, update);
    }

    #[test]
    fn replay_diff_request_and_result_round_trip() {
        let request = ReplayDiffRequest {
            snapshot_frontier: vec![1, 2],
            updates: vec![ReplayDiffUpdate {
                bytes: vec![3, 4],
                before_frontier: vec![5],
                after_frontier: vec![6],
            }],
            from_frontier: vec![7],
            to_frontier: vec![8],
            package_updates_base64: None,
        };
        let encoded = encode_replay_diff_request(&request).expect("request encodes");
        assert_eq!(decode_replay_diff_request(&encoded).expect("request decodes"), request);

        let result = ReplayDiffResult {
            semantic_diff: SemanticDiff::default(),
            to_snapshot: SemanticSnapshot::default(),
            from_title: "before".to_string(),
            to_title: "after".to_string(),
            legacy_package_semantic_hash_v1: None,
            legacy_package_semantic_hash_v2: None,
            legacy_package_semantic_hash_v3: None,
            legacy_package_semantic_hash_v4: None,
            package_reference_remap_required: None,
            validated_head_snapshot_base64: None,
        };
        let encoded = encode_replay_diff_result(&result).expect("result encodes");
        assert_eq!(decode_replay_diff_result(&encoded).expect("result decodes"), result);
    }

    #[test]
    fn success_outcome_round_trips() {
        for window_cpu_us in [None, Some(0), Some(13_980)] {
            for setup_guard in [None, Some(Fired::Allocation), Some(Fired::Cpu)] {
                let outcome = Outcome::Success {
                    snapshot: b"exported snapshot bytes".to_vec(),
                    window_cpu_us,
                    window_report: None,
                    setup_guard,
                };
                let payload = encode_outcome(&outcome).expect("encode succeeds");
                let decoded = decode_outcome(&payload).expect("decode succeeds");
                assert_eq!(decoded, outcome);
            }
        }
    }

    #[test]
    fn success_outcome_with_a_window_report_round_trips() {
        let report = WindowReport {
            blocks: 5_000,
            navigators: 7,
            chars: 123_456,
            depth_bound: 3,
            longest_bound: 99,
        };
        for window_cpu_us in [None, Some(0), Some(13_980)] {
            for setup_guard in [None, Some(Fired::Allocation), Some(Fired::Cpu)] {
                let outcome = Outcome::Success {
                    snapshot: b"exported snapshot bytes".to_vec(),
                    window_cpu_us,
                    window_report: Some(report),
                    setup_guard,
                };
                let payload = encode_outcome(&outcome).expect("encode succeeds");
                assert_eq!(decode_outcome(&payload).expect("decode succeeds"), outcome);
            }
        }
        // A report cut short is a corrupt payload, not a report with fewer fields.
        let payload = encode_outcome(&Outcome::Success {
            snapshot: b"snap".to_vec(),
            window_cpu_us: Some(1),
            window_report: Some(report),
            setup_guard: None,
        })
        .expect("encode succeeds");
        for cut in 1..=48 {
            let truncated = payload.get(..payload.len() - cut).expect("cut within the payload");
            let decoded = decode_outcome(truncated);
            assert!(
                decoded.is_err(),
                "a payload cut by {cut} bytes must not decode: {decoded:?}"
            );
        }
    }

    #[test]
    fn success_outcome_with_unknown_flags_is_rejected() {
        let mut payload = encode_outcome(&Outcome::Success {
            snapshot: b"snap".to_vec(),
            window_cpu_us: None,
            window_report: None,
            setup_guard: None,
        })
        .expect("encode succeeds");
        *payload.last_mut().expect("a payload") = 0x80;
        assert!(decode_outcome(&payload).is_err());
    }

    #[test]
    fn success_outcome_with_a_truncated_telemetry_field_is_rejected() {
        let mut payload = vec![TAG_SUCCESS];
        write_field(&mut payload, b"snap").expect("write succeeds");
        payload.extend_from_slice(&[1, 2, 3]);
        assert!(decode_outcome(&payload).is_err());
    }

    #[test]
    fn causal_dependency_missing_outcome_round_trips() {
        let outcome = Outcome::Rejected(CollabError::CausalDependencyMissing);
        let payload = encode_outcome(&outcome).expect("encode succeeds");
        let decoded = decode_outcome(&payload).expect("decode succeeds");
        assert_eq!(decoded, outcome);
    }

    #[test]
    fn decode_failed_outcome_round_trips_with_known_input_field() {
        let outcome = Outcome::Rejected(CollabError::DecodeFailed {
            input: "update",
            reason: "bad bytes".to_string(),
        });
        let payload = encode_outcome(&outcome).expect("encode succeeds");
        let decoded = decode_outcome(&payload).expect("decode succeeds");
        assert_eq!(decoded, outcome);
    }

    #[test]
    fn limit_exceeded_outcome_round_trips_for_every_check_snapshot_limit_kind() {
        for limit_kind in [
            "tree_depth",
            "container_count",
            "document_block_count",
            "text_block_chars",
            "document_text_chars",
        ] {
            let outcome = Outcome::Rejected(CollabError::LimitExceeded {
                limit_kind,
                limit: 10,
                observed: 11,
            });
            let payload = encode_outcome(&outcome).expect("encode succeeds");
            let decoded = decode_outcome(&payload).expect("decode succeeds");
            assert_eq!(decoded, outcome, "limit_kind {limit_kind} must round-trip exactly");
        }
    }

    #[test]
    fn decode_rejects_an_empty_payload() {
        let result = decode_outcome(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn decode_rejects_an_unrecognized_tag() {
        let result = decode_outcome(&[255u8]);
        assert!(result.is_err());
    }

    #[test]
    fn read_field_rejects_a_declared_length_over_the_cap() {
        let mut buf = Vec::new();
        write_u32(&mut buf, 100).expect("write succeeds");
        buf.extend_from_slice(b"short");
        let mut cursor = buf.as_slice();
        let result = read_field(&mut cursor, 10);
        assert!(result.is_err());
    }

    #[test]
    fn decision_record_round_trips_for_every_fallback_reason() {
        let mut seen = std::collections::BTreeSet::new();
        let mut all = vec![
            WindowDecision::Undecided,
            WindowDecision::Incremental,
            WindowDecision::Refused,
        ];
        all.extend(FallbackReason::ALL.iter().copied().map(WindowDecision::Fallback));
        for decision in all {
            let code = decision.to_wire();
            assert!(seen.insert(code), "{decision:?} shares its wire code {code}");
            assert_eq!(WindowDecision::from_wire(code), Some(decision));
        }
        assert_eq!(WindowDecision::from_wire(3), None);
        assert_eq!(WindowDecision::from_wire(255), None);
        // Pinned values: a code, once released, never changes meaning.
        assert_eq!(WindowDecision::Fallback(FallbackReason::FactsAbsent).to_wire(), 16);
        assert_eq!(WindowDecision::Fallback(FallbackReason::NotSequential).to_wire(), 24);
        assert_eq!(WindowDecision::Fallback(FallbackReason::LimitReached).to_wire(), 32);
        assert_eq!(
            WindowDecision::Fallback(FallbackReason::NotSequential).label(),
            "fallback:not_sequential"
        );
    }

    #[test]
    fn a_decision_record_splits_off_the_front_of_the_response() {
        let not_sequential = WindowDecision::Fallback(FallbackReason::NotSequential);
        let bytes = [DECISION_RECORD_TAG, not_sequential.to_wire(), 9, 9];
        assert_eq!(
            split_decision_record(&bytes).expect("a valid record"),
            (Some(not_sequential), &[9u8, 9][..])
        );
        // A killed worker's bytes may hold the record and nothing else, or nothing at all.
        assert_eq!(
            split_decision_record(&bytes[..2]).expect("a valid record"),
            (Some(not_sequential), &[][..])
        );
        assert_eq!(split_decision_record(&[]).expect("no record"), (None, &[][..]));
        assert_eq!(
            split_decision_record(&[DECISION_RECORD_TAG]).expect("a lone tag is no record"),
            (None, &[DECISION_RECORD_TAG][..])
        );
        // A frame that does not start with the tag has no record.
        assert_eq!(
            split_decision_record(&[1, 2, 3]).expect("no record"),
            (None, &[1u8, 2, 3][..])
        );
        assert!(split_decision_record(&[DECISION_RECORD_TAG, 200]).is_err());
    }
}
