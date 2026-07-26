//! Kimlik bilgisi cozumlemesi (MASTER-PLAN Bolum 13 + 14.2 `api_keys.key_ref`).
//!
//! Telegram bot token'i, Twilio SID/token'i ve kontrol duzlemi API token'i
//! **koda gomulmez**; keyring'den (ya da onun env yedeginden) gelir. Bu modul
//! yalnizca *cozumleme sozlesmesini* tanimlar: gercek saklama isi
//! `omni-provider/keyring.rs` tarafindadir.
//!
//! Bagimlilik yonu bilerek terstir — `omni-notify` `omni-provider`'a bagli
//! degildir; birlestirme koku (`omnitrix` ikilisi) `KeyManager`'i saran ince
//! bir tip yazip [`CredentialStore`]'u uygular:
//!
//! ```ignore
//! struct KeyringCredentials(omni_provider::keyring::KeyManager);
//!
//! #[async_trait::async_trait]
//! impl omni_notify::CredentialStore for KeyringCredentials {
//!     async fn secret(&self, id: &str) -> Result<Zeroizing<String>, NotifyError> {
//!         self.0.get_key(id).await.map_err(|e| NotifyError::CredentialStore {
//!             id: id.to_owned(),
//!             reason: e.to_string(),
//!         })
//!     }
//! }
//! ```

use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::error::NotifyError;

/// Telegram bot token'i (`@BotFather`).
pub const TELEGRAM_BOT_TOKEN: &str = "telegram-bot-token";
/// Bildirimlerin gonderilecegi Telegram sohbeti.
pub const TELEGRAM_CHAT_ID: &str = "telegram-chat-id";
/// Twilio hesap kimligi.
pub const TWILIO_ACCOUNT_SID: &str = "twilio-account-sid";
/// Twilio auth token'i.
pub const TWILIO_AUTH_TOKEN: &str = "twilio-auth-token";
/// SMS/aramanin cikacagi numara.
pub const TWILIO_FROM_NUMBER: &str = "twilio-from-number";
/// SMS/aramanin gidecegi numara.
pub const TWILIO_TO_NUMBER: &str = "twilio-to-number";
/// Kontrol duzlemine (`omni-control`) baglanirken kullanilan bearer token.
pub const CONTROL_API_TOKEN: &str = "control-api-token";

/// `omni-provider/keyring.rs` ile ayni env yedegi oneki.
const ENV_PREFIX: &str = "OMNITRIX_KEYS_";

/// Kimlik bilgisi kaynagi. Gercekleme keyring, env ya da testte sabit harita
/// olabilir; bu crate hicbirini varsaymaz.
#[async_trait]
pub trait CredentialStore: Send + Sync {
    /// Verilen anahtarin sirrini dondurur.
    ///
    /// # Errors
    /// Anahtar yoksa [`NotifyError::MissingCredential`], depo okunamazsa
    /// [`NotifyError::CredentialStore`] doner.
    async fn secret(&self, id: &str) -> Result<Zeroizing<String>, NotifyError>;

    /// Sir varsa dondurur; yoksa `None`. Kanal opsiyonelligi bunun uzerinden
    /// cozulur (Telegram kurulu, Twilio kurulu degil gibi).
    ///
    /// # Errors
    /// Depo okunamazsa [`NotifyError::CredentialStore`] doner. "Yok" durumu
    /// hata degildir.
    async fn optional_secret(&self, id: &str) -> Result<Option<Zeroizing<String>>, NotifyError> {
        match self.secret(id).await {
            Ok(value) => Ok(Some(value)),
            Err(NotifyError::MissingCredential(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }
}

/// Env degiskeni yedegi: `telegram-bot-token` -> `OMNITRIX_KEYS_TELEGRAM_BOT_TOKEN`.
/// Adlandirma `omni-provider/keyring.rs` ile birebir aynidir; keyring dosyasi
/// yoksa ayni degisken okunur.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvCredentialStore;

impl EnvCredentialStore {
    /// Anahtarin env degiskeni adi.
    #[must_use]
    pub fn env_var_name(id: &str) -> String {
        format!("{ENV_PREFIX}{}", id.to_uppercase().replace('-', "_"))
    }
}

#[async_trait]
impl CredentialStore for EnvCredentialStore {
    async fn secret(&self, id: &str) -> Result<Zeroizing<String>, NotifyError> {
        let name = Self::env_var_name(id);
        match std::env::var(&name) {
            Ok(value) if !value.trim().is_empty() => Ok(Zeroizing::new(value)),
            Ok(_) | Err(std::env::VarError::NotPresent) => {
                Err(NotifyError::MissingCredential(id.to_owned()))
            }
            Err(err) => Err(NotifyError::CredentialStore {
                id: id.to_owned(),
                reason: err.to_string(),
            }),
        }
    }
}

/// Sureç icinde tutulan sabit harita. Testler ve "kimlik bilgisi disaridan
/// enjekte edilir" senaryosu icin; diske bir sey yazmaz.
#[derive(Default)]
pub struct StaticCredentialStore {
    entries: BTreeMap<String, Zeroizing<String>>,
}

impl fmt::Debug for StaticCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Sirlar Debug'a sizmaz; yalnizca anahtar adlari gorunur.
        f.debug_struct("StaticCredentialStore")
            .field("ids", &self.entries.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl StaticCredentialStore {
    /// Bos depo.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sir ekler (zincirlenebilir).
    #[must_use]
    pub fn with(mut self, id: impl Into<String>, secret: impl Into<String>) -> Self {
        self.entries.insert(id.into(), Zeroizing::new(secret.into()));
        self
    }
}

#[async_trait]
impl CredentialStore for StaticCredentialStore {
    async fn secret(&self, id: &str) -> Result<Zeroizing<String>, NotifyError> {
        self.entries
            .get(id)
            .cloned()
            .ok_or_else(|| NotifyError::MissingCredential(id.to_owned()))
    }
}

/// Telegram kanalinin kimlik bilgileri.
#[derive(Clone)]
pub struct TelegramCredentials {
    /// Bot token'i.
    pub bot_token: Zeroizing<String>,
    /// Hedef sohbet kimligi (metin; Telegram negatif grup kimlikleri de verir).
    pub chat_id: String,
}

impl fmt::Debug for TelegramCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramCredentials")
            .field("bot_token", &"[REDACTED]")
            .field("chat_id", &self.chat_id)
            .finish()
    }
}

/// Twilio kanalinin kimlik bilgileri (SMS + arama ayni hesabi kullanir).
#[derive(Clone)]
pub struct TwilioCredentials {
    /// Hesap kimligi (`AC...`).
    pub account_sid: String,
    /// Auth token'i.
    pub auth_token: Zeroizing<String>,
    /// Gonderen numara.
    pub from: String,
    /// Alici numara.
    pub to: String,
}

impl fmt::Debug for TwilioCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TwilioCredentials")
            .field("account_sid", &self.account_sid)
            .field("auth_token", &"[REDACTED]")
            .field("from", &self.from)
            .field("to", &self.to)
            .finish()
    }
}

/// Bildirim katmaninin tum kimlik bilgileri. Eksik kanal `None` kalir; kanal
/// kurulmaz ama surec acilmaya devam eder (bildirim acilisi bloke etmez).
#[derive(Clone, Debug, Default)]
pub struct NotifyCredentials {
    /// Telegram kimlikleri.
    pub telegram: Option<TelegramCredentials>,
    /// Twilio kimlikleri.
    pub twilio: Option<TwilioCredentials>,
    /// Kontrol duzlemi bearer token'i (K9 — auth tum kanallarda zorunlu).
    pub control_token: Option<Zeroizing<String>>,
}

impl NotifyCredentials {
    /// Depodan tum kanallarin kimlik bilgilerini toplar.
    ///
    /// Bir kanalin *hicbir* alani yoksa kanal `None` kalir. Alanlarin bir kismi
    /// varsa bu yapilandirma hatasidir ve [`NotifyError::MissingCredential`]
    /// olarak yukselir — yarim yapilandirilmis kanal sessizce yutulmaz.
    ///
    /// # Errors
    /// Depo okunamazsa ya da kanal yarim yapilandirilmissa hata doner.
    pub async fn load(store: &dyn CredentialStore) -> Result<Self, NotifyError> {
        let telegram = match store.optional_secret(TELEGRAM_BOT_TOKEN).await? {
            Some(bot_token) => {
                let chat_id = store.secret(TELEGRAM_CHAT_ID).await?;
                Some(TelegramCredentials {
                    bot_token,
                    chat_id: chat_id.trim().to_owned(),
                })
            }
            None => None,
        };

        let twilio = match store.optional_secret(TWILIO_ACCOUNT_SID).await? {
            Some(account_sid) => {
                let auth_token = store.secret(TWILIO_AUTH_TOKEN).await?;
                let from = store.secret(TWILIO_FROM_NUMBER).await?;
                let to = store.secret(TWILIO_TO_NUMBER).await?;
                Some(TwilioCredentials {
                    account_sid: account_sid.trim().to_owned(),
                    auth_token,
                    from: from.trim().to_owned(),
                    to: to.trim().to_owned(),
                })
            }
            None => None,
        };

        let control_token = store.optional_secret(CONTROL_API_TOKEN).await?;

        Ok(Self {
            telegram,
            twilio,
            control_token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_degisken_adi_keyring_ile_ayni() {
        assert_eq!(
            EnvCredentialStore::env_var_name(TELEGRAM_BOT_TOKEN),
            "OMNITRIX_KEYS_TELEGRAM_BOT_TOKEN"
        );
        assert_eq!(
            EnvCredentialStore::env_var_name(TWILIO_ACCOUNT_SID),
            "OMNITRIX_KEYS_TWILIO_ACCOUNT_SID"
        );
    }

    #[tokio::test]
    async fn eksik_kanal_none_kalir() {
        let store = StaticCredentialStore::new();
        let creds = match NotifyCredentials::load(&store).await {
            Ok(creds) => creds,
            Err(err) => panic!("yukleme basarisiz: {err}"),
        };
        assert!(creds.telegram.is_none());
        assert!(creds.twilio.is_none());
        assert!(creds.control_token.is_none());
    }

    #[tokio::test]
    async fn yarim_yapilandirilmis_kanal_hata_verir() {
        let store = StaticCredentialStore::new().with(TELEGRAM_BOT_TOKEN, "123:abc");
        let err = NotifyCredentials::load(&store).await.err();
        assert!(matches!(err, Some(NotifyError::MissingCredential(id)) if id == TELEGRAM_CHAT_ID));
    }

    #[tokio::test]
    async fn tam_yapilandirilmis_kanal_yuklenir() {
        let store = StaticCredentialStore::new()
            .with(TELEGRAM_BOT_TOKEN, "123:abc")
            .with(TELEGRAM_CHAT_ID, " -100200 ")
            .with(CONTROL_API_TOKEN, "tok");
        let creds = match NotifyCredentials::load(&store).await {
            Ok(creds) => creds,
            Err(err) => panic!("yukleme basarisiz: {err}"),
        };
        let telegram = match creds.telegram {
            Some(t) => t,
            None => panic!("telegram kimlikleri bekleniyordu"),
        };
        assert_eq!(telegram.chat_id, "-100200");
        assert!(creds.control_token.is_some());
        // Debug ciktisi sirri sizdirmaz.
        assert!(!format!("{telegram:?}").contains("123:abc"));
    }
}
