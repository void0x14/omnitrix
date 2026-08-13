//! Format motorları — stack kimliğinden bağımsız credential okuma/yazma.
//!
//! Bir format ailesi birden fazla aracı kapsar (ör. OpenCode/Kilo aynı
//! provider→{type,key} haritasını kullanır).

mod claude;
mod codex;
mod envfile;
mod hermes;
mod json_scan;
mod provider_map;

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Ham API key kaydı (provider + secret). Format motorları üretir.
#[derive(Clone, Debug)]
pub struct RawKey {
    pub provider_id: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub model_id: Option<String>,
    /// Kaynak alan yolu (debug/özet; secret içermez).
    pub source_field: String,
}

/// Desteklenen credential dosya formatları.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatKind {
    /// `{ "openai": { "type":"api", "key":"..." }, ... }` — OpenCode, Kilo, pi…
    ProviderAuthMap,
    /// Codex `auth.json` (`OPENAI_API_KEY` + tokens).
    CodexAuth,
    /// Claude Code `.credentials.json` + opsiyonel settings `env`.
    ClaudeCredentials,
    /// Hermes `credential_pool` yapısı.
    HermesAuth,
    /// `.env` / dotenv (`OPENAI_API_KEY=...`).
    DotEnv,
    /// Aider YAML conf (`openai-api-key:` / `api-key:` list).
    AiderYaml,
    /// Continue `config.json` provider apiKey alanları.
    ContinueConfig,
    /// Bilinmeyen JSON: bilinen key alan adlarını recursive tara.
    JsonKeyScan,
    /// Süreç ortam değişkenleri (dosya yok).
    ProcessEnv,
}

impl FormatKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::ProviderAuthMap => "provider-auth-map",
            Self::CodexAuth => "codex-auth",
            Self::ClaudeCredentials => "claude-credentials",
            Self::HermesAuth => "hermes-auth",
            Self::DotEnv => "dotenv",
            Self::AiderYaml => "aider-yaml",
            Self::ContinueConfig => "continue-config",
            Self::JsonKeyScan => "json-key-scan",
            Self::ProcessEnv => "process-env",
        }
    }

    /// Bu formata güvenli API-key export mümkün mü?
    pub fn supports_export(self) -> bool {
        matches!(
            self,
            Self::ProviderAuthMap
                | Self::DotEnv
                | Self::AiderYaml
                | Self::ContinueConfig
                | Self::CodexAuth
                | Self::ClaudeCredentials
                | Self::HermesAuth
        )
    }
}

/// Path'ten (veya ProcessEnv için yok sayılarak) API key listesi çıkar.
pub fn extract_api_keys(kind: FormatKind, path: &Path) -> anyhow::Result<Vec<RawKey>> {
    match kind {
        FormatKind::ProviderAuthMap => provider_map::extract(path),
        FormatKind::CodexAuth => codex::extract(path),
        FormatKind::ClaudeCredentials => claude::extract(path),
        FormatKind::HermesAuth => hermes::extract(path),
        FormatKind::DotEnv => envfile::extract_dotenv(path),
        FormatKind::AiderYaml => envfile::extract_aider_yaml(path),
        FormatKind::ContinueConfig => json_scan::extract_continue(path),
        FormatKind::JsonKeyScan => json_scan::extract_scan(path),
        FormatKind::ProcessEnv => envfile::extract_process_env(),
    }
}

/// Keychain export girdilerini hedef formata merge-yaz.
///
/// `overwrite=false` iken mevcut provider/key asla ezilmez.
pub fn merge_export(
    kind: FormatKind,
    path: &Path,
    entries: &[(String, String, Option<String>, Option<String>)],
    overwrite: bool,
) -> anyhow::Result<formats_export::ExportWriteSummary> {
    formats_export::merge_export(kind, path, entries, overwrite)
}

mod formats_export {
    use std::path::Path;

    use super::FormatKind;
    use crate::store::atomic_write;

    #[derive(Clone, Debug, Default)]
    pub struct ExportWriteSummary {
        pub written: usize,
        pub skipped_conflicts: Vec<String>,
        pub skipped_non_api: Vec<String>,
        pub overwritten: Vec<String>,
    }

    pub fn merge_export(
        kind: FormatKind,
        path: &Path,
        entries: &[(String, String, Option<String>, Option<String>)],
        overwrite: bool,
    ) -> anyhow::Result<ExportWriteSummary> {
        match kind {
            FormatKind::ProviderAuthMap => {
                super::provider_map::merge_export(path, entries, overwrite)
            }
            FormatKind::DotEnv => super::envfile::merge_export_dotenv(path, entries, overwrite),
            FormatKind::AiderYaml => {
                super::envfile::merge_export_aider_yaml(path, entries, overwrite)
            }
            FormatKind::CodexAuth => super::codex::merge_export(path, entries, overwrite),
            FormatKind::ClaudeCredentials => super::claude::merge_export(path, entries, overwrite),
            FormatKind::HermesAuth => super::hermes::merge_export(path, entries, overwrite),
            FormatKind::ContinueConfig => {
                super::json_scan::merge_export_continue(path, entries, overwrite)
            }
            FormatKind::JsonKeyScan | FormatKind::ProcessEnv => {
                anyhow::bail!(
                    "format {:?} export desteklemiyor (salt-okuma veya ortam)",
                    kind.label()
                )
            }
        }
    }

    pub fn write_json_pretty(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
        let bytes =
            serde_json::to_vec_pretty(value).map_err(|e| anyhow::anyhow!("json serialize: {e}"))?;
        atomic_write(path, &bytes)
            .map_err(|e| anyhow::anyhow!("yazılamadı ({}): {e}", path.display()))
    }
}

pub(crate) use formats_export::write_json_pretty;
pub(crate) use formats_export::ExportWriteSummary;
