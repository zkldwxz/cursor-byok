//! Initializes and configures SQLite storage.
use std::{str::FromStr, time::Duration};

use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};

use crate::Result;

use super::{migrations, writer::WriteCoordinator};

/// Upper bound for the `-wal` file. The default leaves the file at its
/// high-water mark, which reached 208 MB on a real database.
const JOURNAL_SIZE_LIMIT: i64 = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct Store {
    pub(crate) pool: SqlitePool,
    pub(crate) writes: WriteCoordinator,
}

impl Store {
    pub async fn connect(database_url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            // The WAL high-water mark never shrinks on its own: a single large
            // transaction (a migration rebuild, a big checkpoint import) leaves
            // a file of that size on disk forever. Cap the file and let SQLite
            // truncate it after the next checkpoint.
            .pragma("journal_size_limit", JOURNAL_SIZE_LIMIT.to_string())
            .busy_timeout(Duration::from_secs(5));
        let database_path = options.get_filename().to_owned();
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        migrations::run(&pool, &database_path).await?;
        Ok(Self {
            pool,
            writes: WriteCoordinator::default(),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The configured WAL size cap, exposed so a test can assert the pragma
    /// actually reached SQLite.
    #[cfg(test)]
    pub(crate) const fn journal_size_limit() -> i64 {
        JOURNAL_SIZE_LIMIT
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_wal_file_is_capped_so_it_cannot_keep_its_high_water_mark() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::connect(&format!(
            "sqlite://{}",
            directory.path().join("test.db").display()
        ))
        .await
        .unwrap();
        let limit: i64 = sqlx::query_scalar("PRAGMA journal_size_limit")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(limit, Store::journal_size_limit());
        assert_eq!(limit, 32 * 1024 * 1024);
    }
}
