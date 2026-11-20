//! OAuth 2.1 authorization server for the Sylvode API and MCP server.
//!
//! This module is the shared foundation the endpoint, delegated-access, event-stream, MCP and
//! management packages build on: secrets, the scope vocabulary, typed errors, access-token
//! validation, grant revocation, audit records, rate limiting and the metadata documents.

pub mod audit;
pub mod error;
pub mod grants;
pub mod metadata;
pub mod rate;
pub mod router;
pub mod scopes;
pub mod tokens;
pub mod validate;

