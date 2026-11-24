# Tree block with an editor document

`tree-block-with-editor-doc.snapshot` holds an API tree block (`api-block`, with its own tree
prose) and a ProseMirror paragraph in the `doc` container. The projection makes the editor
document authoritative: the paragraph text is read and the shadow tree prose of `api-block` is
cleared. `tests/editor_authority.rs` pins that, and the snapshot is part of the corpus of
`tests/semantic_snapshot_digest.rs`.

The fixture is synthetic and contains no user data.
