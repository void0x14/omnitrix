//! Tek ajan oturumu (MASTER-PLAN Bolum 20, Faz 1).
//!
//! `AgentSpec` kurulum tarifidir; `AgentSession` kurulmus ajani ve baglam
//! durumunu bir arada tutar. Tur dongusu burada DEGIL, omni-router'dadir.

use std::path::PathBuf;
use std::sync::Arc;

use omni_tools::{AsyncFileSystem, TerminalBackend, ToolNotificationHandle};
use xai_chat_state::ChatStateHandle;
use xai_grok_agent::{Agent, AgentBuilder, AgentDefinition};

use crate::error::AgentError;

/// Tek ajanin kurulum tarifi.
///
/// Tarif sahiplenilmis olarak saklanir ki ajan oturum ortasinda yeniden
/// kurulabilsin — `Agent` disaridan yeniden render edilemez.
pub struct AgentSpec {
    /// Vendored ajan tanimi (persona'dan cozulur).
    pub definition: AgentDefinition,
    /// Ajanin calisma dizini.
    pub working_directory: PathBuf,
    /// Tool durumu icin MUTLAK yol; ust dizini onceden yaratilmis olmalidir.
    pub state_path: PathBuf,
    /// Sistem promptuna eklenecek persona yonergeleri.
    pub persona_instructions: Option<String>,
    /// Diff akisi shim'i; `None` ise vendored yerel dosya sistemi kullanilir.
    pub fs: Option<Arc<dyn AsyncFileSystem>>,
}

impl AgentSpec {
    /// Zorunlu alanlarla yeni tarif.
    pub fn new(definition: AgentDefinition, working_directory: PathBuf, state_path: PathBuf) -> Self {
        Self {
            definition,
            working_directory,
            state_path,
            persona_instructions: None,
            fs: None,
        }
    }

    /// Persona yonergelerini baglar.
    pub fn with_persona_instructions(mut self, instructions: Option<String>) -> Self {
        self.persona_instructions = instructions;
        self
    }

    /// Diff akisi shim'ini baglar (5.2).
    pub fn with_fs(mut self, fs: Arc<dyn AsyncFileSystem>) -> Self {
        self.fs = Some(fs);
        self
    }

    /// Ajani kurar.
    ///
    /// Canli bir tokio calisma zamani SARTTIR: vendored `build()` iceride
    /// `tokio::spawn` cagirir. `state_path` mutlak olmalidir; aksi halde
    /// durum dosyasi surec calisma dizinine dusar.
    pub async fn build(
        self,
        terminal: Arc<dyn TerminalBackend>,
        notify: ToolNotificationHandle,
    ) -> Result<AgentSession, AgentError> {
        if !self.state_path.is_absolute() {
            return Err(AgentError::InvalidSession(format!(
                "durum yolu mutlak degil: {}",
                self.state_path.display()
            )));
        }

        let mut builder = AgentBuilder::new(self.working_directory, terminal, notify)
            .from_definition(self.definition)
            .with_persona_instructions(self.persona_instructions)
            .with_state_path(self.state_path);

        if let Some(fs) = self.fs {
            builder = builder.with_fs(fs);
        }

        let agent = builder.build().await?;
        Ok(AgentSession {
            agent,
            chat: ChatStateHandle::noop(),
        })
    }
}

/// Kurulmus tek ajan + baglam durumu.
pub struct AgentSession {
    agent: Agent,
    chat: ChatStateHandle,
}

impl AgentSession {
    /// Onceden kurulmus parcalardan oturum olusturur.
    pub fn from_parts(agent: Agent, chat: ChatStateHandle) -> Self {
        Self { agent, chat }
    }

    /// Baglam durumu tutamacini degistirir.
    pub fn with_chat_state(mut self, chat: ChatStateHandle) -> Self {
        self.chat = chat;
        self
    }

    /// Sarmalanan vendored ajan.
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// Baglam durumu tutamaci.
    pub fn chat_state(&self) -> &ChatStateHandle {
        &self.chat
    }

    /// Ajan adi.
    pub fn name(&self) -> &str {
        self.agent.name()
    }

    /// Render edilmis sistem promptu — LLM istegine bu gider.
    pub fn system_prompt(&self) -> &str {
        self.agent.system_prompt()
    }
}
