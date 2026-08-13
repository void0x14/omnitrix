//! Genel JSON tarayıcı + Continue config.

use std::path::Path;

use serde_json::Value;

use super::{write_json_pretty, ExportWriteSummary, RawKey};

const KEY_FIELD_NAMES: &[&str] = &[
    "apiKey",
    "api_key",
    "apikey",
    "openaiApiKey",
    "anthropicApiKey",
    "openAiApiKey",
    "openRouterApiKey",
    "geminiApiKey",
    "groqApiKey",
    "deepSeekApiKey",
    "xaiApiKey",
    "mistralApiKey",
    "togetherApiKey",
    "fireworksApiKey",
    "cerebrasApiKey",
    "key",
];

pub fn extract_scan(path: &Path) -> anyhow::Result<Vec<RawKey>> {
    let value = read_json(path)?;
    let mut out = Vec::new();
    walk(&value, "", &mut out);
    Ok(dedupe(out))
}

pub fn extract_continue(path: &Path) -> anyhow::Result<Vec<RawKey>> {
    // Continue: models[].apiKey veya contextProviders vb.
    extract_scan(path)
}

pub fn merge_export_continue(
    path: &Path,
    entries: &[(String, String, Option<String>, Option<String>)],
    overwrite: bool,
) -> anyhow::Result<ExportWriteSummary> {
    // Continue config karmaşık; güvenli yol: üst seviye env benzeri alan yoksa
    // `models` listesine openai-compatible kayıt ekle / güncelle.
    let mut summary = ExportWriteSummary::default();
    let mut value = if path.exists() {
        read_json(path)?
    } else {
        serde_json::json!({ "models": [] })
    };
    let root = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("continue config object olmalı"))?;
    let models = root
        .entry("models".to_string())
        .or_insert_with(|| serde_json::json!([]));
    let arr = models
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("models array olmalı"))?;

    for (provider, api_key, model_id, base_url) in entries {
        if api_key.trim().is_empty() {
            continue;
        }
        let mut found = false;
        for m in arr.iter_mut() {
            let Some(obj) = m.as_object_mut() else {
                continue;
            };
            let title = obj
                .get("title")
                .or_else(|| obj.get("provider"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if title.eq_ignore_ascii_case(provider)
                || obj
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .is_some_and(|p| p.eq_ignore_ascii_case(provider))
            {
                found = true;
                let has = obj
                    .get("apiKey")
                    .and_then(|v| v.as_str())
                    .is_some_and(|k| !k.is_empty());
                if has && !overwrite {
                    summary.skipped_conflicts.push(provider.clone());
                } else {
                    if has {
                        summary.overwritten.push(provider.clone());
                    }
                    obj.insert("apiKey".into(), Value::String(api_key.clone()));
                    summary.written += 1;
                }
                break;
            }
        }
        if !found {
            let mut obj = serde_json::Map::new();
            obj.insert("title".into(), Value::String(provider.clone()));
            obj.insert("provider".into(), Value::String(provider.clone()));
            obj.insert("apiKey".into(), Value::String(api_key.clone()));
            if let Some(m) = model_id {
                obj.insert("model".into(), Value::String(m.clone()));
            }
            if let Some(b) = base_url {
                obj.insert("apiBase".into(), Value::String(b.clone()));
            }
            arr.push(Value::Object(obj));
            summary.written += 1;
        }
    }

    write_json_pretty(path, &value)?;
    Ok(summary)
}

fn walk(value: &Value, path: &str, out: &mut Vec<RawKey>) {
    match value {
        Value::Object(map) => {
            // type=oauth ise bu objedeki token'ları atla
            let ty = map.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if ty == "oauth" || ty == "wellknown" {
                return;
            }
            for (k, v) in map {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if let Some(s) = v.as_str() {
                    if is_key_field(k) && looks_like_secret(s) {
                        let provider = infer_provider(k, path, map);
                        out.push(RawKey {
                            provider_id: provider,
                            api_key: s.to_string(),
                            base_url: None,
                            model_id: None,
                            source_field: child.clone(),
                        });
                    }
                } else {
                    walk(v, &child, out);
                }
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk(v, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn is_key_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    KEY_FIELD_NAMES.iter().any(|k| k.eq_ignore_ascii_case(name))
        || (lower.ends_with("apikey") || lower.ends_with("api_key") || lower == "key")
}

fn looks_like_secret(s: &str) -> bool {
    let t = s.trim();
    t.len() >= 12
        && !t.contains(' ')
        && (t.starts_with("sk-")
            || t.starts_with("gsk_")
            || t.starts_with("xai-")
            || t.starts_with("AIza")
            || t.starts_with("nvapi-")
            || t.len() >= 20)
}

fn infer_provider(field: &str, path: &str, map: &serde_json::Map<String, Value>) -> String {
    if let Some(p) = map.get("provider").and_then(|v| v.as_str()) {
        return p.to_ascii_lowercase();
    }
    if let Some(p) = map.get("title").and_then(|v| v.as_str()) {
        if p.len() < 40 {
            return p.to_ascii_lowercase().replace(' ', "-");
        }
    }
    let fl = field.to_ascii_lowercase();
    if fl.contains("anthropic") || fl.contains("claude") {
        return "anthropic".into();
    }
    if fl.contains("openai") {
        return "openai".into();
    }
    if fl.contains("gemini") || fl.contains("google") {
        return "google".into();
    }
    if fl.contains("openrouter") {
        return "openrouter".into();
    }
    if fl.contains("groq") {
        return "groq".into();
    }
    if fl.contains("deepseek") {
        return "deepseek".into();
    }
    if fl.contains("xai") || fl.contains("grok") {
        return "xai".into();
    }
    // path'in son segmenti
    path.rsplit(['.', '[', ']'])
        .find(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .unwrap_or("unknown")
        .to_ascii_lowercase()
}

fn dedupe(keys: Vec<RawKey>) -> Vec<RawKey> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for k in keys {
        let sig = format!(
            "{}:{}",
            k.provider_id,
            &k.api_key[..k.api_key.len().min(12)]
        );
        if seen.insert(sig) {
            out.push(k);
        }
    }
    out
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let bytes =
        std::fs::read(path).map_err(|e| anyhow::anyhow!("{} okunamadı: {e}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("{} bozuk JSON: {e}", path.display()))
}
