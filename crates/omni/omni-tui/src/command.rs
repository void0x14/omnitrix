//! Yazma yolu (MASTER-PLAN 6.2).
//!
//! TUI kendi komut tipini tanimlamaz; `omni_proto::Command` uretir ve bir
//! kanaldan `omni-control`'e verir. Cekirdek uygular, sonuc `StateEvent` olarak
//! **her iki yuze** geri doner — yani TUI kendi gonderdigi komutun sonucunu da
//! akistan ogrenir, yerel olarak tahmin etmez.

use omni_proto::{AgentId, ApprovalDecision, Command, RoutingStrategy, TaskId};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::TuiError;

/// `interrupts.source` / denetim kaydi icin bu yuzun adi.
pub const SOURCE: &str = "tui";

/// `capability_audit.approver` icin varsayilan kimlik.
pub const DEFAULT_APPROVER: &str = "tui-operator";

/// Komutlarin cekirdege aktigi uc.
///
/// Sinirsiz kanal bilincli secimdir: TUI olay dongusu asla komut gonderirken
/// bloklanmamalidir (kapanis O(1) kalmali, 8.1). Kanal derinligini kontrol
/// duzlemi degil, kullanicinin tus hizi belirler.
#[derive(Debug, Clone)]
pub struct CommandOutbox {
    tx: UnboundedSender<Command>,
    approver: String,
}

impl CommandOutbox {
    /// Yeni kanal acar; alici ucu `omni-control` baglayicisina verilir.
    pub fn channel() -> (Self, UnboundedReceiver<Command>) {
        let (tx, rx) = unbounded_channel();
        (
            Self {
                tx,
                approver: DEFAULT_APPROVER.to_string(),
            },
            rx,
        )
    }

    /// Var olan bir gonderici ucundan kutu kurar.
    pub fn from_sender(tx: UnboundedSender<Command>) -> Self {
        Self {
            tx,
            approver: DEFAULT_APPROVER.to_string(),
        }
    }

    /// Onay komutlarinda kullanilacak kimligi degistirir.
    pub fn with_approver(mut self, approver: impl Into<String>) -> Self {
        self.approver = approver.into();
        self
    }

    /// Onay kimligi.
    pub fn approver(&self) -> &str {
        &self.approver
    }

    /// Komutu kontrol duzlemine gonderir.
    ///
    /// # Errors
    /// Alici uc kapandiysa [`TuiError::CommandChannelClosed`] doner.
    pub fn send(&self, command: Command) -> Result<(), TuiError> {
        let kind = command.kind();
        tracing::debug!(command = kind, "komut kontrol duzlemine yollaniyor");
        self.tx
            .send(command)
            .map_err(|_| TuiError::CommandChannelClosed(kind))
    }

    /// Yeni kok/alt gorev acar.
    ///
    /// # Errors
    /// Baslik bossa [`TuiError::EmptyInput`], kanal kapaliysa
    /// [`TuiError::CommandChannelClosed`] doner.
    pub fn spawn_task(
        &self,
        title: impl Into<String>,
        mode: impl Into<String>,
        parent_id: Option<TaskId>,
    ) -> Result<(), TuiError> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(TuiError::EmptyInput);
        }
        self.send(Command::SpawnTask {
            title: title.trim().to_string(),
            mode: mode.into(),
            parent_id,
            persona: None,
            duration_target: None,
            budget: None,
        })
    }

    /// Calisan ajana kullanici mesaji iletir (6.8).
    ///
    /// # Errors
    /// Mesaj bossa [`TuiError::EmptyInput`], kanal kapaliysa
    /// [`TuiError::CommandChannelClosed`] doner.
    pub fn write_to_agent(
        &self,
        agent_id: AgentId,
        content: impl Into<String>,
    ) -> Result<(), TuiError> {
        let content = content.into();
        if content.trim().is_empty() {
            return Err(TuiError::EmptyInput);
        }
        self.send(Command::WriteToAgent { agent_id, content })
    }

    /// Ajani keser (AS2).
    ///
    /// # Errors
    /// Kanal kapaliysa [`TuiError::CommandChannelClosed`] doner.
    pub fn interrupt(
        &self,
        agent_id: AgentId,
        kind: impl Into<String>,
        reason: Option<String>,
    ) -> Result<(), TuiError> {
        self.send(Command::Interrupt {
            agent_id,
            kind: kind.into(),
            source: SOURCE.to_string(),
            reason,
        })
    }

    /// Yetki broker'i kararini insan onayi ile kapatir (K3).
    ///
    /// # Errors
    /// Kanal kapaliysa [`TuiError::CommandChannelClosed`] doner.
    pub fn approve(
        &self,
        agent_id: AgentId,
        capability: impl Into<String>,
        target: impl Into<String>,
        decision: ApprovalDecision,
    ) -> Result<(), TuiError> {
        self.send(Command::Approve {
            agent_id,
            capability: capability.into(),
            target: target.into(),
            decision,
            approver: self.approver.clone(),
        })
    }

    /// Yonlendirme politikasini degistirir (10.1). Rol->model eslemesi
    /// `config` icinde tasinir; koda literal model adi gomulmez (AS7/I5).
    ///
    /// # Errors
    /// Kanal kapaliysa [`TuiError::CommandChannelClosed`] doner.
    pub fn set_routing(
        &self,
        policy: impl Into<String>,
        strategy: RoutingStrategy,
        config: serde_json::Value,
    ) -> Result<(), TuiError> {
        self.send(Command::SetRouting {
            policy: policy.into(),
            strategy,
            config,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gorev_komutu_kanala_duser() {
        let (outbox, mut rx) = CommandOutbox::channel();
        outbox
            .spawn_task("  dikey dilim  ", "user_driven", None)
            .expect("gonderim");
        let cmd = rx.try_recv().expect("komut");
        match cmd {
            Command::SpawnTask { title, mode, .. } => {
                assert_eq!(title, "dikey dilim");
                assert_eq!(mode, "user_driven");
            }
            other => panic!("beklenmeyen komut: {}", other.kind()),
        }
    }

    #[test]
    fn bos_girdi_reddedilir() {
        let (outbox, mut rx) = CommandOutbox::channel();
        assert!(matches!(
            outbox.spawn_task("   ", "user_driven", None),
            Err(TuiError::EmptyInput)
        ));
        assert!(matches!(
            outbox.write_to_agent(1, "\n"),
            Err(TuiError::EmptyInput)
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn mudahale_kaynagi_tui() {
        let (outbox, mut rx) = CommandOutbox::channel();
        outbox
            .interrupt(9, "pause", Some("operator".into()))
            .expect("gonderim");
        match rx.try_recv().expect("komut") {
            Command::Interrupt {
                agent_id, source, ..
            } => {
                assert_eq!(agent_id, 9);
                assert_eq!(source, SOURCE);
            }
            other => panic!("beklenmeyen komut: {}", other.kind()),
        }
    }

    #[test]
    fn onay_kimligi_tasinir() {
        let (outbox, mut rx) = CommandOutbox::channel();
        let outbox = outbox.with_approver("kullanici");
        outbox
            .approve(3, "write", "src/a.rs", ApprovalDecision::Allow)
            .expect("gonderim");
        match rx.try_recv().expect("komut") {
            Command::Approve {
                approver, decision, ..
            } => {
                assert_eq!(approver, "kullanici");
                assert_eq!(decision, ApprovalDecision::Allow);
            }
            other => panic!("beklenmeyen komut: {}", other.kind()),
        }
    }

    #[test]
    fn kapali_kanal_hata_verir() {
        let (outbox, rx) = CommandOutbox::channel();
        drop(rx);
        assert!(matches!(
            outbox.interrupt(1, "pause", None),
            Err(TuiError::CommandChannelClosed("interrupt"))
        ));
    }

    #[test]
    fn yonlendirme_konfigten_gelir() {
        let (outbox, mut rx) = CommandOutbox::channel();
        outbox
            .set_routing(
                "default",
                RoutingStrategy::Jep,
                serde_json::json!({ "roles": { "judge": "$judge_model" } }),
            )
            .expect("gonderim");
        assert_eq!(rx.try_recv().expect("komut").kind(), "set_routing");
    }
}
