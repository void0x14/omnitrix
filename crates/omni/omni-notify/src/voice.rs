//! Sesli arama metni (MASTER-PLAN Bolum 13 — Twilio + `xai-grok-voice`).
//!
//! Arama **yalniz yuksek-onem esiginde** yapilir; bu modul metni ve TwiML
//! govdesini uretir, esik karari [`crate::policy`]'dedir.
//!
//! Dil katalogu `xai-grok-voice`'tan gelir (I2b, tek yon): config'te yazan dil
//! kodu katalogla dogrulanir, taninmayan kod varsayilana duser. Katalog burada
//! **tekrarlanmaz** — literal dil/model listesi koda gomulmez (I5 ruhu).

use xai_grok_voice::{SttLanguage, VoiceConfig, language_for_api, stt_language_by_code};

use crate::policy::NotifyPolicy;
use crate::trigger::Notification;

/// Aramada tekrarlanma sayisi (Twilio `<Say loop=...>`). Insan telefonu
/// gec acabilir; iki tekrar makul.
pub const DEFAULT_SAY_LOOP: u8 = 2;

/// Sesli aramada okunacak metin + dili.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallScript {
    /// Katalogla dogrulanmis dil kodu (or. `tr`, `en`).
    pub language: &'static str,
    /// Okunacak duz metin.
    pub text: String,
    /// Tekrar sayisi.
    pub loop_count: u8,
}

impl CallScript {
    /// Bildirimden arama metni uretir.
    ///
    /// Metin kasten kisadir: telefonda uzun govde anlasilmaz, ayrinti
    /// Telegram/SMS'te durur.
    #[must_use]
    pub fn from_notification(notification: &Notification, policy: &NotifyPolicy) -> Self {
        let language = policy.resolved_call_language();
        let text = format!(
            "Omnitrix. {}. {}. {}",
            notification.level_tag(),
            kisalt(&notification.title, 120),
            kisalt(&notification.body, 200)
        );
        Self {
            language,
            text,
            loop_count: DEFAULT_SAY_LOOP,
        }
    }

    /// Katalogtaki dil kaydi (log/gorunum icin); dil kataloga girmiyorsa `None`.
    #[must_use]
    pub fn catalog_entry(&self) -> Option<&'static SttLanguage> {
        stt_language_by_code(self.language)
    }

    /// Twilio `<Say>` icin yerel ayar etiketi.
    ///
    /// Twilio bolgeli etiket bekler (`tr-TR`); katalog kodu tek harfli birincil
    /// alt etikettir. Katalogda olmayan kod icin kodun kendisi dondurulur —
    /// Twilio bilinmeyen etiketi kendi varsayilanina duser.
    #[must_use]
    pub fn say_locale(&self) -> String {
        match self.language {
            // Yalnizca yaygin bolgeli eslemeler; katalogun kendisi
            // `xai-grok-voice`'ta durur, burada kopyalanmaz.
            "en" => "en-US".to_owned(),
            "tr" => "tr-TR".to_owned(),
            other => other.to_owned(),
        }
    }

    /// TwiML govdesi (`Twiml` parametresi olarak gonderilir).
    #[must_use]
    pub fn to_twiml(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response><Say language=\"{}\" loop=\"{}\">{}</Say></Response>",
            xml_escape(&self.say_locale()),
            self.loop_count.max(1),
            xml_escape(&self.text)
        )
    }
}

/// Ses ucu ayarlarindan dil tercihini cozer.
///
/// `xai-grok-voice`'un [`VoiceConfig`] varsayilani, config'te `notify` altinda
/// dil verilmemis kurulumlar icin son basvurudur.
#[must_use]
pub fn language_from_voice_config(config: &VoiceConfig) -> &'static str {
    language_for_api(&config.language)
}

/// Metni verilen sinira kirpar (kelime ortasinda kesmemeye calisir).
fn kisalt(text: &str, limit: usize) -> String {
    let temiz = text.replace('\n', " ");
    if temiz.chars().count() <= limit {
        return temiz;
    }
    let kirpilmis: String = temiz.chars().take(limit).collect();
    match kirpilmis.rfind(' ') {
        Some(bosluk) if bosluk > limit / 2 => format!("{}...", &kirpilmis[..bosluk]),
        _ => format!("{kirpilmis}..."),
    }
}

/// XML metin dugumu kacisi.
fn xml_escape(text: &str) -> String {
    let mut cikti = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => cikti.push_str("&amp;"),
            '<' => cikti.push_str("&lt;"),
            '>' => cikti.push_str("&gt;"),
            '"' => cikti.push_str("&quot;"),
            '\'' => cikti.push_str("&apos;"),
            other => cikti.push(other),
        }
    }
    cikti
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{NoticeLevel, StateEvent};

    fn bildirim() -> Notification {
        let notice = omni_proto::NoticeView::new(
            NoticeLevel::Critical,
            "approval_required",
            "ajan #5 exec onayi bekliyor",
            omni_proto::now(),
        );
        let event = StateEvent::Notice(notice);
        match Notification::from_state_event(&event, &NotifyPolicy::default()) {
            Some(n) => n,
            None => panic!("bildirim bekleniyordu"),
        }
    }

    #[test]
    fn arama_metni_dili_configten_alir() {
        let policy = NotifyPolicy {
            call_language: "tr".into(),
            ..NotifyPolicy::default()
        };
        let script = CallScript::from_notification(&bildirim(), &policy);
        assert_eq!(script.language, "tr");
        assert_eq!(script.say_locale(), "tr-TR");
        assert!(script.text.starts_with("Omnitrix. CRITICAL."));
    }

    #[test]
    fn taninmayan_dil_varsayilana_duser() {
        let policy = NotifyPolicy {
            call_language: "elfce".into(),
            ..NotifyPolicy::default()
        };
        let script = CallScript::from_notification(&bildirim(), &policy);
        assert_eq!(script.language, xai_grok_voice::STT_LANGUAGE_DEFAULT);
        assert_eq!(script.say_locale(), "en-US");
    }

    #[test]
    fn katalog_kaydi_cozulur() {
        let policy = NotifyPolicy {
            call_language: "de".into(),
            ..NotifyPolicy::default()
        };
        let script = CallScript::from_notification(&bildirim(), &policy);
        let kayit = match script.catalog_entry() {
            Some(k) => k,
            None => panic!("katalog kaydi bekleniyordu"),
        };
        assert_eq!(kayit.code, "de");
    }

    #[test]
    fn twiml_xml_kacisi_yapar() {
        let script = CallScript {
            language: "en",
            text: "a & b < c > d \"e\"".into(),
            loop_count: 1,
        };
        let twiml = script.to_twiml();
        assert!(twiml.contains("a &amp; b &lt; c &gt; d &quot;e&quot;"));
        assert!(twiml.contains("language=\"en-US\""));
        assert!(twiml.starts_with("<?xml"));
    }

    #[test]
    fn sifir_tekrar_bire_yuvarlanir() {
        let script = CallScript {
            language: "en",
            text: "x".into(),
            loop_count: 0,
        };
        assert!(script.to_twiml().contains("loop=\"1\""));
    }

    #[test]
    fn uzun_metin_kirpilir() {
        let uzun = "kelime ".repeat(80);
        let kirpik = kisalt(&uzun, 40);
        assert!(kirpik.chars().count() <= 43);
        assert!(kirpik.ends_with("..."));
    }

    #[test]
    fn ses_ayarindan_dil_cozulur() {
        let config = VoiceConfig::default();
        assert_eq!(
            language_from_voice_config(&config),
            xai_grok_voice::STT_LANGUAGE_DEFAULT
        );
    }
}
