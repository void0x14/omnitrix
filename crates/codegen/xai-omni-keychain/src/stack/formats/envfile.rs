//! Dotenv, Aider YAML ve process env çıkarıcıları.

use std::collections::BTreeMap;
use std::path::Path;

use super::{ExportWriteSummary, RawKey};

/// Bilinen provider env var eşlemesi (import + process env).
pub fn known_env_providers() -> &'static [(&'static str, &'static str)] {
    &[
        ("OPENAI_API_KEY", "openai"),
        ("ANTHROPIC_API_KEY", "anthropic"),
        ("ANTHROPIC_AUTH_TOKEN", "anthropic"),
        ("GEMINI_API_KEY", "google"),
        ("GOOGLE_API_KEY", "google"),
        ("OPENROUTER_API_KEY", "openrouter"),
        ("GROQ_API_KEY", "groq"),
        ("DEEPSEEK_API_KEY", "deepseek"),
        ("XAI_API_KEY", "xai"),
        ("GROK_API_KEY", "xai"),
        ("MISTRAL_API_KEY", "mistral"),
        ("TOGETHER_API_KEY", "togetherai"),
        ("FIREWORKS_API_KEY", "fireworks-ai"),
        ("CEREBRAS_API_KEY", "cerebras"),
        ("NVIDIA_API_KEY", "nvidia"),
        ("ZAI_API_KEY", "zai"),
        ("MOONSHOT_API_KEY", "moonshotai"),
        ("MINIMAX_API_KEY", "minimax"),
        ("HF_TOKEN", "huggingface"),
        ("HUGGINGFACE_API_KEY", "huggingface"),
        ("COHERE_API_KEY", "cohere"),
        ("PERPLEXITY_API_KEY", "perplexity"),
        ("AI_GATEWAY_API_KEY", "vercel-ai-gateway"),
        ("AWS_BEARER_TOKEN_BEDROCK", "amazon-bedrock"),
        ("AZURE_OPENAI_API_KEY", "azure"),
        ("CODEX_API_KEY", "openai"),
    ]
}

pub fn extract_dotenv(path: &Path) -> anyhow::Result<Vec<RawKey>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("{} okunamadı: {e}", path.display()))?;
    Ok(parse_dotenv_keys(&text))
}

pub fn extract_process_env() -> anyhow::Result<Vec<RawKey>> {
    let mut out = Vec::new();
    for (var, provider) in known_env_providers() {
        if let Ok(val) = std::env::var(var) {
            if !val.trim().is_empty() {
                out.push(RawKey {
                    provider_id: (*provider).into(),
                    api_key: val,
                    base_url: None,
                    model_id: None,
                    source_field: format!("env:{var}"),
                });
            }
        }
    }
    // dedupe by provider (first wins)
    let mut seen = std::collections::HashSet::new();
    out.retain(|k| seen.insert(k.provider_id.clone()));
    Ok(out)
}

pub fn extract_aider_yaml(path: &Path) -> anyhow::Result<Vec<RawKey>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("{} okunamadı: {e}", path.display()))?;
    let mut out = Vec::new();
    // basit satır parser (serde_yaml bağımlılığı yok)
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if val.is_empty() {
                continue;
            }
            if key == "openai-api-key" || key == "openai_api_key" {
                out.push(RawKey {
                    provider_id: "openai".into(),
                    api_key: val,
                    base_url: None,
                    model_id: None,
                    source_field: format!("L{}:{key}", lineno + 1),
                });
            } else if key == "anthropic-api-key" || key == "anthropic_api_key" {
                out.push(RawKey {
                    provider_id: "anthropic".into(),
                    api_key: val,
                    base_url: None,
                    model_id: None,
                    source_field: format!("L{}:{key}", lineno + 1),
                });
            } else if key == "api-key" || key == "api_key" {
                // api-key: provider=value
                if let Some((p, rest)) = val.split_once('=') {
                    out.push(RawKey {
                        provider_id: p.trim().to_string(),
                        api_key: rest.trim().to_string(),
                        base_url: None,
                        model_id: None,
                        source_field: format!("L{}:api-key", lineno + 1),
                    });
                } else if val.contains('=') {
                    let _ = val;
                }
            }
        }
        // list item: - openrouter=sk-...
        if let Some(rest) = line.strip_prefix("- ") {
            if let Some((p, k)) = rest.split_once('=') {
                let provider = p.trim();
                let key = k.trim().trim_matches('"').trim_matches('\'');
                if !provider.is_empty() && !key.is_empty() {
                    out.push(RawKey {
                        provider_id: provider.into(),
                        api_key: key.into(),
                        base_url: None,
                        model_id: None,
                        source_field: format!("L{}:list", lineno + 1),
                    });
                }
            }
        }
    }
    Ok(out)
}

pub fn merge_export_dotenv(
    path: &Path,
    entries: &[(String, String, Option<String>, Option<String>)],
    overwrite: bool,
) -> anyhow::Result<ExportWriteSummary> {
    let mut summary = ExportWriteSummary::default();
    let existing_text = if path.exists() {
        std::fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };
    let mut map = parse_dotenv_map(&existing_text);
    let mut order: Vec<String> = map.keys().cloned().collect();

    for (provider, api_key, _, _) in entries {
        if api_key.trim().is_empty() {
            continue;
        }
        let Some(var) = provider_to_dotenv_var(provider) else {
            summary
                .skipped_non_api
                .push(format!("{provider} (env eşlemesi yok)"));
            continue;
        };
        if map.contains_key(var) && !map.get(var).unwrap().is_empty() && !overwrite {
            summary.skipped_conflicts.push(provider.clone());
            continue;
        }
        if map.contains_key(var) && overwrite {
            summary.overwritten.push(provider.clone());
        }
        if !map.contains_key(var) {
            order.push(var.to_string());
        }
        map.insert(var.to_string(), api_key.clone());
        summary.written += 1;
    }

    let mut out = String::new();
    for k in order {
        if let Some(v) = map.get(&k) {
            out.push_str(&format!("{k}={v}\n"));
        }
    }
    crate::store::atomic_write(path, out.as_bytes())
        .map_err(|e| anyhow::anyhow!("dotenv yazılamadı ({}): {e}", path.display()))?;
    Ok(summary)
}

pub fn merge_export_aider_yaml(
    path: &Path,
    entries: &[(String, String, Option<String>, Option<String>)],
    overwrite: bool,
) -> anyhow::Result<ExportWriteSummary> {
    // Aider: basit append/replace — mevcut dosyayı satır satır koru
    let mut summary = ExportWriteSummary::default();
    let mut lines: Vec<String> = if path.exists() {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };

    for (provider, api_key, _, _) in entries {
        if api_key.trim().is_empty() {
            continue;
        }
        let yaml_key = match provider.to_ascii_lowercase().as_str() {
            "openai" => "openai-api-key",
            "anthropic" => "anthropic-api-key",
            other => {
                // genel api-key listesine ekle
                let needle = format!("{other}=");
                let mut found = false;
                for line in &mut lines {
                    let t = line.trim();
                    if t.starts_with("- ") && t.contains(&needle) {
                        if !overwrite {
                            summary.skipped_conflicts.push(provider.clone());
                        } else {
                            *line = format!("- {other}={api_key}");
                            summary.overwritten.push(provider.clone());
                            summary.written += 1;
                        }
                        found = true;
                        break;
                    }
                }
                if !found {
                    if !lines.iter().any(|l| l.trim() == "api-key:") {
                        lines.push("api-key:".into());
                    }
                    lines.push(format!("- {other}={api_key}"));
                    summary.written += 1;
                }
                continue;
            }
        };
        let mut found = false;
        for line in &mut lines {
            if line.trim().starts_with(yaml_key) {
                if !overwrite {
                    summary.skipped_conflicts.push(provider.clone());
                } else {
                    *line = format!("{yaml_key}: {api_key}");
                    summary.overwritten.push(provider.clone());
                    summary.written += 1;
                }
                found = true;
                break;
            }
        }
        if !found {
            lines.push(format!("{yaml_key}: {api_key}"));
            summary.written += 1;
        }
    }

    let body = lines.join("\n") + "\n";
    crate::store::atomic_write(path, body.as_bytes())
        .map_err(|e| anyhow::anyhow!("aider conf yazılamadı ({}): {e}", path.display()))?;
    Ok(summary)
}

fn parse_dotenv_keys(text: &str) -> Vec<RawKey> {
    let mut out = Vec::new();
    for (var, provider) in known_env_providers() {
        for (lineno, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let rest = line.strip_prefix("export ").unwrap_or(line);
            if let Some((k, v)) = rest.split_once('=') {
                if k.trim() == *var {
                    let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !val.is_empty() {
                        out.push(RawKey {
                            provider_id: (*provider).into(),
                            api_key: val,
                            base_url: None,
                            model_id: None,
                            source_field: format!("L{}:{var}", lineno + 1),
                        });
                    }
                }
            }
        }
    }
    out
}

fn parse_dotenv_map(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let rest = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = rest.split_once('=') {
            map.insert(
                k.trim().to_string(),
                v.trim().trim_matches('"').trim_matches('\'').to_string(),
            );
        }
    }
    map
}

fn provider_to_dotenv_var(provider: &str) -> Option<&'static str> {
    match provider.to_ascii_lowercase().as_str() {
        "openai" => Some("OPENAI_API_KEY"),
        "anthropic" | "claude" => Some("ANTHROPIC_API_KEY"),
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
        "cohere" => Some("COHERE_API_KEY"),
        "perplexity" => Some("PERPLEXITY_API_KEY"),
        _ => None,
    }
}
