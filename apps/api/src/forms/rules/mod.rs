//! The field rule engine of form schema v2 (contract C2): the write plan of a schema, the rule
//! pipeline every writer runs, the plugin rule hook and the error report.

pub mod external;
mod ops;
mod pipeline;
mod plan;
mod report;

pub use pipeline::{
    PipelineInput, PipelineOutput, RulePolicy, WriteContext, apply_defaults, ensure_queryable,
    ensure_read_only_untouched, read_only_errors, run,
};
pub use plan::{SCHEMA_VERSION, SchemaScope, check_schema, write_plan};
pub use report::{
    CarriedError, FieldError, FieldErrors, SCHEMA_INVALID, SchemaIssue, SchemaIssues, Stage, VALIDATION_FAILED,
    carried_summary,
};

#[cfg(test)]
mod tests;
