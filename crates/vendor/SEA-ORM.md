# SeaORM source provenance

The workspace uses SeaORM 1.1.20 from [crates.io](https://crates.io/crates/sea-orm/1.1.20). The upstream source commit is `2a99e6c81b2c2c14aa68860e90f67535e10d81ff`. The SHA-256 of the upstream [crate archive](https://static.crates.io/crates/sea-orm/sea-orm-1.1.20.crate) is `2dc312fedd460a47ea563911761d254a84e7b51d8cc73ec92c929e78f33fa957`. The vendored files include the library source, normalized package manifest, README, and both upstream licenses.

Sylvode uses SeaORM's `with-time` and `with-json` features for PostgreSQL. The latter supplies the actual `TryGetable` implementation used to read JSON columns. Upstream also lists `chrono?/serde` under that feature, which leaves the optional Chrono crate in the generated lockfile even when nothing compiles it. We removed that feature edge, keeping JSON column decoding and the `time` backend. Remove the patch when a compatible upstream release supports this configuration without retaining Chrono in the lockfile.

To compare against the upstream archive:

1. Verify the archive's SHA-256 and extract it.
2. Compare the upstream Rust files in `src/` to `crates/vendor/sea-orm/src/`. In `database/transaction.rs` the two added `drop(self)` statements run the existing `Drop` implementation explicitly at the end of commit and rollback. That destructor observes the closed-state flag. This prevents the pinned compiler from treating the flag assignment as unused when building a path dependency with strict warnings.
3. Compare the manifests. The local manifest removes `chrono?/serde` from `with-json` and omits the upstream test targets, which are not shipped with this library copy. The generated test schema illustration is also omitted.
4. Verify the licenses and README byte-for-byte. Run the workspace PostgreSQL tests, including commit and rollback, and `cargo audit` when updating this dependency.
