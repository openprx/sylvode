# SeaORM source provenance

The workspace uses SeaORM 1.1.20 from [crates.io](https://crates.io/crates/sea-orm/1.1.20). The upstream source commit is `2a99e6c81b2c2c14aa68860e90f67535e10d81ff`. The SHA-256 of the upstream [crate archive](https://static.crates.io/crates/sea-orm/sea-orm-1.1.20.crate) is `2dc312fedd460a47ea563911761d254a84e7b51d8cc73ec92c929e78f33fa957`. The vendored files include the library source, normalized package manifest, README, and both upstream licenses.

The vendored copy differs from the upstream archive in two ways. The `with-json` feature no longer lists `chrono?/serde`, and `database/transaction.rs` has two added `drop(self)` statements. The upstream test targets and the generated test schema illustration are omitted.
