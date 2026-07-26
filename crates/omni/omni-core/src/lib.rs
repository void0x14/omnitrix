//! omni-core — Omnitrix domain cekirdegi (MASTER-PLAN 3.2).
//!
//! Iki isi vardir:
//!
//! 1. **Tek yazar** ([`state`]): sistemdeki her durum mutasyonu [`CoreState`]
//!    uzerinden gecer (Bolum 6.2). UI komutu `apply()`, alt katman geri
//!    bildirimi `ingest()`, ilk yukleme `snapshot()`. Her mutasyon `StateEvent`
//!    uretir; olaylar `omni-control`'e verilir, o da her iki yuze yayar (K7) ve
//!    `omni-storage` writer-actor'e WAL yazar (I7).
//! 2. **Persona defteri** ([`persona`]): `config/personas/` bir kez taranir,
//!    `_schema.md`'ye gore valide edilir, `personas` cache tablosuna yazilir
//!    (Bolum 11.2). Katalogu MASTER-PLAN doldurmaz; sema + yukleyici burada.
//!
//! Kanonik tipler [`omni_proto`]'dan gelir; bu crate kendi UI tipini icat etmez
//! (I3). Uretim yolunda panik yoktur (I6).

#![forbid(unsafe_code)]

pub mod observability;
pub mod oracle;
pub mod persona;
pub mod selfhost;
pub mod state;

pub use omni_proto as proto;
pub use persona::{
    LoadedPersona, PersonaBudget, PersonaDefinition, PersonaError, PersonaRegistry, PersonaSchema,
};
pub use state::{AgentStateMachine, CoreState, RoutingSetting, DEFAULT_PERSONA, TASK_STATUS_OPEN};

use omni_proto::{AgentId, AgentState, TaskId};

/// Cekirdek hatalari. Her biri **reddedilen** bir mutasyona karsilik gelir;
/// hata donduren cagri durumu degistirmez.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Komut/olay bilinmeyen bir ajana isaret ediyor.
    #[error("bilinmeyen ajan: {0}")]
    UnknownAgent(AgentId),

    /// Komut/olay bilinmeyen bir goreve isaret ediyor.
    #[error("bilinmeyen gorev: {0}")]
    UnknownTask(TaskId),

    /// Durum makinesinde tanimsiz gecis (Bolum 6.1).
    #[error("ajan {agent_id}: {prev:?} -> {next:?} gecisi tanimsiz")]
    InvalidTransition {
        /// Gecisin denendigi ajan.
        agent_id: AgentId,
        /// Mevcut durum.
        prev: AgentState,
        /// Istenen durum.
        next: AgentState,
    },

    /// Sonlanmis ajana mutasyon denendi.
    #[error("ajan {agent_id} sonlanmis ({state:?}); islem reddedildi")]
    AgentTerminal {
        /// Hedef ajan.
        agent_id: AgentId,
        /// Sonlanma durumu.
        state: AgentState,
    },

    /// Komut govdesi kendi icinde tutarsiz.
    #[error("gecersiz komut '{command}': {reason}")]
    InvalidCommand {
        /// Komutun kanonik adi.
        command: &'static str,
        /// Gerekce.
        reason: String,
    },

    /// Alt gorev ebeveynin butce zarfini asiyor (AS4).
    #[error("gorev {task_id}: butce zarfi asildi (istenen {requested}, kalan {remaining})")]
    BudgetExceeded {
        /// Zarfi tasiyan ebeveyn gorev.
        task_id: TaskId,
        /// Istenen tahsis.
        requested: f64,
        /// Ebeveynde kalan.
        remaining: f64,
    },

    /// Gorev agaci derinlik tavanini asiyor (7.3/AS3).
    #[error("derinlik tavani asildi: {depth} > {cap}")]
    DepthExceeded {
        /// Olusacak derinlik.
        depth: u16,
        /// Yapilandirilmis tavan.
        cap: u8,
    },

    /// Persona yukleme/dogrulama hatasi (11.2).
    #[error(transparent)]
    Persona(#[from] PersonaError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hata_mesajlari_gerekce_tasir() {
        let err = CoreError::InvalidTransition {
            agent_id: 3,
            prev: AgentState::Done,
            next: AgentState::Planning,
        };
        let metin = err.to_string();
        assert!(metin.contains("ajan 3"));
        assert!(metin.contains("Done"));
        assert!(metin.contains("Planning"));
    }

    #[test]
    fn persona_hatasi_saydam_sarilir() {
        let err: CoreError = PersonaError::Invalid {
            name: "planner".to_owned(),
            reason: "aciklama bos".to_owned(),
        }
        .into();
        assert!(err.to_string().contains("planner"));
    }
}
