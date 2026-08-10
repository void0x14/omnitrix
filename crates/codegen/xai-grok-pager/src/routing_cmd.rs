//! Headless routing mode control surface.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use xai_grok_sampler::{parse_routing_config, resolve_strategy};
use xai_grok_shell::util::config::{atomic_write_string, read_to_string_or_empty};
use xai_grok_shell::util::routing_catalog::{RoutingModeDef, builtin_modes, find_mode};

use crate::app::cli::{RoutingArgs, RoutingCommand};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingModeOutput {
    pub id: String,
    pub family: String,
    pub title: String,
    pub blurb: String,
    pub long_help: String,
}

impl From<&RoutingModeDef> for RoutingModeOutput {
    fn from(mode: &RoutingModeDef) -> Self {
        Self {
            id: mode.id.clone(),
            family: mode.family.to_string(),
            title: mode.title.clone(),
            blurb: mode.blurb.clone(),
            long_help: mode.long_help.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingCommandOutput {
    List(Vec<RoutingModeOutput>),
    Set(String),
    Show(RoutingModeOutput),
    Explain(RoutingModeOutput),
}

pub fn run(args: RoutingArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("routing config icin calisma dizini okunamadi")?;
    match run_at(args, &cwd)? {
        RoutingCommandOutput::List(modes) => {
            for mode in modes {
                println!("{}\t{}\t{}", mode.id, mode.family, mode.blurb);
            }
        }
        RoutingCommandOutput::Set(id) => println!("routing modu secildi: {id}"),
        RoutingCommandOutput::Show(mode) => {
            println!("{}\t{}\t{}", mode.id, mode.family, mode.blurb)
        }
        RoutingCommandOutput::Explain(mode) => println!("{}\n\n{}", mode.title, mode.long_help),
    }
    Ok(())
}

pub fn run_at(args: RoutingArgs, cwd: &Path) -> Result<RoutingCommandOutput> {
    match args.command {
        RoutingCommand::List => Ok(RoutingCommandOutput::List(
            builtin_modes()
                .iter()
                .map(RoutingModeOutput::from)
                .collect(),
        )),
        RoutingCommand::Set { id } => {
            set_mode_at(cwd, &id)?;
            Ok(RoutingCommandOutput::Set(id))
        }
        RoutingCommand::Show => selected_mode_at(cwd).map(RoutingCommandOutput::Show),
        RoutingCommand::Explain { id } => mode_output(&id).map(RoutingCommandOutput::Explain),
    }
}

pub fn set_mode_at(cwd: &Path, id: &str) -> Result<()> {
    let mode = find_mode(id).ok_or_else(|| anyhow!("bilinmeyen routing modu: {id}"))?;
    let path = routing_config_path(cwd);
    let content = read_to_string_or_empty(&path)
        .with_context(|| format!("routing config okunamadi: {}", path.display()))?;
    if !content.is_empty() {
        parse_routing_config(&content)
            .map_err(|error| anyhow!("routing config gecersiz; yazma iptal edildi: {error}"))?;
    }
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| anyhow!("routing config TOML ayrıştırılamadı: {error}"))?;
    doc["strategy"] = toml_edit::value(mode.id.as_str());
    atomic_write_string(&path, &doc.to_string())
        .with_context(|| format!("routing config yazilamadi: {}", path.display()))
}

pub fn selected_mode_at(cwd: &Path) -> Result<RoutingModeOutput> {
    let path = routing_config_path(cwd);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("routing config bulunamadi: {}", path.display()))?;
    let config = parse_routing_config(&content)
        .map_err(|error| anyhow!("routing config gecersiz: {error}"))?;
    let ids: Vec<&str> = builtin_modes()
        .iter()
        .map(|mode| mode.id.as_str())
        .collect();
    let resolved = resolve_strategy(&config.strategy, &ids)
        .map_err(|error| anyhow!("routing modu cozumlenemedi: {error}"))?;
    let canonical = resolved.canonical_id();
    mode_output(canonical.as_ref())
}

pub fn routing_config_path(cwd: &Path) -> PathBuf {
    cwd.join("config").join("routing.toml")
}

fn mode_output(id: &str) -> Result<RoutingModeOutput> {
    find_mode(id)
        .map(RoutingModeOutput::from)
        .ok_or_else(|| anyhow!("bilinmeyen routing modu: {id}"))
}
