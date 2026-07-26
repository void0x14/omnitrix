//! Yazma sozlesmesi (MASTER-PLAN 6.2).
//!
//! UI komutlari `omni-control`'e gider, cekirdek uygular, sonuc `StateEvent`
//! olarak **her iki UI'a** yayilir. Komut tipleri tek yerde tanimlidir; TUI ve
//! WebUI kendi komut tipini icat edemez (B5 tekrari yasak).

use serde::{Deserialize, Serialize};

use crate::{AgentId, ProtoError, TaskId};

/// UI'dan cekirdege giden yazma niyeti.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Yeni gorev ac. `parent_id` verilirse alt gorev olur (derinlik/fan-out
    /// tavanlarini scheduler zorlar, 7.3).
    SpawnTask {
        /// `tasks.title`.
        title: String,
        /// `tasks.mode` — config'ten gelen mod adi.
        mode: String,
        /// `tasks.parent_id`.
        parent_id: Option<TaskId>,
        /// Kok ajanin persona dosyasi; yoksa varsayilan cozulur (K12).
        persona: Option<String>,
        /// `tasks.duration_target` (AS13).
        duration_target: Option<String>,
        /// `tasks.budget_allocated` — zarf; `None` ise ebeveynden devralinir
        /// ya da tam-otonom modda sinirsizdir (AS4/K11).
        budget: Option<f64>,
    },

    /// Calisan ajana kullanici mesaji ilet (6.8 mudahale).
    WriteToAgent {
        /// Hedef ajan.
        agent_id: AgentId,
        /// Mesaj govdesi.
        content: String,
    },

    /// Ajani kes/duraklat (`interrupts` satiri acilir, AS2).
    Interrupt {
        /// Hedef ajan.
        agent_id: AgentId,
        /// `interrupts.kind`.
        kind: String,
        /// `interrupts.source` — komutun geldigi yuz/kanal.
        source: String,
        /// Serbest metin gerekce.
        reason: Option<String>,
    },

    /// Yetki broker'i kararini insan onayi ile kapat (`capability_audit`, K3).
    Approve {
        /// Onayi bekleyen ajan.
        agent_id: AgentId,
        /// `capability_audit.capability`.
        capability: String,
        /// `capability_audit.target`.
        target: String,
        /// `capability_audit.decision`.
        decision: ApprovalDecision,
        /// `capability_audit.approver`.
        approver: String,
    },

    /// Yonlendirme politikasini degistir (`routing_policies`, 10.1).
    SetRouting {
        /// `routing_policies.name`.
        policy: String,
        /// `routing_policies.strategy`.
        strategy: RoutingStrategy,
        /// `routing_policies.config_json` — rol->model esleme dahil; literal
        /// model adi koda gomulmez, config'ten gelir (AS7/I5).
        config: serde_json::Value,
    },
}

impl Command {
    /// Komutun kanonik adi (denetim kaydi ve UI etiketi icin).
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SpawnTask { .. } => "spawn_task",
            Self::WriteToAgent { .. } => "write_to_agent",
            Self::Interrupt { .. } => "interrupt",
            Self::Approve { .. } => "approve",
            Self::SetRouting { .. } => "set_routing",
        }
    }

    /// Komutun hedefledigi ajan, varsa.
    #[must_use]
    pub fn target_agent(&self) -> Option<AgentId> {
        match self {
            Self::WriteToAgent { agent_id, .. }
            | Self::Interrupt { agent_id, .. }
            | Self::Approve { agent_id, .. } => Some(*agent_id),
            Self::SpawnTask { .. } | Self::SetRouting { .. } => None,
        }
    }

    /// Komut durum mutasyonu uretiyor mu? (Hepsi uretir; tek yazar yolu
    /// `omni-core` uzerinden gecer, I7.)
    #[must_use]
    pub fn is_mutating(&self) -> bool {
        true
    }

    /// SSE/WS govdesine cevirir.
    ///
    /// # Errors
    /// Serilestirme basarisiz olursa [`ProtoError::Json`] doner.
    pub fn to_json(&self) -> Result<String, ProtoError> {
        crate::to_json(self)
    }

    /// UI'dan gelen JSON govdesini cozer.
    ///
    /// # Errors
    /// Govde beklenen sekilde degilse [`ProtoError::Json`] doner.
    pub fn from_json(raw: &str) -> Result<Self, ProtoError> {
        crate::from_json(raw)
    }
}

/// Yetki broker'i karari (`capability_audit.decision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Izin verildi.
    Allow,
    /// Reddedildi.
    Deny,
}

impl ApprovalDecision {
    /// SQLite `TEXT` degerinin kanonik hali.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }

    /// SQLite `TEXT` degerinden cozer.
    ///
    /// # Errors
    /// Deger sema disindaysa [`ProtoError::UnknownVariant`] doner.
    pub fn from_db_str(raw: &str) -> Result<Self, ProtoError> {
        match raw {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            _ => Err(ProtoError::UnknownVariant {
                field: "capability_audit.decision",
                value: raw.to_string(),
            }),
        }
    }
}

/// Yonlendirme stratejisi.
///
/// SQLite karsiligi: `routing_policies.strategy CHECK(strategy IN
/// ('round_robin','weighted','fallback','jep'))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    /// Yuku canli anahtarlara sirayla dagit.
    RoundRobin,
    /// Yuku agirliklara gore dagit.
    Weighted,
    /// Hata/bakiye-bitti durumunda siradaki calisana gec.
    Fallback,
    /// Judge-Executor-Planner rol ayrimi (10.1).
    Jep,
}

impl RoutingStrategy {
    /// SQLite `TEXT` degerinin kanonik hali.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::Weighted => "weighted",
            Self::Fallback => "fallback",
            Self::Jep => "jep",
        }
    }

    /// SQLite `TEXT` degerinden cozer.
    ///
    /// # Errors
    /// Deger sema disindaysa [`ProtoError::UnknownVariant`] doner.
    pub fn from_db_str(raw: &str) -> Result<Self, ProtoError> {
        match raw {
            "round_robin" => Ok(Self::RoundRobin),
            "weighted" => Ok(Self::Weighted),
            "fallback" => Ok(Self::Fallback),
            "jep" => Ok(Self::Jep),
            _ => Err(ProtoError::UnknownVariant {
                field: "routing_policies.strategy",
                value: raw.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_task_json_gidis_donus() {
        let cmd = Command::SpawnTask {
            title: "dikey dilim".into(),
            mode: "user_driven".into(),
            parent_id: None,
            persona: Some("planner".into()),
            duration_target: Some("mvp".into()),
            budget: Some(5.0),
        };
        let raw = cmd.to_json().expect("kodlama");
        assert!(raw.contains("\"cmd\":\"spawn_task\""));
        let geri = Command::from_json(&raw).expect("cozme");
        assert_eq!(geri.kind(), "spawn_task");
        assert_eq!(geri.target_agent(), None);
    }

    #[test]
    fn hedef_ajan_cozulur() {
        let cmd = Command::WriteToAgent {
            agent_id: 11,
            content: "devam et".into(),
        };
        assert_eq!(cmd.target_agent(), Some(11));
        assert!(cmd.is_mutating());
    }

    #[test]
    fn karar_db_gidis_donus() {
        for d in [ApprovalDecision::Allow, ApprovalDecision::Deny] {
            let geri = ApprovalDecision::from_db_str(d.as_db_str()).expect("cozme");
            assert_eq!(geri, d);
        }
        assert!(matches!(
            ApprovalDecision::from_db_str("belki"),
            Err(ProtoError::UnknownVariant { .. })
        ));
    }

    #[test]
    fn strateji_db_gidis_donus() {
        for s in [
            RoutingStrategy::RoundRobin,
            RoutingStrategy::Weighted,
            RoutingStrategy::Fallback,
            RoutingStrategy::Jep,
        ] {
            let geri = RoutingStrategy::from_db_str(s.as_db_str()).expect("cozme");
            assert_eq!(geri, s);
        }
        assert!(RoutingStrategy::from_db_str("yok").is_err());
    }

    #[test]
    fn set_routing_config_tasir() {
        let cmd = Command::SetRouting {
            policy: "default".into(),
            strategy: RoutingStrategy::Jep,
            config: serde_json::json!({ "roles": { "judge": "$judge_model" } }),
        };
        let raw = cmd.to_json().expect("kodlama");
        let geri = Command::from_json(&raw).expect("cozme");
        assert_eq!(geri.kind(), "set_routing");
    }
}
