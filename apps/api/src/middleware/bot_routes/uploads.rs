//! Runner bot route classes of the `uploads` router area (`router/uploads.rs`).

use super::{RunnerRoute, r};
use crate::middleware::runner_bot_scope::Class;

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("POST", "/api/v1/uploads/signatures/{file_name}/signed-url", Class::Refused),
];

#[cfg(test)]
mod tests {
    use super::ROUTES;
    use crate::middleware::bot_routes::class_counts;
    use std::collections::BTreeMap;

    /// The count per class is part of the review of a change to this area's table.
    #[test]
    fn the_classes_of_the_area() {
        assert_eq!(
            class_counts(ROUTES),
            BTreeMap::from([("refused", 1)]),
            "the classes of the uploads routes changed"
        );
    }
}
