//! Kontrol duzlemi baglayicisi (MASTER-PLAN 3.2).
//!
//! `omnitrix` KENDI API'sini kurmaz. Tek kontrol duzlemi `omni-control`'dur;
//! paralel bir rota tablosu tek durum kaynagi kuralini bozardi (I3). Bu modul
//! yalnizca uc parcayi birbirine baglar:
//!
//! * [`TokenVerifier`] — zorunlu auth (Bolum 13, K9). Token yoksa API hic
//!   baslatilmaz; "sessizce acik" bir mod yoktur.
//! * [`Broadcaster`] — SSE/WS yayini. TUI ve WebUI ayni akisi tuketir (K7).
//! * [`CoreState`] — tek yazar. Okuma ucu `SnapshotSource`, yazma ucu
//!   `CommandSink` olarak buradaki [`CoreBridge`] uzerinden acilir (6.2).
//!
//! Uretim yolunda panik yoktur: kurulum hatalari [`ApiSetupError`] ile,
//! istek hatalari `omni_control::ControlError` ile tasinir (I6).

#![allow(dead_code)]

use std::sync::Arc;

use futures::future::BoxFuture;
use omni_control::api::router;
use omni_control::stream::DEFAULT_CAPACITY;
use omni_control::{
    Broadcaster, CommandSink, ControlError, ControlState, SnapshotSource, TokenRecord,
    TokenVerifier,
};
use omni_core::CoreState;
use omni_proto::{Command, SystemSnapshot};
use tokio::sync::Mutex;

/// Token hash'lerini tasiyan ortam degiskeni. Dosya katmaninin onunde gelir
/// (env > config_kv > dosya, AS8).
pub const TOKEN_HASH_ENV: &str = "OMNITRIX_API_TOKEN_HASH";

/// Birden fazla kaydin ayraci: `id=<phc>;id=<phc>`.
const RECORD_SEP: char = ';';

/// Kimlik verilmeyen kayitlarin mantiksal adi (log/yetkilendirme izi icin).
const DEFAULT_TOKEN_ID: &str = "operator";

/// Kontrol duzlemi kurulum hatalari.
///
/// Hicbiri "auth kapali" moduna dusmez: hata varsa API baslatilmaz (K9).
#[derive(Debug, thiserror::Error)]
pub enum ApiSetupError {
    /// Ne ortamda ne de yapilandirmada token hash'i var.
    #[error(
        "kontrol duzlemi token'i tanimli degil (OMNITRIX_API_TOKEN_HASH ya da \
         config [api].token_hash); auth zorunludur, API baslatilmadi"
    )]
    MissingToken,

    /// Kayit argon2 PHC dizesi degil — kurulum hatasi.
    #[error("token kaydi cozulemedi: '{0}' argon2 PHC dizesi ('$...') icermiyor")]
    MalformedRecord(String),
}

/// `id=<phc>;<phc>;...` dizesini token kayitlarina cevirir.
///
/// PHC govdesi de `=` icerdiginden (`m=19456,t=2,p=1`) once `$` onekine bakilir:
/// `$` ile baslayan kayit dogrudan PHC'dir ve [`DEFAULT_TOKEN_ID`] alir.
///
/// # Errors
/// Hicbir gecerli kayit yoksa [`ApiSetupError::MissingToken`], bir kayit
/// bicimsizse [`ApiSetupError::MalformedRecord`] doner.
pub fn parse_token_records(raw: &str) -> Result<Vec<TokenRecord>, ApiSetupError> {
    let mut records = Vec::new();

    for entry in raw.split(RECORD_SEP) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }

        let (id, phc) = if entry.starts_with('$') {
            (DEFAULT_TOKEN_ID, entry)
        } else {
            let Some((id, phc)) = entry.split_once('=') else {
                return Err(ApiSetupError::MalformedRecord(entry.to_owned()));
            };
            (id.trim(), phc.trim())
        };

        if id.is_empty() || !phc.starts_with('$') {
            return Err(ApiSetupError::MalformedRecord(entry.to_owned()));
        }
        records.push(TokenRecord::new(id, phc));
    }

    if records.is_empty() {
        return Err(ApiSetupError::MissingToken);
    }
    Ok(records)
}

/// Token dogrulayiciyi cozer: once ortam degiskeni, sonra yapilandirma (AS8).
///
/// Ham token hicbir zaman okunmaz/saklanmaz; yalnizca argon2 PHC dizesi gelir.
///
/// # Errors
/// Iki katmanda da kayit yoksa ya da kayit bicimsizse hata doner.
pub fn token_verifier(config_hashes: Option<&str>) -> Result<TokenVerifier, ApiSetupError> {
    let from_env = std::env::var(TOKEN_HASH_ENV).ok();
    let env_layer = from_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let file_layer = config_hashes
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let Some(raw) = env_layer.or(file_layer) else {
        return Err(ApiSetupError::MissingToken);
    };
    Ok(TokenVerifier::new(parse_token_records(raw)?))
}

/// `CoreState`'i kontrol duzlemine baglayan ince adaptor.
///
/// Okuma `CoreState::snapshot`, yazma `CoreState::apply`. Komuttan cikan
/// `StateEvent` listesi ayni anda yayina dusurulur; boylece TUI ve WebUI
/// sonucu ayni akistan gorur (K7). Mutasyon yalnizca burada, kilit altinda
/// yapilir — tek yazar kurali korunur (6.2).
pub struct CoreBridge {
    core: Arc<Mutex<CoreState>>,
    events: Broadcaster,
}

impl CoreBridge {
    /// Cekirdek + yayinci ciftinden kopru kurar.
    pub fn new(core: Arc<Mutex<CoreState>>, events: Broadcaster) -> Self {
        Self { core, events }
    }
}

impl SnapshotSource for CoreBridge {
    fn snapshot(&self) -> BoxFuture<'_, Result<SystemSnapshot, ControlError>> {
        Box::pin(async move { Ok(self.core.lock().await.snapshot()) })
    }
}

impl CommandSink for CoreBridge {
    fn dispatch(&self, command: Command) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            // Kilit yalnizca mutasyon suresince tutulur; yayin kilit disinda.
            let produced = {
                let mut core = self.core.lock().await;
                core.apply(command)
                    .map_err(|err| ControlError::Command(err.to_string()))?
            };
            for event in produced {
                self.events.publish(event);
            }
            Ok(())
        })
    }
}

/// `omni-control` router'ini tasiyan sunucu sarmalayicisi.
///
/// Kendi rotasi yoktur; `omni_control::api::router` ne veriyorsa onu servis
/// eder (`/v1/snapshot`, `/v1/events`, `/v1/ws`, `/v1/command`).
pub struct ControlPlaneApi {
    addr: String,
    state: ControlState,
}

impl ControlPlaneApi {
    /// Baglayiciyi kurar.
    ///
    /// # Errors
    /// Token hash'i cozulemezse hata doner ve **API baslatilmaz** (K9).
    pub fn new(
        addr: impl Into<String>,
        config_hashes: Option<&str>,
        core: Arc<Mutex<CoreState>>,
    ) -> Result<Self, ApiSetupError> {
        let verifier = token_verifier(config_hashes)?;
        let events = Broadcaster::new(DEFAULT_CAPACITY);
        let bridge = Arc::new(CoreBridge::new(core, events.clone()));
        let state = ControlState::new(verifier, events, bridge.clone(), bridge);
        Ok(Self {
            addr: addr.into(),
            state,
        })
    }

    /// Dinlenecek adres.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Yayinciyi klonlar; alt katmanlar olaylarini buraya dusurur.
    pub fn broadcaster(&self) -> Broadcaster {
        self.state.broadcaster().clone()
    }

    /// Sunucuyu calistirir. Cagiran bunu `tokio::spawn` ile arka plana atar;
    /// TUI ilk frame'i bunu beklemeden cizer (8.2).
    ///
    /// # Errors
    /// Adres baglanamazsa ya da sunucu dususe gecerse hata doner.
    pub async fn serve(self) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(&self.addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "kontrol duzlemi dinlemede");
        axum::serve(listener, router(self.state)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHC: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$aGFzaGhhc2hoYXNoaGFzaA";

    #[test]
    fn bare_phc_gets_default_identity() {
        let records = parse_token_records(PHC).expect("kayit cozulmeli");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id(), DEFAULT_TOKEN_ID);
        assert_eq!(records[0].phc(), PHC);
    }

    #[test]
    fn named_records_are_split_on_first_equals() {
        let raw = format!("tui={PHC} ; webui={PHC}");
        let records = parse_token_records(&raw).expect("kayitlar cozulmeli");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id(), "tui");
        assert_eq!(records[1].id(), "webui");
        // PHC govdesindeki '=' karakterleri bozulmadan gecmeli.
        assert_eq!(records[1].phc(), PHC);
    }

    #[test]
    fn empty_input_is_missing_token() {
        assert!(matches!(
            parse_token_records("  ; ;"),
            Err(ApiSetupError::MissingToken)
        ));
    }

    #[test]
    fn non_phc_record_is_rejected() {
        assert!(matches!(
            parse_token_records("duz-metin-token"),
            Err(ApiSetupError::MalformedRecord(_))
        ));
        assert!(matches!(
            parse_token_records("tui=duz-metin"),
            Err(ApiSetupError::MalformedRecord(_))
        ));
        assert!(matches!(
            parse_token_records(&format!("={PHC}")),
            Err(ApiSetupError::MalformedRecord(_))
        ));
    }

    #[test]
    fn config_layer_is_used_when_env_absent() {
        // Ortam degiskenine dokunulmaz; testler paralel kostugu icin yalnizca
        // dosya katmani beslenir ve env tanimsizken bu katmanin gectigi olculur.
        if std::env::var(TOKEN_HASH_ENV).is_ok() {
            return;
        }
        let verifier = token_verifier(Some(PHC)).expect("dosya katmani gecmeli");
        assert_eq!(verifier.len(), 1);
    }

    #[test]
    fn missing_everywhere_refuses_to_start() {
        if std::env::var(TOKEN_HASH_ENV).is_ok() {
            return;
        }
        assert!(matches!(
            token_verifier(None),
            Err(ApiSetupError::MissingToken)
        ));
        assert!(matches!(
            token_verifier(Some("   ")),
            Err(ApiSetupError::MissingToken)
        ));
    }

    #[tokio::test]
    async fn bridge_applies_command_and_publishes_events() {
        let core = Arc::new(Mutex::new(CoreState::new()));
        let events = Broadcaster::new(16);
        let mut rx = events.subscribe();
        let bridge = CoreBridge::new(Arc::clone(&core), events);

        bridge
            .dispatch(Command::SpawnTask {
                title: "dikey dilim".into(),
                mode: "user_driven".into(),
                parent_id: None,
                persona: None,
                duration_target: None,
                budget: None,
            })
            .await
            .expect("komut uygulanmali");

        // Cekirdek uc olay uretir: gorev, ajan, kaynak olcumu.
        assert_eq!(rx.recv().await.expect("olay").kind(), "task_upserted");
        assert_eq!(rx.recv().await.expect("olay").kind(), "agent_upserted");
        assert_eq!(rx.recv().await.expect("olay").kind(), "resource_tick");

        let snapshot = bridge.snapshot().await.expect("goruntu");
        assert_eq!(snapshot.tasks.len(), 1);
        assert_eq!(snapshot.agents.len(), 1);
    }

    #[tokio::test]
    async fn bridge_reports_core_rejection_as_command_error() {
        let core = Arc::new(Mutex::new(CoreState::new()));
        let bridge = CoreBridge::new(core, Broadcaster::new(4));
        let result = bridge
            .dispatch(Command::WriteToAgent {
                agent_id: 7,
                content: "selam".into(),
            })
            .await;
        assert!(matches!(result, Err(ControlError::Command(_))));
    }
}
