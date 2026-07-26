//! Bölüm 6.2 — okuma ucu: **SSE birincil**, WebSocket çift yönlü (Bölüm 5).
//!
//! Akış sözleşmesi her iki taşımada da aynıdır:
//! 1. İlk yükleme: bir adet `SystemSnapshot`.
//! 2. Sonrası: kesintisiz `StateEvent` akışı.
//!
//! TUI ve WebUI **aynı** akışı tüketir; yalnızca render farklıdır (K7). Bu
//! yüzden yayıncı tektir: bir kanaldan gelen komutun sonucu diğerinde de görünür.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Response;
use futures::stream::{Stream, StreamExt};
use omni_proto::{Command, StateEvent};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::{ControlError, ControlPlane};

/// İlk yükleme çerçevesi/olay adı.
pub const FRAME_SNAPSHOT: &str = "snapshot";
/// Durum olayı çerçevesi/olay adı.
pub const FRAME_STATE: &str = "state";
/// Abone geride kaldığında (yayın tamponu taştı) gönderilen uyarı çerçevesi.
/// İstemci bunu görünce yeniden `snapshot` almalıdır.
pub const FRAME_LAGGED: &str = "lagged";
/// Hata çerçevesi (yalnız WebSocket'te; SSE tarafında akış hatasızdır).
pub const FRAME_ERROR: &str = "error";

/// Yayın tamponunun varsayılan derinliği.
pub const DEFAULT_CAPACITY: usize = 1024;

/// `StateEvent` yayıncısı.
///
/// Olaylar `Arc` ile paylaşılır: 10.000 var-olan ajanlı bir sistemde her
/// aboneye kopya çıkarmak yerine tek tahsis paylaşılır.
#[derive(Clone, Debug)]
pub struct Broadcaster {
    tx: broadcast::Sender<Arc<StateEvent>>,
}

impl Broadcaster {
    /// Belirtilen tampon derinliğiyle yayıncı kurar.
    ///
    /// `capacity` sıfır olamaz; sıfır verilirse 1'e yükseltilir (panik yok, I6).
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(1));
        Self { tx }
    }

    /// Olayı tüm abonelere yayınlar; ulaşılan abone sayısını döndürür.
    ///
    /// Abone yoksa olay düşer — bu hata değildir (headless çalışma normaldir).
    pub fn publish(&self, event: StateEvent) -> usize {
        self.publish_shared(Arc::new(event))
    }

    /// Zaten paylaşılan bir olayı yayınlar (yeniden tahsis yok).
    pub fn publish_shared(&self, event: Arc<StateEvent>) -> usize {
        self.tx.send(event).unwrap_or(0)
    }

    /// Yeni bir abonelik açar.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<StateEvent>> {
        self.tx.subscribe()
    }

    /// Anlık abone sayısı.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Alttaki gönderici (çekirdek tarafının klonlayıp taşıması için).
    pub fn sender(&self) -> &broadcast::Sender<Arc<StateEvent>> {
        &self.tx
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

/// Ortak çerçeve biçimi: `{ "kind": ..., "payload": ... }`.
fn frame(kind: &str, payload: Value) -> Value {
    serde_json::json!({ "kind": kind, "payload": payload })
}

/// Geride kalma uyarısının yükü.
fn lagged_payload(skipped: u64) -> Value {
    serde_json::json!({ "skipped": skipped })
}

/// **SSE ucu (birincil).** `GET` ile açılır; önce anlık görüntü, sonra olay akışı.
///
/// Akış içi hata yoktur: serileştirilemeyen bir olay atlanır ve log'lanır;
/// bağlantı kopmaz.
pub async fn sse_handler<S: ControlPlane>(
    State(state): State<S>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>> + Send + 'static>, ControlError> {
    // Aboneliği anlık görüntüden ÖNCE açıyoruz: aradaki olaylar kaybolmasın.
    let receiver = state.events().subscribe();
    let snapshot = state.snapshot().await?;

    let first = Event::default()
        .event(FRAME_SNAPSHOT)
        .json_data(&snapshot)
        .map_err(|err| ControlError::Serialization(err.to_string()))?;

    let tail = BroadcastStream::new(receiver).filter_map(|item| async move {
        match item {
            Ok(event) => match Event::default().event(FRAME_STATE).json_data(event.as_ref()) {
                Ok(sse) => Some(Ok(sse)),
                Err(err) => {
                    tracing::error!(error = %err, "StateEvent SSE'ye serilestirilemedi, atlandi");
                    None
                }
            },
            Err(BroadcastStreamRecvError::Lagged(skipped)) => {
                tracing::warn!(skipped, "SSE abonesi geride kaldi; yeniden snapshot gerekli");
                Some(Ok(Event::default()
                    .event(FRAME_LAGGED)
                    .data(lagged_payload(skipped).to_string())))
            }
        }
    });

    let stream = futures::stream::once(async move { Ok(first) }).chain(tail);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// **WebSocket ucu (çift yön, Bölüm 5).**
///
/// Aşağı yön SSE ile aynı çerçeveleri taşır; yukarı yönde istemci `Command`
/// JSON'u gönderebilir. Yazma yolu `api` ile aynı çekirdek ucuna düşer.
pub async fn ws_handler<S: ControlPlane>(
    upgrade: WebSocketUpgrade,
    State(state): State<S>,
) -> Response {
    upgrade.on_upgrade(move |socket| ws_session(socket, state))
}

/// Tek bir WebSocket oturumunun döngüsü.
async fn ws_session<S: ControlPlane>(socket: WebSocket, state: S) {
    let mut receiver = state.events().subscribe();
    let (mut sink, mut source) = socket.split();

    // 1. İlk yükleme: SystemSnapshot.
    let first = match state.snapshot().await {
        Ok(snapshot) => match serde_json::to_value(&snapshot) {
            Ok(value) => frame(FRAME_SNAPSHOT, value),
            Err(err) => frame(FRAME_ERROR, serde_json::json!({ "detail": err.to_string() })),
        },
        Err(err) => frame(FRAME_ERROR, serde_json::json!({ "detail": err.to_string() })),
    };
    if !send_frame(&mut sink, first).await {
        return;
    }

    // 2. Sonrası: olay akışı + istemci komutları.
    loop {
        tokio::select! {
            event = receiver.recv() => {
                let outgoing = match event {
                    Ok(event) => match serde_json::to_value(event.as_ref()) {
                        Ok(value) => frame(FRAME_STATE, value),
                        Err(err) => {
                            tracing::error!(error = %err, "StateEvent WS'e serilestirilemedi, atlandi");
                            continue;
                        }
                    },
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "WS abonesi geride kaldi; yeniden snapshot gerekli");
                        frame(FRAME_LAGGED, lagged_payload(skipped))
                    }
                    // Yayıncı kapandı: oturum sonlanır.
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if !send_frame(&mut sink, outgoing).await {
                    break;
                }
            }
            incoming = source.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(reply) = handle_client_text(&state, text.as_str()).await
                            && !send_frame(&mut sink, reply).await
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    // Ping/Pong axum tarafından yanıtlanır; ikili çerçeve kullanılmaz.
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        tracing::debug!(error = %err, "WS oturumu hata ile kapandi");
                        break;
                    }
                }
            }
        }
    }
}

/// İstemciden gelen metni `Command` olarak çözer ve çekirdeğe iletir.
///
/// Dönen değer yalnızca hata durumunda doludur; başarıda sonuç `StateEvent`
/// olarak yayına düşer (Bölüm 6.2), ayrıca ack gönderilmez.
async fn handle_client_text<S: ControlPlane>(state: &S, text: &str) -> Option<Value> {
    let command = match serde_json::from_str::<Command>(text) {
        Ok(command) => command,
        Err(err) => {
            return Some(frame(
                FRAME_ERROR,
                serde_json::json!({ "code": "bad_request", "detail": err.to_string() }),
            ));
        }
    };

    match state.dispatch(command).await {
        Ok(()) => None,
        Err(err) => Some(frame(
            FRAME_ERROR,
            serde_json::json!({ "code": err.code(), "detail": err.to_string() }),
        )),
    }
}

/// Çerçeveyi sokete yazar; bağlantı yaşıyorsa `true` döner.
async fn send_frame<W>(sink: &mut W, payload: Value) -> bool
where
    W: futures::SinkExt<Message> + Unpin,
{
    sink.send(Message::Text(payload.to_string().into()))
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_capacity_is_clamped() {
        let broadcaster = Broadcaster::new(0);
        assert_eq!(broadcaster.subscriber_count(), 0);
    }

    #[test]
    fn publish_without_subscribers_is_not_an_error() {
        let broadcaster = Broadcaster::new(4);
        assert_eq!(broadcaster.subscriber_count(), 0);
    }

    #[test]
    fn subscriber_count_tracks_subscriptions() {
        let broadcaster = Broadcaster::default();
        let first = broadcaster.subscribe();
        assert_eq!(broadcaster.subscriber_count(), 1);
        let second = broadcaster.subscribe();
        assert_eq!(broadcaster.subscriber_count(), 2);
        drop(first);
        drop(second);
        assert_eq!(broadcaster.subscriber_count(), 0);
    }

    #[test]
    fn frame_shape_is_stable() {
        let value = frame(FRAME_STATE, serde_json::json!({ "a": 1 }));
        assert_eq!(value["kind"], FRAME_STATE);
        assert_eq!(value["payload"]["a"], 1);
    }

    #[test]
    fn lagged_payload_carries_skip_count() {
        let value = lagged_payload(42);
        assert_eq!(value["skipped"], 42);
    }

    #[test]
    fn frame_names_are_distinct() {
        let names = [FRAME_SNAPSHOT, FRAME_STATE, FRAME_LAGGED, FRAME_ERROR];
        for (i, a) in names.iter().enumerate() {
            for b in names.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }
}
