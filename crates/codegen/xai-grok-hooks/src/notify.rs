//! Bildirim katmanı — ayrı `omni-notify` katmanı grok hook sistemine taşındı.
//!
//! Kanallar (Telegram bot + Twilio SMS/arama), dedup penceresi ve eskalasyon
//! mantığı artık bu modülde yaşar; `omni-notify`'a bağımlılık yoktur.
//!
//! Akış (omni-notify `dispatch`'inden birebir taşındı):
//! `HookEventEnvelope` → tetikleyici eşlemesi → **dedup penceresi** → kanal
//! eşikleri → gönderim. Susturma kararından sonra hiçbir kanal çağrılmaz.
//!
//! Sözleşmeler:
//! - Yapılandırma yoksa her şey **sessiz no-op**'tur ([`dispatch_event`]).
//! - Üretim yolunda `unwrap`/`expect`/`panic!` yoktur (I6); ağ hatası yalnızca
//!   loglanır, bildirim asla hook akışını bozmaz.
//! - Bot token'ı Telegram URL yolunda taşındığı için hata metinleri URL
//!   içermez ([`http_error`] `without_url()` kullanır).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::event::{HookEventEnvelope, HookEventName, HookPayload};

/// Varsayılan susturma penceresi (saniye) — `omni-notify::dedup::DEFAULT_WINDOW_SECS`.
pub const DEFAULT_WINDOW_SECS: u64 = 300;

/// Varsayılan imza kapasitesi — `omni-notify::dedup::DEFAULT_CAPACITY`.
pub const DEFAULT_CAPACITY: usize = 512;

/// Varsayılan Telegram API kökü.
pub const DEFAULT_API_BASE: &str = "https://api.telegram.org";

/// Varsayılan Twilio API kökü.
pub const DEFAULT_TWILIO_API_BASE: &str = "https://api.twilio.com";

/// Twilio SMS gövde sınırı — `omni-notify::twilio::MAX_SMS_LEN`.
pub const MAX_SMS_LEN: usize = 1600;

/// Yükseltme mesajlarının öneki — `omni-notify::twilio::ESCALATION_PREFIX`.
const ESCALATION_PREFIX: &str = "[ESCALATION] ";

/// Varsayılan biçimleme modu — `omni-notify::telegram::DEFAULT_PARSE_MODE`.
const DEFAULT_PARSE_MODE: &str = "MarkdownV2";

/// Aramada tekrarlanma sayısı — `omni-notify::voice::DEFAULT_SAY_LOOP`.
const DEFAULT_SAY_LOOP: u8 = 2;

/// Bildirim kanalı — `omni-notify::policy::Channel`'dan taşındı.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Telegram bot mesajı.
    Telegram,
    /// Twilio SMS / WhatsApp metni.
    Sms,
    /// Twilio sesli arama.
    Call,
}

impl Channel {
    /// Kanonik kanal adı (log ve imza için).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Sms => "sms",
            Self::Call => "call",
        }
    }

    /// Telefonu çalan kanallar (SMS/arama). Bunlar yalnızca yüksek-önem
    /// eşiğinde tetiklenir — `omni-notify::policy::Channel::is_phone`.
    #[must_use]
    pub fn is_phone(self) -> bool {
        matches!(self, Self::Sms | Self::Call)
    }
}

/// Bildirim önceliği — `omni-proto::NoticeLevel`'ın hook tarafındaki karşılığı.
/// Sıralama: `Info < Warn < Error < Critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warn,
    Error,
    Critical,
}

impl Severity {
    /// Olay adından varsayılan öncelik. `Notification` payload'ının `level`
    /// alanı [`dispatch_event`] içinde bu varsayılanı ezer.
    #[must_use]
    pub fn for_event(event: HookEventName) -> Self {
        match event {
            HookEventName::StopFailure => Self::Critical,
            // Sözleşme: eskalasyon açıksa oturum sonu telefonu çalar.
            HookEventName::SessionEnd => Self::Critical,
            _ => Self::Info,
        }
    }

    /// Kanal eşiği: `level >= esik` ise kanaldan çıkabilir.
    #[must_use]
    pub fn allows(self, min: Self) -> bool {
        self >= min
    }
}

/// `level`/`notification_type` metninden öncelik türetir.
fn severity_from_level(level: Option<&str>) -> Severity {
    match level.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("critical" | "crit" | "fatal" | "emergency") => Severity::Critical,
        Some("error" | "err") => Severity::Error,
        Some("warn" | "warning") => Severity::Warn,
        _ => Severity::Info,
    }
}

fn severity_from_reason(reason: &str) -> Severity {
    match reason.trim().to_ascii_lowercase().as_str() {
        "failed" | "error" | "aborted" | "critical" => Severity::Error,
        _ => Severity::Info,
    }
}

/// Bildirim yapılandırması — `omni-notify`'ın `NotifyPolicy` + kimlik
/// alanlarının hook sistemine taşınmış hali. Tüm alanlar `Option`; yalnızca
/// dolu kanallar kullanılır, eksik yapılandırma sessizce çalışır.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyConfig {
    /// Telegram bot token'ı (keyring'den okunur; `Debug` çıktısı kırmızıya boyar).
    pub telegram_bot_token: Option<String>,
    /// Telegram sohbet kimliği (gruplar negatiftir).
    pub telegram_chat_id: Option<String>,
    /// Twilio account SID.
    pub twilio_sid: Option<String>,
    /// Twilio auth token (basic auth şifresi) — `omni-notify`'ın
    /// `TwilioCredentials::auth_token`'ının karşılığı.
    pub twilio_auth_token: Option<String>,
    /// Twilio gönderici numarası (`From`).
    pub twilio_from: Option<String>,
    /// Twilio alıcı numarası (`To`).
    pub twilio_to: Option<String>,
    /// Eskalasyon açık mı? Açıksa `SessionEnd`/hata olayında telefon kanalı
    /// (SMS + uygun öncelikte arama) tetiklenir — `omni-notify` `EscalationNotifier`.
    pub escalation: bool,
    /// Susturma penceresi (saniye). `0` susturmayı kapatır.
    pub dedup_window_secs: u64,
}

impl std::fmt::Debug for NotifyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Token'lar asla `Debug` çıktısına düşmez (omni-notify notifier'ları
        // ile aynı sözleşme): URL yolunda taşınan sırlar loga sızabilir.
        f.debug_struct("NotifyConfig")
            .field(
                "telegram_bot_token",
                &self.telegram_bot_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("telegram_chat_id", &self.telegram_chat_id)
            .field("twilio_sid", &self.twilio_sid)
            .field(
                "twilio_auth_token",
                &self.twilio_auth_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("twilio_from", &self.twilio_from)
            .field("twilio_to", &self.twilio_to)
            .field("escalation", &self.escalation)
            .field("dedup_window_secs", &self.dedup_window_secs)
            .finish()
    }
}

impl NotifyConfig {
    /// Telegram kanalı kurulu mu?
    #[must_use]
    pub fn has_telegram(&self) -> bool {
        self.telegram_bot_token.is_some() && self.telegram_chat_id.is_some()
    }

    /// Telefon kanalı (Twilio) kurulu mu?
    #[must_use]
    pub fn has_twilio(&self) -> bool {
        self.twilio_sid.is_some()
            && self.twilio_auth_token.is_some()
            && self.twilio_from.is_some()
            && self.twilio_to.is_some()
    }
}

/// Susturma kararı — `omni-notify::dedup::DedupVerdict`'ten taşındı.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupVerdict {
    /// Gönder. `suppressed_since_last`, son gönderimden bu yana bastırılan
    /// tekrar sayısıdır (ilk gönderimde 0).
    Send {
        /// Bastırılmış tekrar sayısı.
        suppressed_since_last: u32,
    },
    /// Gönderme. Pencere hâlâ açık.
    Suppress {
        /// Bu imzanın kaçıncı bastırılan tekrarı olduğu (1'den başlar).
        repeat: u32,
        /// Pencerenin dolmasına kalan saniye.
        retry_after_secs: u64,
    },
}

impl DedupVerdict {
    /// Mesaj gönderilecek mi?
    #[must_use]
    pub fn should_send(&self) -> bool {
        matches!(self, Self::Send { .. })
    }

    /// Gönderilecekse bastırılmış tekrar sayısı.
    #[must_use]
    pub fn suppressed_count(&self) -> u32 {
        match self {
            Self::Send {
                suppressed_since_last,
            } => *suppressed_since_last,
            Self::Suppress { repeat, .. } => *repeat,
        }
    }
}

/// Tek imzanın durumu.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// En son *gönderilen* mesajın anı.
    last_sent: Instant,
    /// En son *görüldüğü* an (temizleme için).
    last_seen: Instant,
    /// Son gönderimden bu yana bastırılan tekrar sayısı.
    suppressed: u32,
}

/// İmza + zaman penceresi tabanlı susturucu — `omni-notify::dedup::DedupWindow`.
#[derive(Debug)]
pub struct DedupWindow {
    window: Duration,
    capacity: usize,
    entries: Mutex<HashMap<String, Entry>>,
}

impl Default for DedupWindow {
    fn default() -> Self {
        Self::new(Duration::from_secs(DEFAULT_WINDOW_SECS))
    }
}

impl DedupWindow {
    /// Verilen pencere ile susturucu kurar. Negatif/sıfır pencere susturmayı
    /// fiilen kapatır (her olay geçer).
    #[must_use]
    pub fn new(window: Duration) -> Self {
        Self::with_capacity(window, DEFAULT_CAPACITY)
    }

    /// Pencere + kapasite ile kurar. Kapasite 0 verilirse 1'e yuvarlanır.
    #[must_use]
    pub fn with_capacity(window: Duration, capacity: usize) -> Self {
        Self {
            window,
            capacity: capacity.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Susturma penceresi.
    #[must_use]
    pub fn window(&self) -> Duration {
        self.window
    }

    /// İzlenen imza sayısı.
    #[must_use]
    pub fn tracked(&self) -> usize {
        lock(&self.entries).len()
    }

    /// Tüm durumu siler.
    pub fn reset(&self) {
        lock(&self.entries).clear();
    }

    /// İmzayı değerlendirir ve kararı döndürür. Karar **durum değiştirir**:
    /// `Send` dönen çağrı gönderim anını işaretler.
    pub fn admit(&self, signature: &str, now: Instant) -> DedupVerdict {
        let mut entries = lock(&self.entries);
        Self::prune(&mut entries, self.window, now, self.capacity);

        match entries.get_mut(signature) {
            None => {
                entries.insert(
                    signature.to_owned(),
                    Entry {
                        last_sent: now,
                        last_seen: now,
                        suppressed: 0,
                    },
                );
                DedupVerdict::Send {
                    suppressed_since_last: 0,
                }
            }
            Some(entry) => {
                entry.last_seen = now;
                let elapsed = now.saturating_duration_since(entry.last_sent);
                if elapsed >= self.window {
                    let suppressed = entry.suppressed;
                    entry.suppressed = 0;
                    entry.last_sent = now;
                    DedupVerdict::Send {
                        suppressed_since_last: suppressed,
                    }
                } else {
                    entry.suppressed = entry.suppressed.saturating_add(1);
                    let kalan = self.window.saturating_sub(elapsed).as_secs();
                    DedupVerdict::Suppress {
                        repeat: entry.suppressed,
                        retry_after_secs: kalan,
                    }
                }
            }
        }
    }

    /// Süresi geçmiş girdileri atar; hâlâ taşıyorsa en eski görüleni düşürür.
    fn prune(
        entries: &mut HashMap<String, Entry>,
        window: Duration,
        now: Instant,
        capacity: usize,
    ) {
        // Pencerenin iki katı kadar görülmeyen imza artık "tekrar" sayılmaz.
        let bayatlama = window.saturating_mul(2);
        entries.retain(|_, entry| now.saturating_duration_since(entry.last_seen) < bayatlama);

        while entries.len() >= capacity {
            let en_eski = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_seen)
                .map(|(key, _)| key.clone());
            match en_eski {
                Some(key) => {
                    entries.remove(&key);
                }
                None => break,
            }
        }
    }
}

/// Bildirim/kanal katmanı hataları — `omni-notify::error::NotifyError`'dan taşındı.
#[derive(Debug, Error)]
pub enum NotifyError {
    /// Taşıma katmanı hatası (bağlantı, zaman aşımı, TLS).
    ///
    /// Gövde **URL içermez**: Telegram bot token'ı URL yolunda taşındığı için
    /// `reqwest::Error`'un varsayılan `Display`'i sırrı log'a sızdırır. Bu
    /// yüzden dönüşüm [`http_error`] üzerinden yapılır.
    #[error("HTTP error: {0}")]
    Http(String),

    /// Uzak API mantıksal hata döndürdü (2xx dışı durum ya da `ok:false`).
    #[error("API error: {0}")]
    Api(String),

    /// Config değeri şema dışı.
    #[error("config gecersiz: {0}")]
    Config(String),

    /// JSON kodlama/çözme hatası.
    #[error("serilestirme hatasi: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for NotifyError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

/// `reqwest` hatasını URL'siz metne çevirir.
///
/// Telegram uç noktası `https://api.telegram.org/bot<TOKEN>/...` şeklindedir;
/// URL'li hata metni token'ı log'a yazar. `without_url()` bunu keser.
#[must_use]
pub fn http_error(err: reqwest::Error) -> NotifyError {
    NotifyError::Http(err.without_url().to_string())
}

/// MarkdownV2 özel karakterlerini kaçırır — `omni-notify::telegram::escape_markdown_v2`.
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

/// SMS gövdesini sınıra kırpar (char sınırında) — `omni-notify::twilio::truncate_sms`.
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

/// `application/x-www-form-urlencoded` gövdesi için yüzde kodlama (space → `+`).
/// `reqwest`'in `form` özelliği workspace'te kapalı olduğundan gövde elle üretilir.
#[must_use]
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// XML metin düğümü kaçışı — `omni-notify::voice::xml_escape`.
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

/// `omni-notify::voice::CallScript::say_locale` — yalnızca yaygın bölgesel
/// eşlemeler; tanınmayan kod olduğu gibi döner.
fn say_locale(language: &str) -> String {
    match language {
        "en" => "en-US".to_owned(),
        "tr" => "tr-TR".to_owned(),
        other => other.to_owned(),
    }
}

/// TwiML gövdesi (`Twiml` parametresi olarak gönderilir) — `omni-notify::voice::CallScript::to_twiml`.
fn to_twiml(text: &str, language: &str, loop_count: u8) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response><Say language=\"{}\" loop=\"{}\">{}</Say></Response>",
        xml_escape(&say_locale(language)),
        loop_count.max(1),
        xml_escape(text)
    )
}

/// Çalışma zamanı: yapılandırma + kalıcı dedup penceresi + paylaşımlı HTTP
/// istemcisi. Bir kez [`install`] ile kurulur; kurulmamışsa her şey sessizdir.
struct NotifyRuntime {
    config: NotifyConfig,
    dedup: Arc<DedupWindow>,
    client: reqwest::Client,
}

static RUNTIME: Mutex<Option<NotifyRuntime>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Yapılandırmayı kurar (önceki yapılandırmayı değiştirir). Dönen değer: kurulum
/// başarılı mı (her zaman `true`; yalnızca API simetrisi için tutulur).
#[must_use]
pub fn install(config: NotifyConfig) -> bool {
    let dedup = DedupWindow::new(Duration::from_secs(config.dedup_window_secs));
    *lock(&RUNTIME) = Some(NotifyRuntime {
        config,
        dedup: Arc::new(dedup),
        client: reqwest::Client::new(),
    });
    true
}

/// Yapılandırma kuruldu mu? Kurulmadıysa [`dispatch_event`] sessiz no-op'tur.
#[must_use]
pub fn is_installed() -> bool {
    lock(&RUNTIME).is_some()
}

/// Kurulu yapılandırmanın kopyası (yoksa `None`).
#[must_use]
pub fn installed_config() -> Option<NotifyConfig> {
    lock(&RUNTIME).as_ref().map(|r| r.config.clone())
}

/// Yapılandırma kopyası + paylaşımlı istemci; kurulum yoksa `None`.
fn runtime_snapshot() -> Option<(NotifyConfig, Arc<DedupWindow>, reqwest::Client)> {
    lock(&RUNTIME)
        .as_ref()
        .map(|r| (r.config.clone(), Arc::clone(&r.dedup), r.client.clone()))
}

/// Dedup imzası: olay + mesaj (aynı mesajın pencerede tekrarı susturulur).
fn signature(event: HookEventName, message: &str) -> String {
    format!("{event}:{message}")
}

/// Bastırılmış tekrar sayısını mesaja ekler ("+ N tekrar bastırıldı").
fn with_suppressed(message: &str, suppressed: u32) -> String {
    if suppressed == 0 {
        message.to_owned()
    } else {
        format!("{message} (+{suppressed} repeats suppressed)")
    }
}

/// Hook olayını kanal gönderimi için hazırlar. Bildirim katmanına girmeyen
/// olaylar `None` döner — dispatcher akışı hiç etkilenmez.
fn message_for(envelope: &HookEventEnvelope) -> Option<(HookEventName, Severity, String)> {
    match &envelope.payload {
        HookPayload::Notification {
            notification_type,
            message,
            title,
            level,
        } => {
            let level_severity = severity_from_level(level.as_deref());
            let type_severity = severity_from_level(Some(notification_type));
            let severity = level_severity.max(type_severity);
            let body = match (title.as_deref(), message.as_deref()) {
                (Some(t), Some(m)) => format!("{t}\n{m}"),
                (Some(t), None) => t.to_string(),
                (None, Some(m)) => m.to_string(),
                (None, None) => notification_type.clone(),
            };
            let text = format!("[{notification_type}] {body}");
            Some((HookEventName::Notification, severity, text))
        }
        HookPayload::SessionEnd {
            reason,
            turn_count,
            tool_call_count,
        } => {
            let mut text = format!("session ended: {reason}");
            if let Some(turns) = turn_count {
                text.push_str(&format!(" · {turns} turns"));
            }
            if let Some(calls) = tool_call_count {
                text.push_str(&format!(" · {calls} tool calls"));
            }
            let severity = severity_from_reason(reason);
            Some((HookEventName::SessionEnd, severity, text))
        }
        HookPayload::StopFailure {
            error,
            error_details,
            ..
        } => {
            let detail = error_details.as_deref().unwrap_or("no details");
            let text = format!("turn failed: {} ({detail})", error.as_str());
            Some((HookEventName::StopFailure, Severity::Critical, text))
        }
        _ => None,
    }
}

/// Dispatcher girişi: `Notification`/`SessionEnd`/`StopFailure` olaylarını
/// bildirim katmanına verir. Yapılandırma kurulmamışsa ya da olay bildirime
/// girmiyorsa sessiz `Ok(())` döner — asla hata yaymaz (I6).
pub async fn dispatch_event(envelope: &HookEventEnvelope) -> Result<(), NotifyError> {
    let Some((config, _, _)) = runtime_snapshot() else {
        return Ok(());
    };
    let Some((event, severity, message)) = message_for(envelope) else {
        return Ok(());
    };
    send_notification_with_severity(&config, event, severity, &message).await
}

/// Ana gönderim girişi: dedup → Telegram → eskalasyon (SMS + arama).
///
/// Öncelik olay adından türetilir ([`Severity::for_event`]); `Notification`
/// payload'ından gelen `level` ayrımı için [`dispatch_event`] kullanılır.
/// Kanal hatası `Err` olarak yayılmaz — loglanır, diğer kanallar çalışmaya
/// devam eder.
///
/// # Errors
/// Yalnızca iç hata durumlarında (nadiren) `Err` döner; normalde ağ hatası
/// sessiz loglanır.
pub async fn send_notification(
    config: &NotifyConfig,
    event: HookEventName,
    message: &str,
) -> Result<(), NotifyError> {
    send_notification_with_severity(config, event, Severity::for_event(event), message).await
}

async fn send_notification_with_severity(
    config: &NotifyConfig,
    event: HookEventName,
    severity: Severity,
    message: &str,
) -> Result<(), NotifyError> {
    let (dedup, client) = match runtime_snapshot() {
        Some((_, dedup, client)) => (dedup, client),
        None => (
            Arc::new(DedupWindow::new(Duration::from_secs(
                config.dedup_window_secs,
            ))),
            reqwest::Client::new(),
        ),
    };

    // 1) Susturma kararı — karardan sonra hiçbir kanal çağrılmaz.
    let verdict = dedup.admit(&signature(event, message), Instant::now());
    let suppressed = match verdict {
        DedupVerdict::Send {
            suppressed_since_last,
        } => suppressed_since_last,
        DedupVerdict::Suppress {
            repeat,
            retry_after_secs,
        } => {
            debug!(
                target: "xai_grok_hooks::notify",
                event = %event,
                repeat,
                retry_after_secs,
                "tekrar eden olay susturuldu"
            );
            return Ok(());
        }
    };
    let text = with_suppressed(message, suppressed);

    // 2) Telegram — eşik `Info` (her seviye) — `omni-notify` telegram_min_level.
    if config.has_telegram() && severity.allows(Severity::Info) {
        if let Err(err) = send_telegram(config, &client, &text).await {
            warn!(
                target: "xai_grok_hooks::notify",
                channel = Channel::Telegram.as_str(),
                %err,
                "kanal gonderimi basarisiz"
            );
        }
    } else {
        debug!(
            target: "xai_grok_hooks::notify",
            channel = Channel::Telegram.as_str(),
            "kanal yapilandirilmamis"
        );
    }

    // 3) Eskalasyon — yalnızca açıkken ve yüksek öncelikli olayda; telefon
    //    yalnızca `Error` ve üstünde (arama yalnızca `Critical`), `SessionEnd`
    //    ve `StopFailure` her zaman telefonu çalar (sözleşme).
    let escalation_event = matches!(
        event,
        HookEventName::SessionEnd | HookEventName::StopFailure
    );
    if config.escalation && (severity >= Severity::Error || escalation_event) {
        escalate(config, &client, severity, escalation_event, &text).await;
    }

    Ok(())
}

/// Telegram `sendMessage` — `omni-notify::telegram::TelegramNotifier::send_message`.
async fn send_telegram(
    config: &NotifyConfig,
    client: &reqwest::Client,
    text: &str,
) -> Result<(), NotifyError> {
    let Some(token) = config.telegram_bot_token.as_deref() else {
        return Ok(());
    };
    let Some(chat_id) = config.telegram_chat_id.as_deref() else {
        return Ok(());
    };
    let payload = json!({
        "chat_id": chat_id,
        "text": escape_markdown_v2(text),
        "parse_mode": DEFAULT_PARSE_MODE,
        "disable_web_page_preview": true,
    });
    let url = format!("{}/bot{}/{}", DEFAULT_API_BASE, token, "sendMessage");
    let resp = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(http_error)?;
    let status = resp.status();
    let body = resp.text().await.map_err(http_error)?;
    if !status.is_success() {
        return Err(NotifyError::Api(format!("status={status} body={body}")));
    }
    info!(target: "xai_grok_hooks::notify", chat = %chat_id, "telegram mesaji gonderildi");
    Ok(())
}

/// Twilio kimlik parametreleri (tümü doluysa).
struct TwilioParams<'a> {
    sid: &'a str,
    token: &'a str,
    from: &'a str,
    to: &'a str,
}

fn twilio_params(config: &NotifyConfig) -> Option<TwilioParams<'_>> {
    Some(TwilioParams {
        sid: config.twilio_sid.as_deref()?,
        token: config.twilio_auth_token.as_deref()?,
        from: config.twilio_from.as_deref()?,
        to: config.twilio_to.as_deref()?,
    })
}

/// Twilio REST uc noktası URL'si.
fn twilio_resource_url(sid: &str, resource: &str) -> String {
    format!(
        "{}/2010-04-01/Accounts/{}/{}",
        DEFAULT_TWILIO_API_BASE, sid, resource
    )
}

/// Form gönderip yanıtı doğrular — `omni-notify::twilio::TwilioNotifier::post_form`.
async fn post_form(
    client: &reqwest::Client,
    url: String,
    sid: &str,
    token: &str,
    params: &[(&str, &str)],
) -> Result<(), NotifyError> {
    let body = params
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let resp = client
        .post(url)
        .basic_auth(sid, Some(token))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
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

/// SMS/WhatsApp metni gönderir — `omni-notify::twilio::TwilioNotifier::send_sms`.
async fn send_sms(
    config: &NotifyConfig,
    client: &reqwest::Client,
    text: &str,
) -> Result<(), NotifyError> {
    let Some(p) = twilio_params(config) else {
        return Ok(());
    };
    let body = truncate_sms(text);
    post_form(
        client,
        twilio_resource_url(p.sid, "Messages.json"),
        p.sid,
        p.token,
        &[("From", p.from), ("To", p.to), ("Body", body.as_str())],
    )
    .await?;
    info!(target: "xai_grok_hooks::notify", to = %p.to, "sms gonderildi");
    Ok(())
}

/// Sesli arama başlatır; metin TwiML olarak gömülü gider — `omni-notify`'ın
/// `TwilioNotifier::place_call` + `CallScript::to_twiml` bileşimi.
async fn place_call(
    config: &NotifyConfig,
    client: &reqwest::Client,
    text: &str,
) -> Result<(), NotifyError> {
    let Some(p) = twilio_params(config) else {
        return Ok(());
    };
    let twiml = to_twiml(text, "en", DEFAULT_SAY_LOOP);
    post_form(
        client,
        twilio_resource_url(p.sid, "Calls.json"),
        p.sid,
        p.token,
        &[("From", p.from), ("To", p.to), ("Twiml", twiml.as_str())],
    )
    .await?;
    info!(target: "xai_grok_hooks::notify", to = %p.to, "sesli arama baslatildi");
    Ok(())
}

/// Eskalasyon: SMS (her zaman) + uygun öncelikte arama. Hata loglanır,
/// yayılmaz; SMS düşerse arama denenmez (omni-notify davranışı).
async fn escalate(
    config: &NotifyConfig,
    client: &reqwest::Client,
    severity: Severity,
    force_call: bool,
    text: &str,
) {
    if !config.has_twilio() {
        debug!(
            target: "xai_grok_hooks::notify",
            channel = Channel::Sms.as_str(),
            "telefon kanali yapilandirilmamis"
        );
        return;
    }
    let govde = format!("{ESCALATION_PREFIX}{}", text.replace('\n', " · "));
    if let Err(err) = send_sms(config, client, &govde).await {
        warn!(
            target: "xai_grok_hooks::notify",
            channel = Channel::Sms.as_str(),
            %err,
            "escalation sms basarisiz"
        );
        return;
    }
    if force_call || severity >= Severity::Critical {
        if let Err(err) = place_call(config, client, text).await {
            warn!(
                target: "xai_grok_hooks::notify",
                channel = Channel::Call.as_str(),
                %err,
                "escalation call basarisiz"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn ilk_olay_gecer() {
        let dedup = DedupWindow::new(Duration::from_secs(60));
        assert_eq!(
            dedup.admit("a", t0()),
            DedupVerdict::Send {
                suppressed_since_last: 0
            }
        );
    }

    #[test]
    fn pencere_icinde_tekrar_bastirilir() {
        let dedup = DedupWindow::new(Duration::from_secs(60));
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        let ikinci = dedup.admit("a", now + Duration::from_secs(1));
        assert!(!ikinci.should_send());
        assert_eq!(
            ikinci,
            DedupVerdict::Suppress {
                repeat: 1,
                retry_after_secs: 59
            }
        );
        let ucuncu = dedup.admit("a", now + Duration::from_secs(2));
        assert_eq!(ucuncu.suppressed_count(), 2);
    }

    #[test]
    fn pencere_dolunca_bastirilan_sayisi_raporlanir() {
        let dedup = DedupWindow::new(Duration::from_secs(60));
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        for i in 1..=5u64 {
            assert!(!dedup.admit("a", now + Duration::from_secs(i)).should_send());
        }
        assert_eq!(
            dedup.admit("a", now + Duration::from_secs(60)),
            DedupVerdict::Send {
                suppressed_since_last: 5
            }
        );
        assert_eq!(
            dedup.admit("a", now + Duration::from_secs(180)),
            DedupVerdict::Send {
                suppressed_since_last: 0
            }
        );
    }

    #[test]
    fn farkli_imzalar_birbirini_susturmaz() {
        let dedup = DedupWindow::new(Duration::from_secs(60));
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        assert!(dedup.admit("b", now).should_send());
    }

    #[test]
    fn sifir_pencere_susturmayi_kapatir() {
        let dedup = DedupWindow::new(Duration::ZERO);
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        assert!(dedup.admit("a", now).should_send());
    }

    #[test]
    fn kapasite_asilinca_en_eski_dusulur() {
        let dedup = DedupWindow::with_capacity(Duration::from_secs(600), 2);
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        assert!(dedup.admit("b", now + Duration::from_secs(1)).should_send());
        assert!(dedup.admit("c", now + Duration::from_secs(2)).should_send());
        assert!(dedup.tracked() <= 2);
    }

    #[test]
    fn bayat_girdi_temizlenir() {
        let dedup = DedupWindow::new(Duration::from_secs(60));
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        // 3 pencere sonra "a" bayat sayılır ve silinir; yeni imza gibi geçer.
        assert_eq!(
            dedup.admit("b", now + Duration::from_secs(180)),
            DedupVerdict::Send {
                suppressed_since_last: 0
            }
        );
        assert_eq!(dedup.tracked(), 1);
    }

    #[test]
    fn markdown_kacisi_ozel_karakterleri_korur() {
        assert_eq!(escape_markdown_v2("a_b*c"), r"a\_b\*c");
        assert_eq!(escape_markdown_v2("bitti."), r"bitti\.");
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
    fn form_kodlama_bosluk_ve_ozel_karakterleri_cevirir() {
        assert_eq!(form_encode("a b&c"), "a+b%26c");
        assert_eq!(form_encode("tr-TR"), "tr-TR");
    }

    #[test]
    fn twiml_xml_kacisi_yapar() {
        let twiml = to_twiml("a & b < c > d \"e\"", "en", 1);
        assert!(twiml.contains("a &amp; b &lt; c &gt; d &quot;e&quot;"));
        assert!(twiml.contains("language=\"en-US\""));
        assert!(twiml.starts_with("<?xml"));
    }

    #[test]
    fn sifir_tekrar_bire_yuvarlanir() {
        let twiml = to_twiml("x", "tr", 0);
        assert!(twiml.contains("loop=\"1\""));
        assert!(twiml.contains("language=\"tr-TR\""));
    }

    #[test]
    fn onem_metinden_turetilir() {
        assert_eq!(severity_from_level(Some("critical")), Severity::Critical);
        assert_eq!(severity_from_level(Some("ERROR")), Severity::Error);
        assert_eq!(severity_from_level(Some("warn")), Severity::Warn);
        assert_eq!(severity_from_level(None), Severity::Info);
        assert_eq!(severity_from_reason("failed"), Severity::Error);
        assert_eq!(severity_from_reason("end_turn"), Severity::Info);
        assert_eq!(
            Severity::for_event(HookEventName::StopFailure),
            Severity::Critical
        );
    }

    #[test]
    fn yapilandirma_kanal_algilama() {
        let config = NotifyConfig::default();
        assert!(!config.has_telegram());
        assert!(!config.has_twilio());
        assert_eq!(config.dedup_window_secs, 0);
        assert_eq!(DEFAULT_WINDOW_SECS, 300);

        let dolu = NotifyConfig {
            telegram_bot_token: Some("t".into()),
            telegram_chat_id: Some("1".into()),
            twilio_sid: Some("AC1".into()),
            twilio_auth_token: Some("gizli".into()),
            twilio_from: Some("+1".into()),
            twilio_to: Some("+2".into()),
            escalation: true,
            dedup_window_secs: 60,
        };
        assert!(dolu.has_telegram());
        assert!(dolu.has_twilio());
        // Auth token asla Debug çıktısına düşmez; SID görünür (omni-notify ile aynı).
        assert!(!format!("{dolu:?}").contains("gizli"));
    }

    #[test]
    fn notify_payload_disinden_olay_mesaj_uretmez() {
        let envelope = HookEventEnvelope {
            hook_event_name: HookEventName::PreToolUse,
            session_id: "s".into(),
            cwd: "/tmp".into(),
            workspace_root: "/tmp".into(),
            timestamp: "t".into(),
            transcript_path: None,
            client_identifier: None,
            prompt_id: None,
            permission_mode: None,
            payload: HookPayload::PreToolUse {
                tool_name: "bash".into(),
                tool_use_id: "u".into(),
                tool_input: serde_json::json!({}),
                tool_input_truncated: false,
                subagent_type: None,
            },
        };
        assert!(message_for(&envelope).is_none());
    }

    #[test]
    fn notify_payload_mesaj_uretir() {
        let envelope = HookEventEnvelope {
            hook_event_name: HookEventName::Notification,
            session_id: "s".into(),
            cwd: "/tmp".into(),
            workspace_root: "/tmp".into(),
            timestamp: "t".into(),
            transcript_path: None,
            client_identifier: None,
            prompt_id: None,
            permission_mode: None,
            payload: HookPayload::Notification {
                notification_type: "error".into(),
                message: Some("disk full".into()),
                title: Some("oom".into()),
                level: Some("error".into()),
            },
        };
        let (event, severity, text) = message_for(&envelope).expect("mesaj uretilmeli");
        assert_eq!(event, HookEventName::Notification);
        assert_eq!(severity, Severity::Error);
        assert!(text.contains("disk full"));
    }

    #[test]
    fn bastirilan_tekrar_mesaja_eklenir() {
        assert_eq!(with_suppressed("x", 0), "x");
        assert_eq!(with_suppressed("x", 3), "x (+3 repeats suppressed)");
    }
}
