#[path = "../src/page_transfer_plan.rs"]
#[allow(dead_code)]
mod plan;
use plan::*;
use uuid::Uuid;
fn page(n: u8) -> Page {
    Page {
        id: Uuid::from_u128(u128::from(n)),
        scope_id: Uuid::from_u128(u128::from(n) + 100),
        active: true,
        canonical_editor: true,
        frontier: vec![n],
        access: Access::Full,
    }
}
const fn limits() -> Limits {
    Limits {
        blocks: 256,
        assets: 8,
        asset_bytes: 50,
        total_asset_bytes: 100,
    }
}
const fn target() -> Target {
    Target {
        parent_cid: None,
        parent_depth: None,
        sibling_index: 0,
        sibling_count: 0,
    }
}
fn blocks() -> Vec<LiveBlock> {
    vec![
        LiveBlock {
            logical_id: Uuid::from_u128(10),
            physical_cid: "source-root".into(),
            generation: 3,
            parent: None,
            preorder: 0,
            canonical_digest: [1; 32],
            assets: vec![],
            child_targets: vec![],
        },
        LiveBlock {
            logical_id: Uuid::from_u128(11),
            physical_cid: "source-child".into(),
            generation: 2,
            parent: Some(Uuid::from_u128(10)),
            preorder: 1,
            canonical_digest: [2; 32],
            assets: vec![],
            child_targets: vec![],
        },
    ]
}
#[test]
fn move_preserves_uuid_not_physical_cid_and_suspends_fixed_shares() {
    let a = page(1);
    let b = page(2);
    let nodes = blocks();
    let shares = vec![
        Share {
            id: Uuid::from_u128(20),
            root: nodes.first().unwrap().logical_id,
            members: nodes.iter().map(|node| node.logical_id).collect(),
        },
        Share {
            id: Uuid::from_u128(21),
            root: Uuid::from_u128(99),
            members: [nodes.get(1).unwrap().logical_id, Uuid::from_u128(99)].into(),
        },
    ];
    let p = build(
        TransferKind::Move,
        &a,
        &b,
        &nodes,
        TransferReview {
            root_cid: "source-root",
            selection: Selection::CurrentSubtree,
            target: &target(),
            source_token: &a.frontier,
            destination_token: &b.frontier,
        },
        &shares,
        limits(),
    )
    .unwrap();
    assert_eq!(
        p.blocks.first().unwrap().identity,
        Identity::Preserve(nodes.first().unwrap().logical_id)
    );
    assert!(p.blocks.iter().all(|block| block.new_destination_carrier));
    assert_eq!(
        p.affected_shares.first().unwrap().1,
        ShareEffect::RebindToDestinationAndSuspend
    );
    assert_eq!(p.affected_shares.get(1).unwrap().1, ShareEffect::SuspendAtCurrentSource);
    assert_eq!(p.native_undo_barrier_pages, vec![a.id, b.id]);
    assert_eq!(
        build(
            TransferKind::Move,
            &a,
            &b,
            &nodes,
            TransferReview {
                root_cid: "source-root",
                selection: Selection::Block,
                target: &target(),
                source_token: &a.frontier,
                destination_token: &b.frontier,
            },
            &[],
            limits()
        )
        .unwrap_err(),
        Error::SubtreeReviewRequired
    );
}
#[test]
fn copy_allocates_new_identity_and_asset_owner_without_source_mutation() {
    let mut a = page(1);
    let mut b = page(2);
    a.access = Access::View;
    b.access = Access::Edit;
    let mut nodes = blocks();
    let asset = Asset {
        id: Uuid::from_u128(30),
        owner_page: a.id,
        bytes: 40,
        sha256: [3; 32],
        active_reference: true,
    };
    for node in &mut nodes {
        node.assets.push(asset.clone());
    }
    let p = build(
        TransferKind::Copy,
        &a,
        &b,
        &nodes,
        TransferReview {
            root_cid: "source-root",
            selection: Selection::CurrentSubtree,
            target: &target(),
            source_token: &a.frontier,
            destination_token: &b.frontier,
        },
        &[],
        limits(),
    )
    .unwrap();
    assert!(p.blocks.iter().all(|node| node.identity == Identity::AllocateFresh));
    assert_eq!(p.total_asset_bytes, 40);
    assert_eq!(p.assets_to_copy.len(), 1);
    assert_eq!(p.native_undo_barrier_pages, vec![b.id]);
    assert_eq!(
        build(
            TransferKind::Move,
            &a,
            &b,
            &nodes,
            TransferReview {
                root_cid: "source-root",
                selection: Selection::CurrentSubtree,
                target: &target(),
                source_token: &a.frontier,
                destination_token: &b.frontier,
            },
            &[],
            limits()
        )
        .unwrap_err(),
        Error::Unauthorized
    );
}
#[test]
fn stale_reviews_private_children_and_depth_never_silently_expand() {
    let a = page(1);
    let b = page(2);
    let mut nodes = blocks();
    assert_eq!(
        build(
            TransferKind::Copy,
            &a,
            &b,
            &nodes,
            TransferReview {
                root_cid: "source-root",
                selection: Selection::Block,
                target: &target(),
                source_token: &[9],
                destination_token: &b.frontier,
            },
            &[],
            limits()
        )
        .unwrap_err(),
        Error::StaleSource
    );
    let deep = Target {
        parent_cid: Some("private-destination-parent".into()),
        parent_depth: Some(6),
        sibling_index: 0,
        sibling_count: 0,
    };
    assert_eq!(
        build(
            TransferKind::Move,
            &a,
            &b,
            &nodes,
            TransferReview {
                root_cid: "source-root",
                selection: Selection::CurrentSubtree,
                target: &deep,
                source_token: &a.frontier,
                destination_token: &b.frontier,
            },
            &[],
            limits()
        )
        .unwrap_err(),
        Error::DepthLimit
    );
    nodes
        .first_mut()
        .unwrap()
        .child_targets
        .push((Uuid::from_u128(50), false));
    assert_eq!(
        build(
            TransferKind::Copy,
            &a,
            &b,
            &nodes,
            TransferReview {
                root_cid: "source-root",
                selection: Selection::CurrentSubtree,
                target: &target(),
                source_token: &a.frontier,
                destination_token: &b.frontier,
            },
            &[],
            limits()
        )
        .unwrap_err(),
        Error::ChildReferenceDenied
    );
}
