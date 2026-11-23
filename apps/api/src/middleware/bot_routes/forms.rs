//! Runner bot route classes of the `forms` router area (`router/forms.rs`).

use super::{RunnerRoute, r};
use crate::middleware::runner_bot_scope::{Class, Find};

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("GET", "/api/v1/form-attachment-package-jobs/{job_id}", Class::Project(Find::Table("form_attachment_package_jobs","job_id"))),
    r("GET", "/api/v1/form-attachment-package-jobs/{job_id}/download", Class::Project(Find::Table("form_attachment_package_jobs","job_id"))),
    r("DELETE", "/api/v1/form-attachments/{attachment_id}", Class::Project(Find::Table("form_attachments","attachment_id"))),
    r("POST", "/api/v1/form-attachments/{attachment_id}/restore", Class::Project(Find::Table("form_attachments","attachment_id"))),
    r("GET", "/api/v1/form-attachments/{attachment_id}/signed-url", Class::Project(Find::Table("form_attachments","attachment_id"))),
    r("GET", "/api/v1/form-export-jobs/{job_id}", Class::Project(Find::Table("form_export_jobs","job_id"))),
    r("GET", "/api/v1/form-import-jobs/{job_id}", Class::Project(Find::Table("form_import_jobs","job_id"))),
    r("DELETE", "/api/v1/form-import-mapping-templates/{template_id}", Class::Project(Find::Table("form_import_mapping_templates","template_id"))),
    r("DELETE", "/api/v1/form-records/{record_id}", Class::Project(Find::Table("form_records","record_id"))),
    r("GET", "/api/v1/form-records/{record_id}", Class::Project(Find::Table("form_records","record_id"))),
    r("PATCH", "/api/v1/form-records/{record_id}", Class::Project(Find::Table("form_records","record_id"))),
    r("GET", "/api/v1/form-records/{record_id}/children", Class::Project(Find::Table("form_records","record_id"))),
    r("POST", "/api/v1/form-records/{record_id}/children", Class::Project(Find::Table("form_records","record_id"))),
    r("DELETE", "/api/v1/form-records/{record_id}/children/{child_record_id}", Class::Project(Find::Table("form_records","record_id"))),
    r("PATCH", "/api/v1/form-records/{record_id}/children/{child_record_id}", Class::Project(Find::Table("form_records","record_id"))),
    r("POST", "/api/v1/form-records/{record_id}/children/{child_record_id}/restore", Class::Project(Find::Table("form_records","record_id"))),
    r("GET", "/api/v1/form-records/{record_id}/comments", Class::Project(Find::Table("form_records","record_id"))),
    r("POST", "/api/v1/form-records/{record_id}/comments", Class::Project(Find::Table("form_records","record_id"))),
    r("GET", "/api/v1/form-records/{record_id}/events", Class::Project(Find::Table("form_records","record_id"))),
    r("GET", "/api/v1/form-records/{record_id}/links", Class::Project(Find::Table("form_records","record_id"))),
    r("POST", "/api/v1/form-records/{record_id}/links", Class::Project(Find::Table("form_records","record_id"))),
    r("POST", "/api/v1/form-records/{record_id}/recalculate", Class::Project(Find::Table("form_records","record_id"))),
    r("POST", "/api/v1/form-records/{record_id}/restore", Class::Project(Find::Table("form_records","record_id"))),
    r("GET", "/api/v1/form-records/{record_id}/signatures/audit-verification", Class::Project(Find::Table("form_records","record_id"))),
    r("DELETE", "/api/v1/form-views/{view_id}", Class::Project(Find::Table("form_views","view_id"))),
    r("PATCH", "/api/v1/form-views/{view_id}", Class::Project(Find::Table("form_views","view_id"))),
    r("DELETE", "/api/v1/forms/{form_id}", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}", Class::Project(Find::Table("project_forms","form_id"))),
    r("PATCH", "/api/v1/forms/{form_id}", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/aggregate", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/attachments", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/attachments", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/attachments/package", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/attachments/package-jobs", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/attachments/package-jobs", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/duplicate", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/events", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/field-dependencies", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/field-usage", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/import-mapping-templates", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/import-mapping-templates", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/permissions", Class::Project(Find::Table("project_forms","form_id"))),
    r("PATCH", "/api/v1/forms/{form_id}/permissions", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/records", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/records", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/records/export", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/records/export-jobs", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/records/export-jobs", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/records/import", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/records/import-file", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/records/import-file-preview", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/records/import-jobs", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/records/import-jobs", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/records/import-preview", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/records/recalculate-preview", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/relation-targets", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/restore", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/schema-summary", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/schema-versions", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/schema-versions/{version}", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/forms/{form_id}/views", Class::Project(Find::Table("project_forms","form_id"))),
    r("POST", "/api/v1/forms/{form_id}/views", Class::Project(Find::Table("project_forms","form_id"))),
    r("GET", "/api/v1/projects/{project_id}/forms", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/forms", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/forms/from-template", Class::Project(Find::Project("project_id"))),
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
            BTreeMap::from([("project-resolved", 65)]),
            "the classes of the forms routes changed"
        );
    }
}
