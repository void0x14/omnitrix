//! Auto-connect için canlı provider/region probe'u.
//!
//! `ProbeRequest`'lerdeki region endpoint'lerini zaman-aşımlı HTTP istekleriyle
//! canlı probeler; sonuçları `ProbeResult` olarak toplar. `pick_winner` ise
//! sonuçları deterministik sırayla en iyi adaya indirger.
//!
//! Güvenlik sözleşmesi: API key yalnızca `Authorization: Bearer` header'ında
//! taşınır; asla URL'ye, error'a veya log çıktısına yazılmaz. `ProbeRequest`
//! kasıtlı olarak `Debug` türetmez (secret içerir).

use std::time::{Duration, Instant};

use futures::future::join_all;
use zeroize::Zeroizing;

/// Açıkça belirtilmediğinde kullanılan varsayılan probe timeout'u (2500ms).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2500);

/// Problenecek tek provider adayı.
///
/// `timeout` verilmezse (ya da `Default` kullanılırsa) `DEFAULT_TIMEOUT`
/// geçerli olur; request üzerindeki açık `timeout` varsayılanı override eder.
/// `Debug` bilinçli türetilmez: `api_key` secret'tır.
pub struct ProbeRequest {
    pub provider_id: String,
    pub api_key: Zeroizing<String>,
    pub base_urls: Vec<String>,
    pub timeout: Duration,
}

impl Default for ProbeRequest {
    fn default() -> Self {
        Self {
            provider_id: String::new(),
            api_key: Zeroizing::new(String::new()),
            base_urls: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// Tek bir base URL probe'unun sonucu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeResult {
    pub provider_id: String,
    pub base_url: String,
    pub region: Option<String>,
    pub ok: bool,
    pub http_status: Option<u16>,
    pub latency_ms: u64,
    pub auth_seems_valid: bool,
}

/// Tüm adayların tüm region endpoint'lerini eşzamanlı probeler.
///
/// Ağ erişimi yapan tek public entrypoint. Sonuçlar input sırasına göre
/// deterministik biçimde döner; hiçbir hata panic üretmez. Bağlantı/timeout
/// hataları `ok = false` sonucuna katlanır, secret asla loglanmaz.
pub async fn probe_candidates(reqs: Vec<ProbeRequest>) -> Vec<ProbeResult> {
    let client = reqwest::Client::new();
    let futures = reqs.into_iter().flat_map(|req| {
        let provider_id = req.provider_id;
        let api_key = req.api_key;
        let timeout = req.timeout;
        let client = client.clone();
        req.base_urls.into_iter().map(move |base_url| {
            let client = client.clone();
            let provider_id = provider_id.clone();
            let api_key = api_key.clone();
            async move { probe_one(&client, &provider_id, api_key, base_url, timeout).await }
        })
    });
    join_all(futures).await
}

/// `{base}/models` endpoint'ini Bearer auth ile probeler.
async fn probe_one(
    client: &reqwest::Client,
    provider_id: &str,
    api_key: Zeroizing<String>,
    base_url: String,
    timeout: Duration,
) -> ProbeResult {
    let base = normalize_base_url(&base_url);
    let probe_url = format!("{base}/models");
    let started = Instant::now();
    // `without_url` reqwest error'ından URL'yi de düşürür (key URL'ye hiç
    // yazılmasa da userinfo içeren base_url ihtimaline karşı ek koruma).
    let outcome = client
        .get(&probe_url)
        .timeout(timeout)
        .bearer_auth(api_key.as_str())
        .send()
        .await
        .map_err(reqwest::Error::without_url);
    let latency_ms = started.elapsed().as_millis() as u64;

    match outcome {
        Ok(response) => {
            let status = response.status().as_u16();
            let success = (200..300).contains(&status);
            let auth_seems_valid = success || status == 401 || status == 403;
            if !success {
                tracing::debug!(provider_id, base_url = %base, "provider probe responded with non-success status");
            }
            ProbeResult {
                provider_id: provider_id.to_string(),
                base_url: base,
                region: region_from_url(&probe_url),
                ok: true,
                http_status: Some(status),
                latency_ms,
                auth_seems_valid,
            }
        }
        Err(_) => {
            tracing::debug!(provider_id, base_url = %base, "provider probe failed");
            ProbeResult {
                provider_id: provider_id.to_string(),
                base_url: base,
                region: region_from_url(&probe_url),
                ok: false,
                http_status: None,
                latency_ms,
                auth_seems_valid: false,
            }
        }
    }
}

/// base URL'yi normalize eder: userinfo (örn. `https://sk-secret@host/`)
/// güvenli biçimde strip edilir, sondaki `/`'ler atılır; gerisi olduğu gibi
/// korunur (aynı endpoint). Probe path'i `{base}/models` olarak eklenir.
///
/// Userinfo strip edilmezse secret hem log/Debug çıktısına sızar hem de
/// reqwest tarafından Basic Authorization'a çevrilir; bu fonksiyon ikisini de
/// engeller. Userinfo içermeyen URL'ler birebir (yalnızca son `/` silinerek)
/// geçer.
fn normalize_base_url(url: &str) -> String {
    let cleaned = match url::Url::parse(url) {
        Ok(mut parsed) if !parsed.username().is_empty() || parsed.password().is_some() => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.as_str().to_string()
        }
        Ok(_) => url.to_string(),
        // Parse edilemeyen girişlerde de authority bölümündeki userinfo'yu
        // elle temizle (log/result'a sızmaması için).
        Err(_) => strip_userinfo_manual(url),
    };
    cleaned.trim_end_matches('/').to_string()
}

/// URL olarak parse edilemeyen string'lerde userinfo'yu elle kaldırır:
/// `scheme://userinfo@host/path` → `scheme://host/path`. Userinfo yoksa
/// (ya da scheme yoksa) girdiyi olduğu gibi döner.
fn strip_userinfo_manual(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme_end + 3..];
    let authority_end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let Some(at) = authority.rfind('@') else {
        return url.to_string();
    };
    let mut out = String::with_capacity(url.len());
    out.push_str(&url[..scheme_end + 3]);
    out.push_str(&authority[at + 1..]);
    out.push_str(&rest[authority_end..]);
    out
}

/// URL'den deterministik region çıkarımı: host label'ları arasında AWS/GCP
/// tarzı region etiketi aranır (örn. `us-west-2`, `eu-central-1`,
/// `europe-west4`). Bilinmeyen/çıkarılamayan durumda `None`.
fn region_from_url(url: &str) -> Option<String> {
    let host = url::Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    host.split('.')
        .find(|label| looks_like_region_label(label))
        .map(str::to_owned)
}

/// Bilinen geo prefix'leri (AWS bölge kısaltmaları + GCP bölge adları).
const REGION_PREFIXES: &[&str] = &[
    "us",
    "eu",
    "ap",
    "sa",
    "ca",
    "me",
    "af",
    "il",
    "mx",
    "gb",
    "asia",
    "europe",
    "northamerica",
    "southamerica",
    "australia",
    "africa",
];

/// `us-west-2` / `europe-west4` gibi bir region etiketi mi?
/// Kural: geo prefix ile başla; geri kalan parçalar alfanumerik ve son parça
/// bir rakamla biter (AWS: son parça saf rakam; GCP: `west4`).
fn looks_like_region_label(label: &str) -> bool {
    let parts: Vec<&str> = label.split('-').collect();
    let Some(first) = parts.first() else {
        return false;
    };
    if !REGION_PREFIXES.contains(first) || parts.len() < 2 {
        return false;
    }
    let tail_alnum = parts
        .iter()
        .skip(1)
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()));
    let ends_with_digit = parts
        .last()
        .is_some_and(|p| p.chars().last().is_some_and(|c| c.is_ascii_digit()));
    tail_alnum && ends_with_digit
}

/// Sonuçları deterministik sırayla en iyi adaya indirger.
///
/// Sıralama: `ok` (canlı HTTP yanıtı; bağlantı hatası en sonda) → gerçek 2xx
/// başarı → `auth_seems_valid` (401/403 auth challenge dahil) → eşleşen
/// offline adayın `confidence`'ı (yüksek önce; aday yoksa 0) → `latency_ms`
/// (düşük önce) → input sırası (stable sort). Sonuç listesi boşsa `None`.
pub fn pick_winner(
    results: &[ProbeResult],
    offline: &[xai_omni_keychain::DetectCandidate],
) -> Option<ProbeResult> {
    let confidence_of = |provider_id: &str| -> u8 {
        offline
            .iter()
            .filter(|c| c.provider_id == provider_id)
            .map(|c| c.confidence)
            .max()
            .unwrap_or(0)
    };
    let mut ranked: Vec<&ProbeResult> = results.iter().collect();
    ranked.sort_by(|a, b| {
        probe_rank_key(b, confidence_of(&b.provider_id))
            .cmp(&probe_rank_key(a, confidence_of(&a.provider_id)))
    });
    ranked.first().map(|r| (*r).clone())
}

/// Probe sonuçlarının deterministik sıralama anahtarı (yüksek önce): canlı
/// HTTP yanıtı → gerçek 2xx başarı → auth sinyali (401/403 challenge dahil) →
/// offline confidence → düşük latency.
///
/// `pick_winner` ile `auto_connect`'in ambiguity kontrolü AYNI anahtarı
/// kullanır; tek kaynak bu fonksiyondur, iki ayrı sıralama tanımı birbirinden
/// sapamaz. 2xx, 401/403'ün önüne geçer: yüksek confidence'lı bir generic
/// `sk-` adayının auth challenge'ı, gerçekten çalışan düşük confidence'lı bir
/// adayı yenemez.
pub(crate) fn probe_rank_key(
    r: &ProbeResult,
    confidence: u8,
) -> (bool, bool, bool, u8, std::cmp::Reverse<u64>) {
    (
        r.ok,
        // 2xx gerçek başarı: 401/403 auth challenge'ın üstünde.
        r.http_status.is_some_and(|s| (200..300).contains(&s)),
        r.auth_seems_valid,
        confidence,
        std::cmp::Reverse(r.latency_ms),
    )
}

#[cfg(test)]
#[path = "provider_probe_tests.rs"]
mod tests; // modül ayrı dosyada (provider_probe_tests.rs)
