//! Sampler katmani — vendored `xai-grok-sampler` aktorunu omni-router icin sarar.
//!
//! MASTER-PLAN Bolum 2 (B3 boslugu): tur dongusu ROUTER'a aittir. Vendored
//! `xai_grok_agent::Agent` tipinin `run`/`turn`/`step` metodu YOKTUR — o tip
//! sadece sistem promptu + ToolBridge + politika demetidir. Dolayisiyla "bir tur
//! calistir" isini burada, sampler aktoru uzerinden kuruyoruz.
//!
//! Aktorun sozlesmesinden gelen iki kritik kisit bu dosyanin sekli belirliyor:
//!
//! 1. `SamplerActor::spawn` icinde `tokio::spawn` cagirir — bu yuzden
//!    [`SamplerLayer::spawn`] mutlaka bir tokio runtime baglaminda cagrilmalidir.
//! 2. Aktorun event kanali UNBOUNDED ve cagiran tarafindan verilir. Drene
//!    edilmezse bellek suresiz buyur; bu yuzden katman kendi drenaj task'ini
//!    ayaga kaldirir ve olcum/izleme kayitlarini `tracing` uzerine dokar.
//!
//! Model adi, API anahtari ve base URL literal olarak BURADA YOKTUR; hepsi
//! cagirandan [`SamplerConfig`] ile gelir (I5).

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use xai_grok_sampler::{
    InferenceLatencyStats, RequestId, RetryPolicy, SamplerActor, SamplerConfig, SamplerHandle,
    SamplingEvent,
};
use xai_grok_sampling_types::{ConversationItem, ConversationRequest, ConversationResponse};

use crate::strategies::RouterError;

/// Sampler aktorunu ve onun event drenaj task'ini bir arada tutan katman.
///
/// Aktor, tum `SamplerHandle` klonlari dusunce kendiliginden sonlanir; bu tip
/// tek sahibi tuttugu icin `SamplerLayer` dusunce aktor de kapanir.
pub struct SamplerLayer {
    /// Aktore giden tek kanal. Klonlanabilir ama biz tek nusha tutuyoruz ki
    /// katman dusunce aktor de kapansin.
    handle: SamplerHandle,
    /// Event drenaj task'i — unbounded event kanalini surekli bosaltir.
    drain: JoinHandle<()>,
}

impl SamplerLayer {
    /// Aktoru ve drenaj task'ini baslatir.
    ///
    /// Tokio runtime baglaminda cagrilmalidir: hem `SamplerActor::spawn` hem de
    /// drenaj task'i `tokio::spawn` kullanir.
    pub fn spawn(config: SamplerConfig, retry: RetryPolicy) -> Self {
        // Event kanali UNBOUNDED; kapasite ayari yok, drenaj bizim sorumlulugumuz.
        let (event_tx, event_rx) = mpsc::unbounded_channel::<SamplingEvent>();

        // TAM 3 arguman: config, retry policy, event gondericisi. HTTP client yok.
        let handle = SamplerActor::spawn(config, retry, event_tx);
        let drain = tokio::spawn(drain_events(event_rx));

        Self { handle, drain }
    }

    /// Tek turluk cagri: verilen promptu kullanici mesaji olarak gonderir ve
    /// toplanmis cevabi gecikme metrikleriyle birlikte dondurur.
    ///
    /// Model/temperature/max_output_tokens gibi alanlar `None` birakilir; aktor
    /// bunlari kendi [`SamplerConfig`] varsayilanlarindan doldurur. Boylece bu
    /// dosyada hicbir literal model adi bulunmaz.
    pub async fn one_turn(
        &self,
        prompt: String,
    ) -> Result<(ConversationResponse, InferenceLatencyStats), RouterError> {
        let request = ConversationRequest {
            items: vec![ConversationItem::user(prompt)],
            ..Default::default()
        };

        let request_id = RequestId::random();
        // NOT: submit_and_collect'in RAII iptal koruyucusu vardir — bu future
        // dusurulurse (timeout / select!) ucustaki istek de iptal edilir.
        let outcome = self
            .handle
            .submit_and_collect(request_id.clone(), request)
            .await;

        match outcome {
            Ok((response, metrics)) => {
                debug!(
                    %request_id,
                    attempts = metrics.attempts,
                    ttft_ms = ?metrics.time_to_first_token_ms,
                    ttlb_ms = metrics.time_to_last_byte_ms,
                    "tur tamamlandi"
                );
                Ok((response, metrics))
            }
            Err(err) => {
                warn!(%request_id, %err, "tur basarisiz");
                Err(RouterError::Sampling(err))
            }
        }
    }

    /// Su an ucusta olan istek sayisi. Aktor kapaliysa 0 doner.
    pub async fn active_count(&self) -> usize {
        self.handle.active_count().await
    }

    /// Ucustaki bir istegi iptal eder. Bilinmeyen kimlik sessizce yok sayilir.
    pub fn cancel(&self, id: RequestId) {
        self.handle.cancel(id);
    }
}

impl Drop for SamplerLayer {
    fn drop(&mut self) {
        // Handle dusunce aktor kapanir ve event kanali sonlanir; drenaj task'ini
        // askida birakmamak icin ayrica iptal ediyoruz.
        self.drain.abort();
    }
}

impl std::fmt::Debug for SamplerLayer {
    // SamplerHandle Debug turetmez; katmani yine de loglanabilir tutuyoruz.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SamplerLayer")
            .field("drain_finished", &self.drain.is_finished())
            .finish_non_exhaustive()
    }
}

/// Aktorun paylasimli event kanalini bosaltir.
///
/// Tum istekler ayni kanali paylasir, bu yuzden her kayit `request_id` ile
/// etiketlenir. Kanal, aktor kapandiginda kapanir ve dongu kendiliginden biter.
async fn drain_events(mut event_rx: mpsc::UnboundedReceiver<SamplingEvent>) {
    while let Some(event) = event_rx.recv().await {
        match event {
            SamplingEvent::StreamStarted { request_id, .. } => {
                trace!(%request_id, "akis basladi");
            }
            SamplingEvent::FirstToken { request_id } => {
                trace!(%request_id, "ilk token");
            }
            SamplingEvent::Completed {
                request_id,
                metrics,
                ..
            } => {
                trace!(%request_id, attempts = metrics.attempts, "akis tamamlandi");
            }
            SamplingEvent::Retrying {
                request_id,
                attempt,
                max_retries,
                reason,
                ..
            } => {
                warn!(%request_id, attempt, max_retries, %reason, "yeniden deneniyor");
            }
            SamplingEvent::Failed { request_id, error } => {
                warn!(%request_id, ?error, "akis basarisiz");
            }
            other => {
                trace!(?other, "sampler olayi");
            }
        }
    }

    debug!("sampler event kanali kapandi; drenaj task'i sonlaniyor");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bos_aktor_aktif_istek_bildirmez() {
        let layer = SamplerLayer::spawn(SamplerConfig::default(), RetryPolicy::default());
        assert_eq!(layer.active_count().await, 0);
    }

    #[tokio::test]
    async fn bilinmeyen_istek_iptali_sessizce_gecer() {
        let layer = SamplerLayer::spawn(SamplerConfig::default(), RetryPolicy::default());
        layer.cancel(RequestId::from("boyle-bir-istek-yok"));
        assert_eq!(layer.active_count().await, 0);
    }

    #[tokio::test]
    async fn katman_dusunce_drenaj_taski_iptal_edilir() {
        let layer = SamplerLayer::spawn(SamplerConfig::default(), RetryPolicy::default());
        assert!(!format!("{layer:?}").is_empty());
        drop(layer);
    }
}
