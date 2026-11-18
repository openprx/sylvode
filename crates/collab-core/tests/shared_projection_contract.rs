use collab_core::block_projection::{SharedContent, SharedMember, SharedProjection, validate_members};
use collab_core::page_inline::{InlineMark, InlineSpan};
use uuid::Uuid;
#[test]
fn actual_shared_projection_serde_fixture_has_only_recipient_vocabulary() {
    let text = "中文😀abc".to_string();
    let spans = vec![
        InlineSpan {
            start: 0,
            end: 10,
            mark: InlineMark::Bold,
            href: None,
        },
        InlineSpan {
            start: 6,
            end: 13,
            mark: InlineMark::Italic,
            href: None,
        },
    ];
    let contents = vec![
        SharedContent::Paragraph {
            text: text.clone(),
            spans: spans.clone(),
        },
        SharedContent::Heading {
            level: 2,
            text: text.clone(),
            spans: spans.clone(),
        },
        SharedContent::Bullet {
            text: text.clone(),
            spans: spans.clone(),
        },
        SharedContent::Ordered {
            order: 3,
            text: text.clone(),
            spans: spans.clone(),
        },
        SharedContent::Todo {
            checked: true,
            text: text.clone(),
            spans: spans.clone(),
        },
        SharedContent::Quote {
            text: text.clone(),
            spans: spans.clone(),
        },
        SharedContent::Toggle {
            collapsed: false,
            text,
            spans,
        },
        SharedContent::Code {
            text: "const x = '<&>';".into(),
        },
        SharedContent::Divider,
        SharedContent::Image {
            asset_alias: Uuid::from_u128(9001),
            alt: "图片".into(),
            caption: "中文😀".into(),
        },
        SharedContent::Attachment {
            asset_alias: Uuid::from_u128(9002),
            name: "说明.json".into(),
            mime: "application/json".into(),
            caption: "附件说明".into(),
        },
        SharedContent::ChildPlaceholder,
    ];
    let members = contents
        .into_iter()
        .enumerate()
        .map(|(index, content)| SharedMember {
            member_id: Uuid::from_u128(100 + index as u128),
            parent_member_id: None,
            order: u32::try_from(index).unwrap(),
            content,
        })
        .collect::<Vec<_>>();
    validate_members(&members).unwrap();
    let projection = SharedProjection {
        share_id: Uuid::from_u128(8001),
        revision: 7,
        members,
    };
    let wrapper = serde_json::json!({"projection":projection,"effective_permission":"edit"});
    let encoded = serde_json::to_string_pretty(&wrapper).unwrap();
    for forbidden in [
        "page_id",
        "physical_cid",
        "logical_id",
        "asset_id",
        "source_title",
        "frontier",
        "history",
        "comments",
    ] {
        assert!(!encoded.contains(forbidden));
    }
    std::fs::write("/tmp/sv-shared-projection.fixture.json", encoded).unwrap();
}
