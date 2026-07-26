//! `omni-tui` — Omnitrix'in terminal yuzu (MASTER-PLAN 3.2 / Bolum 6.2 / Faz 2).
//!
//! **Tek kaynak kurali (I3):** bu crate kendi durum tipini tanimlamaz. Tasidigi
//! her sey `omni-proto`'nun kanonik tipidir — [`omni_proto::SystemSnapshot`],
//! [`omni_proto::StateEvent`], [`omni_proto::ResourceGauge`],
//! [`omni_proto::Command`]. Burada yalnizca *turetilmis gorunum* ve *cizim*
//! vardir.
//!
//! **Okuma yolu (6.2):** ilk yuklemede `SystemSnapshot`, sonrasinda `StateEvent`
//! akisi ile artimli guncelleme. Ayni akis WebUI'ya da gider; farkli olan
//! yalnizca render'dir (K7: "cekirdek tek, yuzler iki").
//!
//! Akis `omni-control`'un SSE/WS ucundan gelir; cerceveler
//! [`stream::ControlFrame`] ile cozulur ve [`App::apply_control`] ile
//! uygulanir. Donen [`stream::ControlOutcome::needs_resync`] `true` ise
//! (yayin tamponu tasti) surucu snapshot'i tazeler. Yerel/test surucusu sira
//! numarali [`omni_proto::StateFrame`] besliyorsa bosluk
//! [`state::ApplyOutcome::Gap`] olarak raporlanir.
//!
//! **Yazma yolu (6.2):** tus vurusu [`omni_proto::Command`] uretir, komut
//! [`command::CommandOutbox`] uzerinden `omni-control`'e gider; cekirdek
//! uygular ve sonuc akistan geri gelir. TUI yerel mutasyon yapmaz.
//!
//! **Diff gorunurlugu (9.3):** her ajanin dokundugu dosyalarin `+`/`-` grafigi
//! [`omni_proto::StateEvent::FileTouched`] akisindan beslenir.
//!
//! # Ornek
//! ```
//! use omni_tui::{App, CommandOutbox};
//!
//! let (outbox, _rx) = CommandOutbox::channel();
//! let mut app = App::new().with_outbox(outbox);
//! app.load(omni_tui::omni_proto::SystemSnapshot::empty(omni_tui::omni_proto::now()));
//! assert!(!app.should_quit());
//! ```

pub mod agent_detail;
pub mod app;
pub mod command;
pub mod dashboard;
pub mod diff_graph;
pub mod interrupt_ui;
pub mod state;
pub mod stream;

pub use agent_detail::AgentDetail;
pub use app::{App, PromptKind, View};
pub use command::CommandOutbox;
pub use dashboard::Dashboard;
pub use diff_graph::{DiffGraph, DiffRow};
pub use interrupt_ui::ApprovalPrompt;
pub use state::{AgentDiffStat, ApplyOutcome, FileDiffStat, UiState};
pub use stream::{ControlFrame, ControlOutcome};

/// Kanonik durum modeli. Cagiranin ayrica `omni-proto` bagimliligi deklare
/// etmesine gerek kalmasin diye yeniden disa vurulur (I3).
pub use omni_proto;

/// Terminal yuzu hatalari.
///
/// Uretim yolunda panik yoktur; her basarisizlik bu tip uzerinden tasinir (I6).
/// Hatalar oldurucu degildir: uygulama hatayi yardim seridinde gosterir ve
/// calismaya devam eder.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// Komut kanalinin alici ucu kapanmis (kontrol duzlemi gitti).
    #[error("komut kanali kapali: {0}")]
    CommandChannelClosed(&'static str),

    /// Komut ucu hic baglanmamis; uygulama salt-okunur.
    #[error("komut ucu bagli degil (salt-okunur yuz)")]
    NoCommandSink,

    /// Islem secili ajan gerektiriyor ama secim yok.
    #[error("secili ajan yok")]
    NoAgentSelected,

    /// Onay bekleyen tool cagrisi yok.
    #[error("onay bekleyen tool cagrisi yok")]
    NoPendingApproval,

    /// Bos girdi gonderilmeye calisildi.
    #[error("bos girdi")]
    EmptyInput,

    /// Kontrol duzleminden gelen cerceve cozulemedi (Bolum 5 tasimasi).
    #[error("kontrol cercevesi cozulemedi: {0}")]
    BadFrame(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hata_metinleri_okunur() {
        assert_eq!(
            TuiError::CommandChannelClosed("spawn_task").to_string(),
            "komut kanali kapali: spawn_task"
        );
        assert_eq!(TuiError::EmptyInput.to_string(), "bos girdi");
        assert!(TuiError::NoCommandSink.to_string().contains("salt-okunur"));
    }

    #[test]
    fn yuz_kanonik_tipleri_disa_vurur() {
        // I3: cagiran omni-proto'yu bu crate uzerinden gorur.
        let snapshot = omni_proto::SystemSnapshot::empty(omni_proto::now());
        let pano = Dashboard::from_snapshot(snapshot);
        assert!(pano.state().agents().is_empty());
        assert_eq!(pano.selected(), None);
    }
}
