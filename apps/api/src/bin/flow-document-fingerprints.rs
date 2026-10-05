//! Emits stable JSON fingerprints for every Flow document in one database.

use std::io::Write as _;

use api::flow::collab::integrity::all_document_fingerprints;
use sea_orm::Database;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let database_url =
        std::env::var("OPENPR_DATABASE_URL").map_err(|_| anyhow::anyhow!("OPENPR_DATABASE_URL is required"))?;
    let db = Database::connect(database_url).await?;
    let fingerprints = all_document_fingerprints(&db)
        .await
        .map_err(|error| anyhow::anyhow!("document fingerprint verification failed: {error}"))?;
    // A full or closed stdout is an error the process exits 1 with, never a `println!` panic.
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", serde_json::to_string(&fingerprints)?)?;
    stdout.flush()?;
    db.close().await?;
    Ok(())
}
