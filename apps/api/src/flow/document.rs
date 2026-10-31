//! Load the accepted head of one collaborative document into a private engine.

use collab_core::LoroCollabEngine;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::collab::bootstrap;
use super::command::map_collab_error;
use crate::error::ApiError;

pub async fn load_head_engine(db: &DatabaseConnection, document_id: Uuid) -> Result<LoroCollabEngine, ApiError> {
    let boot = bootstrap::load(db, document_id).await?;
    let decoded = boot
        .snapshot
        .len()
        .saturating_add(boot.tail_updates.iter().map(|update| update.bytes.len()).sum());
    let observed = u64::try_from(decoded).unwrap_or(u64::MAX);
    bootstrap::check_bootstrap_decoded_bytes(observed)?;
    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|err| map_collab_error(&err))?;
    for update in &boot.tail_updates {
        engine
            .import_update(&update.bytes)
            .map_err(|err| map_collab_error(&err))?;
    }
    Ok(engine)
}
