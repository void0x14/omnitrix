//! Claude Code: `.credentials.json` + `settings.json` env bloğu.

use std::path::Path;

use serde_json::{json, Map, Value};

use super::{write_json_pretty, ExportWriteSummary, RawKey};

pub fn extract(path: &Path) -> anyhow::Result<Vec<RawKey>> {
    let mut out = Vec::new();
    if path
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n == "settings.json" || n.ends_with("settings.local.json"))
    {
        out.extend(extract_settings_env(path)?);
        return Ok(out);
    }

    // credentials.json
    if path.exists() {
        let value = read_json(path)?;
        // anthropicApiKey düz alan
        if let Some(key) = value.get("anthropicApiKey").and_then(|v| v.as_str()) {
            if key.trim().len() > 8 {
                out.push(RawKey {
                    provider_id: "anthropic".into(),
                    api_key: key.to_string(),
                    base_url: None,
                    model_id: None,
                    source_field: "anthropicApiKey".into(),
                });
            }
        }
        // apiKey alternatifi
        if let Some(key) = value.get("apiKey").and_then(|v| v.as_str()) {
            if key.trim().len() > 8 && out.is_empty() {
                out.push(RawKey {
                    provider_id: "anthropic".into(),
                    api_key: key.to_string(),
                    base_url: None,
                    model_id: None,
                    source_field: "apiKey".into(),
                });
            }
        }
    }

    // yan settings.json env
    if let Some(parent) = path.parent() {
        let settings = parent.join("settings.json");
        if settings.exists() {
            out.extend(extract_settings_env(&settings)?);
        }
    }
    Ok(out)
}

fn extract_settings_env(path: &Path) -> anyhow::Result<Vec<RawKey>> {
    let value = read_json(path)?;
    let mut out = Vec::new();
    let Some(env) = value.get("env").and_then(|v| v.as_object()) else {
        return Ok(out);
    };
    for (k, v) in env {
        let Some(val) = v.as_str() else { continue };
        if val.trim().is_empty() {
            continue;
        }
        if let Some(provider) = env_var_to_provider(k) {
            out.push(RawKey {
                provider_id: provider.into(),
                api_key: val.to_string(),
                base_url: None,
                model_id: None,
                source_field: format!("env.{k}"),
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
    // Claude'a export: settings.json env bloğuna yaz (credentials oauth'u bozma)
    let settings_path = if path
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.contains("settings"))
    {
        path.to_path_buf()
    } else if let Some(parent) = path.parent() {
        parent.join("settings.json")
    } else {
        path.to_path_buf()
    };

    let mut summary = ExportWriteSummary::default();
    let mut value = if settings_path.exists() {
        read_json(&settings_path)?
    } else {
        json!({})
    };
    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings.json object olmalı"))?;
    let env = obj
        .entry("env".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let env_obj = env
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings.env object olmalı"))?;

    for (provider, api_key, _, _) in entries {
        if api_key.trim().is_empty() {
            continue;
        }
        let Some(var) = provider_to_env_var(provider) else {
            summary
                .skipped_non_api
                .push(format!("{provider} (claude env eşlemesi yok)"));
            continue;
        };
        let existing = env_obj
            .get(var)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        if existing.is_some() && !overwrite {
            summary.skipped_conflicts.push(provider.clone());
            continue;
        }
        if existing.is_some() && overwrite {
            summary.overwritten.push(provider.clone());
        }
        env_obj.insert(var.into(), Value::String(api_key.clone()));
        summary.written += 1;
    }

    write_json_pretty(&settings_path, &value)?;
    Ok(summary)
}

fn env_var_to_provider(var: &str) -> Option<&'static str> {
    match var {
        "ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN" => Some("anthropic"),
        "OPENAI_API_KEY" => Some("openai"),
        "GEMINI_API_KEY" | "GOOGLE_API_KEY" => Some("google"),
        "OPENROUTER_API_KEY" => Some("openrouter"),
        "GROQ_API_KEY" => Some("groq"),
        "DEEPSEEK_API_KEY" => Some("deepseek"),
        "XAI_API_KEY" | "GROK_API_KEY" => Some("xai"),
        "MISTRAL_API_KEY" => Some("mistral"),
        "TOGETHER_API_KEY" => Some("togetherai"),
        "FIREWORKS_API_KEY" => Some("fireworks-ai"),
        "CEREBRAS_API_KEY" => Some("cerebras"),
        "NVIDIA_API_KEY" => Some("nvidia"),
        "ZAI_API_KEY" | "Z_AI_API_KEY" => Some("zai"),
        "MOONSHOT_API_KEY" => Some("moonshotai"),
        "MINIMAX_API_KEY" => Some("minimax"),
        "HF_TOKEN" | "HUGGINGFACE_API_KEY" => Some("huggingface"),
        _ => None,
    }
}

fn provider_to_env_var(provider: &str) -> Option<&'static str> {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => Some("ANTHROPIC_API_KEY"),
        "openai" => Some("OPENAI_API_KEY"),
        "google" | "gemini" => Some("GEMINI_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "groq" => Some("GROQ_API_KEY"),
        "deepseek" => Some("DEEPSEEK_API_KEY"),
        "xai" | "grok" => Some("XAI_API_KEY"),
        "mistral" => Some("MISTRAL_API_KEY"),
        "togetherai" | "together" => Some("TOGETHER_API_KEY"),
        "fireworks-ai" | "fireworks" => Some("FIREWORKS_API_KEY"),
        "cerebras" => Some("CEREBRAS_API_KEY"),
        "nvidia" => Some("NVIDIA_API_KEY"),
        "zai" | "zhipuai" => Some("ZAI_API_KEY"),
        "moonshotai" | "moonshot" => Some("MOONSHOT_API_KEY"),
        "minimax" => Some("MINIMAX_API_KEY"),
        "huggingface" => Some("HF_TOKEN"),
        _ => None,
    }
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("{} okunamadı: {e}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("{} bozuk JSON: {e}", path.display()))
}
