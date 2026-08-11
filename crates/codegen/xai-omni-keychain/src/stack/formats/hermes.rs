//! Hermes-agent `auth.json` credential_pool formatı.

use std::path::Path;

use serde_json::{json, Map, Value};

use super::{write_json_pretty, ExportWriteSummary, RawKey};

pub fn extract(path: &Path) -> anyhow::Result<Vec<RawKey>> {
    let value = read_json(path)?;
    let mut out = Vec::new();
    let Some(pool) = value.get("credential_pool").and_then(|v| v.as_object()) else {
        return Ok(out);
    };
    for (provider, entries) in pool {
        let Some(arr) = entries.as_array() else {
            continue;
        };
        for (i, entry) in arr.iter().enumerate() {
            let Some(obj) = entry.as_object() else {
                continue;
            };
            let auth_type = obj
                .get("auth_type")
                .and_then(|v| v.as_str())
                .unwrap_or("api_key");
            // oauth havuz kayıtlarını atla (token bozulmasın / taşınmasın)
            if auth_type.contains("oauth") {
                continue;
            }
            let key = obj
                .get("access_token")
                .or_else(|| obj.get("api_key"))
                .or_else(|| obj.get("key"))
                .and_then(|v| v.as_str());
            let Some(key) = key.filter(|k| !k.trim().is_empty()) else {
                continue;
            };
            let base_url = obj
                .get("base_url")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            out.push(RawKey {
                provider_id: provider.clone(),
                api_key: key.to_string(),
                base_url,
                model_id: None,
                source_field: format!("credential_pool.{provider}[{i}]"),
            });
            // provider başına ilk geçerli key yeterli
            break;
        }
    }
    Ok(out)
}

pub fn merge_export(
    path: &Path,
    entries: &[(String, String, Option<String>, Option<String>)],
    overwrite: bool,
) -> anyhow::Result<ExportWriteSummary> {
    let mut summary = ExportWriteSummary::default();
    let mut value = if path.exists() {
        read_json(path)?
    } else {
        json!({
            "version": 1,
            "providers": {},
            "active_provider": null,
            "credential_pool": {}
        })
    };
    let root = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hermes auth.json object olmalı"))?;
    let pool = root
        .entry("credential_pool".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let pool_obj = pool
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("credential_pool object olmalı"))?;

    for (provider, api_key, _, base_url) in entries {
        if api_key.trim().is_empty() {
            continue;
        }
        let existing_nonempty = pool_obj
            .get(provider)
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());
        if existing_nonempty && !overwrite {
            summary.skipped_conflicts.push(provider.clone());
            continue;
        }
        if existing_nonempty && overwrite {
            summary.overwritten.push(provider.clone());
        }
        let mut entry = Map::new();
        entry.insert("id".into(), Value::String(format!("omnitrix-{provider}")));
        entry.insert("label".into(), Value::String(provider.clone()));
        entry.insert("auth_type".into(), Value::String("api_key".into()));
        entry.insert("priority".into(), json!(100));
        entry.insert("source".into(), Value::String("omnitrix".into()));
        entry.insert("access_token".into(), Value::String(api_key.clone()));
        if let Some(b) = base_url {
            entry.insert("base_url".into(), Value::String(b.clone()));
        }
        pool_obj.insert(provider.clone(), Value::Array(vec![Value::Object(entry)]));
        summary.written += 1;
    }

    write_json_pretty(path, &value)?;
    Ok(summary)
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("{} okunamadı: {e}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("{} bozuk JSON: {e}", path.display()))
}
