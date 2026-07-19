use async_trait::async_trait;
use parking_lot::Mutex;
use std::path::Path;

use crate::traits::{KvStore, StorageError};

const TABLE: redb::TableDefinition<&[u8], &[u8]> = redb::TableDefinition::new("default");

pub struct RedbStore {
    db: Mutex<redb::Database>,
}

impl RedbStore {
    pub fn new(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StorageError::Internal(format!("Failed to create parent dir: {e}")))?;
        }
        let db = if path.exists() {
            redb::Database::open(path)
                .map_err(|e| StorageError::Internal(format!("Failed to open redb database: {e}")))?
        } else {
            redb::Database::create(path)
                .map_err(|e| StorageError::Internal(format!("Failed to create redb database: {e}")))?
        };
        Ok(Self {
            db: Mutex::new(db),
        })
    }

    pub fn checkpoint(&self) -> Result<(), StorageError> {
        let db = self.db.lock();
        let txn = db
            .begin_write()
            .map_err(|e| StorageError::Internal(format!("Checkpoint txn error: {e}")))?;
        txn.commit()
            .map_err(|e| StorageError::Internal(format!("Checkpoint commit error: {e}")))
    }

    pub fn compact(&self) -> Result<(), StorageError> {
        let mut db = self.db.lock();
        let _ = db.compact()
            .map_err(|e| StorageError::Internal(format!("Compact error: {e}")))?;
        Ok(())
    }

    pub fn insert_batch(&self, entries: &[(String, Vec<u8>)]) -> Result<(), StorageError> {
        let db = self.db.lock();
        let txn = db
            .begin_write()
            .map_err(|e| StorageError::Internal(format!("Failed to begin write txn: {e}")))?;
        {
            let mut table = txn
                .open_table(TABLE)
                .map_err(|e| StorageError::Internal(format!("Failed to open table: {e}")))?;
            for (key, value) in entries {
                table
                    .insert(key.as_bytes(), value.as_slice())
                    .map_err(|e| StorageError::Internal(format!("Batch insert error for '{key}': {e}")))?;
            }
        }
        txn.commit()
            .map_err(|e| StorageError::Internal(format!("Batch commit error: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl KvStore for RedbStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let db = self.db.lock();
        let txn = db
            .begin_read()
            .map_err(|e| StorageError::Internal(format!("Failed to begin read txn: {e}")))?;
        let table = txn
            .open_table(TABLE)
            .map_err(|e| StorageError::Internal(format!("Failed to open table: {e}")))?;
        match table.get(key.as_bytes()) {
            Ok(Some(sl)) => Ok(Some(sl.value().to_vec())),
            Ok(None) => Ok(None),
            Err(e) => Err(StorageError::Internal(format!("Read error: {e}"))),
        }
    }

    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let db = self.db.lock();
        let txn = db
            .begin_write()
            .map_err(|e| StorageError::Internal(format!("Failed to begin write txn: {e}")))?;
        {
            let mut table = txn
                .open_table(TABLE)
                .map_err(|e| StorageError::Internal(format!("Failed to open table: {e}")))?;
            table
                .insert(key.as_bytes(), value)
                .map_err(|e| StorageError::Internal(format!("Insert error: {e}")))?;
        }
        txn.commit()
            .map_err(|e| StorageError::Internal(format!("Commit error: {e}")))?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let db = self.db.lock();
        let txn = db
            .begin_write()
            .map_err(|e| StorageError::Internal(format!("Failed to begin write txn: {e}")))?;
        {
            let mut table = txn
                .open_table(TABLE)
                .map_err(|e| StorageError::Internal(format!("Failed to open table: {e}")))?;
            table
                .remove(key.as_bytes())
                .map_err(|e| StorageError::Internal(format!("Remove error: {e}")))?;
        }
        txn.commit()
            .map_err(|e| StorageError::Internal(format!("Commit error: {e}")))?;
        Ok(())
    }
}
