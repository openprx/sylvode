//! Typed model5 body nodes. Binary ownership and child authority are application validators.
use crate::CollabError;
use serde_json::Value;
fn invalid(reason: &str) -> CollabError {
    CollabError::OperationFailed { reason: reason.into() }
}
pub fn is_atom(kind: &str) -> bool {
    matches!(kind, "image" | "attachment" | "childPage")
}
pub fn validate(kind: &str, attrs: &Value) -> Result<(), CollabError> {
    let attrs = attrs
        .as_object()
        .ok_or_else(|| invalid("editor_atom_invalid_attributes"))?;
    let allowed: &[&str] = match kind {
        "image" => &["asset_id", "alt", "caption", "indent"],
        "attachment" => &["asset_id", "name", "mime", "caption", "indent"],
        "childPage" => &["child_page_id", "indent"],
        _ => return Err(invalid("editor_atom_unknown_kind")),
    };
    if attrs.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid("editor_atom_unknown_attribute"));
    }
    let id_key = if kind == "childPage" {
        "child_page_id"
    } else {
        "asset_id"
    };
    if attrs
        .get(id_key)
        .and_then(Value::as_str)
        .is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(invalid("editor_atom_invalid_reference_id"));
    }
    for (key, max) in [("alt", 4096), ("caption", 4096), ("name", 255), ("mime", 128)] {
        if let Some(value) = attrs.get(key)
            && value.as_str().is_none_or(|value| value.len() > max)
        {
            return Err(invalid("editor_atom_invalid_text_attribute"));
        }
    }
    Ok(())
}
pub fn searchable_text(kind: &str, attrs: &Value) -> String {
    if kind == "childPage" {
        return String::new();
    }
    [if kind == "image" { "alt" } else { "name" }, "caption"]
        .iter()
        .filter_map(|key| attrs.get(*key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use crate::{CollabEngine, LoroCollabEngine, Operation};
    #[test]
    fn external_src_is_not_a_media_authority_and_raw_updates_cannot_drop_the_floor() {
        use loro::{Container, ValueOrContainer};
        assert!(super::validate("image",&serde_json::json!({"asset_id":"11111111-1111-4111-8111-111111111111","src":"https://example.test/image.png","indent":0})).is_err());
        let mut engine = LoroCollabEngine::new_empty(883);
        engine.initialize_page_editor().unwrap();
        engine.initialize_page_structure().unwrap();
        let cid=engine.insert_page_atom("media",None,1,"image",&serde_json::json!({"asset_id":"11111111-1111-4111-8111-111111111111","alt":"","caption":"","indent":0})).unwrap();
        engine
            .apply_content_operation(&Operation::DeleteNode { id: cid.into() })
            .unwrap();
        let raw = loro::LoroDoc::new();
        raw.import(&engine.export_snapshot().unwrap()).unwrap();
        raw.set_peer_id(889).unwrap();
        let before = raw.oplog_vv();
        raw.get_map(crate::editor_structure::META)
            .insert("model", "rich_page_v4")
            .unwrap();
        let still_hidden = LoroCollabEngine::load(&raw.export(loro::ExportMode::Snapshot).unwrap()).unwrap();
        assert_eq!(still_hidden.minimum_editor_model().unwrap(), 5);
        let Some(ValueOrContainer::Container(Container::List(list))) = raw.get_map("doc").get("children") else {
            panic!("synthetic root list")
        };
        list.delete(1, 1).unwrap();
        raw.commit();
        let bytes = raw.export(loro::ExportMode::updates(&before)).unwrap();
        assert!(engine.import_update_causally_complete(&bytes).is_err());
    }
    #[test]
    fn media_floor_survives_delete_and_history_restore_while_plain_v4_stays_readable() {
        let mut engine = LoroCollabEngine::new_empty(810);
        engine.initialize_page_editor().unwrap();
        engine.initialize_page_structure().unwrap();
        assert_eq!(engine.minimum_editor_model().unwrap(), 4);
        let old = engine.frontier();
        let v4 = engine.legacy_package_semantic_snapshot_v4().unwrap();
        let cid=engine.insert_page_atom("asset",None,1,"image",&serde_json::json!({"asset_id":"00000000-0000-4000-8000-000000000001","alt":"synthetic","caption":"","indent":0})).unwrap();
        assert_eq!(engine.minimum_editor_model().unwrap(), 5);
        assert!(engine.legacy_package_semantic_snapshot_v4().is_err());
        engine
            .apply_content_operation(&Operation::DeleteNode { id: cid.into() })
            .unwrap();
        assert_eq!(engine.minimum_editor_model().unwrap(), 5);
        engine.revert_to_frontier(&old).unwrap();
        assert_eq!(engine.minimum_editor_model().unwrap(), 5);
        assert!(engine.legacy_package_semantic_snapshot_v4().is_err());
        let restored = LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap();
        assert_eq!(restored.minimum_editor_model().unwrap(), 5);
        let plain = LoroCollabEngine::load(&engine.fork_at_frontier(&old).unwrap().export_snapshot().unwrap()).unwrap();
        assert_eq!(plain.legacy_package_semantic_snapshot_v4().unwrap(), v4);
    }
}
