//! Merge-only senkron orkestrasyonu.

use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};

use crate::store::{auto_category, Keychain};

use super::catalog::{default_or_existing_path, StackDef};
use super::formats::{self, extract_api_keys, FormatKind, RawKey};

/// Çakışma politikası. Varsayılan: mevcut kaydı koru.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MergePolicy {
    #[default]
    SkipConflicts,
    Overwrite,
}

impl MergePolicy {
    pub fn overwrite(self) -> bool {
        matches!(self, Self::Overwrite)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncDirection {
    #[default]
    IntoOmnitrix,
    FromOmnitrix,
}

impl SyncDirection {
    pub fn label(self) -> &'static str {
        match self {
            Self::IntoOmnitrix => "stack → omnitrix",
            Self::FromOmnitrix => "omnitrix → stack",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncSummary {
    pub stack_id: String,
    pub path: String,
    pub direction: SyncDirection,
    pub transferred: usize,
    pub skipped_conflicts: Vec<String>,
    pub skipped_other: Vec<String>,
    pub overwritten: Vec<String>,
}

impl SyncSummary {
    pub fn format_tr(&self) -> String {
        format!(
            "{} [{}]: {} key · conflict atlandı: {} · diğer atlanan: {} · üzerine yazılan: {} · {}",
            self.direction.label(),
            self.stack_id,
            self.transferred,
            self.skipped_conflicts.len(),
            self.skipped_other.len(),
            self.overwritten.len(),
            self.path,
        )
    }
}

#[derive(Clone, Debug)]
pub struct PreviewEntry {
    pub provider_id: String,
    pub masked: String,
    pub conflict: bool,
    pub source_field: String,
}

#[derive(Clone, Debug)]
pub struct SyncPreview {
    pub stack_id: String,
    pub stack_label: String,
    pub path: PathBuf,
    pub direction: SyncDirection,
    pub candidates: Vec<PreviewEntry>,
    pub conflict_count: usize,
}

/// Stack → Omnitrix önizleme.
pub fn preview_import(
    def: &StackDef,
    path: Option<&Path>,
    keychain: &Keychain,
) -> anyhow::Result<SyncPreview> {
    let path = resolve_path(def, path)?;
    let keys = load_keys(def, &path)?;
    let mut candidates = Vec::with_capacity(keys.len());
    let mut conflict_count = 0;
    for k in keys {
        let cat = auto_category(&k.provider_id);
        let conflict = keychain.has_provider(&cat, &k.provider_id);
        if conflict {
            conflict_count += 1;
        }
        candidates.push(PreviewEntry {
            provider_id: k.provider_id,
            masked: mask_preview(&k.api_key),
            conflict,
            source_field: k.source_field,
        });
    }
    Ok(SyncPreview {
        stack_id: def.id.into(),
        stack_label: def.label.into(),
        path,
        direction: SyncDirection::IntoOmnitrix,
        candidates,
        conflict_count,
    })
}

/// Omnitrix → Stack önizleme.
pub fn preview_export(
    def: &StackDef,
    path: Option<&Path>,
    keychain: &Keychain,
) -> anyhow::Result<SyncPreview> {
    if !def.capability.export || !def.format.supports_export() {
        anyhow::bail!("{} export desteklemiyor", def.id);
    }
    let path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_or_existing_path(def));
    let existing = existing_provider_set(def, &path);
    let mut candidates = Vec::new();
    let mut conflict_count = 0;
    for e in keychain.export_entries() {
        if e.api_key.trim().is_empty() {
            continue;
        }
        let conflict = existing
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&e.provider_id));
        if conflict {
            conflict_count += 1;
        }
        candidates.push(PreviewEntry {
            provider_id: e.provider_id,
            masked: mask_preview(&e.api_key),
            conflict,
            source_field: "keychain".into(),
        });
    }
    Ok(SyncPreview {
        stack_id: def.id.into(),
        stack_label: def.label.into(),
        path,
        direction: SyncDirection::FromOmnitrix,
        candidates,
        conflict_count,
    })
}

/// Stack → Omnitrix import (merge).
pub fn import_from_stack(
    def: &StackDef,
    path: Option<&Path>,
    keychain: &mut Keychain,
    policy: MergePolicy,
) -> anyhow::Result<SyncSummary> {
    if !def.capability.import {
        anyhow::bail!("{} import desteklemiyor", def.id);
    }
    let path = resolve_path(def, path)?;
    let keys = load_keys(def, &path)?;
    let mut summary = SyncSummary {
        stack_id: def.id.into(),
        path: path.display().to_string(),
        direction: SyncDirection::IntoOmnitrix,
        ..Default::default()
    };
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    for k in keys {
        if k.api_key.trim().is_empty() {
            summary.skipped_other.push(k.provider_id);
            continue;
        }
        let cat = auto_category(&k.provider_id);
        let conflict = keychain.has_provider(&cat, &k.provider_id);
        if conflict && !policy.overwrite() {
            summary.skipped_conflicts.push(k.provider_id);
            continue;
        }
        keychain.add_key_import(
            &cat,
            &k.provider_id,
            &k.api_key,
            k.model_id,
            k.base_url,
            now.clone(),
        );
        summary.transferred += 1;
        if conflict {
            summary.overwritten.push(k.provider_id);
        }
    }
    Ok(summary)
}

/// Omnitrix → Stack export (merge).
pub fn export_to_stack(
    def: &StackDef,
    path: Option<&Path>,
    keychain: &Keychain,
    policy: MergePolicy,
) -> anyhow::Result<SyncSummary> {
    if !def.capability.export || !def.format.supports_export() {
        anyhow::bail!("{} export desteklemiyor", def.id);
    }
    let path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_or_existing_path(def));
    let entries: Vec<(String, String, Option<String>, Option<String>)> = keychain
        .export_entries()
        .into_iter()
        .filter(|e| !e.api_key.trim().is_empty())
        .map(|e| (e.provider_id, e.api_key, e.model_id, e.base_url))
        .collect();

    let wr = formats::merge_export(def.format, &path, &entries, policy.overwrite())?;
    Ok(SyncSummary {
        stack_id: def.id.into(),
        path: path.display().to_string(),
        direction: SyncDirection::FromOmnitrix,
        transferred: wr.written,
        skipped_conflicts: wr.skipped_conflicts,
        skipped_other: wr.skipped_non_api,
        overwritten: wr.overwritten,
    })
}

fn resolve_path(def: &StackDef, path: Option<&Path>) -> anyhow::Result<PathBuf> {
    if def.format == FormatKind::ProcessEnv {
        return Ok(PathBuf::from("<process-env>"));
    }
    if let Some(p) = path {
        if !p.exists() && def.format != FormatKind::ProcessEnv {
            anyhow::bail!("path yok: {}", p.display());
        }
        return Ok(p.to_path_buf());
    }
    resolve_existing_or_err(def)
}

fn resolve_existing_or_err(def: &StackDef) -> anyhow::Result<PathBuf> {
    super::catalog::resolve_stack_path(def)
        .ok_or_else(|| anyhow::anyhow!("{} için credential path bulunamadı", def.id))
}

fn load_keys(def: &StackDef, path: &Path) -> anyhow::Result<Vec<RawKey>> {
    if def.format == FormatKind::ProcessEnv {
        return extract_api_keys(def.format, path);
    }
    if path.is_dir() {
        // dizin: içindeki json dosyalarını tara
        let mut all = Vec::new();
        let rd = std::fs::read_dir(path)
            .map_err(|e| anyhow::anyhow!("{} okunamadı: {e}", path.display()))?;
        for ent in rd.flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(mut keys) = extract_api_keys(FormatKind::JsonKeyScan, &p) {
                    all.append(&mut keys);
                }
            }
        }
        return Ok(all);
    }
    extract_api_keys(def.format, path)
}

fn existing_provider_set(def: &StackDef, path: &Path) -> Vec<String> {
    if !path.exists() || path.is_dir() {
        return Vec::new();
    }
    extract_api_keys(def.format, path)
        .map(|ks| ks.into_iter().map(|k| k.provider_id).collect())
        .unwrap_or_default()
}

fn mask_preview(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() >= 8 {
        let head: String = chars[..3].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    } else if chars.len() >= 5 {
        let head: String = chars[..2].iter().collect();
        let tail: String = chars[chars.len() - 2..].iter().collect();
        format!("{head}…{tail}")
    } else if chars.is_empty() {
        "(boş)".into()
    } else {
        "…".into()
    }
}
