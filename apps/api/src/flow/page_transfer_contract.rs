//! Next-batch transfer wire contract. Not registered in the frozen sharing API.
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Move,
    Copy,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    Block,
    CurrentSubtree,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareInput {
    pub kind: Kind,
    pub target_object_id: Uuid,
    pub root_block_cid: String,
    pub selection: Selection,
    pub target_parent_block_cid: Option<String>,
    pub target_index: u32,
    pub expected_source_frontier: String,
    pub expected_target_frontier: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitInput {
    pub expected_source_frontier: String,
    pub expected_target_frontier: String,
    pub selection_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UndoInput {
    pub expected_source_frontier: String,
    pub expected_target_frontier: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Prepared,
    Accepted,
    NotApplied,
    Undone,
    Expired,
    Cancelled,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Review {
    pub source_object_id: Uuid,
    pub target_object_id: Uuid,
    pub source_frontier: String,
    pub target_frontier: String,
    pub selection_digest: String,
    pub block_count: u32,
    pub asset_count: u32,
    pub total_asset_bytes: u64,
    pub affected_share_count: u32,
    pub native_undo_barrier_pages: Vec<Uuid>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetState {
    Ready,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assets {
    pub state: AssetState,
    pub copied_count: u32,
    pub total_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prepared {
    pub transfer_id: Uuid,
    pub kind: Kind,
    pub state: State,
    pub expires_at: String,
    pub review: Review,
    pub assets: Assets,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptedHead {
    pub object_id: Uuid,
    pub accepted_seq: i64,
    pub frontier: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Identity {
    Preserved,
    New,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mapping {
    pub source_block_cid: String,
    pub target_block_cid: String,
    pub identity: Identity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Undo {
    Available {
        expected_source_frontier: String,
        expected_target_frontier: String,
    },
    Unavailable,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub changed_page_ids: Vec<Uuid>,
    pub source: AcceptedHead,
    pub target: AcceptedHead,
    pub mappings: Vec<Mapping>,
    pub source_placeholder_cid: Option<String>,
    pub affected_share_count: u32,
    pub undo: Undo,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Accepted {
    pub transfer_id: Uuid,
    pub kind: Kind,
    pub state: State,
    pub receipt: Option<Receipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub transfer_id: Uuid,
    pub kind: Kind,
    pub state: State,
    pub expires_at: String,
    pub receipt: Option<Receipt>,
    pub failure_reason: Option<String>,
}
