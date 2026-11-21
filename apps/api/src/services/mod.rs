pub mod ai_task_reply;
pub mod ai_task_service;
pub mod ai_task_view;
pub mod bot_connection_status;
pub mod bot_connections;
pub mod bot_identity;
pub mod bots;
pub mod comments;
pub mod execution_settings;
pub mod governance_audit_service;
pub mod impact_review_service;
pub mod issues;
pub mod object_storage;
pub mod permission_service;
pub mod proposals;
pub mod runner_config_block;
pub mod teardown;
pub mod trust_score_service;
pub mod veto_service;
pub mod webhooks;
pub mod work_item_service;
pub mod workflow_service;

#[cfg(test)]
mod bot_connection_tests;

#[cfg(test)]
mod connection_gate_tests;
#[cfg(test)]
mod connection_teardown_tests;

#[cfg(test)]
mod managed_rows_tests;

#[cfg(test)]
mod runner_protocol_migration_tests;

#[cfg(test)]
pub mod ai_task_fixture;
