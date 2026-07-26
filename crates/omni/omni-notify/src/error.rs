//! Kanal-bagimsiz hata tipi (MASTER-PLAN Bolum 13).
//!
//! Dort kanalin (public IPv6 / Tailscale / Telegram / Twilio) hepsi ayni hata
//! tipini kullanir; cagiran hangi kanalin dustugunu tek `match` ile ayirir.
//! Uretim yolunda `unwrap`/`expect`/`panic!` yoktur — her basarisizlik bu tip
//! uzerinden `Result` ile tasinir (I6).

/// Bildirim/kopru katmani hatalari.
#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    /// Tasima katmani hatasi (baglanti, zaman asimi, TLS).
    ///
    /// Govde **URL icermez**: Telegram bot token'i URL yolunda tasindigi icin
    /// `reqwest::Error`'un varsayilan `Display`'i sirri log'a sizdirir. Bu
    /// yuzden donusum [`http_error`] uzerinden yapilir.
    #[error("HTTP error: {0}")]
    Http(String),

    /// Uzak API mantiksal hata dondurdu (2xx disi durum ya da `ok:false`).
    #[error("API error: {0}")]
    Api(String),

    /// Keyring'de aranan kimlik bilgisi yok.
    #[error("kimlik bilgisi bulunamadi: {0}")]
    MissingCredential(String),

    /// Kimlik deposu okunamadi (dosya izni, sifre cozme hatasi vb.).
    #[error("kimlik deposu hatasi ({id}): {reason}")]
    CredentialStore {
        /// Aranan kimlik bilgisi anahtari.
        id: String,
        /// Depodan gelen aciklama.
        reason: String,
    },

    /// Kanal bu surecte kurulmamis (kimlik bilgisi eksik oldugu icin).
    #[error("kanal yapilandirilmamis: {0}")]
    ChannelUnconfigured(&'static str),

    /// Config degeri sema disi.
    #[error("config gecersiz: {0}")]
    Config(String),

    /// JSON kodlama/cozme hatasi.
    #[error("serilestirme hatasi: {0}")]
    Serialization(String),

    /// Telegram'dan gelen metin bir komuta cevrilemedi.
    #[error("komut ayristirilamadi: {0}")]
    CommandParse(String),
}

impl From<serde_json::Error> for NotifyError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

impl From<omni_proto::ProtoError> for NotifyError {
    fn from(err: omni_proto::ProtoError) -> Self {
        Self::Serialization(err.to_string())
    }
}

/// `reqwest` hatasini URL'siz metne cevirir.
///
/// Telegram uc noktasi `https://api.telegram.org/bot<TOKEN>/...` seklindedir;
/// URL'li hata metni token'i log'a yazar. `without_url()` bunu keser.
#[must_use]
pub fn http_error(err: reqwest::Error) -> NotifyError {
    NotifyError::Http(err.without_url().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eksik_kimlik_hatasi_anahtari_tasir() {
        let err = NotifyError::MissingCredential("telegram-bot-token".into());
        assert!(err.to_string().contains("telegram-bot-token"));
    }

    #[test]
    fn proto_hatasi_serilestirme_hatasina_donusur() {
        let proto_err = omni_proto::ProtoError::UnknownVariant {
            field: "level",
            value: "yok".into(),
        };
        let err: NotifyError = proto_err.into();
        assert!(matches!(err, NotifyError::Serialization(_)));
    }
}
