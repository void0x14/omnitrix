use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::traits::{BlobStore, StorageError};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobMeta {
    uncompressed_size: u64,
    compressed: bool,
}

#[derive(Debug, Clone)]
pub struct GcReport {
    pub removed_count: usize,
    pub freed_bytes: u64,
    pub remaining_count: usize,
}

#[derive(Clone)]
pub struct CasBlobStore {
    base_path: PathBuf,
    compression_level: i32,
    max_blob_age: Option<chrono::Duration>,
    max_total_size: Option<u64>,
}

fn hash_to_path(hash: &str, base: &Path) -> PathBuf {
    let (prefix, rest) = hash.split_at(2.min(hash.len()));
    let (mid, file) = if rest.len() > 2 {
        rest.split_at(2)
    } else {
        (rest, "")
    };
    let mut dir = base.join(prefix);
    if !mid.is_empty() {
        dir = dir.join(mid);
    }
    let name = if file.is_empty() { prefix } else { file };
    dir.join(format!("{}.zst", name))
}

fn meta_path(blob_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.meta", blob_path.display()))
}

fn collect_blob_paths(base: &Path) -> Result<Vec<PathBuf>, StorageError> {
    let mut paths = Vec::new();
    collect_blob_paths_recursive(base, &mut paths)?;
    Ok(paths)
}

fn collect_blob_paths_recursive(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), StorageError> {
    if !dir.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(dir)
        .map_err(|e| StorageError::Internal(format!("CAS read_dir {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| StorageError::Internal(format!("CAS entry: {e}")))?;
        let path = entry.path();
        if path.is_dir() {
            collect_blob_paths_recursive(&path, paths)?;
        } else if path.extension().is_some_and(|ext| ext == "zst") {
            paths.push(path);
        }
    }
    Ok(())
}

impl CasBlobStore {
    pub fn new(base_path: &Path) -> Result<Self, StorageError> {
        fs::create_dir_all(base_path)
            .map_err(|e| StorageError::Internal(format!("Failed to create CAS base: {e}")))?;
        Ok(Self {
            base_path: base_path.to_path_buf(),
            compression_level: 3,
            max_blob_age: None,
            max_total_size: None,
        })
    }

    pub fn with_compression_level(mut self, level: i32) -> Self {
        self.compression_level = level.clamp(0, 22);
        self
    }

    pub fn with_max_blob_age(mut self, age: chrono::Duration) -> Self {
        self.max_blob_age = Some(age);
        self
    }

    pub fn with_max_total_size(mut self, size: u64) -> Self {
        self.max_total_size = Some(size);
        self
    }

    pub fn store(&self, data: &[u8], compress: bool) -> Result<String, StorageError> {
        let hash = blake3::hash(data).to_hex().to_string();
        let path = hash_to_path(&hash, &self.base_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| StorageError::Internal(format!("CAS mkdir: {e}")))?;
        }

        let (blob_data, compressed_flag) = if compress {
            let compressed = zstd::encode_all(data, self.compression_level)
                .map_err(|e| StorageError::Internal(format!("Zstd compress: {e}")))?;
            (compressed, true)
        } else {
            (data.to_vec(), false)
        };

        let meta = BlobMeta {
            uncompressed_size: data.len() as u64,
            compressed: compressed_flag,
        };
        let meta_json = serde_json::to_vec(&meta)
            .map_err(|e| StorageError::Internal(format!("CAS meta serialize: {e}")))?;

        let mpath = meta_path(&path);
        let mut mf = fs::File::create(&mpath)
            .map_err(|e| StorageError::Internal(format!("CAS meta create: {e}")))?;
        mf.write_all(&meta_json)
            .map_err(|e| StorageError::Internal(format!("CAS meta write: {e}")))?;

        let mut f = fs::File::create(&path)
            .map_err(|e| StorageError::Internal(format!("CAS create: {e}")))?;
        f.write_all(&blob_data)
            .map_err(|e| StorageError::Internal(format!("CAS write: {e}")))?;

        Ok(hash)
    }

    pub fn load(&self, hash: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let path = hash_to_path(hash, &self.base_path);
        if !path.exists() {
            return Ok(None);
        }
        let mut f = fs::File::open(&path)
            .map_err(|e| StorageError::Internal(format!("CAS open: {e}")))?;
        let mut raw = Vec::new();
        f.read_to_end(&mut raw)
            .map_err(|e| StorageError::Internal(format!("CAS read: {e}")))?;

        let mpath = meta_path(&path);
        let compressed = if mpath.exists() {
            let mut mf = fs::File::open(&mpath)
                .map_err(|e| StorageError::Internal(format!("CAS meta open: {e}")))?;
            let mut meta_json = Vec::new();
            mf.read_to_end(&mut meta_json)
                .map_err(|e| StorageError::Internal(format!("CAS meta read: {e}")))?;
            let meta: BlobMeta = serde_json::from_slice(&meta_json)
                .map_err(|e| StorageError::Internal(format!("CAS meta deserialize: {e}")))?;
            meta.compressed
        } else {
            true
        };

        if compressed {
            let data = zstd::decode_all(&raw[..])
                .map_err(|e| StorageError::Internal(format!("Zstd decompress: {e}")))?;
            Ok(Some(data))
        } else {
            Ok(Some(raw))
        }
    }

    pub fn delete(&self, hash: &str) -> Result<(), StorageError> {
        let path = hash_to_path(hash, &self.base_path);
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| StorageError::Internal(format!("CAS delete: {e}")))?;
        }
        let mpath = meta_path(&path);
        if mpath.exists() {
            let _ = fs::remove_file(&mpath);
        }
        Ok(())
    }

    pub fn gc(&self) -> Result<GcReport, StorageError> {
        let max_age = match self.max_blob_age {
            Some(age) => age,
            None => {
                return Ok(GcReport {
                    removed_count: 0,
                    freed_bytes: 0,
                    remaining_count: self.blob_count()?,
                });
            }
        };

        let paths = collect_blob_paths(&self.base_path)?;
        let now = SystemTime::now();
        let cutoff = now.checked_sub(max_age.to_std().map_err(
            |e| StorageError::Internal(format!("CAS gc: duration convert: {e}")),
        )?)
        .ok_or_else(|| StorageError::Internal("CAS gc: time overflow".into()))?;

        let mut removed_count = 0usize;
        let mut freed_bytes = 0u64;

        for path in &paths {
            let modified = fs::metadata(path)
                .and_then(|m| m.modified())
                .ok();
            let should_remove = modified.is_some_and(|t| t < cutoff);
            if should_remove {
                if let Ok(meta) = fs::metadata(path) {
                    freed_bytes += meta.len();
                }
                let _ = fs::remove_file(path);
                let mpath = meta_path(path);
                let _ = fs::remove_file(&mpath);
                removed_count += 1;
            }
        }

        let remaining_count = self.blob_count()?;

        Ok(GcReport {
            removed_count,
            freed_bytes,
            remaining_count,
        })
    }

    pub fn total_size(&self) -> Result<u64, StorageError> {
        let paths = collect_blob_paths(&self.base_path)?;
        let mut total = 0u64;
        for path in &paths {
            if let Ok(meta) = fs::metadata(path) {
                total += meta.len();
            }
        }
        Ok(total)
    }

    pub fn blob_count(&self) -> Result<usize, StorageError> {
        let paths = collect_blob_paths(&self.base_path)?;
        Ok(paths.len())
    }
}

#[async_trait]
impl BlobStore for CasBlobStore {
    async fn put(&self, data: &[u8]) -> Result<String, StorageError> {
        let this = self.clone();
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || this.store(&data, true))
            .await
            .map_err(|e| StorageError::Internal(format!("CAS spawn: {e}")))?
    }

    async fn get(&self, hash: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let this = self.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || this.load(&hash))
            .await
            .map_err(|e| StorageError::Internal(format!("CAS spawn: {e}")))?
    }

    async fn delete(&self, hash: &str) -> Result<(), StorageError> {
        let this = self.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || this.delete(&hash))
            .await
            .map_err(|e| StorageError::Internal(format!("CAS spawn: {e}")))?
    }
}
