//! Auto-connect orkestrasyonu: API key → provider + base URL + region +
//! models.dev model listesi (P0.3).
//!
//! Akış sırası: keychain `detect_providers_from_key` → katalogdan aday
//! provider/base URL çözümü → `probe_candidates` → `pick_winner`/ambiguous
//! kontrolü → winner provider'ın katalog `ModelInfo` kayıtları.
//!
//! Güvenlik sözleşmesi: API key hiçbir error Display/Debug, log veya kalıcı
//! çıktıya yazılmaz; key yalnızca `ProbeRequest.api_key` (Zeroizing) içinde
//! probe seam'ine taşınır.

use zeroize::Zeroizing;

use super::models_dev::{base_url_for_provider, provider_models, CatalogCache, ModelInfo};
use super::provider_probe::{pick_winner, probe_candidates, ProbeRequest, ProbeResult, DEFAULT_TIMEOUT};

/// Auto-connect sonucu: provider + base URL + region + model listesi.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoConnectOutcome {
    pub provider_id: String,
    pub base_url: String,
    pub region: Option<String>,
    pub models: Vec<ModelInfo>,
    /// Detect aşamasında bulunan aday sayısı (katalog/probe filtreleri bu
    /// sayıyı geriye dönük değiştirmez).
    pub candidates_considered: usize,
}

/// Auto-connect hataları. Hiçbir variant API key içermez; caller variant
/// match ile ayrıştırabilir.
#[derive(Debug, thiserror::Error)]
pub enum AutoConnectError {
    /// Boş/bilinmeyen key veya keychain tespit adayı yok.
    #[error("no provider detected from api key")]
    NoDetectedProviders,
    /// Aday(lar) katalogda bulunamadı veya base URL çözülemedi.
    #[error("provider '{provider_id}' has no models.dev catalog entry or base url")]
    MissingCatalogEntry { provider_id: String },
    /// Probe sonuçlarında canlı winner yok (bağlantı/timeout).
    #[error("no live probe winner")]
    NoProbeWinner,
    /// En iyi sıralama anahtarları eşit olan farklı provider'lar (tie).
    #[error("ambiguous probe tie between providers: {providers:?}")]
    Ambiguous { providers: Vec<String> },
    /// Winner provider'ın uygulanabilir modeli yok.
    #[error("provider '{provider_id}' has no applicable models")]
    EmptyModels { provider_id: String },
}

/// API key'den provider + base URL + region + model listesini üretir.
///
/// Public giriş noktası: keychain detect'i koşar, adayları katalogdan
/// çözer ve P0.2 public probe seam'ini (`probe_candidates`) çağırır.
pub async fn auto_connect_from_key(
    api_key: &str,
    catalog: &CatalogCache,
) -> Result<AutoConnectOutcome, AutoConnectError> {
    let candidates = xai_omni_keychain::detect_providers_from_key(api_key);
    auto_connect_with(api_key, candidates, catalog, probe_candidates).await
}

/// Detect adaylarını ve probe uygulamasını enjekte eden orkestrasyon çekirdeği.
///
/// `probe` ağ/HTTP ayrıntısı uygular; testler aynı production akışını
/// gerçek internet olmadan koşmak için mock probe enjekte eder. In-crate
/// child test modülü (`mod tests`) erişebilir; public API yüzeyine girmez.
pub(crate) async fn auto_connect_with<F, Fut>(
    api_key: &str,
    candidates: Vec<xai_omni_keychain::DetectCandidate>,
    catalog: &CatalogCache,
    probe: F,
) -> Result<AutoConnectOutcome, AutoConnectError>
where
    F: FnOnce(Vec<ProbeRequest>) -> Fut,
    Fut: std::future::Future<Output = Vec<ProbeResult>>,
{
    let candidates_considered = candidates.len();
    if candidates_considered == 0 {
        return Err(AutoConnectError::NoDetectedProviders);
    }

    // Adayları katalogdan çöz: provider kaydı + base URL. Çözülemeyen
    // adaylar atlanır; hiçbiri çözülemezse typed error.
    let mut probe_reqs: Vec<ProbeRequest> = Vec::new();
    for candidate in &candidates {
        let Some(entry) = catalog.providers.get(&candidate.provider_id) else {
            continue;
        };
        let Some(base_url) = base_url_for_provider(entry) else {
            continue;
        };
        probe_reqs.push(ProbeRequest {
            provider_id: candidate.provider_id.clone(),
            api_key: Zeroizing::new(api_key.to_string()),
            base_urls: vec![base_url],
            timeout: DEFAULT_TIMEOUT,
        });
    }
    if probe_reqs.is_empty() {
        return Err(AutoConnectError::MissingCatalogEntry {
            provider_id: candidates[0].provider_id.clone(),
        });
    }

    let results = probe(probe_reqs).await;

    // "Live winner": pick_winner en iyiyi seçer ama hepsi bağlantı/timeout
    // hatası (ok = false) ise canlı winner yoktur.
    let winner = match pick_winner(&results, &candidates) {
        Some(r) if r.ok => r,
        _ => return Err(AutoConnectError::NoProbeWinner),
    };

    // Ambiguous kontrolü: P0.2'nin deterministik sıralamasında winner'a
    // tamamen eşit anahtar taşıyan farklı bir provider varsa tie hatası.
    let confidence_of = |provider_id: &str| -> u8 {
        candidates
            .iter()
            .filter(|c| c.provider_id == provider_id)
            .map(|c| c.confidence)
            .max()
            .unwrap_or(0)
    };
    let rank_key = |r: &ProbeResult| -> (bool, bool, u8, std::cmp::Reverse<u64>) {
        (
            r.ok,
            r.auth_seems_valid,
            confidence_of(&r.provider_id),
            std::cmp::Reverse(r.latency_ms),
        )
    };
    let winner_key = rank_key(&winner);
    let mut tied: Vec<String> = results
        .iter()
        .filter(|r| rank_key(r) == winner_key && r.provider_id != winner.provider_id)
        .map(|r| r.provider_id.clone())
        .collect();
    // Sıralı unique: ambiguity mesajındaki provider listesi deterministik ve
    // tekrarsız kalır (aynı provider bitişik olmayan result'larda da çıksa).
    tied.sort();
    tied.dedup();
    if !tied.is_empty() {
        return Err(AutoConnectError::Ambiguous { providers: tied });
    }

    // Winner provider'ın katalog ModelInfo kayıtları (IndexMap sırasıyla).
    let models: Vec<ModelInfo> = match provider_models(catalog, &winner.provider_id) {
        Some(models) if !models.is_empty() => models.values().cloned().collect(),
        Some(_) => {
            return Err(AutoConnectError::EmptyModels {
                provider_id: winner.provider_id.clone(),
            });
        }
        None => {
            return Err(AutoConnectError::MissingCatalogEntry {
                provider_id: winner.provider_id.clone(),
            });
        }
    };

    Ok(AutoConnectOutcome {
        provider_id: winner.provider_id.clone(),
        base_url: winner.base_url,
        region: winner.region,
        models,
        candidates_considered,
    })
}

#[cfg(test)]
#[path = "auto_connect_tests.rs"]
mod tests; // modül ayrı dosyada (auto_connect_tests.rs)
