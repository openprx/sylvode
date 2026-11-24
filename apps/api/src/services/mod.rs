pub mod ai_task_reply;
pub mod ai_task_service;
pub mod ai_task_view;
pub mod bot_connection_status;
pub mod bot_connections;
pub mod bot_identity;
pub mod bots;
pub mod comments;
pub mod email_text;
pub mod execution_settings;
pub mod governance_audit_service;
pub mod impact_review_service;
pub mod issue_view;
pub mod issues;
pub mod mentions;
pub mod notifications;
pub mod object_storage;
pub mod permission_service;
pub mod proposals;
pub mod runner_config_block;
pub mod sprint_moves;
pub mod teardown;
pub mod trust_score_service;
pub mod veto_service;
pub mod webhooks;
pub mod work_item_service;
pub mod workflow_service;

#[cfg(test)]
pub(crate) mod bot_connection_tests;

#[cfg(test)]
mod ai_task_chain_tests;
#[cfg(test)]
mod connection_epoch_tests;
#[cfg(test)]
mod connection_gate_tests;
#[cfg(test)]
mod connection_missing_tests;
#[cfg(test)]
mod connection_teardown_tests;

#[cfg(test)]
mod managed_rows_tests;

#[cfg(test)]
mod multi_connection_migration_tests;
#[cfg(test)]
mod multi_connection_tests;
#[cfg(test)]
mod runner_protocol_migration_tests;

#[cfg(test)]
pub mod ai_task_fixture;
