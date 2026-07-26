//! Bildirim politikasi: kanal esikleri + tetikleyici tanimlari (Bolum 13).
//!
//! **Esik config'ten gelir, koda gomulu degildir.** `omni-config`'in etkin
//! JSON agacindan `notify` alt agaci alinip [`NotifyPolicy::from_json`]'a
//! verilir; bu crate `omni-config`'e baglanmaz (config okuma tek noktadadir).
//!
//! Varsayilanlar Bolum 13 tablosunu kodlar:
//! - Telegram: her seviye (komut + bildirim kanali).
//! - SMS: yalnizca `error` ve ustu.
//! - Arama: yalnizca `critical` — telefonu insan mudahalesi disinda hicbir sey
//!   caldirmaz.

use chrono::Duration;
use omni_proto::NoticeLevel;
use serde::{Deserialize, Serialize};

use crate::error::NotifyError;
use crate::dedup::DEFAULT_WINDOW_SECS;

/// Politikanin okundugu config alt agaci (`notify.*`).
pub const CONFIG_KEY: &str = "notify";

/// Bildirimin cikabilecegi kanal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Telegram bot mesaji.
    Telegram,
    /// Twilio SMS / WhatsApp metni.
    Sms,
    /// Twilio sesli arama.
    Call,
}

impl Channel {
    /// Kanonik kanal adi (log ve imza icin).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Sms => "sms",
            Self::Call => "call",
        }
    }

    /// Telefonu calan kanallar (SMS/arama). Bunlar yalnizca yuksek-onem
    /// esiginde tetiklenir.
    #[must_use]
    pub fn is_phone(self) -> bool {
        matches!(self, Self::Sms | Self::Call)
    }
}

fn varsayilan_telegram_esik() -> NoticeLevel {
    NoticeLevel::Info
}

fn varsayilan_sms_esik() -> NoticeLevel {
    NoticeLevel::Error
}

fn varsayilan_call_esik() -> NoticeLevel {
    NoticeLevel::Critical
}

fn varsayilan_pencere() -> i64 {
    DEFAULT_WINDOW_SECS
}

fn varsayilan_bitmis_durumlar() -> Vec<String> {
    vec!["done".into(), "completed".into(), "closed".into()]
}

fn varsayilan_basarisiz_durumlar() -> Vec<String> {
    vec!["failed".into(), "error".into(), "aborted".into()]
}

fn varsayilan_onay_turleri() -> Vec<String> {
    vec![
        "approval".into(),
        "capability_approval".into(),
        "human_approval".into(),
    ]
}

fn varsayilan_dil() -> String {
    xai_grok_voice::STT_LANGUAGE_DEFAULT.to_owned()
}

/// Bildirim politikasi. Tum alanlar config'ten gelir; eksik alan varsayilana
/// duser (yapilandirilmamis kurulum sessizce calisir ama telefon calmaz).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotifyPolicy {
    /// Telegram alt esigi.
    pub telegram_min_level: NoticeLevel,
    /// SMS alt esigi.
    pub sms_min_level: NoticeLevel,
    /// Arama alt esigi.
    pub call_min_level: NoticeLevel,
    /// Susturma penceresi (saniye).
    pub dedup_window_secs: i64,
    /// "Gorev bitti" sayilan `tasks.status` degerleri.
    pub done_statuses: Vec<String>,
    /// "Gorev basarisiz" sayilan `tasks.status` degerleri.
    pub failed_statuses: Vec<String>,
    /// Insan onayi gerektiren `interrupts.kind` degerleri.
    pub approval_interrupt_kinds: Vec<String>,
    /// Sesli aramada konusulacak dil (STT katalog kodu; bkz. `xai-grok-voice`).
    pub call_language: String,
    /// Telegram'dan komut kabul edilen sohbet kimlikleri. Bos liste =
    /// **hicbiri** — kanal komut kabul etmez, yalnizca bildirim gonderir (K9).
    pub telegram_allowed_chat_ids: Vec<i64>,
}

impl Default for NotifyPolicy {
    fn default() -> Self {
        Self {
            telegram_min_level: varsayilan_telegram_esik(),
            sms_min_level: varsayilan_sms_esik(),
            call_min_level: varsayilan_call_esik(),
            dedup_window_secs: varsayilan_pencere(),
            done_statuses: varsayilan_bitmis_durumlar(),
            failed_statuses: varsayilan_basarisiz_durumlar(),
            approval_interrupt_kinds: varsayilan_onay_turleri(),
            call_language: varsayilan_dil(),
            telegram_allowed_chat_ids: Vec::new(),
        }
    }
}

impl NotifyPolicy {
    /// Config alt agacindan politika kurar.
    ///
    /// `null` deger varsayilani verir; boylece `notify` anahtari hic
    /// tanimlanmamis kurulum da calisir.
    ///
    /// # Errors
    /// Alan tipi sema disiysa [`NotifyError::Config`] doner.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, NotifyError> {
        if value.is_null() {
            return Ok(Self::default());
        }
        serde_json::from_value(value.clone()).map_err(|err| NotifyError::Config(err.to_string()))
    }

    /// Kanalin alt esigi.
    #[must_use]
    pub fn min_level(&self, channel: Channel) -> NoticeLevel {
        match channel {
            Channel::Telegram => self.telegram_min_level,
            Channel::Sms => self.sms_min_level,
            Channel::Call => self.call_min_level,
        }
    }

    /// Bu seviye bu kanaldan cikabilir mi? Esigin altindaki hicbir sey telefonu
    /// caldirmaz (Bolum 13 kapisi).
    #[must_use]
    pub fn allows(&self, channel: Channel, level: NoticeLevel) -> bool {
        level >= self.min_level(channel)
    }

    /// Susturma penceresi.
    #[must_use]
    pub fn dedup_window(&self) -> Duration {
        Duration::seconds(self.dedup_window_secs.max(0))
    }

    /// Durum "gorev bitti" mi?
    #[must_use]
    pub fn is_done_status(&self, status: &str) -> bool {
        esles(&self.done_statuses, status)
    }

    /// Durum "gorev basarisiz" mi?
    #[must_use]
    pub fn is_failed_status(&self, status: &str) -> bool {
        esles(&self.failed_statuses, status)
    }

    /// Mudahale turu insan onayi gerektiriyor mu?
    #[must_use]
    pub fn is_approval_kind(&self, kind: &str) -> bool {
        esles(&self.approval_interrupt_kinds, kind)
    }

    /// Sesli aramada kullanilacak, katalogla dogrulanmis dil kodu.
    ///
    /// `xai-grok-voice` katalogu tek dogruluk kaynagidir; taninmayan kod
    /// varsayilana duser (I5 ruhu — literal katalog burada tekrarlanmaz).
    #[must_use]
    pub fn resolved_call_language(&self) -> &'static str {
        xai_grok_voice::language_for_api(&self.call_language)
    }
}

/// Kucuk/buyuk harf ve bosluk duyarsiz liste eslemesi.
fn esles(liste: &[String], deger: &str) -> bool {
    let hedef = deger.trim();
    liste
        .iter()
        .any(|aday| aday.trim().eq_ignore_ascii_case(hedef))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varsayilan_esikler_bolum13_tablosunu_kodlar() {
        let policy = NotifyPolicy::default();
        // Telegram her seviyeyi alir.
        assert!(policy.allows(Channel::Telegram, NoticeLevel::Info));
        // SMS yalnizca error ve ustu.
        assert!(!policy.allows(Channel::Sms, NoticeLevel::Warn));
        assert!(policy.allows(Channel::Sms, NoticeLevel::Error));
        // Arama yalnizca critical.
        assert!(!policy.allows(Channel::Call, NoticeLevel::Error));
        assert!(policy.allows(Channel::Call, NoticeLevel::Critical));
    }

    #[test]
    fn esik_configten_okunur() {
        let raw = serde_json::json!({
            "sms_min_level": "critical",
            "call_min_level": "critical",
            "dedup_window_secs": 30,
            "telegram_allowed_chat_ids": [ -100200 ]
        });
        let policy = match NotifyPolicy::from_json(&raw) {
            Ok(p) => p,
            Err(err) => panic!("politika okunamadi: {err}"),
        };
        assert!(!policy.allows(Channel::Sms, NoticeLevel::Error));
        assert_eq!(policy.dedup_window(), Duration::seconds(30));
        assert_eq!(policy.telegram_allowed_chat_ids, vec![-100_200]);
        // Verilmeyen alanlar varsayilanda kalir.
        assert_eq!(policy.telegram_min_level, NoticeLevel::Info);
    }

    #[test]
    fn null_config_varsayilani_verir() {
        let policy = match NotifyPolicy::from_json(&serde_json::Value::Null) {
            Ok(p) => p,
            Err(err) => panic!("politika okunamadi: {err}"),
        };
        assert_eq!(policy.call_min_level, NoticeLevel::Critical);
    }

    #[test]
    fn bozuk_config_hata_verir() {
        let raw = serde_json::json!({ "sms_min_level": "cok-acil" });
        assert!(matches!(
            NotifyPolicy::from_json(&raw),
            Err(NotifyError::Config(_))
        ));
    }

    #[test]
    fn durum_eslemesi_harf_duyarsiz() {
        let policy = NotifyPolicy::default();
        assert!(policy.is_done_status("DONE"));
        assert!(policy.is_failed_status(" failed "));
        assert!(!policy.is_done_status("running"));
        assert!(policy.is_approval_kind("capability_approval"));
    }

    #[test]
    fn arama_dili_katalogla_dogrulanir() {
        let policy = NotifyPolicy {
            call_language: "tr".into(),
            ..NotifyPolicy::default()
        };
        assert_eq!(policy.resolved_call_language(), "tr");

        let policy = NotifyPolicy {
            call_language: "klingonca".into(),
            ..NotifyPolicy::default()
        };
        assert_eq!(
            policy.resolved_call_language(),
            xai_grok_voice::STT_LANGUAGE_DEFAULT
        );
    }

    #[test]
    fn negatif_pencere_sifira_kirpilir() {
        let policy = NotifyPolicy {
            dedup_window_secs: -5,
            ..NotifyPolicy::default()
        };
        assert_eq!(policy.dedup_window(), Duration::zero());
    }
}
