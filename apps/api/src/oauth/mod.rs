//! OAuth 2.1 authorization server for the Sylvode API and MCP server.
//!
//! This module is the shared foundation the endpoint, delegated-access, event-stream, MCP and
//! management packages build on: secrets, the scope vocabulary, typed errors, access-token
//! validation, grant revocation, audit records, rate limiting and the metadata documents.

pub mod audit;
pub mod delegated;
pub mod error;
pub mod grants;
pub mod internal;
pub mod metadata;
pub mod rate;
pub mod router;
pub mod scopes;
pub mod tokens;
pub mod validate;

#[cfg(test)]
mod db_tests;
#[cfg(test)]
mod delegated_tests;
#[cfg(test)]
mod hygiene_tests;
#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod test_support;
