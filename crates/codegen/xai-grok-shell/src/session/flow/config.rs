//! Flow yapılandırması — `config/flow/*.toml` runtime yüklemesi.
//!
//! Yükleme sırası (deterministik): `$OMNITRIX_CONFIG_DIR/flow/*.toml` →
//! `<cwd>/config/flow/*.toml` → gömülü varsayılanlar. Dosyalar mtime
//! üzerinden her `activate`/`notify` çağrısında tazelenir (canlı reload).
//! Eksik/bozuk dosya ASLA hata üretmez: varsayılana düşer (I6, determinizm).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::classifier::FlowRules;

/// Bildirim kanalları (notify.toml).
#[derive(Debug, Clone, Default)]
pub struct NotifyConfig {
    pub channels: Vec<NotifyChannel>,
}

#[derive(Debug, Clone)]
pub struct NotifyChannel {
    pub kind: NotifyChannelKind,
    pub enabled: bool,
    pub label: String,
    /// Channel'a özgü parametreler (url/token/chat_id/username/password...).
    pub params: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyChannelKind {
    Telegram,
    Webhook,
    Sms,
    Call,
}

/// flows.toml aşama override'ları (id eşleşmesiyle).
#[derive(Debug, Clone, Default)]
pub struct FlowOverrides {
    pub stage_directives: HashMap<String, String>,
    pub stage_tool_groups: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct FlowConfig {
    pub rules: Option<FlowRules>,
    pub notify: Option<NotifyConfig>,
    pub overrides: Option<FlowOverrides>,
}

impl FlowConfig {
    pub fn load() -> Self {
        let dir = config_flow_dir();
        Self {
            rules: dir.as_ref().and_then(|d| load_rules(&d.join("rules.toml"))),
            notify: dir.as_ref().and_then(|d| load_notify(&d.join("notify.toml"))),
            overrides: dir.as_ref().and_then(|d| load_overrides(&d.join("flows.toml"))),
        }
    }

    /// Yapılandırma dosyalarının son değişme zamanı (reload kararı için).
    pub fn fingerprint(&self) -> (u64, u64, u64) {
        let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).map(|t| {
            t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
        }).unwrap_or(0);
        let dir = config_flow_dir();
        (
            dir.as_ref().map(|d| m(&d.join("rules.toml"))).unwrap_or(0),
            dir.as_ref().map(|d| m(&d.join("notify.toml"))).unwrap_or(0),
            dir.as_ref().map(|d| m(&d.join("flows.toml"))).unwrap_or(0),
        )
    }
}

/// `$OMNITRIX_CONFIG_DIR/flow` → `<cwd>/config/flow` → yok (gömülü).
pub fn config_flow_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("OMNITRIX_CONFIG_DIR") {
        let p = PathBuf::from(dir).join("flow");
        if p.is_dir() {
            return Some(p);
        }
    }
    let local = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join("config").join("flow"));
    if let Some(p) = local {
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

fn load_rules(path: &Path) -> Option<FlowRules> {
    let text = std::fs::read_to_string(path).ok()?;
    let raw: toml::Value = toml::from_str(&text).ok()?;
    let table = raw.get("rules")?;
    let mut rules = FlowRules::default();
    if let Some(v) = table.get("commit_keywords").and_then(|v| v.as_array()) {
        rules.commit_keywords = v.iter().filter_map(|s| s.as_str().map(str::to_string)).collect();
    }
    if let Some(v) = table.get("research_keywords").and_then(|v| v.as_array()) {
        rules.research_keywords = v.iter().filter_map(|s| s.as_str().map(str::to_string)).collect();
    }
    if let Some(v) = table.get("write_keywords").and_then(|v| v.as_array()) {
        rules.write_keywords = v.iter().filter_map(|s| s.as_str().map(str::to_string)).collect();
    }
    if let Some(v) = table.get("direct_max_len").and_then(|v| v.as_integer()) {
        rules.direct_max_len = v.max(0) as usize;
    }
    if let Some(v) = table.get("commit_max_len").and_then(|v| v.as_integer()) {
        rules.commit_max_len = v.max(0) as usize;
    }
    if let Some(v) = table.get("mvp_max_len").and_then(|v| v.as_integer()) {
        rules.mvp_max_len = v.max(0) as usize;
    }
    if let Some(v) = table.get("max_redirects_per_stage").and_then(|v| v.as_integer()) {
        rules.max_redirects_per_stage = v.max(1) as u32;
    }
    Some(rules)
}

fn load_notify(path: &Path) -> Option<NotifyConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    let raw: toml::Value = toml::from_str(&text).ok()?;
    let mut config = NotifyConfig::default();
    for (key, value) in raw.as_table()? {
        let kind = match key {
            "telegram" => NotifyChannelKind::Telegram,
            "webhook" => NotifyChannelKind::Webhook,
            "sms" => NotifyChannelKind::Sms,
            "call" => NotifyChannelKind::Call,
            _ => continue,
        };
        let enabled = value
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut params = HashMap::new();
        if let Some(table) = value.as_table() {
            for (k, v) in table {
                if let Some(s) = v.as_str() {
                    params.insert(k.clone(), s.to_string());
                } else if let Some(i) = v.as_integer() {
                    params.insert(k.clone(), i.to_string());
                }
            }
        }
        let label = params.get("label").cloned().unwrap_or_else(|| key.to_string());
        config.channels.push(NotifyChannel { kind, enabled, label, params });
    }
    Some(config)
}

fn load_overrides(path: &Path) -> Option<FlowOverrides> {
    let text = std::fs::read_to_string(path).ok()?;
    let raw: toml::Value = toml::from_str(&text).ok()?;
    let mut overrides = FlowOverrides::default();
    for (_, flow) in raw.as_table()? {
        let Some(stages) = flow.get("stages").and_then(|v| v.as_array()) else {
            continue;
        };
        for stage in stages {
            let Some(id) = stage.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Some(d) = stage.get("directive").and_then(|v| v.as_str()) {
                overrides.stage_directives.insert(id.to_string(), d.to_string());
            }
            if let Some(tools) = stage.get("tools").and_then(|v| v.as_array()) {
                let groups: Vec<String> = tools
                    .iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect();
                if !groups.is_empty() {
                    overrides.stage_tool_groups.insert(id.to_string(), groups);
                }
            }
        }
    }
    Some(overrides)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fallback_without_config() {
        let cfg = FlowConfig::load();
        // Dosya yoksa None kalır; governor varsayılanı kullanır.
        if config_flow_dir().is_none() {
            assert!(cfg.rules.is_none());
        }
    }

    #[test]
    fn parse_rules_roundtrip() {
        let dir = std::env::temp_dir().join(format!("flow_cfg_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("rules.toml");
        std::fs::write(
            &path,
            "[rules]\ncommit_keywords = [\"commit\"]\ndirect_max_len = 55\nmax_redirects_per_stage = 7\n",
        )
        .ok();
        let rules = load_rules(&path).unwrap();
        assert_eq!(rules.commit_keywords, vec!["commit"]);
        assert_eq!(rules.direct_max_len, 55);
        assert_eq!(rules.max_redirects_per_stage, 7);
    }
}
