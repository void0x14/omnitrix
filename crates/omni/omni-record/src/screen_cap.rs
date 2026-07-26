use crate::compressor::{CompressError, ZstdCompressor};
use chrono::Utc;
use omni_storage::cas::CasBlobStore;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RecordingMeta {
    pub blob_ref: String,
    pub media_type: String,
    pub bytes: usize,
    pub codec: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Capture {
    pub blob_hash: [u8; 32],
    pub compressed_bytes: usize,
    pub original_bytes: usize,
    pub recording_meta: Option<RecordingMeta>,
}

pub struct ScreenCapture;

impl ScreenCapture {
    pub async fn capture(data: &[u8]) -> Result<Capture, CompressError> {
        let original_bytes = data.len();
        let compressed = ZstdCompressor::compress(data)?;
        let hash = blake3::hash(&compressed);
        let compressed_bytes = compressed.len();

        Ok(Capture {
            blob_hash: *hash.as_bytes(),
            compressed_bytes,
            original_bytes,
            recording_meta: None,
        })
    }

    pub async fn capture_to_cas(
        data: &[u8],
        cas: &CasBlobStore,
        media_type: &str,
        codec: Option<&str>,
    ) -> Result<Capture, CompressError> {
        let original_bytes = data.len();
        let compressed = ZstdCompressor::compress(data)?;
        let hash = blake3::hash(&compressed);
        let compressed_bytes = compressed.len();
        let cas = cas.clone();

        let blob_ref = tokio::task::spawn_blocking(move || cas.store(&compressed, false))
            .await
            .map_err(|e| CompressError::Io(std::io::Error::other(e.to_string())))?
            .map_err(|e| CompressError::Io(std::io::Error::other(e.to_string())))?;

        let meta = RecordingMeta {
            blob_ref: blob_ref.clone(),
            media_type: media_type.to_string(),
            bytes: original_bytes,
            codec: codec.map(String::from),
        };

        Ok(Capture {
            blob_hash: *hash.as_bytes(),
            compressed_bytes,
            original_bytes,
            recording_meta: Some(meta),
        })
    }
}

pub fn write_recording_meta(
    conn: &rusqlite::Connection,
    agent_id: i64,
    meta: &RecordingMeta,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO recordings (agent_id, media_type, blob_ref, bytes, codec, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            agent_id,
            meta.media_type,
            meta.blob_ref,
            meta.bytes as i64,
            meta.codec,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}
