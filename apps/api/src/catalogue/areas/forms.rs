//! Catalogue entries of the `forms` router area (`router/forms.rs`).

use crate::catalogue::types::dsl::{
    Admin, Delete, Destructive, Entry, Find, Get, Global, Patch, Post, Project, Read, Secret, Write, bot,
};

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("forms.attachments.create", Post, "/api/v1/forms/{form_id}/attachments", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.attachments.delete", Delete, "/api/v1/form-attachments/{attachment_id}", Project(Find::Table("form_attachments","attachment_id")), Destructive),
    bot("forms.attachments.export/package", Get, "/api/v1/forms/{form_id}/attachments/package", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.attachments.export/package_job", Get, "/api/v1/form-attachment-package-jobs/{job_id}", Project(Find::Table("form_attachment_package_jobs","job_id")), Write),
    bot("forms.attachments.export/package_job_download", Get, "/api/v1/form-attachment-package-jobs/{job_id}/download", Project(Find::Table("form_attachment_package_jobs","job_id")), Write),
    bot("forms.attachments.export/package_jobs", Get, "/api/v1/forms/{form_id}/attachments/package-jobs", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.attachments.export/package_jobs_create", Post, "/api/v1/forms/{form_id}/attachments/package-jobs", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.attachments.read", Get, "/api/v1/form-attachments/{attachment_id}", Project(Find::Table("form_attachments","attachment_id")), Read),
    bot("forms.attachments.read/list", Get, "/api/v1/forms/{form_id}/attachments", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.attachments.restore", Post, "/api/v1/form-attachments/{attachment_id}/restore", Project(Find::Table("form_attachments","attachment_id")), Write),
    bot("forms.attachments.sign", Get, "/api/v1/form-attachments/{attachment_id}/signed-url", Project(Find::Table("form_attachments","attachment_id")), Secret),
    bot("forms.exports.export", Get, "/api/v1/forms/{form_id}/records/export", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.exports.export/jobs", Post, "/api/v1/forms/{form_id}/records/export-jobs", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.exports.read/job", Get, "/api/v1/form-export-jobs/{job_id}", Project(Find::Table("form_export_jobs","job_id")), Read),
    bot("forms.exports.read/jobs", Get, "/api/v1/forms/{form_id}/records/export-jobs", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.field_types.read", Get, "/api/v1/field-types", Global, Read),
    bot("forms.forms.create", Post, "/api/v1/projects/{project_id}/forms", Project(Find::Project("project_id")), Write),
    bot("forms.forms.create/duplicate", Post, "/api/v1/forms/{form_id}/duplicate", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.forms.delete", Delete, "/api/v1/forms/{form_id}", Project(Find::Table("project_forms","form_id")), Destructive),
    bot("forms.forms.read", Get, "/api/v1/forms/{form_id}", Project(Find::Table("project_forms","form_id")), Read).oauth().plugin(),
    bot("forms.forms.read/events", Get, "/api/v1/forms/{form_id}/events", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.forms.read/field_dependencies", Get, "/api/v1/forms/{form_id}/field-dependencies", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.forms.read/field_usage", Get, "/api/v1/forms/{form_id}/field-usage", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.forms.read/list", Get, "/api/v1/projects/{project_id}/forms", Project(Find::Project("project_id")), Read).oauth(),
    bot("forms.forms.read/relation_targets", Get, "/api/v1/forms/{form_id}/relation-targets", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.forms.read/rules_validate", Post, "/api/v1/forms/rules/validate", Project(Find::Body), Read),
    bot("forms.forms.read/schema_summary", Get, "/api/v1/forms/{form_id}/schema-summary", Project(Find::Table("project_forms","form_id")), Read).plugin(),
    bot("forms.forms.read/schema_validate", Post, "/api/v1/forms/schema/validate", Project(Find::Body), Read),
    bot("forms.forms.read/schema_version", Get, "/api/v1/forms/{form_id}/schema-versions/{version}", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.forms.read/schema_versions", Get, "/api/v1/forms/{form_id}/schema-versions", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.forms.restore", Post, "/api/v1/forms/{form_id}/restore", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.forms.update", Patch, "/api/v1/forms/{form_id}", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.imports.create/file", Post, "/api/v1/forms/{form_id}/records/import-file", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.imports.create/jobs", Post, "/api/v1/forms/{form_id}/records/import-jobs", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.imports.create/mapping_template", Post, "/api/v1/forms/{form_id}/import-mapping-templates", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.imports.create/records", Post, "/api/v1/forms/{form_id}/records/import", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.imports.delete/mapping_template", Delete, "/api/v1/form-import-mapping-templates/{template_id}", Project(Find::Table("form_import_mapping_templates","template_id")), Destructive),
    bot("forms.imports.read/file_preview", Post, "/api/v1/forms/{form_id}/records/import-file-preview", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.imports.read/job", Get, "/api/v1/form-import-jobs/{job_id}", Project(Find::Table("form_import_jobs","job_id")), Read),
    bot("forms.imports.read/jobs", Get, "/api/v1/forms/{form_id}/records/import-jobs", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.imports.read/mapping_templates", Get, "/api/v1/forms/{form_id}/import-mapping-templates", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.imports.read/preview", Post, "/api/v1/forms/{form_id}/records/import-preview", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.permissions.read", Get, "/api/v1/forms/{form_id}/permissions", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.permissions.update", Patch, "/api/v1/forms/{form_id}/permissions", Project(Find::Table("project_forms","form_id")), Admin),
    bot("forms.record_comments.create", Post, "/api/v1/form-records/{record_id}/comments", Project(Find::Table("form_records","record_id")), Write).comment().plugin(),
    bot("forms.record_comments.read", Get, "/api/v1/form-records/{record_id}/comments", Project(Find::Table("form_records","record_id")), Read).plugin(),
    bot("forms.records.create", Post, "/api/v1/forms/{form_id}/records", Project(Find::Table("project_forms","form_id")), Write).oauth().plugin(),
    bot("forms.records.delete", Delete, "/api/v1/form-records/{record_id}", Project(Find::Table("form_records","record_id")), Destructive),
    bot("forms.records.delete/child", Delete, "/api/v1/form-records/{record_id}/children/{child_record_id}", Project(Find::Table("form_records","record_id")), Destructive),
    bot("forms.records.read", Get, "/api/v1/form-records/{record_id}", Project(Find::Table("form_records","record_id")), Read).oauth().plugin(),
    bot("forms.records.read/aggregate", Get, "/api/v1/forms/{form_id}/aggregate", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.records.read/children", Get, "/api/v1/form-records/{record_id}/children", Project(Find::Table("form_records","record_id")), Read),
    bot("forms.records.read/events", Get, "/api/v1/form-records/{record_id}/events", Project(Find::Table("form_records","record_id")), Read),
    bot("forms.records.read/links", Get, "/api/v1/form-records/{record_id}/links", Project(Find::Table("form_records","record_id")), Read),
    bot("forms.records.read/list", Get, "/api/v1/forms/{form_id}/records", Project(Find::Table("project_forms","form_id")), Read).oauth().plugin(),
    bot("forms.records.read/recalculate_preview", Post, "/api/v1/forms/{form_id}/records/recalculate-preview", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.records.read/signature_audit", Get, "/api/v1/form-records/{record_id}/signatures/audit-verification", Project(Find::Table("form_records","record_id")), Read),
    bot("forms.records.read/validate", Post, "/api/v1/forms/{form_id}/validate", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.records.recalculate", Post, "/api/v1/form-records/{record_id}/recalculate", Project(Find::Table("form_records","record_id")), Write),
    bot("forms.records.restore", Post, "/api/v1/form-records/{record_id}/restore", Project(Find::Table("form_records","record_id")), Write),
    bot("forms.records.restore/child", Post, "/api/v1/form-records/{record_id}/children/{child_record_id}/restore", Project(Find::Table("form_records","record_id")), Write),
    bot("forms.records.update", Patch, "/api/v1/form-records/{record_id}", Project(Find::Table("form_records","record_id")), Write).oauth().plugin(),
    bot("forms.records.update/child", Patch, "/api/v1/form-records/{record_id}/children/{child_record_id}", Project(Find::Table("form_records","record_id")), Write),
    bot("forms.records.update/children_add", Post, "/api/v1/form-records/{record_id}/children", Project(Find::Table("form_records","record_id")), Write),
    bot("forms.records.update/links", Post, "/api/v1/form-records/{record_id}/links", Project(Find::Table("form_records","record_id")), Write),
    bot("forms.views.create", Post, "/api/v1/forms/{form_id}/views", Project(Find::Table("project_forms","form_id")), Write),
    bot("forms.views.delete", Delete, "/api/v1/form-views/{view_id}", Project(Find::Table("form_views","view_id")), Destructive),
    bot("forms.views.read", Get, "/api/v1/forms/{form_id}/views", Project(Find::Table("project_forms","form_id")), Read),
    bot("forms.views.update", Patch, "/api/v1/form-views/{view_id}", Project(Find::Table("form_views","view_id")), Write),
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
            BTreeMap::from([("no project", 1), ("project-resolved", 68)]),
            "the classes of the forms routes changed"
        );
    }
}
