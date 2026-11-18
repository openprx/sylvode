//! Recipient vocabulary: no source Page, CID, history, private discussion or asset UUID fields.
use crate::page_inline::InlineSpan;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SHARE_MEMBER_MAX: usize = 256;
pub const SHARE_PATCH_COMMAND_MAX: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SharedContent {
    Paragraph {
        text: String,
        spans: Vec<InlineSpan>,
    },
    Heading {
        level: u8,
        text: String,
        spans: Vec<InlineSpan>,
    },
    Bullet {
        text: String,
        spans: Vec<InlineSpan>,
    },
    Ordered {
        order: u32,
        text: String,
        spans: Vec<InlineSpan>,
    },
    Todo {
        checked: bool,
        text: String,
        spans: Vec<InlineSpan>,
    },
    Quote {
        text: String,
        spans: Vec<InlineSpan>,
    },
    Toggle {
        collapsed: bool,
        text: String,
        spans: Vec<InlineSpan>,
    },
    Code {
        text: String,
    },
    Divider,
    Image {
        asset_alias: Uuid,
        alt: String,
        caption: String,
    },
    Attachment {
        asset_alias: Uuid,
        name: String,
        mime: String,
        caption: String,
    },
    /// A source target is never expanded or exposed without separate recipient authorization.
    ChildPlaceholder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedMember {
    /// Random share-local identity, distinct from private logical UUID and physical CID.
    pub member_id: Uuid,
    pub parent_member_id: Option<Uuid>,
    pub order: u32,
    pub content: SharedContent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedProjection {
    pub share_id: Uuid,
    /// Independent content revision. Never a Page head sequence or document frontier.
    pub revision: u64,
    pub members: Vec<SharedMember>,
}

fn deserialize_shared_spans<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<InlineSpan>, D::Error> {
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let mut spans = Vec::with_capacity(values.len());
    for value in values {
        if value.as_object().is_none_or(|object| {
            object
                .keys()
                .any(|key| !matches!(key.as_str(), "start" | "end" | "mark" | "href"))
        }) {
            return Err(serde::de::Error::custom("unknown shared span field"));
        }
        spans.push(serde_json::from_value(value).map_err(serde::de::Error::custom)?);
    }
    Ok(spans)
}

/// Recipient writes address only share-local members. Structure edits require a separate
/// fixed-membership check; this initial vocabulary cannot delete hidden source descendants.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SharedCommand {
    ReplaceText {
        member_id: Uuid,
        text: String,
        #[serde(deserialize_with = "deserialize_shared_spans")]
        spans: Vec<InlineSpan>,
    },
    SetMarks {
        member_id: Uuid,
        edits: Vec<crate::page_inline::MarkEdit>,
    },
    SetChecked {
        member_id: Uuid,
        checked: bool,
    },
    SetCollapsed {
        member_id: Uuid,
        collapsed: bool,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPatch {
    pub expected_revision: u64,
    pub commands: Vec<SharedCommand>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchError {
    StaleRevision,
    InvalidCommandCount,
    UnknownMember,
    InvalidMemberKind,
}

pub fn validate_patch(projection: &SharedProjection, patch: &SharedPatch) -> Result<(), PatchError> {
    if patch.expected_revision != projection.revision {
        return Err(PatchError::StaleRevision);
    }
    if patch.commands.is_empty() || patch.commands.len() > SHARE_PATCH_COMMAND_MAX {
        return Err(PatchError::InvalidCommandCount);
    }
    for command in &patch.commands {
        let id = match command {
            SharedCommand::ReplaceText { member_id, .. }
            | SharedCommand::SetMarks { member_id, .. }
            | SharedCommand::SetChecked { member_id, .. }
            | SharedCommand::SetCollapsed { member_id, .. } => member_id,
        };
        let member = projection
            .members
            .iter()
            .find(|member| member.member_id == *id)
            .ok_or(PatchError::UnknownMember)?;
        let valid = match command {
            SharedCommand::ReplaceText { .. } => matches!(
                member.content,
                SharedContent::Paragraph { .. }
                    | SharedContent::Heading { .. }
                    | SharedContent::Bullet { .. }
                    | SharedContent::Ordered { .. }
                    | SharedContent::Todo { .. }
                    | SharedContent::Quote { .. }
                    | SharedContent::Toggle { .. }
                    | SharedContent::Code { .. }
            ),
            SharedCommand::SetMarks { .. } => matches!(
                member.content,
                SharedContent::Paragraph { .. }
                    | SharedContent::Heading { .. }
                    | SharedContent::Bullet { .. }
                    | SharedContent::Ordered { .. }
                    | SharedContent::Todo { .. }
                    | SharedContent::Quote { .. }
                    | SharedContent::Toggle { .. }
            ),
            SharedCommand::SetChecked { .. } => matches!(member.content, SharedContent::Todo { .. }),
            SharedCommand::SetCollapsed { .. } => matches!(member.content, SharedContent::Toggle { .. }),
        };
        if !valid {
            return Err(PatchError::InvalidMemberKind);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionError {
    Empty,
    TooManyMembers,
    DuplicateMember,
    ForeignParent,
    Cycle,
    ExcessiveDepth,
}

/// Validate only the captured fixed membership; private future descendants cannot enter implicitly.
pub fn validate_members(members: &[SharedMember]) -> Result<(), ProjectionError> {
    use std::collections::BTreeMap;
    if members.is_empty() {
        return Err(ProjectionError::Empty);
    }
    if members.len() > SHARE_MEMBER_MAX {
        return Err(ProjectionError::TooManyMembers);
    }
    let mut parents = BTreeMap::new();
    for member in members {
        if parents.insert(member.member_id, member.parent_member_id).is_some() {
            return Err(ProjectionError::DuplicateMember);
        }
    }
    for member in members {
        let mut parent = member.parent_member_id;
        let mut visited = std::collections::BTreeSet::from([member.member_id]);
        let mut depth = 0;
        while let Some(id) = parent {
            if !visited.insert(id) {
                return Err(ProjectionError::Cycle);
            }
            parent = *parents.get(&id).ok_or(ProjectionError::ForeignParent)?;
            depth += 1;
            if depth > 7 {
                return Err(ProjectionError::ExcessiveDepth);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_members_reject_hidden_parent_and_cycles() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let mut members = vec![SharedMember {
            member_id: a,
            parent_member_id: None,
            order: 0,
            content: SharedContent::Divider,
        }];
        assert_eq!(validate_members(&members), Ok(()));
        members.first_mut().unwrap().parent_member_id = Some(b);
        assert_eq!(validate_members(&members), Err(ProjectionError::ForeignParent));
        members.push(SharedMember {
            member_id: b,
            parent_member_id: Some(a),
            order: 0,
            content: SharedContent::ChildPlaceholder,
        });
        assert_eq!(validate_members(&members), Err(ProjectionError::Cycle));
    }
    #[test]
    fn typed_patch_requires_current_revision_and_fixed_member() {
        let id = Uuid::from_u128(1);
        let projection = SharedProjection {
            share_id: Uuid::from_u128(2),
            revision: 5,
            members: vec![SharedMember {
                member_id: id,
                parent_member_id: None,
                order: 0,
                content: SharedContent::Todo {
                    checked: false,
                    text: "visible".into(),
                    spans: vec![],
                },
            }],
        };
        let mut patch = SharedPatch {
            expected_revision: 4,
            commands: vec![SharedCommand::SetChecked {
                member_id: id,
                checked: true,
            }],
        };
        assert_eq!(validate_patch(&projection, &patch), Err(PatchError::StaleRevision));
        patch.expected_revision = 5;
        assert_eq!(validate_patch(&projection, &patch), Ok(()));
        patch.commands = vec![SharedCommand::SetChecked {
            member_id: Uuid::from_u128(3),
            checked: true,
        }];
        assert_eq!(validate_patch(&projection, &patch), Err(PatchError::UnknownMember));
        assert!(
            serde_json::from_value::<SharedPatch>(
                serde_json::json!({"expected_revision":5,"commands":[],"raw_update":"forbidden"})
            )
            .is_err()
        );
    }

    #[test]
    fn recipient_wire_rejects_source_metadata_and_private_discussion() {
        let body =
            serde_json::json!({"kind":"paragraph","text":"allowed","spans":[],"page_id":"private","comments":[]});
        assert!(serde_json::from_value::<SharedContent>(body).is_err());
        let value = serde_json::to_value(SharedContent::ChildPlaceholder).unwrap();
        assert_eq!(value, serde_json::json!({"kind":"child_placeholder"}));
    }
}
