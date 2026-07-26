//! Tek ajan oturumu (MASTER-PLAN Bolum 20, Faz 1).
//!
//! `AgentSpec` kurulum tarifidir; `AgentSession` kurulmus ajani ve baglam
//! durumunu bir arada tutar. Tur dongusu burada DEGIL, omni-router'dadir:
//! vendored `Agent`'in `run`/`turn`/`step` metodu YOKTUR. Oturum yalnizca
//! `Agent` (icinde `ToolBridge`) + `ChatStateHandle` demetini disariya verir.
//!
//! Faz 1'de baglanan vendored `with_*` metotlari SADECE sunlardir:
//! `with_tools`, `with_disallowed_tools`, `with_fs`, `with_state_path`.
//! `with_persona_instructions`, `with_compaction_policy`, `with_memory_backend`,
//! `with_parent_scheduler_handle` ve `with_permission_mode` bilerek DISARIDA
//! birakilmistir — sonraki fazlarda baglanir.

use std::path::PathBuf;
use std::sync::Arc;

use omni_tools::{AsyncFileSystem, TerminalBackend, ToolNotificationHandle};
use xai_chat_state::ChatStateHandle;
use xai_grok_agent::config::PermissionMode;
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
    ///
    /// Vendored taraf bu dosyanin KENDISINI yazmaz: yazilan dosya
    /// `<ust dizin>/resources_state.json`'dur ve `<ust dizin>` ayni zamanda
    /// oturum klasoru olur. Yol bos/goreli birakilirsa durum surecin calisma
    /// dizinine duser — bu yuzden asagida mutlaklik zorunlu tutulur.
    pub state_path: PathBuf,
    /// Tanimdaki acik tool listesini ezen istege bagli liste.
    pub tools: Option<Vec<String>>,
    /// Tanimdaki kapali tool listesini ezen istege bagli liste.
    pub disallowed_tools: Option<Vec<String>>,
    /// Diff akisi shim'i; `None` ise vendored yerel dosya sistemi kullanilir.
    pub fs: Option<Arc<dyn AsyncFileSystem>>,
}

impl AgentSpec {
    /// Zorunlu alanlarla yeni tarif.
    pub fn new(
        definition: AgentDefinition,
        working_directory: PathBuf,
        state_path: PathBuf,
    ) -> Self {
        Self {
            definition,
            working_directory,
            state_path,
            tools: None,
            disallowed_tools: None,
            fs: None,
        }
    }

    /// Acik tool listesini baglar (vendored `with_tools`).
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Kapali tool listesini baglar (vendored `with_disallowed_tools`).
    pub fn with_disallowed_tools(mut self, tools: Vec<String>) -> Self {
        self.disallowed_tools = Some(tools);
        self
    }

    /// Diff akisi shim'ini baglar (5.2, vendored `with_fs`).
    pub fn with_fs(mut self, fs: Arc<dyn AsyncFileSystem>) -> Self {
        self.fs = Some(fs);
        self
    }

    /// Tool durumu yolunu baglar (vendored `with_state_path`).
    pub fn with_state_path(mut self, state_path: PathBuf) -> Self {
        self.state_path = state_path;
        self
    }

    /// Tarifi kurulumdan once dogrular. I6: panik yok, hata `Result` ile.
    fn validate(&self) -> Result<(), AgentError> {
        if self.definition.name.trim().is_empty() {
            return Err(AgentError::InvalidSession("ajan adi bos".to_owned()));
        }
        if !self.working_directory.is_absolute() {
            return Err(AgentError::InvalidSession(format!(
                "calisma dizini mutlak degil: {}",
                self.working_directory.display()
            )));
        }
        if !self.state_path.is_absolute() {
            return Err(AgentError::InvalidSession(format!(
                "durum yolu mutlak degil: {}",
                self.state_path.display()
            )));
        }
        match self.state_path.parent() {
            Some(parent) if parent.is_dir() => Ok(()),
            Some(parent) => Err(AgentError::InvalidSession(format!(
                "durum yolunun ust dizini yok: {}",
                parent.display()
            ))),
            None => Err(AgentError::InvalidSession(format!(
                "durum yolunun ust dizini cozulemedi: {}",
                self.state_path.display()
            ))),
        }
    }

    /// Tool listesi ezmelerini tanima isler.
    ///
    /// TUZAK: vendored `AgentBuilder::resolve_definition`, `from_definition`
    /// cagrildiysa ERKEN DONER ve `with_tools` / `with_disallowed_tools`
    /// degerlerini YOK SAYAR. Bu yuzden ezmeler once tanima yazilir, sonra
    /// ayni degerler builder'a da verilir; iki yol da ayni sonucu uretir.
    fn resolved_definition(&self) -> AgentDefinition {
        let mut definition = self.definition.clone();
        if let Some(tools) = &self.tools {
            definition.tools = tools.clone();
        }
        if let Some(disallowed) = &self.disallowed_tools {
            definition.disallowed_tools = disallowed.clone();
        }
        definition
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
        self.validate()?;

        let definition = self.resolved_definition();
        let tools = definition.tools.clone();
        let disallowed_tools = definition.disallowed_tools.clone();

        let mut builder = AgentBuilder::new(self.working_directory, terminal, notify)
            .from_definition(definition)
            .with_tools(tools)
            .with_disallowed_tools(disallowed_tools)
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
///
/// Bu tip bir CALISTIRICI DEGILDIR. Tur dongusu omni-router'a aittir; burada
/// yalnizca dongunun ihtiyac duydugu parcalar okunur.
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
    ///
    /// `ToolBridge` buradan alinir: `session.agent().tool_bridge()`. Kopru
    /// tipi bilerek yeniden yayinlanmaz — xai-grok-tools bagimliligi
    /// omni-tools'ta kalir (Bolum 2 sozlesmesi).
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// Baglam durumu tutamaci (ucuz klonlanir).
    pub fn chat_state(&self) -> &ChatStateHandle {
        &self.chat
    }

    /// Baglam durumu tutamacinin klonu — router dongusune tasinabilir.
    pub fn chat_state_handle(&self) -> ChatStateHandle {
        self.chat.clone()
    }

    /// Ajan adi.
    pub fn name(&self) -> &str {
        self.agent.name()
    }

    /// Ajan aciklamasi.
    pub fn description(&self) -> &str {
        self.agent.description()
    }

    /// Kurulumda kullanilan cozulmus tanim.
    pub fn definition(&self) -> &AgentDefinition {
        self.agent.definition()
    }

    /// Yetki modu ETIKETI. Zorlama yapmaz; kapiyi omni-tools broker'i tutar.
    pub fn permission_mode(&self) -> &PermissionMode {
        self.agent.permission_mode()
    }

    /// Render edilmis sistem promptu — LLM istegine bu gider.
    pub fn system_prompt(&self) -> &str {
        self.agent.system_prompt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PersonaSpec;

    fn spec_with(working_directory: PathBuf, state_path: PathBuf) -> AgentSpec {
        let definition = PersonaSpec::new("omni-slice", "Faz 1 dikey dilim ajani").to_definition();
        AgentSpec::new(definition, working_directory, state_path)
    }

    #[test]
    fn goreli_durum_yolu_reddedilir() {
        let spec = spec_with(PathBuf::from("/tmp"), PathBuf::from("durum.json"));
        assert!(matches!(
            spec.validate(),
            Err(AgentError::InvalidSession(_))
        ));
    }

    #[test]
    fn goreli_calisma_dizini_reddedilir() {
        let spec = spec_with(PathBuf::from("calisma"), PathBuf::from("/tmp/durum.json"));
        assert!(matches!(
            spec.validate(),
            Err(AgentError::InvalidSession(_))
        ));
    }

    #[test]
    fn olmayan_ust_dizin_reddedilir() {
        let spec = spec_with(
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/omni-agent-olmayan-dizin/durum.json"),
        );
        assert!(matches!(
            spec.validate(),
            Err(AgentError::InvalidSession(_))
        ));
    }

    #[test]
    fn tool_ezmeleri_tanima_islenir() {
        let spec = spec_with(PathBuf::from("/tmp"), PathBuf::from("/tmp/durum.json"))
            .with_tools(vec!["read_file".to_owned()])
            .with_disallowed_tools(vec!["write".to_owned()]);
        let definition = spec.resolved_definition();
        assert_eq!(definition.tools, vec!["read_file".to_owned()]);
        assert_eq!(definition.disallowed_tools, vec!["write".to_owned()]);
    }

    #[test]
    fn ezme_yoksa_tanim_korunur() {
        let mut spec = spec_with(PathBuf::from("/tmp"), PathBuf::from("/tmp/durum.json"));
        spec.definition.tools = vec!["grep".to_owned()];
        let definition = spec.resolved_definition();
        assert_eq!(definition.tools, vec!["grep".to_owned()]);
    }
}
