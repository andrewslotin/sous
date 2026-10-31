//! Database setup: a writable connection for migrations, a read-only pool for requests.
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::path::Path;

fn options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .foreign_keys(true)
}

/// Apply migrations using a dedicated writable connection, then close it.
pub async fn migrate(path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            options(path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;
    let result = sqlx::migrate!().run(&pool).await;
    pool.close().await;
    result?;
    Ok(())
}

/// Open the dedicated writable pool used only by the MCP `save_week` tool.
pub async fn open_write_pool(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            options(path)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_secs(5)),
        )
        .await
}

/// Open the read-only pool used by request handlers.
pub async fn open_read_pool(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options(path).read_only(true))
        .await
}
