//! Catalogue entries of the `uploads` router area (`router/uploads.rs`).

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    console("forms.attachments.read/download", Get, "/api/v1/form-attachments/{attachment_id}/download", Public, Read),
    console("uploads.files.read", Get, "/uploads/{file_name}", Public, Read),
    console("uploads.files.read/api", Get, "/api/v1/uploads/{file_name}", Public, Read),
    console("uploads.files.read/api_previews", Get, "/api/v1/uploads/previews/{file_name}", Public, Read),
    console("uploads.files.read/api_signature_download", Get, "/api/v1/uploads/signatures/{file_name}/download", Public, Read),
    console("uploads.files.read/api_signatures", Get, "/api/v1/uploads/signatures/{file_name}", Public, Read),
    console("uploads.files.read/api_thumbnails", Get, "/api/v1/uploads/thumbnails/{file_name}", Public, Read),
    console("uploads.files.read/api_variants", Get, "/api/v1/uploads/variants/{file_name}", Public, Read),
    console("uploads.files.read/previews", Get, "/uploads/previews/{file_name}", Public, Read),
    console("uploads.files.read/signatures", Get, "/uploads/signatures/{file_name}", Public, Read),
    console("uploads.files.read/thumbnails", Get, "/uploads/thumbnails/{file_name}", Public, Read),
    console("uploads.files.read/variants", Get, "/uploads/variants/{file_name}", Public, Read),
    bot("uploads.signatures.sign", Post, "/api/v1/uploads/signatures/{file_name}/signed-url", Workspace, Secret),
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
            BTreeMap::from([("refused", 1)]),
            "the classes of the uploads routes changed"
        );
    }
}
