//! Per-model runtime API key store (Omnitrix keychain köprüsü).
//!
//! Keychain'den `borrow()` edilen API key'ler config.toml'a ASLA düz metin
//! yazılmaz (config düz metindir). Bunun yerine bu process-global store'a
//! yazılır ve oturum süresince credential çözümüne beslenir:
//!
//! - `ModelEntry::own_credential()` (agent/config.rs) runtime key'i son
//!   fallback olarak okur → `resolve_credentials` (chat/subagent/aux) ve
//!   `byok_from_models` → `sync_process_static_api_key` (tools/voice
//!   static fallthrough) tek seferde kapsanır.
//! - Anahtar modelin API id'sine (`ModelInfo.model`, isteklerdeki routing
//!   slug) göre anahtarlanır — `resolve_credentials` yalnızca
//!   `&ModelEntry` alır, catalog key'i bilmez.
//!
//! Yaşam süresi: connect/borrow akışı tarafından yazılır, process çıkışında
//! biter (kalıcı dosya YOK — tıpkı `key_ingestion` havuzu gibi).
//! `clear_runtime_keys()` kimlik/kilit geçişlerinde çağrılabilir.
//!
//! Güvenlik notu: değerler `String` olarak RAM'de tutulur (kaynak
//! `BorrowedKey` drop'ta `Zeroizing` ile sıfırlanır); bu store, mevcut
//! `AuthManager.process_static_api_key` ve `key_ingestion::LIVE_KEYS`
//! davranışıyla aynı seviyededir.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// model-id → API key. Process-scoped; asla diske yazılmaz.
static RUNTIME_KEYS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn runtime_keys() -> &'static Mutex<HashMap<String, String>> {
    RUNTIME_KEYS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `model_id` (routing slug) için oturum key'ini yazar. `None`/boş → kaydı
/// siler (temizleme yolu). `model_id` boşsa no-op.
pub fn set_runtime_model_key(model_id: &str, key: Option<String>) {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return;
    }
    let mut guard = match runtime_keys().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    match key {
        Some(key) => {
            let key = key.trim().to_string();
            if key.is_empty() {
                guard.remove(model_id);
            } else {
                guard.insert(model_id.to_string(), key);
            }
        }
        None => {
            guard.remove(model_id);
        }
    }
}

/// `model_id` için oturum key'i; yoksa `None`. Boş değer asla dönmez.
pub fn runtime_model_key(model_id: &str) -> Option<String> {
    let guard = match runtime_keys().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.get(model_id.trim()).cloned()
}

/// Tüm oturum key'lerini siler (lock/kimlik geçişleri için).
pub fn clear_runtime_keys() {
    let mut guard = match runtime_keys().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get_roundtrip() {
        clear_runtime_keys();
        assert_eq!(runtime_model_key("gpt-4o"), None);
        set_runtime_model_key("gpt-4o", Some("sk-abc".to_string()));
        assert_eq!(runtime_model_key("gpt-4o").as_deref(), Some("sk-abc"));
        set_runtime_model_key("gpt-4o", None);
        assert_eq!(runtime_model_key("gpt-4o"), None);
        clear_runtime_keys();
    }

    #[test]
    fn empty_and_blank_values_never_stored() {
        clear_runtime_keys();
        set_runtime_model_key("m1", Some("   ".to_string()));
        assert_eq!(runtime_model_key("m1"), None);
        set_runtime_model_key("m1", Some("".to_string()));
        assert_eq!(runtime_model_key("m1"), None);
        set_runtime_model_key("", Some("sk-x".to_string()));
        assert!(runtime_keys().lock().unwrap().is_empty());
        clear_runtime_keys();
    }

    #[test]
    fn keys_are_per_model_and_trimmed() {
        clear_runtime_keys();
        set_runtime_model_key("m-a", Some(" key-a ".to_string()));
        set_runtime_model_key("m-b", Some("key-b".to_string()));
        assert_eq!(runtime_model_key("m-a").as_deref(), Some("key-a"));
        assert_eq!(runtime_model_key("m-b").as_deref(), Some("key-b"));
        assert_eq!(runtime_model_key("m-c"), None);
        clear_runtime_keys();
        assert_eq!(runtime_model_key("m-a"), None);
    }

    #[test]
    fn lookup_trims_query_model_id() {
        clear_runtime_keys();
        set_runtime_model_key("gpt-4o", Some("sk-1".to_string()));
        assert_eq!(runtime_model_key(" gpt-4o ").as_deref(), Some("sk-1"));
        clear_runtime_keys();
    }
}
