//! Master key'in RAM'de TTL'li tutulması. TTL bitince anahtar zeroize edilir.

use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasterKeyTtl {
    Session,
    Seconds(u64),
}

impl Default for MasterKeyTtl {
    fn default() -> Self {
        Self::Seconds(15 * 60) // 15 dk
    }
}

impl MasterKeyTtl {
    pub fn as_secs(&self) -> u64 {
        match self {
            Self::Session => u64::MAX,
            Self::Seconds(s) => *s,
        }
    }
}

pub struct MasterKeyCache {
    key: Option<(Zeroizing<[u8; 32]>, Instant, MasterKeyTtl)>,
}

impl Default for MasterKeyCache {
    fn default() -> Self {
        Self { key: None }
    }
}

impl MasterKeyCache {
    pub fn set(&mut self, key: Zeroizing<[u8; 32]>, ttl: MasterKeyTtl) {
        self.key = Some((key, Instant::now(), ttl));
    }

    /// TTL dolmamış anahtarı döner; dolmuşsa sıfırlar ve None döner.
    pub fn get(&mut self) -> Option<Zeroizing<[u8; 32]>> {
        let (key, at, ttl) = self.key.as_ref()?;
        if ttl.as_secs() != u64::MAX && at.elapsed() > Duration::from_secs(ttl.as_secs()) {
            self.key = None;
            return None;
        }
        Some(key.clone())
    }

    pub fn clear(&mut self) {
        self.key = None;
    }

    pub fn is_locked(&self) -> bool {
        self.key.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_roundtrip() {
        let mut c = MasterKeyCache::default();
        let k = Zeroizing::new([7u8; 32]);
        c.set(k.clone(), MasterKeyTtl::Seconds(60));
        assert_eq!(*c.get().unwrap(), *k);
    }

    #[test]
    fn expired_is_cleared() {
        let mut c = MasterKeyCache::default();
        c.set(Zeroizing::new([1u8; 32]), MasterKeyTtl::Seconds(0));
        assert!(c.get().is_none());
        assert!(c.is_locked());
    }

    #[test]
    fn clear_locks() {
        let mut c = MasterKeyCache::default();
        c.set(Zeroizing::new([2u8; 32]), MasterKeyTtl::Session);
        c.clear();
        assert!(c.is_locked());
    }
}
