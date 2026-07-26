//! Persona -> `AgentDefinition` kopruleyicisi (MASTER-PLAN 3.2 / 11.x).
//!
//! Faz 1'de persona KATALOGU yoktur; yalnizca tek ajanlik dilim icin gereken
//! minimal alan eslemesi burada durur. Katalog Faz 5'te baglanir.
//!
//! I5: bu dosyada HICBIR literal model adi yoktur. Model kimligi disaridan
//! `PersonaSpec.model` alani ile gelir; verilmezse vendored `Inherit` kalir.

use xai_grok_agent::AgentDefinition;
use xai_grok_agent::config::{ModelOverride, PermissionMode};

use crate::error::AgentError;

/// Bir personanin ajan tanimina donusen alanlari.
#[derive(Debug, Clone)]
pub struct PersonaSpec {
    /// Ajan adi (`AgentDefinition.name`).
    pub name: String,
    /// Kisa aciklama (`AgentDefinition.description`).
    pub description: String,
    /// Sistem promptuna eklenen persona yonergeleri.
    ///
    /// Faz 1'de KULLANILMAZ: `AgentBuilder::with_persona_instructions` sonraki
    /// fazda baglanir. Alan simdiden tasindigi icin katalog gelince tanim
    /// degismeden calisir.
    pub instructions: Option<String>,
    /// Acilacak tool adlari; bos ise vendored varsayilan set kullanilir.
    pub tools: Vec<String>,
    /// Kapatilacak tool adlari.
    pub disallowed_tools: Vec<String>,
    /// Yetki modu. DIKKAT: bu yalnizca bir POLITIKA ETIKETIDIR; vendored ajan
    /// uzerinde hicbir zorlama yapmaz. Zorlama omni-tools broker'indadir (K3).
    pub permission_mode: PermissionMode,
    /// Tur ust siniri; `None` ise vendored varsayilan.
    pub max_turns: Option<u32>,
    /// Model secimi. `Inherit` ust oturumun modelini kullanir; `Override(id)`
    /// ile gelen kimlik CAGIRANIN sorumlulugudur — burada literal yoktur (I5).
    pub model: ModelOverride,
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
            model: ModelOverride::default(),
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

    /// Model kimligini disaridan baglar (I5: literal burada uretilmez).
    ///
    /// Bos ya da yalnizca bosluk iceren kimlik `Inherit` sayilir; boylece
    /// yapilandirmadan gelen bos dize sessizce gecersiz bir modele donusmez.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        let id = model_id.into();
        self.model = if id.trim().is_empty() {
            ModelOverride::Inherit
        } else {
            ModelOverride::Override(id)
        };
        self
    }

    /// Model secimini dogrudan baglar.
    pub fn with_model(mut self, model: ModelOverride) -> Self {
        self.model = model;
        self
    }

    /// Tanimi uretmeden once tutarlilik denetimi.
    ///
    /// Vendored taraf bu alanlari deserialize yolunda dogrular; biz tanimi
    /// elle kurdugumuz icin ayni kurallari burada tekrarliyoruz. I6: panik
    /// yok, hata `Result` ile tasinir.
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.name.trim().is_empty() {
            return Err(AgentError::InvalidPersona("ajan adi bos".to_owned()));
        }
        if self.description.trim().is_empty() {
            return Err(AgentError::InvalidPersona(format!(
                "'{}' icin aciklama bos",
                self.name
            )));
        }
        // Vendored `deserialize_nonzero_u32` sifiri hata sayar; elle kurulan
        // tanimda ayni kurali biz uygulariz.
        if self.max_turns == Some(0) {
            return Err(AgentError::InvalidPersona(format!(
                "'{}' icin tur ust siniri sifir olamaz",
                self.name
            )));
        }
        if let ModelOverride::Override(id) = &self.model
            && id.trim().is_empty()
        {
            return Err(AgentError::InvalidPersona(format!(
                "'{}' icin model kimligi bos",
                self.name
            )));
        }
        Ok(())
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
            model: self.model.clone(),
            ..AgentDefinition::builtin_defaults(&self.name, &self.description)
        }
    }

    /// Once dogrular, sonra cevirir.
    pub fn to_definition_checked(&self) -> Result<AgentDefinition, AgentError> {
        self.validate()?;
        Ok(self.to_definition())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varsayilan_persona_gecerli() {
        let spec = PersonaSpec::new("omni-slice", "Faz 1 dikey dilim ajani");
        assert!(spec.validate().is_ok());
        let def = spec.to_definition();
        assert_eq!(def.name, "omni-slice");
        assert_eq!(def.description, "Faz 1 dikey dilim ajani");
        assert!(matches!(def.model, ModelOverride::Inherit));
    }

    #[test]
    fn tool_ve_politika_alanlari_tasinir() {
        let spec = PersonaSpec::new("omni-slice", "aciklama")
            .with_tools(vec!["read_file".to_owned()])
            .with_disallowed_tools(vec!["write".to_owned()])
            .with_permission_mode(PermissionMode::Plan)
            .with_max_turns(8);
        let def = spec.to_definition();
        assert_eq!(def.tools, vec!["read_file".to_owned()]);
        assert_eq!(def.disallowed_tools, vec!["write".to_owned()]);
        assert_eq!(def.permission_mode, PermissionMode::Plan);
        assert_eq!(def.max_turns, Some(8));
    }

    #[test]
    fn model_kimligi_disaridan_gelir() {
        // Kimlik testte bile literal olarak yazilmaz; cagiran uretir.
        let id = format!("{}-{}", "model", 1);
        let spec = PersonaSpec::new("omni-slice", "aciklama").with_model_id(id.clone());
        assert!(matches!(spec.to_definition().model, ModelOverride::Override(v) if v == id));
    }

    #[test]
    fn bos_model_kimligi_inherit_olur() {
        let spec = PersonaSpec::new("omni-slice", "aciklama").with_model_id("   ");
        assert!(matches!(spec.model, ModelOverride::Inherit));
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn bos_ad_reddedilir() {
        let spec = PersonaSpec::new("  ", "aciklama");
        assert!(matches!(spec.validate(), Err(AgentError::InvalidPersona(_))));
    }

    #[test]
    fn sifir_tur_reddedilir() {
        let spec = PersonaSpec::new("omni-slice", "aciklama").with_max_turns(0);
        assert!(matches!(
            spec.to_definition_checked(),
            Err(AgentError::InvalidPersona(_))
        ));
    }
}
