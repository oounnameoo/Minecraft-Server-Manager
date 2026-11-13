use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::Utc;
use include_dir::{Dir, include_dir};
use rusqlite::{Connection, params};

static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../../migrations");

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open sqlite database {}", path.display()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .context("configure sqlite pragmas")?;

        apply_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn conn_arc(&self) -> Arc<Mutex<Connection>> {
        self.conn.clone()
    }

    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let guard = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("database mutex poisoned"))?;
        f(&guard)
    }
}

fn apply_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY, applied_at TEXT NOT NULL);",
    )
    .context("create schema_migrations table")?;

    let mut files: Vec<_> = MIGRATIONS_DIR
        .files()
        .filter(|f| f.path().extension().and_then(|e| e.to_str()) == Some("sql"))
        .collect();
    files.sort_by_key(|f| f.path().to_string_lossy().into_owned());

    for file in files {
        let name = file
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(1) FROM schema_migrations WHERE name = ?1",
                params![&name],
                |row| row.get(0),
            )
            .with_context(|| format!("check migration {name}"))?;
        if count > 0 {
            continue;
        }
        let sql = file
            .contents_utf8()
            .with_context(|| format!("read migration {name} as utf-8"))?;
        conn.execute_batch(sql)
            .with_context(|| format!("apply migration {name}"))?;
        conn.execute(
            "INSERT INTO schema_migrations (name, applied_at) VALUES (?1, ?2)",
            params![&name, Utc::now().to_rfc3339()],
        )
        .with_context(|| format!("record migration {name}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn opens_and_applies_all_migrations() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("mcm.db");
        let db = Db::open(&db_path).unwrap();
        let count: i64 = db
            .with_conn(|c| {
                Ok(c.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))?)
            })
            .unwrap();
        assert!(
            count >= 27,
            "expected >= 27 migrations applied, got {count}"
        );

        // Verify auto_start and start_command columns exist on servers
        let cols: i64 = db
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('servers') WHERE name IN ('auto_start', 'start_command')",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(cols, 2);
    }
}
