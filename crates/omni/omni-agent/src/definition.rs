//! Persona -> `AgentDefinition` kopruleyicisi (MASTER-PLAN 3.2 / 11.x).
//!
//! Faz 1'de persona KATALOGU yoktur; yalnizca tek ajanlik dilim icin gereken
//! minimal alan eslemesi burada durur. Katalog Faz 5'te baglanir.

use xai_grok_agent::AgentDefinition;
use xai_grok_agent::config::PermissionMode;

/// Bir personanin ajan tanimina donusen alanlari.
#[derive(Debug, Clone)]
pub struct PersonaSpec {
    /// Ajan adi (`AgentDefinition.name`).
    pub name: String,
    /// Kisa aciklama (`AgentDefinition.description`).
    pub description: String,
    /// Sistem promptuna eklenen persona yonergeleri.
    pub instructions: Option<String>,
    /// Acilacak tool adlari; bos ise varsayilan set kullanilir.
    pub tools: Vec<String>,
    /// Kapatilacak tool adlari.
    pub disallowed_tools: Vec<String>,
    /// Yetki modu. DIKKAT: bu yalnizca bir POLITIKA ETIKETIDIR; vendored ajan
    /// uzerinde hicbir zorlama yapmaz. Zorlama omni-tools broker'indadir (K3).
    pub permission_mode: PermissionMode,
    /// Tur ust siniri; `None` ise vendored varsayilan.
    pub max_turns: Option<u32>,
}

impl PersonaSpec {
    /// Ad ve aciklama disindaki her sey vendored varsayilanlarda kalir.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            instructions: None,
            tools: Vec::new(),
            disallowed_tools: Vec::new(),
            permission_mode: PermissionMode::default(),
            max_turns: None,
        }
    }

    /// Persona yonergelerini baglar.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Acik tool listesini baglar.
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    /// Kapali tool listesini baglar.
    pub fn with_disallowed_tools(mut self, tools: Vec<String>) -> Self {
        self.disallowed_tools = tools;
        self
    }

    /// Yetki modunu baglar.
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Tur ust sinirini baglar.
    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = Some(max_turns);
        self
    }

    /// Vendored ajan tanimina cevirir.
    ///
    /// `AgentDefinition`'in `Default` implementasyonu YOKTUR; taban olarak
    /// `builtin_defaults(name, description)` kullanilir.
    pub fn to_definition(&self) -> AgentDefinition {
        AgentDefinition {
            tools: self.tools.clone(),
            disallowed_tools: self.disallowed_tools.clone(),
            permission_mode: self.permission_mode.clone(),
            max_turns: self.max_turns,
            ..AgentDefinition::builtin_defaults(&self.name, &self.description)
        }
    }
}
