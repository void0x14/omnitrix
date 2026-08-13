//! Native, non-blocking Omnitrix composition root.
//!
//! Every in-process service is installed immediately from in-memory values. Disk and
//! configuration discovery runs in a spawned task and only updates shared
//! metadata, so neither CLI dispatch nor the TUI first frame waits for it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::omni_bridge::{
    AgentRow, KeysSummary, OmniBackup, OmniEventSink, OmniKeys, OmniNotify, OmniNotifyChannels,
    OmniPhase, OmniRouter, OmniSnapshot, OmniSnapshotProvider,
};

#[derive(Debug)]
struct RuntimeState {
    phase: OmniPhase,
    providers: usize,
    storage_bytes: u64,
    key_summary: KeysSummary,
    healthy: bool,
    acp_messages: u64,
    tool_calls: u64,
    prompts: u64,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            phase: OmniPhase::Starting,
            providers: 0,
            storage_bytes: 0,
            key_summary: KeysSummary::default(),
            healthy: false,
            acp_messages: 0,
            tool_calls: 0,
            prompts: 0,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct NativeSnapshot(Arc<Mutex<RuntimeState>>);

impl OmniSnapshotProvider for NativeSnapshot {
    fn snapshot(&self) -> OmniSnapshot {
        let state = lock(&self.0);
        let agents = crate::omni_bridge::live_agents();
        OmniSnapshot {
            phase: state.phase,
            providers: state.providers,
            active_agents: agents.iter().filter(|agent| agent.status != "idle").count(),
            storage_bytes: state.storage_bytes,
            healthy: state.healthy,
            agents,
        }
    }
}

struct NativeKeys(Arc<Mutex<RuntimeState>>);

impl OmniKeys for NativeKeys {
    fn summary(&self) -> KeysSummary {
        lock(&self.0).key_summary.clone()
    }
}

struct NativeEvents(Arc<Mutex<RuntimeState>>);

impl OmniEventSink for NativeEvents {
    fn on_acp_message(&self, json: &str) {
        let mut state = lock(&self.0);
        state.acp_messages = state.acp_messages.saturating_add(1);
        tracing::trace!(target: "omnitrix::events", bytes = json.len(), "acp message");
    }

    fn on_tool_call(&self, name: &str, args: &str) {
        let mut state = lock(&self.0);
        state.tool_calls = state.tool_calls.saturating_add(1);
        tracing::trace!(target: "omnitrix::events", tool = name, bytes = args.len(), "tool call");
    }

    fn on_prompt(&self, text: &str) {
        let mut state = lock(&self.0);
        state.prompts = state.prompts.saturating_add(1);
        tracing::trace!(target: "omnitrix::events", bytes = text.len(), "prompt");
    }
}

struct NativeRouter {
    root: PathBuf,
}

impl OmniRouter for NativeRouter {
    fn set_strategy(&self, strategy: &str) -> Result<String, String> {
        crate::routing_cmd::set_mode_at(&self.root, strategy).map_err(|error| error.to_string())?;
        Ok(format!("routing modu secildi: {strategy}"))
    }

    fn set_role_model(&self, _role: &str, _model: &str) -> Result<String, String> {
        Err("rol-model atamasi /routing katalog yuzeyinde desteklenmiyor".to_string())
    }

    fn summary(&self) -> String {
        match crate::routing_cmd::selected_mode_at(&self.root) {
            Ok(mode) => format!(
                "routing: id={} aile={} baslik={}\n{}",
                mode.id, mode.family, mode.title, mode.blurb
            ),
            Err(error) => format!("routing okunamadi: {error}"),
        }
    }
}

struct NativeNotify {
    config: xai_grok_hooks::notify::NotifyConfig,
    handle: tokio::runtime::Handle,
}

impl OmniNotify for NativeNotify {
    fn channels(&self) -> OmniNotifyChannels {
        OmniNotifyChannels {
            telegram: self.config.has_telegram(),
            phone: self.config.has_twilio(),
        }
    }

    fn send_test(&self) -> Result<(), String> {
        if self.channels().is_empty() {
            return Err("bildirim kanali yapilandirilmamis".to_string());
        }
        let config = self.config.clone();
        self.handle.spawn(async move {
            let result = xai_grok_hooks::notify::send_notification(
                &config,
                xai_grok_hooks::event::HookEventName::Notification,
                "Omnitrix deneme bildirimi",
            )
            .await;
            if let Err(error) = result {
                tracing::warn!(target: "omnitrix::notify", %error, "notification failed");
            }
        });
        Ok(())
    }
}

struct NativeBackup {
    handle: tokio::runtime::Handle,
}

impl OmniBackup for NativeBackup {
    fn backup_now(&self) -> Result<String, String> {
        let config = xai_grok_shell::session::backup::backup_config_from_env().unwrap_or_default();
        let sessions = xai_grok_config::grok_home().join("sessions");
        self.handle.spawn(async move {
            match xai_grok_shell::session::backup::create_backup(&sessions, &config).await {
                Ok(report) => tracing::info!(
                    target: "omnitrix::backup",
                    path = %report.path,
                    sessions = report.sessions,
                    bytes = report.bytes,
                    sha256 = %report.sha256,
                    "backup completed"
                ),
                Err(error) => tracing::warn!(target: "omnitrix::backup", %error, "backup failed"),
            }
        });
        Ok("arka planda baslatildi".to_string())
    }
}

static RUNTIME_STATE: std::sync::OnceLock<Arc<Mutex<RuntimeState>>> = std::sync::OnceLock::new();

/// Install native bridges immediately and start metadata discovery in the
/// background. All bridges share the same process-local state; there is no
/// second synchronisation/runtime process.
pub fn start_async() {
    let state = Arc::new(Mutex::new(RuntimeState::default()));
    if RUNTIME_STATE.set(Arc::clone(&state)).is_err() {
        return;
    }
    let _ = crate::omni_bridge::install(Arc::new(NativeSnapshot(Arc::clone(&state))));
    let _ = crate::omni_bridge::install_keys(Arc::new(NativeKeys(Arc::clone(&state))));
    crate::omni_bridge::install_sink(Arc::new(NativeEvents(Arc::clone(&state))));

    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _ = crate::omni_bridge::install_router(Arc::new(NativeRouter { root }));

    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let notify_config = notify_config_from_env();
        let _ = xai_grok_hooks::notify::install(notify_config.clone());
        crate::omni_bridge::install_notify(Arc::new(NativeNotify {
            config: notify_config,
            handle: handle.clone(),
        }));
        crate::omni_bridge::install_backup(Arc::new(NativeBackup { handle }));
        tokio::spawn(refresh_state(state));
    }
}

/// Re-read the authoritative OpenCode/config sources after `/keys` or
/// connection changes. The UI remains responsive while probes run.
pub fn refresh() {
    let Some(state) = RUNTIME_STATE.get().cloned() else {
        return;
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(refresh_state(state));
    }
}

async fn refresh_state(state: Arc<Mutex<RuntimeState>>) {
    let result = tokio::task::spawn_blocking(discover_blocking).await;
    let mut current = lock(&state);
    match result {
        Ok(Ok(discovery)) => {
            current.phase = OmniPhase::Ready;
            current.healthy = true;
            current.providers = discovery.providers;
            current.storage_bytes = discovery.storage_bytes;
            current.key_summary = discovery.key_summary;
        }
        Ok(Err(error)) => {
            current.phase = OmniPhase::Degraded;
            current.healthy = false;
            tracing::warn!(target: "omnitrix::runtime", %error, "metadata discovery failed");
        }
        Err(error) => {
            current.phase = OmniPhase::Degraded;
            current.healthy = false;
            tracing::warn!(target: "omnitrix::runtime", %error, "metadata task failed");
        }
    }
}

struct Discovery {
    providers: usize,
    storage_bytes: u64,
    key_summary: KeysSummary,
}

fn discover_blocking() -> anyhow::Result<Discovery> {
    let provider_urls = configured_provider_urls();
    let key_summary = discover_opencode_keys(&provider_urls);
    let providers = configured_provider_count().max(key_summary.by_provider.len());
    let storage_bytes = directory_size(&xai_grok_config::grok_home().join("sessions"));
    Ok(Discovery {
        providers,
        storage_bytes,
        key_summary,
    })
}

fn discover_opencode_keys(provider_urls: &BTreeMap<String, String>) -> KeysSummary {
    #[derive(serde::Deserialize)]
    struct OpenCodeRecord {
        #[serde(rename = "type", default)]
        kind: Option<String>,
        #[serde(default)]
        key: Option<zeroize::Zeroizing<String>>,
    }

    let Some(def) = xai_omni_keychain::find_stack_def("opencode") else {
        return KeysSummary::default();
    };
    let Some(path) = xai_omni_keychain::resolve_stack_path(def) else {
        return KeysSummary::default();
    };
    let Ok(raw) = std::fs::read(path) else {
        return KeysSummary::default();
    };
    let raw = zeroize::Zeroizing::new(raw);
    let Ok(map) = serde_json::from_slice::<BTreeMap<String, OpenCodeRecord>>(raw.as_slice()) else {
        return KeysSummary::default();
    };
    let mut counts = BTreeMap::<String, usize>::new();
    let mut dead = 0usize;
    for (provider, value) in map {
        if matches!(value.kind.as_deref(), Some("oauth" | "wellknown")) {
            continue;
        }
        let Some(key) = value.key else {
            continue;
        };
        if key.trim().is_empty() {
            continue;
        }
        let base_url = provider_urls
            .get(&provider)
            .map(String::as_str)
            .or_else(|| default_provider_url(&provider, key.as_str()));
        if base_url
            .is_some_and(|url| xai_grok_shell::agent::config::verify_key_live(url, key.as_str()))
        {
            *counts.entry(provider).or_default() += 1;
        } else {
            dead = dead.saturating_add(1);
        }
    }
    let live = counts.values().sum();
    KeysSummary {
        live,
        dead,
        by_provider: counts.into_iter().collect(),
    }
}

fn configured_provider_count() -> usize {
    configured_provider_urls().len()
}

fn configured_provider_urls() -> BTreeMap<String, String> {
    let path = xai_grok_config::grok_home().join("config.toml");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&raw) else {
        return BTreeMap::new();
    };
    value
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .map(|providers| {
            providers
                .iter()
                .filter_map(|(id, value)| {
                    let url = value.get("base_url")?.as_str()?.trim();
                    (!url.is_empty()).then(|| (id.clone(), url.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn default_provider_url(provider: &str, key: &str) -> Option<&'static str> {
    let normalized = provider.to_ascii_lowercase();
    let kind = if normalized.contains("anthropic") {
        Some(xai_grok_shell::agent::auth_method::ProviderKind::Anthropic)
    } else if normalized.contains("openrouter") {
        Some(xai_grok_shell::agent::auth_method::ProviderKind::OpenRouter)
    } else if normalized.contains("deepseek") {
        Some(xai_grok_shell::agent::auth_method::ProviderKind::DeepSeek)
    } else if normalized == "xai" || normalized.contains("grok") {
        Some(xai_grok_shell::agent::auth_method::ProviderKind::Xai)
    } else if normalized.contains("google") || normalized.contains("gemini") {
        Some(xai_grok_shell::agent::auth_method::ProviderKind::Google)
    } else if normalized.contains("openai") {
        Some(xai_grok_shell::agent::auth_method::ProviderKind::OpenAI)
    } else {
        xai_grok_shell::agent::auth_method::ProviderKind::detect_from_key(key)
    };
    kind.map(|kind| kind.default_base_url())
}

fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.metadata() {
            Ok(metadata) if metadata.is_dir() => directory_size(&entry.path()),
            Ok(metadata) => metadata.len(),
            Err(_) => 0,
        })
        .sum()
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn notify_config_from_env() -> xai_grok_hooks::notify::NotifyConfig {
    xai_grok_hooks::notify::NotifyConfig {
        telegram_bot_token: env("OMNITRIX_KEYS_TELEGRAM_BOT_TOKEN"),
        telegram_chat_id: env("OMNITRIX_KEYS_TELEGRAM_CHAT_ID"),
        twilio_sid: env("OMNITRIX_KEYS_TWILIO_ACCOUNT_SID"),
        twilio_auth_token: env("OMNITRIX_KEYS_TWILIO_AUTH_TOKEN"),
        twilio_from: env("OMNITRIX_KEYS_TWILIO_FROM"),
        twilio_to: env("OMNITRIX_KEYS_TWILIO_TO"),
        escalation: true,
        dedup_window_secs: 60,
    }
}

/// Publish the authoritative pager agent state to `/omni-dashboard`.
pub fn publish_agents<'a>(
    agents: impl Iterator<Item = (usize, &'a crate::app::agent_view::AgentView)>,
) {
    let rows = agents
        .map(|(id, agent)| {
            let status = if agent.session.state.is_turn_running() {
                "running"
            } else if agent.session.state.is_cancelling() {
                "cancelling"
            } else if agent.session.state.is_busy() {
                "command"
            } else {
                "idle"
            };
            AgentRow {
                id: i64::try_from(id).unwrap_or(i64::MAX),
                tier: "native".to_string(),
                status: status.to_string(),
                task_title: crate::views::session_title::entry_title(agent),
            }
        })
        .collect();
    crate::omni_bridge::publish_live_agents(rows);
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[serial_test::serial(OMNI_BRIDGE)]
    async fn bootstrap_installs_immediate_snapshot() {
        super::start_async();
        let snapshot = crate::omni_bridge::snapshot().expect("snapshot bridge installed");
        assert!(matches!(
            snapshot.phase,
            crate::omni_bridge::OmniPhase::Starting
                | crate::omni_bridge::OmniPhase::Ready
                | crate::omni_bridge::OmniPhase::Degraded
        ));
        assert!(crate::omni_bridge::router().is_some());
        assert!(crate::omni_bridge::notify().is_some());
        assert!(crate::omni_bridge::backup().is_some());
        assert!(crate::omni_bridge::keys_summary().is_some());
    }
}
