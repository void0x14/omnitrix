//! `omni-notify` — uzak erisim kanallari + bildirim (MASTER-PLAN Bolum 13, K9, 6.6).
//!
//! Tek control-plane API'si `omni-control`'dur; bu crate onun **istemcisi** olan
//! iki kanali tasir:
//!
//! | Kanal | Tasima | Not |
//! |---|---|---|
//! | Telegram bot | [`telegram`] | Komut + bildirim |
//! | WhatsApp/SMS/arama | [`twilio`] + [`voice`] | **Yalniz yuksek-onem esiginde** |
//!
//! Public IPv6 ve Tailscale kanallari ayni API'yi dogrudan konusur; bu crate'te
//! kod gerektirmezler (tasima farki, sozlesme ayni).
//!
//! Isleyis:
//! 1. [`trigger::Notification::from_state_event`] — `omni-proto` akisindan uc
//!    tetikleyiciyi ayirir: gorev bitimi, kritik hata, insan-onayi mudahalesi.
//! 2. [`dedup::DedupWindow`] — ayni imzali olay pencere dolmadan tekrar gelirse
//!    susturulur; bastirilan tekrar sayisi bir sonraki mesajda raporlanir.
//! 3. [`policy::NotifyPolicy`] — kanal esikleri **config'ten** gelir; esigin
//!    altindaki hicbir sey telefonu caldirmaz.
//! 4. [`dispatch::NotifyDispatcher`] — sirayi tek noktada yurutur.
//!
//! Invariantlar: durum tipleri `omni-proto`'dan gelir (I3); `xai-grok-voice`
//! yalnizca okunur (I2); model/dil katalogu koda gomulmez (I5); uretim yolunda
//! `unwrap`/`expect`/`panic!` yoktur (I6); kimlik bilgileri keyring'den gelir,
//! koda gomulmez ([`credential`]).

pub mod control_client;
pub mod credential;
pub mod dedup;
pub mod dispatch;
pub mod error;
pub mod policy;
pub mod telegram;
pub mod trigger;
pub mod twilio;
pub mod voice;

pub use control_client::ControlClient;
pub use credential::{
    CredentialStore, EnvCredentialStore, NotifyCredentials, StaticCredentialStore,
    TelegramCredentials, TwilioCredentials,
};
pub use dedup::{DedupVerdict, DedupWindow};
pub use dispatch::{DispatchReport, NotifyDispatcher};
pub use error::NotifyError;
pub use policy::{Channel, NotifyPolicy};
pub use telegram::{TelegramBridge, TelegramNotifier, TelegramRequest, parse_telegram_command};
pub use trigger::{Notification, NotifyTrigger};
pub use twilio::{EscalationNotifier, EscalationOutcome, TwilioNotifier};
pub use voice::CallScript;
