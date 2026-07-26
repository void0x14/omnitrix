//! WebSocket ucu — cift yonlu alternatif (Bolum 5: SSE birincil, WS cift yon).
//!
//! Asagi yon [`live::Fragment`] ile ayni HTML parcalarini tasir; yukari yonde
//! istemci kanonik `Command` JSON'u gonderir. Yazma yolu form POST'u ile **ayni**
//! cekirdek ucuna duser (Bolum 6.2): sonuc `StateEvent` olarak her iki yuze
//! yayilir, bu yuzden basarida ack gonderilmez.
//!
//! Cerceve bicimi `omni-control` ile ayni zarftir — `{ "kind": ..., "payload": ... }` —
//! yalnizca `payload` icerigi JSON durum degil, sunucuda uretilmis HTML'dir (K7).

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures::stream::StreamExt;
use omni_control::ControlPlane;
use omni_proto::Command;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::live::{self, Fragment};

/// HTML parcasi tasiyan cerceve.
pub const FRAME_FRAGMENT: &str = "fragment";
/// Akista bosluk olustu; istemci sayfayi yeniden yuklemeli.
pub const FRAME_RELOAD: &str = "reload";
/// Hata cercevesi (yalnizca yukari yon icin).
pub const FRAME_ERROR: &str = "error";

/// Ortak cerceve bicimi.
fn frame(kind: &str, payload: Value) -> Value {
    serde_json::json!({ "kind": kind, "payload": payload })
}

/// Parcayi cerceveye sarar; serilestirilemezse `None`.
fn fragment_frame(fragment: &Fragment) -> Option<Value> {
    match serde_json::to_value(fragment) {
        Ok(payload) => Some(frame(FRAME_FRAGMENT, payload)),
        Err(err) => {
            tracing::error!(error = %err, "HTML parcasi WS'e serilestirilemedi, atlandi");
            None
        }
    }
}

/// **WS ucu.** Yukseltmeyi kabul eder ve oturumu baslatir.
pub async fn ws_handler<S: ControlPlane>(
    upgrade: WebSocketUpgrade,
    State(state): State<S>,
) -> Response {
    upgrade.on_upgrade(move |socket| ws_session(socket, state))
}

/// Tek bir WebSocket oturumu.
///
/// 1. Ilk cerceve: tum pano (anlik goruntuden SSR).
/// 2. Sonrasi: olay basina bir parca + istemci komutlari.
async fn ws_session<S: ControlPlane>(socket: WebSocket, state: S) {
    // Abonelik anlik goruntuden once acilir: aradaki olaylar kaybolmasin.
    let mut receiver = state.events().subscribe();
    let (mut sink, mut source) = socket.split();

    let first = match state.snapshot().await {
        Ok(snapshot) => fragment_frame(&live::board_fragment(&snapshot)),
        Err(err) => Some(frame(
            FRAME_ERROR,
            serde_json::json!({ "code": err.code(), "detail": err.to_string() }),
        )),
    };
    match first {
        Some(value) => {
            if !send_frame(&mut sink, value).await {
                return;
            }
        }
        None => return,
    }

    loop {
        tokio::select! {
            event = receiver.recv() => {
                let outgoing = match event {
                    Ok(event) => match fragment_frame(&live::fragment_for(event.as_ref())) {
                        Some(value) => value,
                        None => continue,
                    },
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "WebUI WS abonesi geride kaldi; yeniden yukleme gerekli");
                        frame(FRAME_RELOAD, serde_json::json!({ "skipped": skipped }))
                    }
                    // Yayinci kapandi: oturum sonlanir.
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
                    // Ping/Pong axum tarafindan yanitlanir; ikili cerceve kullanilmaz.
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        tracing::debug!(error = %err, "WebUI WS oturumu hata ile kapandi");
                        break;
                    }
                }
            }
        }
    }
}

/// Istemciden gelen metni kanonik `Command` olarak cozer ve cekirdege iletir.
///
/// Donen deger yalnizca hata durumunda doludur; basarida sonuc akista gorunur.
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

/// Cerceveyi sokete yazar; baglanti yasiyorsa `true` doner.
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
    use omni_proto::SystemSnapshot;

    #[test]
    fn cerceve_bicimi_kararli() {
        let value = frame(FRAME_FRAGMENT, serde_json::json!({ "a": 1 }));
        assert_eq!(value["kind"], FRAME_FRAGMENT);
        assert_eq!(value["payload"]["a"], 1);
    }

    #[test]
    fn cerceve_adlari_ayrik() {
        let names = [FRAME_FRAGMENT, FRAME_RELOAD, FRAME_ERROR];
        for (i, a) in names.iter().enumerate() {
            for b in names.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn pano_cercevesi_html_tasir() {
        let snapshot = SystemSnapshot::empty(omni_proto::now());
        let value = fragment_frame(&live::board_fragment(&snapshot)).expect("cerceve");
        assert_eq!(value["kind"], FRAME_FRAGMENT);
        assert_eq!(value["payload"]["target"], "board");
        let html = value["payload"]["html"].as_str().expect("html");
        assert!(html.contains("id=\"agents\""));
    }

    #[test]
    fn komut_govdesi_cozulur() {
        let raw = serde_json::json!({
            "cmd": "write_to_agent",
            "agent_id": 4,
            "content": "devam"
        })
        .to_string();
        let command: Command = serde_json::from_str(&raw).expect("cozme");
        assert_eq!(command.target_agent(), Some(4));
    }
}
