//! The embedded migrations, and the copy taken before any of them runs.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use sqlx::SqlitePool;
use sqlx::migrate::{Migrate, Migrator};

use crate::Result;

static MIGRATOR: Migrator = sqlx::migrate!();

/// Apply pending migrations, copying an existing database first.
pub(crate) async fn run(writer: &SqlitePool, path: &Path, now: Timestamp) -> Result<()> {
    let mut connection = writer.acquire().await?;
    connection
        .ensure_migrations_table(&MIGRATOR.table_name)
        .await?;
    let applied: Vec<i64> = connection
        .list_applied_migrations(&MIGRATOR.table_name)
        .await?
        .into_iter()
        .map(|migration| migration.version)
        .collect();
    let pending = MIGRATOR
        .iter()
        .filter(|migration| !applied.contains(&migration.version))
        .map(|migration| migration.version)
        .max();

    if let (Some(version), false) = (pending, applied.is_empty()) {
        let copy = backup_path(path, version, now);
        // `VACUUM INTO` takes a consistent copy of a live database. It is
        // not a table query, so it has no offline metadata to check.
        sqlx::query("VACUUM INTO ?")
            .bind(copy.to_string_lossy().into_owned())
            .execute(&mut *connection)
            .await?;
    }
    drop(connection);

    MIGRATOR.run(writer).await?;
    Ok(())
}

fn backup_path(path: &Path, version: i64, now: Timestamp) -> PathBuf {
    let now = now.as_millisecond();
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".before-{version}-{now}"));
    path.with_file_name(name)
}
