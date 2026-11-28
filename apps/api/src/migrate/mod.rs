//! The migration runner: ordered migration files, the ledger that records them, and the startup
//! checks that follow.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// Advisory lock namespace for the migration runner. The first key separates this lock from any
/// other advisory lock in the system, the second one names the runner within that namespace.
const MIGRATION_LOCK_NAMESPACE: i32 = 0x0F09_2026;
const MIGRATION_LOCK_ID: i32 = 1;

/// Brings the database to the schema described by the shipped migration files.
///
/// Every file the ledger does not record is executed in file name order, each in its own
/// transaction together with its ledger row, so a file that arrives out of order (a number below
/// one that is already recorded) still runs. The runner refuses to start when a recorded file
/// changed or is no longer shipped, and when a migration fails.
pub async fn run_migrations(db: &DatabaseConnection) -> anyhow::Result<()> {
    let Some((ledger_name, ledger_sql)) = MIGRATIONS.first() else {
        return Err(anyhow::anyhow!("migration list is empty, refusing to start"));
    };

    // The ledger bootstrap is idempotent by construction, so it is the one file that may run on
    // every start: nothing can be read from or written to the ledger before it exists. It still
    // takes the runner lock, because two replicas issuing `CREATE TABLE IF NOT EXISTS` at the
    // same instant can collide on the catalogue instead of one of them seeing the finished table.
    bootstrap_migration_ledger(db, ledger_name, ledger_sql)
        .await
        .map_err(|err| anyhow::anyhow!("migration ledger bootstrap failed: {err}"))?;
    record_migration_if_absent(db, ledger_name, &migration_checksum(ledger_sql)).await?;

    let ledger = load_migration_ledger(db).await?;
    let pending = plan_migrations(MIGRATIONS, &ledger)?;

    let mut applied = 0_usize;
    for (name, sql) in pending {
        if run_one_migration(db, name, sql).await? {
            applied += 1;
            tracing::info!(migration = %name, "migration applied");
        }
    }
    tracing::info!(applied, "migration run finished");
    Ok(())
}

/// Decides what a run has to execute, without touching the database.
///
/// `ledger` maps every recorded migration name to its checksum. A recorded file whose content
/// changed, or a recorded name without a shipped file, is an error: the database no longer
/// matches the build, and starting anyway would run code against a schema nobody can describe.
/// The result lists the unrecorded files in the order of `files`.
fn plan_migrations<'a>(
    files: &[(&'a str, &'a str)],
    ledger: &HashMap<String, String>,
) -> anyhow::Result<Vec<(&'a str, &'a str)>> {
    let mut pending = Vec::new();
    for (name, sql) in files {
        match ledger.get(*name) {
            Some(recorded) if *recorded == migration_checksum(sql) => {}
            Some(_) => {
                return Err(anyhow::anyhow!(
                    "migration {name} changed after it was applied; a recorded migration file must never be edited, add a new migration instead"
                ));
            }
            None => pending.push((*name, *sql)),
        }
    }
    let mut unshipped: Vec<&str> = ledger
        .keys()
        .map(String::as_str)
        .filter(|name| !files.iter().any(|(file, _)| file == name))
        .collect();
    unshipped.sort_unstable();
    if let Some(name) = unshipped.first() {
        return Err(anyhow::anyhow!("migration {name} is recorded but not shipped"));
    }
    Ok(pending)
}

/// Runs one migration under the runner lock, inside the transaction that also writes the ledger
/// row. Execution and bookkeeping are therefore one atomic step: an interrupted run leaves no
/// half claimed ledger, and a second replica re-reads the ledger under the same lock instead of
/// acting on a snapshot taken before it was blocked. Returns whether this call applied the file.
async fn run_one_migration(db: &DatabaseConnection, name: &str, sql: &str) -> anyhow::Result<bool> {
    let txn = db.begin().await?;
    lock_migration_runner(&txn).await?;

    if migration_is_recorded(&txn, name).await? {
        txn.commit().await?;
        return Ok(false);
    }

    if let Err(err) = txn.execute_unprepared(sql).await {
        if let Err(rollback_err) = txn.rollback().await {
            tracing::warn!(migration = %name, error = %rollback_err, "migration rollback failed");
        }
        tracing::error!(migration = %name, error = %err, "migration failed");
        return Err(anyhow::anyhow!("migration {name} failed: {err}"));
    }

    record_migration_with_conn(&txn, name, &migration_checksum(sql)).await?;
    txn.commit().await?;
    Ok(true)
}

async fn migration_is_recorded<C: ConnectionTrait>(conn: &C, name: &str) -> anyhow::Result<bool> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 AS present FROM schema_migrations WHERE name = $1",
            vec![name.into()],
        ))
        .await?;
    Ok(row.is_some())
}

async fn record_migration_with_conn<C: ConnectionTrait>(conn: &C, name: &str, checksum: &str) -> anyhow::Result<()> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO schema_migrations (name, checksum, status, error, applied_at)
            VALUES ($1, $2, 'applied', NULL, now())
        ",
        vec![name.into(), checksum.into()],
    ))
    .await?;
    Ok(())
}

/// Records the ledger bootstrap without touching an existing row, so `applied_at` keeps meaning
/// "when this database first got the migration" instead of "when the API last started".
async fn record_migration_if_absent(db: &DatabaseConnection, name: &str, checksum: &str) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO schema_migrations (name, checksum, status, error, applied_at)
            VALUES ($1, $2, 'applied', NULL, now())
            ON CONFLICT (name) DO NOTHING
        ",
        vec![name.into(), checksum.into()],
    ))
    .await?;
    Ok(())
}

async fn load_migration_ledger(db: &DatabaseConnection) -> anyhow::Result<HashMap<String, String>> {
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT name, checksum FROM schema_migrations".to_string(),
        ))
        .await?;

    let mut ledger = HashMap::with_capacity(rows.len());
    for row in rows {
        ledger.insert(
            row.try_get::<String>("", "name")?,
            row.try_get::<String>("", "checksum")?,
        );
    }
    Ok(ledger)
}

/// Creates the ledger table itself, under the runner lock so concurrent starts serialise.
async fn bootstrap_migration_ledger(db: &DatabaseConnection, name: &str, sql: &str) -> anyhow::Result<()> {
    let txn = db.begin().await?;
    lock_migration_runner(&txn).await?;
    if let Err(err) = txn.execute_unprepared(sql).await {
        if let Err(rollback_err) = txn.rollback().await {
            tracing::warn!(migration = %name, error = %rollback_err, "migration rollback failed");
        }
        return Err(err.into());
    }
    txn.commit().await?;
    Ok(())
}

/// Takes the transaction scoped runner lock. It is released by the commit or the rollback that
/// ends the transaction, so it can never be left behind on a pooled connection.
async fn lock_migration_runner<C: ConnectionTrait>(conn: &C) -> anyhow::Result<()> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1, $2)",
        vec![MIGRATION_LOCK_NAMESPACE.into(), MIGRATION_LOCK_ID.into()],
    ))
    .await?;
    Ok(())
}

/// Content fingerprint used to notice that a migration file changed after it was applied.
fn migration_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}

pub async fn verify_governance_schema(db: &DatabaseConnection) -> anyhow::Result<()> {
    const CHECKS: &[(&str, &str)] = &[
        ("proposals table", "SELECT 1 FROM proposals LIMIT 1"),
        ("proposal_type enum", "SELECT 'feature'::proposal_type"),
        ("proposal_status enum", "SELECT 'draft'::proposal_status"),
        ("author_type enum", "SELECT 'human'::author_type"),
        ("trust_scores table", "SELECT 1 FROM trust_scores LIMIT 1"),
        ("trust_score_logs table", "SELECT 1 FROM trust_score_logs LIMIT 1"),
        ("ai_participants table", "SELECT 1 FROM ai_participants LIMIT 1"),
        ("decision_domains table", "SELECT 1 FROM decision_domains LIMIT 1"),
        ("veto_events table", "SELECT 1 FROM veto_events LIMIT 1"),
        ("appeals table", "SELECT 1 FROM appeals LIMIT 1"),
        ("impact_reviews table", "SELECT 1 FROM impact_reviews LIMIT 1"),
        ("review_participants table", "SELECT 1 FROM review_participants LIMIT 1"),
        ("ai_learning_records table", "SELECT 1 FROM ai_learning_records LIMIT 1"),
        (
            "decision_audit_reports table",
            "SELECT 1 FROM decision_audit_reports LIMIT 1",
        ),
        ("feedback_loop_links table", "SELECT 1 FROM feedback_loop_links LIMIT 1"),
        ("governance_configs table", "SELECT 1 FROM governance_configs LIMIT 1"),
        (
            "governance_audit_logs table",
            "SELECT 1 FROM governance_audit_logs LIMIT 1",
        ),
        ("check_results table", "SELECT 1 FROM check_results LIMIT 1"),
        ("trust_level enum", "SELECT 'observer'::trust_level"),
        ("participant_type enum", "SELECT 'human'::participant_type"),
        ("veto_status enum", "SELECT 'active'::veto_status"),
        ("appeal_status enum", "SELECT 'pending'::appeal_status"),
        ("review_status enum", "SELECT 'pending'::review_status"),
        ("review_rating enum", "SELECT 'A'::review_rating"),
    ];

    for (name, sql) in CHECKS {
        if let Err(e) = db
            .query_one(Statement::from_string(DbBackend::Postgres, (*sql).to_string()))
            .await
        {
            return Err(anyhow::anyhow!("governance schema check failed for {name}: {e}"));
        }
    }

    Ok(())
}

#[cfg(test)]
// Test modules index fixtures directly, print skip reasons, and mirror wire shapes;
// panicking there is the assertion rather than a defect.
#[allow(
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::doc_markdown,
    clippy::redundant_pub_crate,
    clippy::struct_field_names,
    clippy::case_sensitive_file_extension_comparisons
)]
mod tests;

/// Migration runner behaviour against a real PostgreSQL server.
///
/// Adoption, replay and retry only differ on databases that are not empty, so a fresh compose
/// database exercises none of them. Each test here creates its own database, shapes it into the
/// state under test and runs the real runner against it.
///
/// Set `OPENPR_TEST_DATABASE_URL` to a maintenance connection string, for example
/// `postgres://user:pw@127.0.0.1:5432/postgres`. Without it these tests report that they were
/// skipped instead of pretending to pass.
#[cfg(test)]
// Test modules index fixtures directly, print skip reasons, and mirror wire shapes;
// panicking there is the assertion rather than a defect.
#[allow(
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::doc_markdown,
    clippy::redundant_pub_crate,
    clippy::struct_field_names,
    clippy::case_sensitive_file_extension_comparisons
)]
pub(crate) mod db_tests;
