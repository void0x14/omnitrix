//! Bildirim dagitici (MASTER-PLAN Bolum 13).
//!
//! Akis: `StateEvent` -> tetikleyici eslemesi -> **dedup penceresi** -> kanal
//! esikleri -> gonderim. Susturma kararindan sonra hicbir kanal cagrilmaz;
//! boylece "Telegram susturuldu ama SMS gitti" gibi bir yol kalmaz.
//!
//! Dagitici kendi durumunu tutmaz; tek durum kaynagi cekirdektir (I3). Tek
//! tuttugu sey susturma penceresidir.

use std::sync::Arc;

use omni_proto::StateEvent;
use tracing::{debug, warn};

use crate::credential::NotifyCredentials;
use crate::dedup::{DedupVerdict, DedupWindow};
use crate::error::NotifyError;
use crate::policy::{Channel, NotifyPolicy};
use crate::telegram::TelegramNotifier;
use crate::trigger::Notification;
use crate::twilio::{EscalationNotifier, TwilioNotifier};

/// Tek olayin dagitim sonucu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchReport {
    /// Olay bir tetikleyiciye girdi mi?
    pub triggered: bool,
    /// Dedup penceresi tarafindan bastirildi mi?
    pub suppressed: bool,
    /// Son gonderimden bu yana bastirilan tekrar sayisi.
    pub suppressed_repeats: u32,
    /// Telegram mesaji gitti mi?
    pub telegram_sent: bool,
    /// SMS gitti mi?
    pub sms_sent: bool,
    /// Arama baslatildi mi?
    pub call_placed: bool,
    /// Kanal hatalari (tek kanalin dusmesi digerini engellemez).
    pub failures: Vec<String>,
}

impl DispatchReport {
    /// Tetiklenmemis olay raporu.
    #[must_use]
    pub fn untriggered() -> Self {
        Self {
            triggered: false,
            suppressed: false,
            suppressed_repeats: 0,
            telegram_sent: false,
            sms_sent: false,
            call_placed: false,
            failures: Vec::new(),
        }
    }

    /// Hicbir kanal tetiklenmedi mi?
    #[must_use]
    pub fn is_silent(&self) -> bool {
        !self.telegram_sent && !self.sms_sent && !self.call_placed
    }
}

/// Kanallari politika + susturma penceresi arkasinda toplayan dagitici.
#[derive(Debug)]
pub struct NotifyDispatcher {
    policy: NotifyPolicy,
    dedup: Arc<DedupWindow>,
    telegram: Option<TelegramNotifier>,
    escalation: Option<EscalationNotifier>,
}

impl NotifyDispatcher {
    /// Politika ile bos dagitici kurar (hicbir kanal yapilandirilmamis).
    #[must_use]
    pub fn new(policy: NotifyPolicy) -> Self {
        let dedup = Arc::new(DedupWindow::new(policy.dedup_window()));
        Self {
            policy,
            dedup,
            telegram: None,
            escalation: None,
        }
    }

    /// Keyring'den yuklenmis kimliklerle kanallari kurar. Eksik kanal atlanir;
    /// bildirim katmani acilisi bloke etmez.
    #[must_use]
    pub fn from_credentials(policy: NotifyPolicy, credentials: &NotifyCredentials) -> Self {
        let telegram = TelegramNotifier::from_notify_credentials(credentials);
        let escalation = TwilioNotifier::from_notify_credentials(credentials)
            .map(|notifier| EscalationNotifier::new(notifier, policy.clone()));
        let dedup = Arc::new(DedupWindow::new(policy.dedup_window()));
        Self {
            policy,
            dedup,
            telegram,
            escalation,
        }
    }

    /// Telegram kanalini takar.
    #[must_use]
    pub fn with_telegram(mut self, notifier: TelegramNotifier) -> Self {
        self.telegram = Some(notifier);
        self
    }

    /// Twilio yukseltme kanalini takar.
    #[must_use]
    pub fn with_escalation(mut self, escalation: EscalationNotifier) -> Self {
        self.escalation = Some(escalation);
        self
    }

    /// Susturma penceresini paylasilan bir ornekle degistirir (birden fazla
    /// dagitici ayni pencereyi kullanabilsin diye).
    #[must_use]
    pub fn with_dedup(mut self, dedup: Arc<DedupWindow>) -> Self {
        self.dedup = dedup;
        self
    }

    /// Politika.
    #[must_use]
    pub fn policy(&self) -> &NotifyPolicy {
        &self.policy
    }

    /// Susturma penceresi.
    #[must_use]
    pub fn dedup(&self) -> &Arc<DedupWindow> {
        &self.dedup
    }

    /// Telegram kanali kurulu mu?
    #[must_use]
    pub fn has_telegram(&self) -> bool {
        self.telegram.is_some()
    }

    /// Telefon kanali kurulu mu?
    #[must_use]
    pub fn has_phone(&self) -> bool {
        self.escalation.is_some()
    }

    /// Durum olayini degerlendirir ve gereken kanallara dagitir.
    ///
    /// Kanal hatasi rapora yazilir, `Err` dondurulmez: bir kanalin dusmesi
    /// digerini engellememelidir. Hicbir kanal cagrilamadiginda da rapor doner.
    pub async fn dispatch(&self, event: &StateEvent) -> DispatchReport {
        let Some(notification) = Notification::from_state_event(event, &self.policy) else {
            return DispatchReport::untriggered();
        };
        self.dispatch_notification(&notification).await
    }

    /// Hazir bildirimi dagitir (dedup dahil).
    pub async fn dispatch_notification(&self, notification: &Notification) -> DispatchReport {
        let mut report = DispatchReport::untriggered();
        report.triggered = true;

        let verdict = self.dedup.admit(&notification.signature(), notification.ts);
        let suppressed = match verdict {
            DedupVerdict::Send {
                suppressed_since_last,
            } => suppressed_since_last,
            DedupVerdict::Suppress {
                repeat,
                retry_after_secs,
            } => {
                debug!(
                    target: "omni::notify",
                    code = %notification.code,
                    repeat,
                    retry_after_secs,
                    "tekrar eden olay susturuldu"
                );
                report.suppressed = true;
                report.suppressed_repeats = repeat;
                return report;
            }
        };
        report.suppressed_repeats = suppressed;

        if self.policy.allows(Channel::Telegram, notification.level) {
            match &self.telegram {
                Some(telegram) => match telegram.send_notification(notification, suppressed).await {
                    Ok(()) => report.telegram_sent = true,
                    Err(err) => Self::kaydet(&mut report, Channel::Telegram, &err),
                },
                None => debug!(target: "omni::notify", "telegram kanali yapilandirilmamis"),
            }
        }

        // SMS/arama esigi yukseltme kanalinin kendi icinde zorlanir.
        match &self.escalation {
            Some(escalation) => match escalation.escalate(notification, suppressed).await {
                Ok(outcome) => {
                    report.sms_sent = outcome.sms_sent;
                    report.call_placed = outcome.call_placed;
                }
                Err(err) => Self::kaydet(&mut report, Channel::Sms, &err),
            },
            None => debug!(target: "omni::notify", "telefon kanali yapilandirilmamis"),
        }

        report
    }

    /// Kanal hatasini rapora ve log'a yazar.
    fn kaydet(report: &mut DispatchReport, channel: Channel, err: &NotifyError) {
        warn!(target: "omni::notify", channel = channel.as_str(), %err, "kanal gonderimi basarisiz");
        report.failures.push(format!("{}: {err}", channel.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{NoticeLevel, NoticeView, TaskView};

    fn notice_event(level: NoticeLevel, code: &str) -> StateEvent {
        StateEvent::Notice(NoticeView::new(level, code, "govde", omni_proto::now()))
    }

    fn dagitici(policy: NotifyPolicy) -> NotifyDispatcher {
        // Erisilemez uc noktalar: esik/dedup kararlari ag'a cikmadan olcülür.
        let telegram = TelegramNotifier::new("t", "1").with_api_base("http://127.0.0.1:1");
        let twilio = TwilioNotifier::new("AC1", "t", "+1", "+2").with_api_base("http://127.0.0.1:1");
        let escalation = EscalationNotifier::new(twilio, policy.clone());
        NotifyDispatcher::new(policy)
            .with_telegram(telegram)
            .with_escalation(escalation)
    }

    #[tokio::test]
    async fn tetikleyici_disi_olay_sessiz_kalir() {
        let dispatcher = dagitici(NotifyPolicy::default());
        let event = StateEvent::ResourceTick(omni_proto::ResourceGauge::at(omni_proto::now()));
        let report = dispatcher.dispatch(&event).await;
        assert!(!report.triggered);
        assert!(report.is_silent());
    }

    #[tokio::test]
    async fn tekrar_eden_olay_ikinci_kez_susturulur() {
        let dispatcher = dagitici(NotifyPolicy::default());
        let ilk = dispatcher.dispatch(&notice_event(NoticeLevel::Error, "oom")).await;
        assert!(ilk.triggered);
        assert!(!ilk.suppressed);

        let ikinci = dispatcher.dispatch(&notice_event(NoticeLevel::Error, "oom")).await;
        assert!(ikinci.suppressed);
        assert_eq!(ikinci.suppressed_repeats, 1);
        // Susturulan olayda hicbir kanal cagrilmaz.
        assert!(ikinci.is_silent());
        assert!(ikinci.failures.is_empty());
    }

    #[tokio::test]
    async fn esik_altindaki_bildirim_telefonu_caldirmaz() {
        // Telegram kanalini kapatip yalnizca telefon yolunu olcuyoruz.
        let policy = NotifyPolicy::default();
        let twilio = TwilioNotifier::new("AC1", "t", "+1", "+2").with_api_base("http://127.0.0.1:1");
        let dispatcher = NotifyDispatcher::new(policy.clone())
            .with_escalation(EscalationNotifier::new(twilio, policy));

        let task = TaskView {
            id: 1,
            parent_id: None,
            root_id: 1,
            title: "bitti".into(),
            mode: "mvp".into(),
            status: "done".into(),
            depth: 0,
            budget_allocated: None,
            budget_spent: None,
            duration_target: None,
            created_at: omni_proto::now(),
            closed_at: Some(omni_proto::now()),
        };
        let report = dispatcher.dispatch(&StateEvent::TaskUpserted(task)).await;
        assert!(report.triggered);
        // Gorev bitimi Info seviyesindedir: SMS esiginin altinda.
        assert!(!report.sms_sent);
        assert!(!report.call_placed);
        assert!(report.failures.is_empty());
    }

    #[tokio::test]
    async fn kanal_hatasi_rapora_yazilir_panik_olmaz() {
        let dispatcher = dagitici(NotifyPolicy::default());
        let report = dispatcher
            .dispatch(&notice_event(NoticeLevel::Critical, "provider_down"))
            .await;
        assert!(report.triggered);
        assert!(!report.suppressed);
        // Uc noktalar erisilemez oldugu icin iki kanal da hata yazar.
        assert!(!report.failures.is_empty());
        assert!(report.is_silent());
    }

    #[tokio::test]
    async fn yapilandirilmamis_kanal_hata_uretmez() {
        let dispatcher = NotifyDispatcher::new(NotifyPolicy::default());
        assert!(!dispatcher.has_telegram());
        assert!(!dispatcher.has_phone());
        let report = dispatcher
            .dispatch(&notice_event(NoticeLevel::Critical, "x"))
            .await;
        assert!(report.triggered);
        assert!(report.failures.is_empty());
        assert!(report.is_silent());
    }

    #[tokio::test]
    async fn bos_kimliklerle_kanal_kurulmaz() {
        let dispatcher =
            NotifyDispatcher::from_credentials(NotifyPolicy::default(), &NotifyCredentials::default());
        assert!(!dispatcher.has_telegram());
        assert!(!dispatcher.has_phone());
    }

    /// Yalnizca tek istegi yanitlayan yerel sahte Telegram uc noktasi.
    /// Gercek HTTP el sikismasi isteriz: `dispatch` icindeki gonderim yolunun
    /// (reqwest -> `sendMessage`) uc noktaya gercekten dokundugunu dogrular.
    async fn fake_telegram_api() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("dinleyici");
        let addr = listener.local_addr().expect("adres");
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    let _ = socket.read(&mut buf).await;
                    let body = br#"{"ok":true,"result":{"message_id":1,"chat":{"id":1},"text":"x"}}"#;
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn saglikli_sahte_kanala_bildirim_gonderilir() {
        // Kanal enjekte edilir; uc nokta erisilemez degil, gercek bir sahte
        // sunucudur — kanal cagrisi basarili sayilir ve rapora yansir.
        let base = fake_telegram_api().await;
        let telegram = TelegramNotifier::new("t", "1").with_api_base(&base);
        let dispatcher = NotifyDispatcher::new(NotifyPolicy::default()).with_telegram(telegram);

        let report = dispatcher
            .dispatch(&notice_event(NoticeLevel::Error, "sahte-test"))
            .await;
        assert!(report.triggered, "tetikleyiciye giren olay islenmeli");
        assert!(report.telegram_sent, "saglikli kanala giden bildirim gonderilmeli");
        assert!(report.failures.is_empty(), "hata beklenmiyordu: {:?}", report.failures);
    }
}
