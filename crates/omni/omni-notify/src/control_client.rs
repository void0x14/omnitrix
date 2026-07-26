//! Kontrol duzlemi istemcisi (MASTER-PLAN Bolum 13).
//!
//! Dort kanalin hepsi **tek** control-plane API'sinin (`omni-control`)
//! istemcisidir; kendi sunucusunu kurmaz. Telegram koprusu de bu istemci
//! uzerinden yazar: gelen metin `omni-proto::Command`'a cevrilir, token ile
//! `POST /v1/command`'a gonderilir. Token yoksa istemci **kurulmaz** — auth
//! opsiyonel degildir (K9).
//!
//! Bu modul `omni-control`'e derleme bagimliligi kurmaz (kanal, sunucuyu degil
//! HTTP sozlesmesini tuketir); yol sabitleri oradaki `PATH_*` ile birebir ayni
//! tutulur.

use omni_proto::{Command, SystemSnapshot};
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tracing::debug;
use zeroize::Zeroizing;

use crate::error::{NotifyError, http_error};

/// `omni_control::api::PATH_SNAPSHOT` karsiligi.
pub const PATH_SNAPSHOT: &str = "/v1/snapshot";
/// `omni_control::api::PATH_COMMAND` karsiligi.
pub const PATH_COMMAND: &str = "/v1/command";
/// `omni_control::auth::TOKEN_HEADER` karsiligi (tarayici disi kanallar).
pub const TOKEN_HEADER: &str = "x-omni-token";

/// Kontrol duzlemine token ile baglanan ince istemci.
pub struct ControlClient {
    base_url: String,
    client: Client,
    headers: HeaderMap,
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Token basliklari Debug'a sizmaz.
        f.debug_struct("ControlClient")
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl ControlClient {
    /// Taban URL + token ile istemci kurar.
    ///
    /// # Errors
    /// Token bos ya da baslik degeri olarak kodlanamiyorsa
    /// [`NotifyError::Config`] doner.
    pub fn new(base_url: &str, token: &Zeroizing<String>) -> Result<Self, NotifyError> {
        let temiz = token.trim();
        if temiz.is_empty() {
            return Err(NotifyError::Config(
                "kontrol duzlemi token'i bos olamaz (K9)".into(),
            ));
        }

        let mut headers = HeaderMap::new();
        let mut bearer = HeaderValue::from_str(&format!("Bearer {temiz}"))
            .map_err(|_| NotifyError::Config("token ASCII disi karakter iceriyor".into()))?;
        bearer.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, bearer);

        let mut alternatif = HeaderValue::from_str(temiz)
            .map_err(|_| NotifyError::Config("token ASCII disi karakter iceriyor".into()))?;
        alternatif.set_sensitive(true);
        let ad = HeaderName::from_static(TOKEN_HEADER);
        headers.insert(ad, alternatif);

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            client: Client::new(),
            headers,
        })
    }

    /// Kimlik bilgilerinden istemci kurar; token yoksa kanal kurulmaz.
    ///
    /// # Errors
    /// Token yoksa [`NotifyError::ChannelUnconfigured`] doner.
    pub fn from_credentials(
        base_url: &str,
        credentials: &crate::credential::NotifyCredentials,
    ) -> Result<Self, NotifyError> {
        let token = credentials
            .control_token
            .as_ref()
            .ok_or(NotifyError::ChannelUnconfigured("control-api"))?;
        Self::new(base_url, token)
    }

    /// Test/mock icin HTTP istemcisini degistirir.
    #[must_use]
    pub fn with_http_client(mut self, client: Client) -> Self {
        self.client = client;
        self
    }

    /// Taban URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Uc nokta URL'si.
    #[must_use]
    pub fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// `POST /v1/command` — komutu cekirdege iletir.
    ///
    /// Basari yaniti gövdesizdir (`202 Accepted`); komutun sonucu akistan
    /// okunur (Bolum 6.2).
    ///
    /// # Errors
    /// Tasima hatasinda [`NotifyError::Http`], 2xx disi yanitta
    /// [`NotifyError::Api`] doner.
    pub async fn send_command(&self, command: &Command) -> Result<(), NotifyError> {
        let response = self
            .client
            .post(self.endpoint(PATH_COMMAND))
            .headers(self.headers.clone())
            .json(command)
            .send()
            .await
            .map_err(http_error)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(NotifyError::Api(format!(
                "komut reddedildi: status={status} body={body}"
            )));
        }
        debug!(target: "omni::notify", kind = command.kind(), "komut kontrol duzlemine iletildi");
        Ok(())
    }

    /// `GET /v1/snapshot` — kanonik anlik goruntu.
    ///
    /// # Errors
    /// Tasima hatasinda [`NotifyError::Http`], 2xx disi yanitta
    /// [`NotifyError::Api`], govde cozulemezse [`NotifyError::Serialization`]
    /// doner.
    pub async fn snapshot(&self) -> Result<SystemSnapshot, NotifyError> {
        let response = self
            .client
            .get(self.endpoint(PATH_SNAPSHOT))
            .headers(self.headers.clone())
            .send()
            .await
            .map_err(http_error)?;

        let status = response.status();
        let body = response.text().await.map_err(http_error)?;
        if !status.is_success() {
            return Err(NotifyError::Api(format!(
                "anlik goruntu alinamadi: status={status} body={body}"
            )));
        }
        Ok(serde_json::from_str(&body)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> Zeroizing<String> {
        Zeroizing::new("gizli-token".to_owned())
    }

    #[test]
    fn bos_token_reddedilir() {
        let err = ControlClient::new("http://[::1]:7777", &Zeroizing::new("   ".to_owned())).err();
        assert!(matches!(err, Some(NotifyError::Config(_))));
    }

    #[test]
    fn taban_url_sonundaki_slash_kirpilir() {
        let client = match ControlClient::new("http://[::1]:7777/", &token()) {
            Ok(c) => c,
            Err(err) => panic!("istemci kurulamadi: {err}"),
        };
        assert_eq!(client.endpoint(PATH_COMMAND), "http://[::1]:7777/v1/command");
    }

    #[test]
    fn token_debug_ciktisina_sizmaz() {
        let client = match ControlClient::new("http://[::1]:7777", &token()) {
            Ok(c) => c,
            Err(err) => panic!("istemci kurulamadi: {err}"),
        };
        assert!(!format!("{client:?}").contains("gizli-token"));
    }

    #[test]
    fn token_yoksa_kanal_kurulmaz() {
        let creds = crate::credential::NotifyCredentials::default();
        let err = ControlClient::from_credentials("http://[::1]:7777", &creds).err();
        assert!(matches!(err, Some(NotifyError::ChannelUnconfigured("control-api"))));
    }
}
