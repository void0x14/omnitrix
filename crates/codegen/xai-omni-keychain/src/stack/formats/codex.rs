//! OpenAI Codex CLI `~/.codex/auth.json`.

use std::path::Path;

use serde_json::{json, Map, Value};

use super::{write_json_pretty, ExportWriteSummary, RawKey};

pub fn extract(path: &Path) -> anyhow::Result<Vec<RawKey>> {
    let value = read_json(path)?;
    let mut out = Vec::new();
    if let Some(key) = value.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
        if !key.trim().is_empty() {
            out.push(RawKey {
                provider_id: "openai".into(),
                api_key: key.to_string(),
                base_url: None,
                model_id: None,
                source_field: "OPENAI_API_KEY".into(),
            });
        }
    }
    // bazı kurulumlarda nested
    if let Some(key) = value
        .pointer("/tokens/api_key")
        .and_then(|v| v.as_str())
        .filter(|k| !k.trim().is_empty())
    {
        if out.is_empty() {
            out.push(RawKey {
                provider_id: "openai".into(),
                api_key: key.to_string(),
                base_url: None,
                model_id: None,
                source_field: "tokens.api_key".into(),
            });
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
        json!({ "auth_mode": "apikey" })
    };
    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("codex auth.json object olmalı"))?;

    for (provider, api_key, _, _) in entries {
        if !provider.eq_ignore_ascii_case("openai") && provider != "codex" {
            summary
                .skipped_non_api
                .push(format!("{provider} (codex yalnızca openai)"));
            continue;
        }
        if api_key.trim().is_empty() {
            continue;
        }
        let existing = obj
            .get("OPENAI_API_KEY")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        if existing.is_some() && !overwrite {
            summary.skipped_conflicts.push("openai".into());
            continue;
        }
        if existing.is_some() && overwrite {
            summary.overwritten.push("openai".into());
        }
        obj.insert("OPENAI_API_KEY".into(), Value::String(api_key.clone()));
        // oauth tokens alanına dokunma
        if !obj.contains_key("auth_mode") {
            obj.insert("auth_mode".into(), Value::String("apikey".into()));
        }
        summary.written += 1;
        break;
    }

    write_json_pretty(path, &value)?;
    Ok(summary)
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let bytes =
        std::fs::read(path).map_err(|e| anyhow::anyhow!("{} okunamadı: {e}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("{} bozuk JSON: {e}", path.display()))
}

#[allow(dead_code)]
fn empty_obj() -> Map<String, Value> {
    Map::new()
}
