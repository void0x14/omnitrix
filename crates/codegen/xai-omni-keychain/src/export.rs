//! Kategorili, şifreli export/import (.omx).
//! Metadata düz metin; key'ler ayrı export şifresiyle AES-256-GCM.
//!
//! Dosya formatı (master password'den bağımsız export şifresiyle kilitlenir):
//! ```json
//! {
//!   "format": "omnitrix-keychain-export",
//!   "version": 1,
//!   "exported_at": "RFC3339",
//!   "category_scope": ["personal", "work"] | null,
//!   "salt_b64": "...",
//!   "kdf_params": { "m_cost": 65536, "t_cost": 3, "p_cost": 1 },
//!   "ciphertext_b64": "..."
//! }
//! ```
//! Şifreli gövde yalnızca `{ "entries": [...] }`; ham key'ler sadece bu
//! blokta durur, başlıkta asla düz metin key bulunmaz.

use std::collections::BTreeSet;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::crypto::{self, KdfParams, NONCE_LEN, SALT_LEN};
use crate::store::{ExportEntryData, Keychain};

/// Export dosya formatı tanımlayıcısı (başlıkta `format` alanı).
pub const EXPORT_FORMAT: &str = "omnitrix-keychain-export";
const EXPORT_VERSION: u32 = 1;

/// Export kapsamı: tüm kayıtlar ya da belirli kategoriler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportScope {
    All,
    Categories(Vec<String>),
}

/// Import sonucu özeti.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    /// Başarıyla eklenen/üzerine yazılan kayıt sayısı.
    pub imported_keys: usize,
    /// Üzerine yazılanlar: "kategori/provider" listesi.
    pub overwritten: Vec<String>,
    /// Atlananlar (conflict + overwrite=false): "kategori/provider" listesi.
    pub skipped: Vec<String>,
}

/// Şifreli gövde içindeki tek kayıt.
#[derive(Serialize, Deserialize)]
struct ExportEntry {
    category: String,
    provider_id: String,
    api_key: String,
    model_id: Option<String>,
    base_url: Option<String>,
    created_at: String,
}

#[derive(Serialize, Deserialize)]
struct ExportBody {
    entries: Vec<ExportEntry>,
}

/// Düz metin başlık; yalnızca metadata + şifreleme parametreleri taşır.
#[derive(Serialize, Deserialize)]
struct ExportEnvelope {
    format: String,
    version: u32,
    exported_at: String,
    category_scope: Option<Vec<String>>,
    salt_b64: String,
    kdf_params: KdfParams,
    ciphertext_b64: String,
}

impl ExportEntry {
    fn from_data(d: ExportEntryData) -> Self {
        Self {
            category: d.category,
            provider_id: d.provider_id,
            api_key: d.api_key,
            model_id: d.model_id,
            base_url: d.base_url,
            created_at: d.created_at,
        }
    }
}

/// Keychain içeriğini export şifresiyle AES-256-GCM şifreleyerek dışa aktarır.
///
/// - `scope` `Categories([])` ise hata döner (anlamsız export).
/// - Başlık düz metindir ama yalnızca metadata taşır; ham key'ler yalnızca
///   `ciphertext_b64` içinde durur.
/// - Export şifresi master password'den bağımsızdır (kendi salt + Argon2id).
/// - Keychain'in kendisi değiştirilmez; kayıtlar RAM'deki çözülmüş payload'dan
///   okunur (cache/TTL'den bağımsız).
pub fn export_keychain(
    keychain: &Keychain,
    scope: ExportScope,
    export_password: &str,
) -> anyhow::Result<Vec<u8>> {
    let category_scope: Option<BTreeSet<String>> = match &scope {
        ExportScope::All => None,
        ExportScope::Categories(names) => {
            if names.is_empty() {
                anyhow::bail!("export scope is empty: at least one category is required");
            }
            Some(names.iter().cloned().collect())
        }
    };

    let entries: Vec<ExportEntry> = keychain
        .export_entries()
        .into_iter()
        .filter(|e| match &category_scope {
            None => true,
            Some(set) => set.contains(&e.category),
        })
        .map(ExportEntry::from_data)
        .collect();

    let body = ExportBody { entries };
    let body_json = serde_json::to_vec(&body)
        .map_err(|e| anyhow::anyhow!("serialize export body: {e}"))?;
    let salt = crypto::random_salt();
    let kdf_params = KdfParams::default();
    let key = crypto::derive_key(export_password, &salt, &kdf_params);
    let ciphertext_b64 = crypto::encrypt(&key, &body_json)?;
    crypto::wipe(body_json);

    let envelope = ExportEnvelope {
        format: EXPORT_FORMAT.to_string(),
        version: EXPORT_VERSION,
        exported_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        category_scope: category_scope.map(|s| s.into_iter().collect()),
        salt_b64: B64.encode(salt),
        kdf_params,
        ciphertext_b64,
    };
    serde_json::to_vec_pretty(&envelope)
        .map_err(|e| anyhow::anyhow!("serialize export file: {e}"))
}

/// Export dosyasını keychain'e birleştirir (merge).
///
/// - Aynı (category, provider_id) kaydı varsa: `overwrite` true → üzerine
///   yazar ve `overwritten`'a ekler; false → atlar ve `skipped`'e ekler.
/// - Diğer kategoriler/kayıtlar etkilenmez (yalnızca per-entry ekleme).
/// - Yanlış export şifresi → "wrong export password" hatası; yapısal bozukluk
///   farklı mesajlarla ayrışır.
/// - Kayıtlar `add_key_import` yoluyla girer; `created_at` export verisinden
///   korunur, kaynak `Imported` olur. Diske yazma çağıranın `save()` işidir.
pub fn import_keychain(
    keychain: &mut Keychain,
    bytes: &[u8],
    export_password: &str,
    overwrite: bool,
) -> anyhow::Result<ImportSummary> {
    let envelope: ExportEnvelope = serde_json::from_slice(bytes)
        .map_err(|e| anyhow::anyhow!("invalid export file: {e}"))?;
    if envelope.format != EXPORT_FORMAT {
        anyhow::bail!(
            "not an omnitrix keychain export (format: {:?})",
            envelope.format
        );
    }
    if envelope.version != EXPORT_VERSION {
        anyhow::bail!("unsupported export version: {}", envelope.version);
    }
    let salt = B64
        .decode(&envelope.salt_b64)
        .map_err(|_| anyhow::anyhow!("invalid export salt encoding"))?;
    if salt.len() != SALT_LEN {
        anyhow::bail!("invalid export salt length");
    }
    if !valid_kdf_params(&envelope.kdf_params) {
        anyhow::bail!("invalid KDF parameters in export file");
    }
    if !structurally_valid_ciphertext(&envelope.ciphertext_b64) {
        anyhow::bail!("malformed export ciphertext");
    }

    let key = crypto::derive_key(export_password, &salt, &envelope.kdf_params);
    let pt = crypto::decrypt(&key, &envelope.ciphertext_b64)
        .map_err(|_| anyhow::anyhow!("wrong export password"))?;
    let body: ExportBody = serde_json::from_slice(&pt[..])
        .map_err(|e| anyhow::anyhow!("invalid export payload: {e}"))?;

    let mut summary = ImportSummary::default();
    for entry in body.entries {
        let conflict = keychain.has_provider(&entry.category, &entry.provider_id);
        if conflict && !overwrite {
            summary
                .skipped
                .push(format!("{}/{}", entry.category, entry.provider_id));
            continue;
        }
        keychain.add_key_import(
            &entry.category,
            &entry.provider_id,
            &entry.api_key,
            entry.model_id,
            entry.base_url,
            entry.created_at,
        );
        summary.imported_keys += 1;
        if conflict {
            summary
                .overwritten
                .push(format!("{}/{}", entry.category, entry.provider_id));
        }
    }
    Ok(summary)
}

/// KdfParams'ı Argon2id'nin panik yapabileceği değerlere karşı doğrular
/// (import dosyası = güvenilmeyen veri; store.rs'deki aynı kısıtlar).
/// Argon2 `Params::new` panik yapar: p_cost 2'nin kuvveti değilse veya
/// m_cost < 8 * p_cost ise. m_cost üst sınırı 1<<22 KiB (~4 GiB) bellek
/// taşmasını, alt sınır 8192 KiB zayıf parametre indirgemesini engeller.
/// 8 * p_cost taşmasına karşı aritmetik u64'te yapılır.
fn valid_kdf_params(p: &KdfParams) -> bool {
    let m_cost = p.m_cost as u64;
    let p_cost = p.p_cost as u64;
    p.t_cost >= 1
        && p.p_cost >= 1
        && p.p_cost.is_power_of_two()
        && m_cost >= 8192
        && m_cost >= 8 * p_cost
        && m_cost <= (1 << 22)
}

/// GCM auth hatası (→ wrong password) ile yapısal bozukluğu ayırt etmek için
/// ciphertext'in yapısal olarak geçerli olduğunu önceden doğrula.
fn structurally_valid_ciphertext(encoded: &str) -> bool {
    B64.decode(encoded).map(|raw| raw.len() >= NONCE_LEN).unwrap_or(false)
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests; // modül ayrı dosyada (export_tests.rs)
