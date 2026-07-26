//! Telegram kanali (MASTER-PLAN Bolum 13 — "komut + bildirim").
//!
//! Iki yonu vardir:
//! - **Bildirim:** [`TelegramNotifier`] `sendMessage` ile mesaj gonderir.
//! - **Komut:** [`TelegramBridge`] `getUpdates` ile gelen metni okur,
//!   `omni-proto::Command`'a cevirir ve kontrol duzlemine iletir. Kanal kendi
//!   is mantigini calistirmaz; tek yazma yolu API'dir (Bolum 6.2).
//!
//! Yetki: sohbet kimligi allowlist'i **kanal kimligi**dir, token dogrulamasinin
//! yerine gecmez — komut yine `omni-control`'un auth katmanindan gecer (K9).

use omni_proto::{AgentId, ApprovalDecision, Command};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::control_client::ControlClient;
use crate::credential::{NotifyCredentials, TelegramCredentials};
use crate::error::{NotifyError, http_error};
use crate::policy::NotifyPolicy;
use crate::trigger::Notification;

/// Varsayilan bicimleme modu.
const DEFAULT_PARSE_MODE: &str = "MarkdownV2";

/// Varsayilan Telegram API koku.
pub const DEFAULT_API_BASE: &str = "https://api.telegram.org";

/// `getUpdates` uzun yoklama suresi (saniye).
pub const DEFAULT_POLL_TIMEOUT_SECS: u32 = 25;

/// Komutlarin `Command::Interrupt.source` / `Command::Approve.approver`
/// alanina yazilan kanal adi.
pub const CHANNEL_SOURCE: &str = "telegram";

/// Telegram bot mesaji gondericisi.
pub struct TelegramNotifier {
    bot_token: Zeroizing<String>,
    chat_id: String,
    api_base: String,
    client: Client,
}

impl std::fmt::Debug for TelegramNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramNotifier")
            .field("chat_id", &self.chat_id)
            .field("api_base", &self.api_base)
            .field("bot_token", &"[REDACTED]")
            .finish()
    }
}

impl TelegramNotifier {
    /// Token + sohbet kimliginden gonderici kurar.
    ///
    /// Token **koda gomulmez**; cagiran onu keyring'den alir
    /// ([`crate::credential`]).
    #[must_use]
    pub fn new(bot_token: &str, chat_id: &str) -> Self {
        Self {
            bot_token: Zeroizing::new(bot_token.to_owned()),
            chat_id: chat_id.to_owned(),
            api_base: DEFAULT_API_BASE.to_owned(),
            client: Client::new(),
        }
    }

    /// Keyring'den yuklenmis kimliklerden kurar.
    #[must_use]
    pub fn from_credentials(credentials: &TelegramCredentials) -> Self {
        Self {
            bot_token: credentials.bot_token.clone(),
            chat_id: credentials.chat_id.clone(),
            api_base: DEFAULT_API_BASE.to_owned(),
            client: Client::new(),
        }
    }

    /// Kanal kurulu degilse `None` doner.
    #[must_use]
    pub fn from_notify_credentials(credentials: &NotifyCredentials) -> Option<Self> {
        credentials.telegram.as_ref().map(Self::from_credentials)
    }

    /// API kokunu degistirir (test/mock ya da ozel vekil).
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

    /// Hedef sohbet kimligi.
    #[must_use]
    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    /// Bot metodu URL'si. Token URL yolundadir; bu yuzden hicbir hata metnine
    /// URL konmaz (bkz. [`http_error`]).
    fn method_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.api_base, self.bot_token.as_str(), method)
    }

    /// Mesaj gonderir.
    ///
    /// # Errors
    /// Tasima hatasinda [`NotifyError::Http`], 2xx disi yanitta
    /// [`NotifyError::Api`] doner.
    pub async fn send_message(
        &self,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<(), NotifyError> {
        let payload = json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": parse_mode.unwrap_or(DEFAULT_PARSE_MODE),
            "disable_web_page_preview": true,
        });
        let resp = self
            .client
            .post(self.method_url("sendMessage"))
            .json(&payload)
            .send()
            .await
            .map_err(http_error)?;
        let status = resp.status();
        let body = resp.text().await.map_err(http_error)?;
        if !status.is_success() {
            return Err(NotifyError::Api(format!("status={status} body={body}")));
        }
        info!(target: "omni::notify", chat = %self.chat_id, "telegram mesaji gonderildi");
        Ok(())
    }

    /// Bildirimi MarkdownV2 kacislariyla gonderir.
    ///
    /// # Errors
    /// [`Self::send_message`] ile ayni.
    pub async fn send_notification(
        &self,
        notification: &Notification,
        suppressed: u32,
    ) -> Result<(), NotifyError> {
        let govde = escape_markdown_v2(&notification.plain_text(suppressed));
        self.send_message(&govde, Some(DEFAULT_PARSE_MODE)).await
    }

    /// `getUpdates` — `offset`'ten sonraki guncellemeler.
    ///
    /// # Errors
    /// Tasima/`ok:false` durumunda [`NotifyError::Http`] / [`NotifyError::Api`].
    pub async fn fetch_updates(
        &self,
        offset: i64,
        timeout_secs: u32,
    ) -> Result<Vec<TelegramUpdate>, NotifyError> {
        let payload = json!({
            "offset": offset,
            "timeout": timeout_secs,
            "allowed_updates": ["message"],
        });
        let resp = self
            .client
            .post(self.method_url("getUpdates"))
            .json(&payload)
            .send()
            .await
            .map_err(http_error)?;
        let status = resp.status();
        let body = resp.text().await.map_err(http_error)?;
        if !status.is_success() {
            return Err(NotifyError::Api(format!("status={status} body={body}")));
        }
        let parsed: GetUpdatesResponse = serde_json::from_str(&body)?;
        if !parsed.ok {
            return Err(NotifyError::Api(
                parsed.description.unwrap_or_else(|| "ok=false".into()),
            ));
        }
        Ok(parsed.result)
    }
}

/// `getUpdates` yaniti.
#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    ok: bool,
    #[serde(default)]
    result: Vec<TelegramUpdate>,
    #[serde(default)]
    description: Option<String>,
}

/// Tek guncelleme.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUpdate {
    /// Monoton artan guncelleme kimligi.
    pub update_id: i64,
    /// Mesaj govdesi (yalnizca `message` tipi istenir).
    #[serde(default)]
    pub message: Option<TelegramMessage>,
}

/// Mesaj.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramMessage {
    /// Sohbet.
    pub chat: TelegramChat,
    /// Metin govdesi.
    #[serde(default)]
    pub text: Option<String>,
}

/// Sohbet kimligi.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramChat {
    /// Telegram sohbet kimligi (gruplar negatiftir).
    pub id: i64,
}

/// Telegram metninin cozumlenmis hali.
#[derive(Debug, Clone)]
pub enum TelegramRequest {
    /// Cekirdege gidecek yazma komutu.
    Command(Box<Command>),
    /// Anlik goruntu istegi (`/status`).
    Snapshot,
    /// Kullanim metni (`/help`).
    Help,
}

/// Kullanim metni — komut sozdizimi tek yerde durur.
pub const HELP_TEXT: &str = "\
/status — anlik goruntu
/spawn <mod> <baslik> — gorev ac
/write <ajan_id> <mesaj> — ajana yaz
/interrupt <ajan_id> <tur> [gerekce] — ajani kes
/approve <ajan_id> <yetki> <hedef> <allow|deny> — yetki karari
/help — bu metin";

/// Telegram metnini komuta cevirir.
///
/// Komut adindaki `@botadi` soneki yok sayilir (grup sohbetlerinde Telegram
/// bunu ekler).
///
/// # Errors
/// Metin komut degilse ya da argumanlar eksik/bozuksa
/// [`NotifyError::CommandParse`] doner.
pub fn parse_telegram_command(text: &str) -> Result<TelegramRequest, NotifyError> {
    let temiz = text.trim();
    let mut parcalar = temiz.splitn(2, char::is_whitespace);
    let ham_komut = parcalar.next().unwrap_or_default();
    let kalan = parcalar.next().unwrap_or("").trim();

    if !ham_komut.starts_with('/') {
        return Err(NotifyError::CommandParse("komut '/' ile baslamali".into()));
    }
    let komut = ham_komut
        .trim_start_matches('/')
        .split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();

    match komut.as_str() {
        "status" => Ok(TelegramRequest::Snapshot),
        "help" | "start" => Ok(TelegramRequest::Help),

        "spawn" => {
            let (mode, title) = ikiye_bol(kalan, "/spawn <mod> <baslik>")?;
            Ok(TelegramRequest::Command(Box::new(Command::SpawnTask {
                title: title.to_owned(),
                mode: mode.to_owned(),
                parent_id: None,
                persona: None,
                duration_target: None,
                budget: None,
            })))
        }

        "write" => {
            let (ham_id, content) = ikiye_bol(kalan, "/write <ajan_id> <mesaj>")?;
            Ok(TelegramRequest::Command(Box::new(Command::WriteToAgent {
                agent_id: ajan_id(ham_id)?,
                content: content.to_owned(),
            })))
        }

        "interrupt" => {
            let (ham_id, geri_kalan) = ikiye_bol(kalan, "/interrupt <ajan_id> <tur> [gerekce]")?;
            let mut tur_ve_gerekce = geri_kalan.splitn(2, char::is_whitespace);
            let kind = tur_ve_gerekce.next().unwrap_or_default().trim();
            if kind.is_empty() {
                return Err(NotifyError::CommandParse(
                    "kullanim: /interrupt <ajan_id> <tur> [gerekce]".into(),
                ));
            }
            let reason = tur_ve_gerekce
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            Ok(TelegramRequest::Command(Box::new(Command::Interrupt {
                agent_id: ajan_id(ham_id)?,
                kind: kind.to_owned(),
                source: CHANNEL_SOURCE.to_owned(),
                reason,
            })))
        }

        "approve" => {
            let alanlar: Vec<&str> = kalan.split_whitespace().collect();
            let [ham_id, capability, target, karar] = alanlar.as_slice() else {
                return Err(NotifyError::CommandParse(
                    "kullanim: /approve <ajan_id> <yetki> <hedef> <allow|deny>".into(),
                ));
            };
            let decision = match karar.to_ascii_lowercase().as_str() {
                "allow" | "izin" => ApprovalDecision::Allow,
                "deny" | "ret" => ApprovalDecision::Deny,
                other => {
                    return Err(NotifyError::CommandParse(format!(
                        "karar 'allow' ya da 'deny' olmali: {other}"
                    )));
                }
            };
            Ok(TelegramRequest::Command(Box::new(Command::Approve {
                agent_id: ajan_id(ham_id)?,
                capability: (*capability).to_owned(),
                target: (*target).to_owned(),
                decision,
                approver: CHANNEL_SOURCE.to_owned(),
            })))
        }

        other => Err(NotifyError::CommandParse(format!(
            "bilinmeyen komut: /{other}"
        ))),
    }
}

/// Metni ilk bosluktan ikiye boler; iki parca da doluysa dondurur.
fn ikiye_bol<'a>(text: &'a str, kullanim: &str) -> Result<(&'a str, &'a str), NotifyError> {
    let mut parcalar = text.splitn(2, char::is_whitespace);
    let ilk = parcalar.next().unwrap_or_default().trim();
    let ikinci = parcalar.next().unwrap_or_default().trim();
    if ilk.is_empty() || ikinci.is_empty() {
        return Err(NotifyError::CommandParse(format!("kullanim: {kullanim}")));
    }
    Ok((ilk, ikinci))
}

/// Ajan kimligini cozer.
fn ajan_id(raw: &str) -> Result<AgentId, NotifyError> {
    raw.trim()
        .trim_start_matches('#')
        .parse::<AgentId>()
        .map_err(|_| NotifyError::CommandParse(format!("ajan kimligi sayi olmali: {raw}")))
}

/// MarkdownV2 ozel karakterlerini kacirir (Telegram Bot API sozlesmesi).
#[must_use]
pub fn escape_markdown_v2(text: &str) -> String {
    const OZEL: &[char] = &[
        '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
        '\\',
    ];
    let mut cikti = String::with_capacity(text.len() * 2);
    for ch in text.chars() {
        if OZEL.contains(&ch) {
            cikti.push('\\');
        }
        cikti.push(ch);
    }
    cikti
}

/// Tek yoklama turunun sonucu.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BridgeReport {
    /// Alinan guncelleme sayisi.
    pub received: usize,
    /// Kontrol duzlemine iletilen komut sayisi.
    pub forwarded: usize,
    /// Allowlist disi sohbetten geldigi icin reddedilen mesaj sayisi.
    pub rejected: usize,
    /// Ayristirilamayan ya da iletilemeyen mesaj sayisi.
    pub invalid: usize,
}

/// Telegram -> kontrol duzlemi koprusu.
///
/// Kendi durumunu tutmaz; tek durum kaynagi cekirdektir (I3). Yalnizca
/// `update_id` imleci ile hangi guncellemeye kadar okundugunu hatirlar.
#[derive(Debug)]
pub struct TelegramBridge {
    notifier: TelegramNotifier,
    control: ControlClient,
    allowed_chats: Vec<i64>,
    offset: i64,
    poll_timeout_secs: u32,
}

impl TelegramBridge {
    /// Kopru kurar. Allowlist bos ise **hicbir** komut kabul edilmez.
    #[must_use]
    pub fn new(notifier: TelegramNotifier, control: ControlClient, policy: &NotifyPolicy) -> Self {
        Self {
            notifier,
            control,
            allowed_chats: policy.telegram_allowed_chat_ids.clone(),
            offset: 0,
            poll_timeout_secs: DEFAULT_POLL_TIMEOUT_SECS,
        }
    }

    /// Uzun yoklama suresini degistirir.
    #[must_use]
    pub fn with_poll_timeout(mut self, secs: u32) -> Self {
        self.poll_timeout_secs = secs;
        self
    }

    /// Sohbet komut gonderebilir mi?
    #[must_use]
    pub fn is_allowed(&self, chat_id: i64) -> bool {
        self.allowed_chats.contains(&chat_id)
    }

    /// Okuma imleci.
    #[must_use]
    pub fn offset(&self) -> i64 {
        self.offset
    }

    /// Bir yoklama turu calistirir: guncellemeleri al, yetkilendir, komuta
    /// cevir, kontrol duzlemine ilet.
    ///
    /// Tek bir mesajin hatasi turu dusurmez; sayaclara yansir ve kullaniciya
    /// hata metni doner.
    ///
    /// # Errors
    /// `getUpdates` cagrisi basarisiz olursa hata doner.
    pub async fn poll_once(&mut self) -> Result<BridgeReport, NotifyError> {
        let updates = self
            .notifier
            .fetch_updates(self.offset, self.poll_timeout_secs)
            .await?;
        let mut report = BridgeReport::default();

        for update in updates {
            report.received += 1;
            self.offset = self.offset.max(update.update_id + 1);

            let Some(message) = update.message else {
                continue;
            };
            let Some(text) = message.text.as_deref() else {
                continue;
            };
            if !self.is_allowed(message.chat.id) {
                report.rejected += 1;
                warn!(
                    target: "omni::notify",
                    chat = message.chat.id,
                    "allowlist disi sohbetten komut reddedildi"
                );
                continue;
            }

            match parse_telegram_command(text) {
                Ok(TelegramRequest::Command(command)) => {
                    match self.control.send_command(&command).await {
                        Ok(()) => report.forwarded += 1,
                        Err(err) => {
                            report.invalid += 1;
                            warn!(target: "omni::notify", %err, "komut iletilemedi");
                            self.uyar(&format!("komut iletilemedi: {err}")).await;
                        }
                    }
                }
                Ok(TelegramRequest::Snapshot) => match self.control.snapshot().await {
                    Ok(snapshot) => {
                        report.forwarded += 1;
                        let ozet = format!(
                            "ajan {} · gorev {} · saglayici {}",
                            snapshot.agents.len(),
                            snapshot.tasks.len(),
                            snapshot.providers.len()
                        );
                        self.uyar(&ozet).await;
                    }
                    Err(err) => {
                        report.invalid += 1;
                        warn!(target: "omni::notify", %err, "anlik goruntu alinamadi");
                    }
                },
                Ok(TelegramRequest::Help) => {
                    self.uyar(HELP_TEXT).await;
                }
                Err(err) => {
                    report.invalid += 1;
                    self.uyar(&format!("{err}\n\n{HELP_TEXT}")).await;
                }
            }
        }

        Ok(report)
    }

    /// Kullaniciya bilgi metni gonderir; gonderim hatasi turu dusurmez.
    async fn uyar(&self, text: &str) {
        if let Err(err) = self
            .notifier
            .send_message(&escape_markdown_v2(text), None)
            .await
        {
            warn!(target: "omni::notify", %err, "telegram yaniti gonderilemedi");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kontrol_istemcisi() -> ControlClient {
        match ControlClient::new("http://[::1]:7777", &Zeroizing::new("token".to_owned())) {
            Ok(c) => c,
            Err(err) => panic!("istemci kurulamadi: {err}"),
        }
    }

    #[test]
    fn status_komutu_anlik_goruntu_ister() {
        assert!(matches!(
            parse_telegram_command("/status"),
            Ok(TelegramRequest::Snapshot)
        ));
        // Grup sohbetinde bot soneki.
        assert!(matches!(
            parse_telegram_command("/status@omnitrix_bot"),
            Ok(TelegramRequest::Snapshot)
        ));
    }

    #[test]
    fn spawn_komutu_gorev_acar() {
        let istek = match parse_telegram_command("/spawn mvp diff akisini bitir") {
            Ok(TelegramRequest::Command(cmd)) => *cmd,
            other => panic!("komut bekleniyordu: {other:?}"),
        };
        match istek {
            Command::SpawnTask { title, mode, .. } => {
                assert_eq!(mode, "mvp");
                assert_eq!(title, "diff akisini bitir");
            }
            other => panic!("SpawnTask bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn write_komutu_ajana_mesaj_iletir() {
        let istek = match parse_telegram_command("/write 12 dur ve rapor ver") {
            Ok(TelegramRequest::Command(cmd)) => *cmd,
            other => panic!("komut bekleniyordu: {other:?}"),
        };
        match istek {
            Command::WriteToAgent { agent_id, content } => {
                assert_eq!(agent_id, 12);
                assert_eq!(content, "dur ve rapor ver");
            }
            other => panic!("WriteToAgent bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn interrupt_komutu_kanali_kaynak_yazar() {
        let istek = match parse_telegram_command("/interrupt #4 pause butce doldu") {
            Ok(TelegramRequest::Command(cmd)) => *cmd,
            other => panic!("komut bekleniyordu: {other:?}"),
        };
        match istek {
            Command::Interrupt {
                agent_id,
                kind,
                source,
                reason,
            } => {
                assert_eq!(agent_id, 4);
                assert_eq!(kind, "pause");
                assert_eq!(source, CHANNEL_SOURCE);
                assert_eq!(reason.as_deref(), Some("butce doldu"));
            }
            other => panic!("Interrupt bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn interrupt_gerekcesiz_de_calisir() {
        let istek = match parse_telegram_command("/interrupt 4 pause") {
            Ok(TelegramRequest::Command(cmd)) => *cmd,
            other => panic!("komut bekleniyordu: {other:?}"),
        };
        assert!(matches!(istek, Command::Interrupt { reason: None, .. }));
    }

    #[test]
    fn approve_komutu_karari_cozer() {
        let istek = match parse_telegram_command("/approve 9 fs.exec /usr/bin/git allow") {
            Ok(TelegramRequest::Command(cmd)) => *cmd,
            other => panic!("komut bekleniyordu: {other:?}"),
        };
        match istek {
            Command::Approve {
                agent_id,
                capability,
                target,
                decision,
                approver,
            } => {
                assert_eq!(agent_id, 9);
                assert_eq!(capability, "fs.exec");
                assert_eq!(target, "/usr/bin/git");
                assert_eq!(decision, ApprovalDecision::Allow);
                assert_eq!(approver, CHANNEL_SOURCE);
            }
            other => panic!("Approve bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn bozuk_komutlar_reddedilir() {
        for metin in [
            "merhaba",
            "/spawn",
            "/spawn mvp",
            "/write abc mesaj",
            "/approve 9 fs.exec /usr/bin/git belki",
            "/bilinmeyen",
        ] {
            assert!(
                matches!(
                    parse_telegram_command(metin),
                    Err(NotifyError::CommandParse(_))
                ),
                "reddedilmeliydi: {metin}"
            );
        }
    }

    #[test]
    fn markdown_kacisi_ozel_karakterleri_korur() {
        assert_eq!(escape_markdown_v2("a_b*c"), r"a\_b\*c");
        assert_eq!(escape_markdown_v2("bitti."), r"bitti\.");
    }

    #[test]
    fn notifier_debug_token_sizdirmaz() {
        let notifier = TelegramNotifier::new("123:GIZLI", "-100");
        assert!(!format!("{notifier:?}").contains("GIZLI"));
        assert_eq!(notifier.chat_id(), "-100");
    }

    #[test]
    fn api_koku_degistirilebilir() {
        let notifier = TelegramNotifier::new("t", "1").with_api_base("http://127.0.0.1:9/");
        assert_eq!(
            notifier.method_url("sendMessage"),
            "http://127.0.0.1:9/bott/sendMessage"
        );
    }

    #[test]
    fn bos_allowlist_komut_kabul_etmez() {
        let bridge = TelegramBridge::new(
            TelegramNotifier::new("t", "1"),
            kontrol_istemcisi(),
            &NotifyPolicy::default(),
        );
        assert!(!bridge.is_allowed(-100));
        assert_eq!(bridge.offset(), 0);
    }

    #[test]
    fn allowlist_sohbeti_kabul_eder() {
        let policy = NotifyPolicy {
            telegram_allowed_chat_ids: vec![-100_200],
            ..NotifyPolicy::default()
        };
        let bridge = TelegramBridge::new(
            TelegramNotifier::new("t", "1"),
            kontrol_istemcisi(),
            &policy,
        );
        assert!(bridge.is_allowed(-100_200));
        assert!(!bridge.is_allowed(1));
    }
}
