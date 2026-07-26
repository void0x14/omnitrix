use std::path::Path;

use async_trait::async_trait;
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};

use crate::checksum;

pub type Result<T> = std::result::Result<T, TargetError>;

#[derive(Debug, thiserror::Error)]
pub enum TargetError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupTargetConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

pub struct S3BackupTarget {
    config: BackupTargetConfig,
    client: reqwest::Client,
}

impl S3BackupTarget {
    pub fn new(config: BackupTargetConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    fn object_url(&self, key: &str) -> String {
        format!(
            "{}/{}/{}",
            self.config.endpoint.trim_end_matches('/'),
            self.config.bucket,
            key
        )
    }

    // TODO: AWS Signature V4 imzası ekle
    // Şu an istekler imzasız gönderiliyor — S3 compatiable storage çoğu
    // serviste (MinIO, AWS S3, Backblaze B2 vb.) çalışmaz.
    // Implementation plan:
    //   1. Payload SHA256 hash'ini hesapla
    //   2. Canonical request oluştur (HTTP method, URI, query, headers, signed headers, payload hash)
    //   3. StringToSign oluştur (algorithm, request date, credential scope, canonical request hash)
    //   4. HMAC-SHA256 ile signing key türet (secret → date → region → service → "aws4_request")
    //   5. Signature'ı hesapla
    //   6. Authorization header'ını oluştur: AWS4-HMAC-SHA256 Credential=..., SignedHeaders=..., Signature=...
    // fn sign_request(&self, method: &str, url: &str, payload: &[u8]) -> Result<reqwest::Request> {
    //     todo!("SigV4 signing")
    // }

    const CHECKSUM_HEADER: &'static str = "X-Checksum-Blake3";
}

#[async_trait]
pub trait BackupTarget: Send + Sync {
    async fn push(&self, source_path: &Path, destination: &str) -> Result<()>;
    async fn pull(&self, remote_path: &str, local_destination: &Path) -> Result<()>;
}

#[async_trait]
impl BackupTarget for S3BackupTarget {
    async fn push(&self, source_path: &Path, destination: &str) -> Result<()> {
        let data = tokio::fs::read(source_path).await?;
        let url = self.object_url(destination);

        let hash = checksum::compute(&data);

        // TODO: AWS Signature V4 ile imzala — şu an imzasız PUT, çoğu S3
        // backend'de 403 döner

        let resp = self
            .client
            .put(&url)
            .header(Self::CHECKSUM_HEADER, HeaderValue::from_str(&hash).unwrap())
            .body(data)
            .send()
            .await?;

        if let Some(echoed) = resp
            .headers()
            .get(Self::CHECKSUM_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            if echoed != hash {
                return Err(TargetError::ChecksumMismatch {
                    expected: hash,
                    actual: echoed.to_string(),
                });
            }
        }

        tracing::info!(destination, hash, "push completed");
        Ok(())
    }

    async fn pull(&self, remote_path: &str, local_destination: &Path) -> Result<()> {
        let url = self.object_url(remote_path);

        // TODO: AWS Signature V4 ile imzala

        let resp = self.client.get(&url).send().await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(TargetError::NotFound(remote_path.to_string()));
        }

        let expected_hash = resp
            .headers()
            .get(Self::CHECKSUM_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let data = resp.bytes().await?;

        if let Some(expected) = expected_hash {
            let actual = checksum::compute(&data);
            if actual != expected {
                return Err(TargetError::ChecksumMismatch { expected, actual });
            }
        }

        if let Some(parent) = local_destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(local_destination, &data).await?;

        tracing::info!(remote_path, local = %local_destination.display(), "pull completed");
        Ok(())
    }
}
