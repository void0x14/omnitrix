//! Provider → { type, key, metadata? } haritası (OpenCode / Kilo / pi benzeri).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

use super::{write_json_pretty, ExportWriteSummary, RawKey};

pub fn extract(path: &Path) -> anyhow::Result<Vec<RawKey>> {
    let map = read_object_map(path)?;
    let mut out = Vec::new();
    for (provider, val) in &map {
        let Some(obj) = val.as_object() else {
            continue;
        };
        let ty = obj.get("type").and_then(|v| v.as_str()).unwrap_or("api");
        // type yoksa ama key varsa api kabul et
        if ty != "api" && obj.get("key").is_none() {
            continue;
        }
        if ty == "oauth" || ty == "wellknown" {
            continue;
        }
        let Some(key) = obj.get("key").and_then(|v| v.as_str()) else {
            continue;
        };
        if key.trim().is_empty() {
            continue;
        }
        out.push(RawKey {
            provider_id: provider.clone(),
            api_key: key.to_string(),
            base_url: None,
            model_id: None,
            source_field: format!("{provider}.key"),
        });
    }
    Ok(out)
}

pub fn merge_export(
    path: &Path,
    entries: &[(String, String, Option<String>, Option<String>)],
    overwrite: bool,
) -> anyhow::Result<ExportWriteSummary> {
    let mut map = if path.exists() {
        read_object_map(path)?
    } else {
        BTreeMap::new()
    };
    let mut summary = ExportWriteSummary::default();

    for (provider, api_key, _model, _base) in entries {
        if api_key.trim().is_empty() {
            continue;
        }
        if let Some(existing) = map.get(provider) {
            let ty = existing.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if ty == "oauth" || ty == "wellknown" {
                summary
                    .skipped_non_api
                    .push(format!("{provider} (mevcut {ty} korundu)"));
                continue;
            }
            let has_key = existing
                .get("key")
                .and_then(|v| v.as_str())
                .is_some_and(|k| !k.trim().is_empty());
            if has_key && !overwrite {
                summary.skipped_conflicts.push(provider.clone());
                continue;
            }
            if has_key && overwrite {
                summary.overwritten.push(provider.clone());
            }
        }

        let mut obj = Map::new();
        // önceki metadata korunur
        if let Some(Value::Object(prev)) = map.get(provider) {
            if let Some(meta) = prev.get("metadata") {
                obj.insert("metadata".into(), meta.clone());
            }
        }
        obj.insert("type".into(), Value::String("api".into()));
        obj.insert("key".into(), Value::String(api_key.clone()));
        map.insert(provider.clone(), Value::Object(obj));
        summary.written += 1;
    }

    write_json_pretty(path, &btree_to_value(&map))?;
    Ok(summary)
}

fn read_object_map(path: &Path) -> anyhow::Result<BTreeMap<String, Value>> {
    let bytes =
        std::fs::read(path).map_err(|e| anyhow::anyhow!("{} okunamadı: {e}", path.display()))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("{} bozuk JSON: {e}", path.display()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{} kökü object olmalı", path.display()))?;
    Ok(obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn btree_to_value(map: &BTreeMap<String, Value>) -> Value {
    let mut obj = Map::new();
    for (k, v) in map {
        obj.insert(k.clone(), v.clone());
    }
    Value::Object(obj)
}
