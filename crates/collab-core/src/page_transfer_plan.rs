//! Independent next-batch transfer planning. Not registered in the P3A engine or HTTP routes.
//! Access, live blocks and frontiers are server-loaded facts, never client-nominated UUID data.
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferKind {
    Move,
    Copy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Block,
    CurrentSubtree,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Denied,
    View,
    Edit,
    Full,
}
#[derive(Clone, Debug)]
pub struct Page {
    pub id: Uuid,
    pub scope_id: Uuid,
    pub active: bool,
    pub canonical_editor: bool,
    pub frontier: Vec<u8>,
    pub access: Access,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub id: Uuid,
    pub owner_page: Uuid,
    pub bytes: u64,
    pub sha256: [u8; 32],
    pub active_reference: bool,
}
#[derive(Clone, Debug)]
pub struct LiveBlock {
    pub logical_id: Uuid,
    pub physical_cid: String,
    pub generation: u64,
    pub parent: Option<Uuid>,
    pub preorder: u32,
    /// Full canonical typed content, including reference slots. Not the redacted share hash.
    pub canonical_digest: [u8; 32],
    pub assets: Vec<Asset>,
    pub child_targets: Vec<(Uuid, bool)>,
}
#[derive(Clone, Debug)]
pub struct Share {
    pub id: Uuid,
    pub root: Uuid,
    pub members: BTreeSet<Uuid>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareEffect {
    RebindToDestinationAndSuspend,
    SuspendAtCurrentSource,
}
#[derive(Clone, Debug)]
pub struct Target {
    pub parent_cid: Option<String>,
    pub parent_depth: Option<u8>,
    pub sibling_index: u32,
    pub sibling_count: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub blocks: usize,
    pub assets: usize,
    pub asset_bytes: u64,
    pub total_asset_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    Preserve(Uuid),
    AllocateFresh,
}
#[derive(Clone, Debug)]
pub struct PlannedBlock {
    pub source_cid: String,
    pub source_logical: Uuid,
    pub expected_generation: u64,
    pub relative_parent: Option<Uuid>,
    pub relative_depth: u8,
    pub identity: Identity,
    /// Every destination carrier is newly created, including an identity-preserving move.
    pub new_destination_carrier: bool,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub kind: TransferKind,
    pub source_page: Uuid,
    pub destination_page: Uuid,
    pub blocks: Vec<PlannedBlock>,
    pub assets_to_copy: Vec<Asset>,
    pub total_asset_bytes: u64,
    pub affected_shares: Vec<(Uuid, ShareEffect)>,
    pub native_undo_barrier_pages: Vec<Uuid>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    SamePage,
    Unauthorized,
    Inactive,
    LegacyNeedsAdmission,
    StaleSource,
    StaleDestination,
    InvalidGraph,
    MissingRoot,
    SubtreeReviewRequired,
    TargetPosition,
    DepthLimit,
    BlockLimit,
    InvalidAsset,
    AssetLimit,
    ChildReferenceDenied,
    InvalidLimits,
}

#[derive(Clone, Copy)]
pub struct TransferReview<'a> {
    pub root_cid: &'a str,
    pub selection: Selection,
    pub target: &'a Target,
    pub source_token: &'a [u8],
    pub destination_token: &'a [u8],
}

pub fn build(
    kind: TransferKind,
    source: &Page,
    destination: &Page,
    blocks: &[LiveBlock],
    review: TransferReview<'_>,
    shares: &[Share],
    limits: Limits,
) -> Result<Plan, Error> {
    let TransferReview {
        root_cid,
        selection,
        target,
        source_token,
        destination_token,
    } = review;
    if source.id == destination.id {
        return Err(Error::SamePage);
    }
    let (source_level, destination_level) = if kind == TransferKind::Move {
        (Access::Full, Access::Full)
    } else {
        (Access::View, Access::Edit)
    };
    if source.access < source_level || destination.access < destination_level {
        return Err(Error::Unauthorized);
    }
    if !source.active || !destination.active {
        return Err(Error::Inactive);
    }
    if !source.canonical_editor || !destination.canonical_editor {
        return Err(Error::LegacyNeedsAdmission);
    }
    // Opaque server tokens are preserved. Local synchronization uses decoded VV elsewhere.
    if source.frontier != source_token {
        return Err(Error::StaleSource);
    }
    if destination.frontier != destination_token {
        return Err(Error::StaleDestination);
    }
    if limits.blocks == 0 || limits.assets == 0 || limits.asset_bytes == 0 || limits.total_asset_bytes == 0 {
        return Err(Error::InvalidLimits);
    }
    if target.parent_cid.is_some() != target.parent_depth.is_some() || target.sibling_index > target.sibling_count {
        return Err(Error::TargetPosition);
    }
    let by_id = blocks
        .iter()
        .map(|block| (block.logical_id, block))
        .collect::<BTreeMap<_, _>>();
    let cids = blocks
        .iter()
        .map(|block| block.physical_cid.as_str())
        .collect::<BTreeSet<_>>();
    if by_id.len() != blocks.len()
        || cids.len() != blocks.len()
        || blocks
            .iter()
            .any(|block| block.logical_id.is_nil() || block.physical_cid.is_empty() || block.generation == 0)
    {
        return Err(Error::InvalidGraph);
    }
    let root = blocks
        .iter()
        .find(|block| block.physical_cid == root_cid)
        .ok_or(Error::MissingRoot)?;
    let mut descendants = Vec::new();
    for block in blocks {
        let mut id = Some(block.logical_id);
        let mut seen = BTreeSet::new();
        let mut depth = 0_u8;
        let mut relative = None;
        while let Some(current) = id {
            if !seen.insert(current) {
                return Err(Error::InvalidGraph);
            }
            if current == root.logical_id {
                relative = Some(depth);
            }
            let node = by_id.get(&current).ok_or(Error::InvalidGraph)?;
            id = node.parent;
            depth = depth.checked_add(1).ok_or(Error::InvalidGraph)?;
            if depth > 8 {
                return Err(Error::DepthLimit);
            }
        }
        if let Some(relative) = relative {
            descendants.push((block, relative));
        }
    }
    if kind == TransferKind::Move && selection == Selection::Block && descendants.len() > 1 {
        return Err(Error::SubtreeReviewRequired);
    }
    if selection == Selection::Block {
        descendants.retain(|(block, _)| block.logical_id == root.logical_id);
    }
    if descendants.len() > limits.blocks {
        return Err(Error::BlockLimit);
    }
    descendants.sort_by_key(|(block, _)| block.preorder);
    let destination_depth = target
        .parent_depth
        .map_or(Some(0), |depth| depth.checked_add(1))
        .ok_or(Error::DepthLimit)?;
    if descendants
        .iter()
        .any(|(_, depth)| destination_depth.checked_add(*depth).is_none_or(|depth| depth > 7))
    {
        return Err(Error::DepthLimit);
    }
    let selected = descendants
        .iter()
        .map(|(block, _)| block.logical_id)
        .collect::<BTreeSet<_>>();
    let mut assets = BTreeMap::new();
    for (block, _) in &descendants {
        if block.child_targets.iter().any(|(_, readable)| !readable) {
            return Err(Error::ChildReferenceDenied);
        }
        for asset in &block.assets {
            if asset.owner_page != source.id
                || !asset.active_reference
                || asset.bytes == 0
                || asset.bytes > limits.asset_bytes
            {
                return Err(Error::InvalidAsset);
            }
            if assets.get(&asset.id).is_some_and(|old| *old != asset) {
                return Err(Error::InvalidAsset);
            }
            assets.insert(asset.id, asset);
        }
    }
    let total = assets
        .values()
        .try_fold(0_u64, |sum, asset| sum.checked_add(asset.bytes))
        .ok_or(Error::AssetLimit)?;
    if assets.len() > limits.assets || total > limits.total_asset_bytes {
        return Err(Error::AssetLimit);
    }
    let affected = if kind == TransferKind::Move {
        shares
            .iter()
            .filter(|share| !share.members.is_disjoint(&selected))
            .map(|share| {
                (
                    share.id,
                    if selected.contains(&share.root) && share.members.is_subset(&selected) {
                        ShareEffect::RebindToDestinationAndSuspend
                    } else {
                        ShareEffect::SuspendAtCurrentSource
                    },
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(Plan {
        kind,
        source_page: source.id,
        destination_page: destination.id,
        blocks: descendants
            .into_iter()
            .map(|(block, depth)| PlannedBlock {
                source_cid: block.physical_cid.clone(),
                source_logical: block.logical_id,
                expected_generation: block.generation,
                relative_parent: block.parent.filter(|parent| selected.contains(parent)),
                relative_depth: depth,
                identity: if kind == TransferKind::Move {
                    Identity::Preserve(block.logical_id)
                } else {
                    Identity::AllocateFresh
                },
                new_destination_carrier: true,
            })
            .collect(),
        assets_to_copy: assets.into_values().cloned().collect(),
        total_asset_bytes: total,
        affected_shares: affected,
        native_undo_barrier_pages: if kind == TransferKind::Move {
            vec![source.id, destination.id]
        } else {
            vec![destination.id]
        },
    })
}
