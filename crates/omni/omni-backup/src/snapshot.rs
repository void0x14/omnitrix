use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::checksum;

pub type Result<T> = std::result::Result<T, SnapshotError>;

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checksum error: {0}")]
    Checksum(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotReport {
    pub id: Uuid,
    pub scope: String,
    pub size: u64,
    pub checksum: String,
    pub timestamp: DateTime<Utc>,
    pub path: String,
}

pub struct SnapshotManager;

fn cas_blob_path(cas_root: &Path, hash: &str) -> PathBuf {
    let (a, rest) = hash.split_at(2);
    let (b, c) = rest.split_at(2);
    cas_root.join(a).join(b).join(c)
}

impl SnapshotManager {
    pub fn create_snapshot(
        path: impl AsRef<Path>,
        cas_root: Option<&Path>,
    ) -> Result<SnapshotReport> {
        let path = path.as_ref();
        let id = Uuid::new_v4();
        let timestamp = Utc::now();
        let scope = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let conn = rusqlite::Connection::open(path)?;
        let snapshot_path = format!("{}.snapshot.db", path.display());

        conn.execute("VACUUM INTO ?1", [&snapshot_path])?;
        drop(conn);

        let metadata = std::fs::metadata(&snapshot_path)?;
        let size = metadata.len();
        let checksum = checksum::hash_file(&snapshot_path)?;

        if let Some(root) = cas_root {
            let blob_path = cas_blob_path(root, &checksum);
            if !blob_path.exists() {
                if let Some(parent) = blob_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(&snapshot_path, &blob_path)?;
                return Ok(SnapshotReport {
                    id,
                    scope,
                    size,
                    checksum,
                    timestamp,
                    path: blob_path.to_string_lossy().to_string(),
                });
            }
            // CAS hit — remove temp snapshot, return existing path
            let _ = fs::remove_file(&snapshot_path);
            return Ok(SnapshotReport {
                id,
                scope,
                size,
                checksum,
                timestamp,
                path: blob_path.to_string_lossy().to_string(),
            });
        }

        Ok(SnapshotReport {
            id,
            scope,
            size,
            checksum,
            timestamp,
            path: snapshot_path,
        })
    }
}
