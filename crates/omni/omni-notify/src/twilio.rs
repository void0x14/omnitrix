//! Twilio kanali: WhatsApp/SMS + sesli arama (MASTER-PLAN Bolum 13).
//!
//! **Yalniz yuksek-onem esiginde.** Esik `NoticeLevel` uzerinden config'ten
//! gelir; esigin altindaki hicbir bildirim telefonu caldirmaz. Karar
//! [`EscalationNotifier::escalate`] icinde tek noktada verilir ki "bir yerde
//! unutulmus ikinci yol" olmasin.

use reqwest::Client;
use tracing::{debug, info};
use zeroize::Zeroizing;

use crate::credential::{NotifyCredentials, TwilioCredentials};
use crate::error::{NotifyError, http_error};
use crate::policy::{Channel, NotifyPolicy};
use crate::trigger::Notification;
use crate::voice::CallScript;

/// Geriye donuk ad: kanal hatalari artik tek tipte toplanir.
pub type TwilioError = NotifyError;

/// Varsayilan Twilio API koku.
pub const DEFAULT_API_BASE: &str = "https://api.twilio.com";

/// Twilio SMS govde siniri.
pub const MAX_SMS_LEN: usize = 1600;

/// Yukseltme mesajlarinin oneki.
const ESCALATION_PREFIX: &str = "[ESCALATION] ";

/// Twilio REST istemcisi (SMS + arama).
pub struct TwilioNotifier {
    account_sid: String,
    auth_token: Zeroizing<String>,
    from: String,
    to: String,
    api_base: String,
    client: Client,
}

impl std::fmt::Debug for TwilioNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TwilioNotifier")
            .field("account_sid", &self.account_sid)
            .field("auth_token", &"[REDACTED]")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("api_base", &self.api_base)
            .finish()
    }
}

impl TwilioNotifier {
    /// Acik parametrelerle kurar. Kimlik bilgileri **koda gomulmez**; cagiran
    /// bunlari keyring'den alir ([`crate::credential`]).
    #[must_use]
    pub fn new(account_sid: &str, auth_token: &str, from: &str, to: &str) -> Self {
        Self {
            account_sid: account_sid.to_owned(),
            auth_token: Zeroizing::new(auth_token.to_owned()),
            from: from.to_owned(),
            to: to.to_owned(),
            api_base: DEFAULT_API_BASE.to_owned(),
            client: Client::new(),
        }
    }

    /// Keyring'den yuklenmis kimliklerden kurar.
    #[must_use]
    pub fn from_credentials(credentials: &TwilioCredentials) -> Self {
        Self {
            account_sid: credentials.account_sid.clone(),
            auth_token: credentials.auth_token.clone(),
            from: credentials.from.clone(),
            to: credentials.to.clone(),
            api_base: DEFAULT_API_BASE.to_owned(),
            client: Client::new(),
        }
    }

    /// Kanal kurulu degilse `None` doner.
    #[must_use]
    pub fn from_notify_credentials(credentials: &NotifyCredentials) -> Option<Self> {
        credentials.twilio.as_ref().map(Self::from_credentials)
    }

    /// API kokunu degistirir (test/mock).
    #[must_use]
    pub fn with_api_base(mut self, api_base: &str) -> Self {
        self.api_base = api_base.trim_end_matches('/').to_owned();
        self
    }

    /// HTTP istemcisini degistirir.
    #[must_use]
    pub fn with_http_client(mut self, client: Client) -> Self {
        self.client = client;
        self
    }

    /// Alici numara.
    #[must_use]
    pub fn to(&self) -> &str {
        &self.to
    }

    /// Kaynak uc noktasi URL'si.
    fn resource_url(&self, resource: &str) -> String {
        format!(
            "{}/2010-04-01/Accounts/{}/{}",
            self.api_base, self.account_sid, resource
        )
    }

    /// Form gonderip yaniti dogrular.
    async fn post_form(&self, url: String, params: &[(&str, &str)]) -> Result<(), NotifyError> {
        let resp = self
            .client
            .post(url)
            .basic_auth(&self.account_sid, Some(self.auth_token.as_str()))
            .form(params)
            .send()
            .await
            .map_err(http_error)?;
        let status = resp.status();
        let body = resp.text().await.map_err(http_error)?;
        if !status.is_success() {
            return Err(NotifyError::Api(format!("status={status} body={body}")));
        }
        Ok(())
    }

    /// SMS/WhatsApp metni gonderir. Govde siniri asarsa kirpilir.
    ///
    /// # Errors
    /// Tasima hatasinda [`NotifyError::Http`], 2xx disi yanitta
    /// [`NotifyError::Api`] doner.
    pub async fn send_sms(&self, text: &str) -> Result<(), NotifyError> {
        let body = truncate_sms(text);
        self.post_form(
            self.resource_url("Messages.json"),
            &[
                ("From", self.from.as_str()),
                ("To", self.to.as_str()),
                ("Body", body.as_str()),
            ],
        )
        .await?;
        info!(target: "omni::notify", to = %self.to, "sms gonderildi");
        Ok(())
    }

    /// Sesli arama baslatir; metin TwiML olarak gomulu gider (harici webhook
    /// gerekmez).
    ///
    /// # Errors
    /// [`Self::send_sms`] ile ayni.
    pub async fn place_call(&self, script: &CallScript) -> Result<(), NotifyError> {
        let twiml = script.to_twiml();
        self.post_form(
            self.resource_url("Calls.json"),
            &[
                ("From", self.from.as_str()),
                ("To", self.to.as_str()),
                ("Twiml", twiml.as_str()),
            ],
        )
        .await?;
        info!(
            target: "omni::notify",
            to = %self.to,
            language = script.language,
            "sesli arama baslatildi"
        );
        Ok(())
    }
}

/// SMS govdesini sinira kirpar.
#[must_use]
pub fn truncate_sms(text: &str) -> String {
    if text.len() <= MAX_SMS_LEN {
        return text.to_owned();
    }
    let mut sinir = MAX_SMS_LEN - 3;
    while sinir > 0 && !text.is_char_boundary(sinir) {
        sinir -= 1;
    }
    format!("{}...", &text[..sinir])
}

/// Yukseltmenin hangi kanallara ciktigi.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EscalationOutcome {
    /// SMS gonderildi mi?
    pub sms_sent: bool,
    /// Arama baslatildi mi?
    pub call_placed: bool,
}

impl EscalationOutcome {
    /// Hicbir kanal tetiklenmedi mi? (Esigin altinda kalindi.)
    #[must_use]
    pub fn is_silent(&self) -> bool {
        !self.sms_sent && !self.call_placed
    }
}

/// Esik zorlayan yukseltme kanali. SMS ve arama karari **burada** verilir.
#[derive(Debug)]
pub struct EscalationNotifier {
    inner: TwilioNotifier,
    policy: NotifyPolicy,
}

impl EscalationNotifier {
    /// Gonderici + politika ile kurar.
    #[must_use]
    pub fn new(inner: TwilioNotifier, policy: NotifyPolicy) -> Self {
        Self { inner, policy }
    }

    /// Alttaki Twilio istemcisi.
    #[must_use]
    pub fn notifier(&self) -> &TwilioNotifier {
        &self.inner
    }

    /// Politika.
    #[must_use]
    pub fn policy(&self) -> &NotifyPolicy {
        &self.policy
    }

    /// Bildirimi esige gore yukseltir.
    ///
    /// - Seviye SMS esiginin altindaysa **hicbir sey** yapilmaz.
    /// - SMS esigini geciyorsa metin gider.
    /// - Arama esigini de geciyorsa telefon calar.
    ///
    /// # Errors
    /// Gonderim basarisiz olursa hata doner; SMS basarili olup arama duserse
    /// hata doner ama SMS geri alinmaz (kanal en-az-bir-kez semantiktir).
    pub async fn escalate(
        &self,
        notification: &Notification,
        suppressed: u32,
    ) -> Result<EscalationOutcome, NotifyError> {
        let level = notification.level;
        let mut outcome = EscalationOutcome::default();

        if !self.policy.allows(Channel::Sms, level) {
            debug!(
                target: "omni::notify",
                code = %notification.code,
                "esik altinda: telefon kanali tetiklenmedi"
            );
            return Ok(outcome);
        }

        let govde = format!(
            "{ESCALATION_PREFIX}{}",
            notification.plain_text(suppressed).replace('\n', " · ")
        );
        self.inner.send_sms(&govde).await?;
        outcome.sms_sent = true;

        if self.policy.allows(Channel::Call, level) {
            let script = CallScript::from_notification(notification, &self.policy);
            self.inner.place_call(&script).await?;
            outcome.call_placed = true;
        }

        Ok(outcome)
    }

    /// Serbest metni yukseltme oneki ile SMS olarak gonderir (esik kontrolu
    /// **yoktur**; cagiran karari kendisi vermis demektir).
    ///
    /// # Errors
    /// [`TwilioNotifier::send_sms`] ile ayni.
    pub async fn send_escalation(&self, text: &str) -> Result<(), NotifyError> {
        self.inner
            .send_sms(&format!("{ESCALATION_PREFIX}{text}"))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::NotifyTrigger;
    use omni_proto::NoticeLevel;

    /// Esik testleri icin dogrudan kurulmus bildirim: `Info` seviyesi akistan
    /// uretilemez (disariya tasinmaz), ama esik davranisi burada olculur.
    fn bildirim(level: NoticeLevel) -> Notification {
        Notification {
            trigger: NotifyTrigger::CriticalError,
            level,
            code: "provider_down".into(),
            title: "saglayici dustu".into(),
            body: "govde".into(),
            agent_id: None,
            task_id: None,
            ts: omni_proto::now(),
        }
    }

    fn yukseltici(policy: NotifyPolicy) -> EscalationNotifier {
        // Erisilemez uc nokta: esik altinda kalan cagri ag'a hic cikmamalidir.
        let notifier = TwilioNotifier::new("AC1", "gizli", "+100", "+200")
            .with_api_base("http://127.0.0.1:1");
        EscalationNotifier::new(notifier, policy)
    }

    #[tokio::test]
    async fn esik_altindaki_bildirim_telefonu_caldirmaz() {
        let yuk = yukseltici(NotifyPolicy::default());
        let sonuc = match yuk.escalate(&bildirim(NoticeLevel::Info), 0).await {
            Ok(s) => s,
            Err(err) => panic!("sessiz kalmaliydi: {err}"),
        };
        assert!(sonuc.is_silent());
    }

    #[tokio::test]
    async fn esigi_yukseltmek_error_seviyesini_de_susturur() {
        let policy = NotifyPolicy {
            sms_min_level: NoticeLevel::Critical,
            ..NotifyPolicy::default()
        };
        let yuk = yukseltici(policy);
        let sonuc = match yuk.escalate(&bildirim(NoticeLevel::Error), 0).await {
            Ok(s) => s,
            Err(err) => panic!("sessiz kalmaliydi: {err}"),
        };
        assert!(sonuc.is_silent());
    }

    #[tokio::test]
    async fn esik_ustunde_gonderim_denenir() {
        let yuk = yukseltici(NotifyPolicy::default());
        // Uc nokta erisilemez oldugu icin HTTP hatasi beklenir; onemli olan
        // cagrinin esigi gecip *denenmis* olmasi.
        let sonuc = yuk.escalate(&bildirim(NoticeLevel::Critical), 3).await;
        assert!(matches!(sonuc, Err(NotifyError::Http(_))));
    }

    #[test]
    fn uzun_sms_kirpilir() {
        let uzun = "a".repeat(MAX_SMS_LEN + 100);
        let kirpik = truncate_sms(&uzun);
        assert_eq!(kirpik.len(), MAX_SMS_LEN);
        assert!(kirpik.ends_with("..."));
    }

    #[test]
    fn kisa_sms_degismez() {
        assert_eq!(truncate_sms("kisa"), "kisa");
    }

    #[test]
    fn cok_baytli_sinirda_kirpma_panik_yapmaz() {
        let uzun = "ç".repeat(MAX_SMS_LEN);
        let kirpik = truncate_sms(&uzun);
        assert!(kirpik.len() <= MAX_SMS_LEN);
    }

    #[test]
    fn debug_ciktisi_token_sizdirmaz() {
        let notifier = TwilioNotifier::new("AC1", "COK-GIZLI", "+1", "+2");
        assert!(!format!("{notifier:?}").contains("COK-GIZLI"));
        assert_eq!(notifier.to(), "+2");
    }

    #[test]
    fn kaynak_url_hesabi_icerir() {
        let notifier = TwilioNotifier::new("AC1", "t", "+1", "+2").with_api_base("http://x/");
        assert_eq!(
            notifier.resource_url("Messages.json"),
            "http://x/2010-04-01/Accounts/AC1/Messages.json"
        );
    }
}
