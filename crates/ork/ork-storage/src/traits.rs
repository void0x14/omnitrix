use async_trait::async_trait;

#[derive(Debug, Clone, thiserror::Error)]
pub enum StorageError {
    #[error("Key not found: {0}")]
    NotFound(String),
    #[error("Storage error: {0}")]
    Internal(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
}

#[async_trait]
pub trait KvStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError>;
    async fn delete(&self, key: &str) -> Result<(), StorageError>;
}

#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn put(&self, data: &[u8]) -> Result<String, StorageError>;
    async fn get(&self, hash: &str) -> Result<Option<Vec<u8>>, StorageError>;
    async fn delete(&self, hash: &str) -> Result<(), StorageError>;
}
