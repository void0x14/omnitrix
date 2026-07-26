use std::io::Read;
use std::path::Path;

pub fn compute(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

pub fn verify(data: &[u8], expected_hash: &str) -> bool {
    let computed = compute(data);
    computed == expected_hash
}

pub fn hash_file(path: impl AsRef<Path>) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path.as_ref())?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn verify_file(path: impl AsRef<Path>, expected_hash: &str) -> std::io::Result<bool> {
    let actual = hash_file(path)?;
    Ok(actual == expected_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_and_verify() {
        let data = b"hello world";
        let hash = compute(data);
        assert_eq!(hash.len(), 64);
        assert!(verify(data, &hash));
        assert!(!verify(b"wrong data", &hash));
    }

    #[test]
    fn test_compute_deterministic() {
        let data = b"test data";
        assert_eq!(compute(data), compute(data));
    }
}
