//! Catalogue entries of the `catalogue` router area (`router/catalogue.rs`).

use crate::catalogue::types::dsl::{Entry, Get, Global, Read, bot};

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("catalogue.capabilities.read", Get, "/api/v1/catalogue", Global, Read),
    bot("catalogue.openapi.read", Get, "/api/v1/openapi.json", Global, Read),
];

#[cfg(test)]
mod tests {
    use super::ENTRIES;
    use crate::catalogue::bot_class_counts;
    use std::collections::BTreeMap;

    /// The count per class is part of the review of a change to this area's table.
    #[test]
    fn the_classes_of_the_area() {
        assert_eq!(
            bot_class_counts(ENTRIES),
            BTreeMap::from([("no project", 2)]),
            "the classes of the catalogue routes changed"
        );
    }
}
