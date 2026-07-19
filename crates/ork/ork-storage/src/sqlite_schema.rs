use std::path::Path;

use rusqlite::Connection;

use crate::traits::StorageError;

const MIGRATION_0001_SQL: &str = include_str!("../migrations/0001_providers.sql");
const MIGRATION_0002_SQL: &str = include_str!("../migrations/0002_models_health.sql");
const MIGRATION_0003_SQL: &str = include_str!("../migrations/0003_tasks_agents.sql");
const MIGRATION_0004_SQL: &str = include_str!("../migrations/0004_tool_audit.sql");
const MIGRATION_0005_SQL: &str = include_str!("../migrations/0005_penalty_trust.sql");
const MIGRATION_0006_SQL: &str = include_str!("../migrations/0006_recordings.sql");
const MIGRATION_0007_SQL: &str = include_str!("../migrations/0007_backups_journal.sql");

const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "providers", MIGRATION_0001_SQL),
    (2, "models_health", MIGRATION_0002_SQL),
    (3, "tasks_agents", MIGRATION_0003_SQL),
    (4, "tool_audit", MIGRATION_0004_SQL),
    (5, "penalty_trust", MIGRATION_0005_SQL),
    (6, "recordings", MIGRATION_0006_SQL),
    (7, "backups_journal", MIGRATION_0007_SQL),
];

fn migration_dir() -> Option<std::path::PathBuf> {
    let candidates: &[fn() -> Option<std::path::PathBuf>] = &[
        || {
            let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../migrations");
            p.exists().then_some(p)
        },
        || {
            let p: std::path::PathBuf = "migrations".into();
            p.exists().then_some(p)
        },
    ];
    candidates.iter().find_map(|f| f())
}

fn load_migration_sql(version: i64, name: &str) -> Result<String, StorageError> {
    if let Some(dir) = migration_dir() {
        let file_name = format!("{:04}_ork_{}.sql", version, name);
        let path = dir.join(&file_name);
        if path.exists() {
            return std::fs::read_to_string(&path)
                .map_err(|e| StorageError::Internal(format!("Read {path:?}: {e}")));
        }
    }
    let (_, _, sql) = MIGRATIONS
        .iter()
        .find(|(v, n, _)| *v == version && *n == name)
        .ok_or_else(|| StorageError::Internal(format!("Embedded SQL not found for {version} {name}")))?;
    Ok((*sql).to_string())
}

pub struct SchemaManager {
    conn: Connection,
}

impl SchemaManager {
    pub fn new(path: &Path) -> Result<Self, StorageError> {
        let conn =
            Connection::open(path).map_err(|e| StorageError::Internal(format!("SQLite open: {e}")))?;

        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| StorageError::Internal(format!("pragma journal_mode: {e}")))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| StorageError::Internal(format!("pragma foreign_keys: {e}")))?;

        Ok(Self { conn })
    }

    fn schema_version(&self) -> Result<i64, StorageError> {
        let exists: bool = self
            .conn
            .prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='schema_version'")
            .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
            .map(|n| n > 0)
            .unwrap_or(false);

        if !exists {
            return Ok(0);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .map_err(|e| StorageError::Internal(e.to_string()))?;
        let version: i64 = stmt.query_row([], |r| r.get(0)).unwrap_or(0);
        Ok(version)
    }

    fn ensure_schema_version_table(&self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        ).map_err(|e| StorageError::Internal(e.to_string()))?;
        Ok(())
    }

    pub fn run_migrations(&self) -> Result<(), StorageError> {
        self.ensure_schema_version_table()?;

        let current = self.schema_version()?;
        tracing::info!(current, "Running migrations");

        let mut applied = 0u32;
        for &(version, name, embedded_sql) in MIGRATIONS {
            if version <= current {
                continue;
            }

            let sql = match load_migration_sql(version, name) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(version, name, error=%e, "Falling back to embedded SQL");
                    embedded_sql.to_string()
                }
            };

            self.conn
                .execute_batch(&sql)
                .map_err(|e| StorageError::Internal(format!(
                    "Migration {version} ({name}) failed: {e}"
                )))?;

            self.conn
                .execute(
                    "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
                    rusqlite::params![version, name],
                )
                .map_err(|e| StorageError::Internal(format!("Version insert failed: {e}")))?;

            applied += 1;
            tracing::info!(version, name, "Migration applied");
        }

        tracing::info!(applied, "Migrations complete");
        Ok(())
    }

    pub fn migration_0001_providers(&self) -> Result<(), StorageError> {
        self.apply_migration(1, "providers", MIGRATION_0001_SQL)
    }

    pub fn migration_0002_models_health(&self) -> Result<(), StorageError> {
        self.apply_migration(2, "models_health", MIGRATION_0002_SQL)
    }

    pub fn migration_0003_tasks_agents(&self) -> Result<(), StorageError> {
        self.apply_migration(3, "tasks_agents", MIGRATION_0003_SQL)
    }

    pub fn migration_0004_tool_audit(&self) -> Result<(), StorageError> {
        self.apply_migration(4, "tool_audit", MIGRATION_0004_SQL)
    }

    pub fn migration_0005_penalty_trust(&self) -> Result<(), StorageError> {
        self.apply_migration(5, "penalty_trust", MIGRATION_0005_SQL)
    }

    pub fn migration_0006_recordings(&self) -> Result<(), StorageError> {
        self.apply_migration(6, "recordings", MIGRATION_0006_SQL)
    }

    pub fn migration_0007_backups_journal(&self) -> Result<(), StorageError> {
        self.apply_migration(7, "backups_journal", MIGRATION_0007_SQL)
    }

    fn apply_migration(
        &self,
        version: i64,
        name: &str,
        sql: &str,
    ) -> Result<(), StorageError> {
        if self.schema_version()? >= version {
            return Ok(());
        }
        self.ensure_schema_version_table()?;

        self.conn
            .execute_batch(sql)
            .map_err(|e| StorageError::Internal(format!(
                "Migration {version} ({name}) failed: {e}"
            )))?;

        self.conn
            .execute(
                "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
                rusqlite::params![version, name],
            )
            .map_err(|e| StorageError::Internal(format!("Version insert failed: {e}")))?;

        tracing::info!(version, name, "Migration applied");
        Ok(())
    }
}
