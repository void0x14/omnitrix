//! Bildirim kanalları — notify aşaması (system-triggered, AI DEĞİL).
//!
//! Kanallar `config/flow/notify.toml`'dan yüklenir:
//!   [telegram]  enabled = true  token = "..." chat_id = "..."
//!   [webhook]   enabled = true  url = "..." headers_json = '{"X":"y"}'
//!   [sms]       enabled = true  url = "..." username = "..." password = "..." msgheader = "..."
//!   [call]      enabled = true  url = "..." username = "..." password = "..."
//!
//! Determinizm + fail-soft: kanal hatası akışı ASLA düşürmez; her kanalın
//! sonucu rapora yazılır (I6: panic yok, unwrap yok — tüm sonuçlar Ok/Err
//! ele alınır). Kanalsız yapılandırma = skip (başarılı, "kanal yok" notu).

use std::time::Duration;

use super::config::{NotifyChannel, NotifyChannelKind, NotifyConfig};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NotifyReport {
    pub sent: usize,
    pub failed: usize,
    pub channels: Vec<NotifyChannelReport>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NotifyChannelReport {
    pub label: String,
    pub ok: bool,
    pub detail: String,
}

/// Tüm kanallara bildirim gönderir (paralel, süre sınırlı, fail-soft).
pub async fn dispatch(config: &NotifyConfig, title: &str, body: &str) -> NotifyReport {
    let enabled: Vec<&NotifyChannel> = config.channels.iter().filter(|c| c.enabled).collect();
    if enabled.is_empty() {
        return NotifyReport {
            sent: 0,
            failed: 0,
            channels: vec![NotifyChannelReport {
                label: "notify.toml".to_string(),
                ok: true,
                detail: "kanal yapılandırılmadı; yalnızca olay kaydı".to_string(),
            }],
        };
    }
    let mut handles = Vec::new();
    for channel in enabled {
        let channel = channel.clone();
        let label = channel.label.clone();
        let title = title.to_string();
        let body = body.to_string();
        handles.push(tokio::spawn(async move {
            let r = tokio::time::timeout(Duration::from_secs(15), send_one(channel, &title, &body))
                .await;
            match r {
                Ok(Ok(())) => NotifyChannelReport {
                    label: label.clone(),
                    ok: true,
                    detail: "gönderildi".to_string(),
                },
                Ok(Err(e)) => NotifyChannelReport {
                    label: label.clone(),
                    ok: false,
                    detail: e,
                },
                Err(_) => NotifyChannelReport {
                    label: label.clone(),
                    ok: false,
                    detail: "15sn zaman aşımı".to_string(),
                },
            }
        }));
    }
    let mut reports = Vec::new();
    for h in handles {
        if let Ok(r) = h.await {
            reports.push(r);
        }
    }
    let sent = reports.iter().filter(|r| r.ok).count();
    let failed = reports.len() - sent;
    NotifyReport {
        sent,
        failed,
        channels: reports,
    }
}

async fn send_one(channel: NotifyChannel, title: &str, body: &str) -> Result<(), String> {
    match channel.kind {
        NotifyChannelKind::Telegram => send_telegram(&channel, title, body).await,
        NotifyChannelKind::Webhook => send_webhook(&channel, title, body).await,
        NotifyChannelKind::Sms => send_sms(&channel, title, body).await,
        NotifyChannelKind::Call => send_call(&channel, title).await,
    }
}

async fn send_telegram(channel: &NotifyChannel, _title: &str, body: &str) -> Result<(), String> {
    let token = param(channel, "token")?;
    let chat_id = param(channel, "chat_id")?;
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .form(&[("chat_id", chat_id), ("text", body)])
        .send()
        .await
        .map_err(|e| format!("telegram istek hatası: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("telegram HTTP {status}"))
    }
}

async fn send_webhook(channel: &NotifyChannel, title: &str, body: &str) -> Result<(), String> {
    let url = param(channel, "url")?;
    let client = reqwest::Client::new();
    let mut req = client.post(url).json(&serde_json::json!({
        "title": title,
        "body": body,
        "source": "omnitrix-flow",
    }));
    if let Some(headers_json) = channel.params.get("headers_json") {
        if let Ok(headers) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(headers_json)
        {
            for (k, v) in headers {
                if let Some(v) = v.as_str() {
                    req = req.header(&k, v);
                }
            }
        }
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("webhook istek hatası: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("webhook HTTP {}", resp.status()))
    }
}

/// NetGSM uyumlu SMS (yapılandırılabilir URL — Twilio vb. de uyar).
async fn send_sms(channel: &NotifyChannel, _title: &str, body: &str) -> Result<(), String> {
    let url = param(channel, "url")?;
    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .form(&[
            ("usercode", param(channel, "username")?.to_string()),
            ("password", param(channel, "password")?.to_string()),
            (
                "msgheader",
                channel
                    .params
                    .get("msgheader")
                    .cloned()
                    .unwrap_or_else(|| "OMNITRIX".to_string()),
            ),
            ("gsmno", param(channel, "gsmno")?.to_string()),
            ("message", body.to_string()),
        ])
        .send()
        .await
        .map_err(|e| format!("sms istek hatası: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("sms HTTP {}", resp.status()))
    }
}

/// Çağrı kanalı: yapılandırılmış HTTP sağlayıcı (ör. NetGSM sesli arama).
async fn send_call(channel: &NotifyChannel, title: &str) -> Result<(), String> {
    let url = param(channel, "url")?;
    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .form(&[
            ("usercode", param(channel, "username")?.to_string()),
            ("password", param(channel, "password")?.to_string()),
            ("gsmno", param(channel, "gsmno")?.to_string()),
            ("message", title.to_string()),
        ])
        .send()
        .await
        .map_err(|e| format!("çağrı istek hatası: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("çağrı HTTP {}", resp.status()))
    }
}

fn param<'a>(channel: &'a NotifyChannel, key: &str) -> Result<&'a str, String> {
    channel
        .params
        .get(key)
        .map(|s| s.as_str())
        .ok_or_else(|| format!("{} kanalı eksik parametre: {key}", channel.label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::flow::config::NotifyConfig;

    #[test]
    fn no_channels_is_soft_success() {
        let cfg = NotifyConfig::default();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = rt.block_on(dispatch(&cfg, "t", "b"));
        assert_eq!(report.sent, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.channels.len(), 1);
    }

    #[test]
    fn missing_param_reports_failure_not_panic() {
        let cfg = NotifyConfig {
            channels: vec![crate::session::flow::config::NotifyChannel {
                kind: NotifyChannelKind::Telegram,
                enabled: true,
                label: "telegram".to_string(),
                params: std::collections::HashMap::new(),
            }],
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = rt.block_on(dispatch(&cfg, "t", "b"));
        assert_eq!(report.sent, 0);
        assert_eq!(report.failed, 1);
    }
}
