//! The root containers of a Flow page document that carry a fixed meaning, and the one container
//! type each of them is allowed to have.
//!
//! A Loro root container is identified by its name *and* its type, so a document can hold several
//! roots with one name. Every reader of a reserved root asks for the expected type
//! (`editor_structure::root_container`), and the client write path refuses an update that creates
//! a reserved root of any other type (`floor_guard::reserved_root_violation`). Both use this table
//! and nothing else, so they cannot disagree about which name means which type.
use loro::ContainerType;

/// The block document root (a map with `children`).
pub const DOC: &str = "doc";
/// The structured-page placement tree.
pub const TREE: &str = "flow_editor_structure";
/// The structured-page metadata map (`model`, `version`, `initialized`).
pub const META: &str = "flow_editor_structure_meta";
/// The editor alias map.
pub const ALIASES: &str = "flow_editor_aliases";
/// The navigator order map.
pub const ORDER: &str = "order";
/// The tree-only engine's node tree.
pub const ENGINE_TREE: &str = "tree";
/// The tree-only engine's node metadata map.
pub const ENGINE_META: &str = "meta";

/// The reason of the refusal of an update or document that holds a reserved root of the wrong type.
pub const SHADOW_ROOT_REASON: &str = "reserved_root_container_type";
/// The reason of the refusal of an update or document whose ops could not all be inspected.
pub const SCAN_INCOMPLETE_REASON: &str = "reserved_root_scan_incomplete";

/// Every reserved root name with its only allowed container type.
pub const RESERVED_ROOTS: [(&str, ContainerType); 7] = [
    (DOC, ContainerType::Map),
    (TREE, ContainerType::Tree),
    (META, ContainerType::Map),
    (ALIASES, ContainerType::Map),
    (ORDER, ContainerType::Map),
    (ENGINE_TREE, ContainerType::Tree),
    (ENGINE_META, ContainerType::Map),
];

/// The container type the reserved root `name` must have, or `None` when `name` is not reserved.
#[must_use]
pub fn expected_type(name: &str) -> Option<ContainerType> {
    RESERVED_ROOTS
        .iter()
        .find_map(|(reserved, kind)| (*reserved == name).then_some(*kind))
}

/// Whether a root container `name` of type `kind` is a reserved name with the wrong type.
#[must_use]
pub fn is_shadow_root(name: &str, kind: ContainerType) -> bool {
    expected_type(name).is_some_and(|expected| expected != kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_looked_up_by_exact_name() {
        for (index, (name, kind)) in RESERVED_ROOTS.iter().enumerate() {
            assert_eq!(expected_type(name), Some(*kind));
            assert!(!is_shadow_root(name, *kind));
            assert!(RESERVED_ROOTS.iter().skip(index + 1).all(|(other, _)| other != name));
        }
        assert_eq!(expected_type("Doc"), None);
        assert_eq!(expected_type(""), None);
        assert!(is_shadow_root(DOC, ContainerType::Text));
        assert!(!is_shadow_root("free", ContainerType::Text));
    }
}
