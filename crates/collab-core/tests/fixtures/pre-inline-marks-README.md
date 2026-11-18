# Pre-inline-marks package fingerprint fixture

Generated from the **unmodified** `crates/collab-core` source at commit
`d523d7c724b4122f7b83cb30e6614a36f51a1c65`, compiled as `collab_core_legacy`.
`generate-pre-inline-marks.rs` is the generator. It creates an API tree block and a
ProseMirror paragraph in the `doc` container using the pre-change editor schema, then loads
that binary snapshot through the old engine and writes its exact canonical JSON and hash.
The snapshot contains both texts. The old semantic projection intentionally contains only
`Legacy API text`, demonstrating why accepting only the modern hash rejects old packages.

This fixture is synthetic and contains no user data. It is not evidence of an old browser
session or a live database export. The API package tests wrap this old snapshot and hash in
an archive to exercise strict compatibility, with archive member hashes still verified.
