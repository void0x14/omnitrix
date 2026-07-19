use std::io::{Read, Write};

#[derive(Debug, thiserror::Error)]
pub enum CompressError {
    #[error("zstd compression failed: {0}")]
    Zstd(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage error: {0}")]
    Storage(String),
}

impl From<ork_storage::traits::StorageError> for CompressError {
    fn from(e: ork_storage::traits::StorageError) -> Self {
        Self::Storage(e.to_string())
    }
}

pub struct ZstdCompressor;

impl ZstdCompressor {
    pub fn compress(data: &[u8]) -> Result<Vec<u8>, CompressError> {
        let mut compressed = Vec::new();
        let mut encoder = zstd::Encoder::new(&mut compressed, 3)
            .map_err(|e| CompressError::Zstd(e.to_string()))?;
        encoder
            .write_all(data)
            .map_err(|e| CompressError::Zstd(e.to_string()))?;
        encoder
            .finish()
            .map_err(|e| CompressError::Zstd(e.to_string()))?;
        Ok(compressed)
    }

    pub fn decompress(data: &[u8]) -> Result<Vec<u8>, CompressError> {
        let mut decoder =
            zstd::Decoder::new(data).map_err(|e| CompressError::Zstd(e.to_string()))?;
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| CompressError::Zstd(e.to_string()))?;
        Ok(decompressed)
    }

    pub fn compress_stream<R: Read, W: Write>(reader: R, writer: W, level: i32) -> Result<(), CompressError> {
        let mut encoder = zstd::Encoder::new(writer, level)
            .map_err(|e| CompressError::Zstd(e.to_string()))?;
        let mut buf = [0u8; 8192];
        let mut reader = reader;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            encoder
                .write_all(&buf[..n])
                .map_err(|e| CompressError::Zstd(e.to_string()))?;
        }
        encoder
            .finish()
            .map_err(|e| CompressError::Zstd(e.to_string()))?;
        Ok(())
    }

    pub fn decompress_stream<R: Read>(reader: R) -> Result<Vec<u8>, CompressError> {
        let mut decoder =
            zstd::Decoder::new(reader).map_err(|e| CompressError::Zstd(e.to_string()))?;
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| CompressError::Zstd(e.to_string()))?;
        Ok(decompressed)
    }
}
