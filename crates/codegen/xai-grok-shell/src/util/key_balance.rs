//! API key bakiye sorgusu — en iyi çaba, asla UI'yi bloke etmez.
//!
//! Her provider'ın halka açık faturalama ucu yoktur; bilinenler:
//! - OpenAI / custom-openai (one-api tarzı proxy'ler): `GET {base}/v1/dashboard/billing/subscription`
//!   → `hard_limit_usd` (hesap bütçesi — kullanılabilir bakiye sinyali).
//! - DeepSeek: `GET https://api.deepseek.com/user/balance` → `balance_infos[].total_balance`.
//!
//! Diğer provider'lar (Anthropic, Google, Groq, xAI…) halka açık bakiye ucu
//! yayınlamadığı için `None` döner. Tüm istekler 8 saniye timeout'ludur;
//! 401/403/404 ve parse hataları sessizce `None`'a düşer (key geçersiz
//! olabilir, UI kilitlenmez). Sonuç keychain'de `KeyEntry.balance` alanına
//! yazılır ve keys manager'da "yüksek bakiyeli" gibi türetilmiş kategoriler
//! için kullanılır.

use std::time::Duration;

use anyhow::Context as _;

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// OpenAI tarzı bir endpoint'in bakiye/bütçe değeri (USD).
///
/// `base_url` verilmişse o endpoint üzerinden dener (custom/one-api); yoksa
/// OpenAI resmî ucunu kullanır.
pub async fn probe_balance(
    client: &reqwest::Client,
    provider_id: &str,
    base_url: Option<&str>,
    api_key: &str,
) -> Option<f64> {
    match provider_id {
        "openai" | "custom-openai" => probe_openai(client, base_url, api_key).await,
        "deepseek" => probe_deepseek(client, api_key).await,
        // Anthropic / Google / Groq / xAI…: halka açık bakiye ucu yok.
        _ => None,
    }
}

async fn probe_openai(
    client: &reqwest::Client,
    base_url: Option<&str>,
    api_key: &str,
) -> Option<f64> {
    let base = base_url
        .unwrap_or("https://api.openai.com/v1")
        .trim_end_matches('/');
    let url = format!("{base}/dashboard/billing/subscription");
    let response = client
        .get(&url)
        .timeout(PROBE_TIMEOUT)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .context("openai billing subscription request failed")
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let json: serde_json::Value = response.json().await.ok()?;
    json.get("hard_limit_usd").and_then(|v| v.as_f64())
}

async fn probe_deepseek(client: &reqwest::Client, api_key: &str) -> Option<f64> {
    let response = client
        .get("https://api.deepseek.com/user/balance")
        .timeout(PROBE_TIMEOUT)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .context("deepseek balance request failed")
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let json: serde_json::Value = response.json().await.ok()?;
    json.get("balance_infos")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|info| info.get("total_balance"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
}
