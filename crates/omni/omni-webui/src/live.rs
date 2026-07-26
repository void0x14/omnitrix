//! Canli guncelleme: `StateEvent` akisi → HTML parcasi (Bolum 5, birincil SSE).
//!
//! Sozlesme `omni-control` ile aynidir (Bolum 6.2): once anlik goruntu, sonra
//! kesintisiz olay akisi. Fark yalnizca **render**tir (K7) — WebUI teli uzerinde
//! JSON durum degil, sunucuda uretilmis HTML parcasi tasir. Boylece istemcide
//! sablon motoru, durum agaci ya da build zinciri gerekmez (K8).
//!
//! Kurtarma: abone geride kalirsa `reload` olayi gonderilir; sayfa yeniden SSR
//! edilir ve `SystemSnapshot`'tan turer. `*Upserted` olaylari idempotent oldugu
//! icin bosluk sonrasi durum kendini duzeltir (crash-only, Bolum 8).

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use omni_control::{ControlError, ControlPlane};
use omni_proto::{StateEvent, SystemSnapshot};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::view::{self, ids};

/// HTML parcasi tasiyan SSE olay adi.
pub const EVENT_FRAGMENT: &str = "fragment";
/// Akista bosluk olustu; istemci sayfayi yeniden yuklemeli.
pub const EVENT_RELOAD: &str = "reload";

/// Parca hedefte bulunamadiginda kapsayiciya ekleme konumu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Swap {
    /// Kapsayicinin sonuna ekle (tablo satirlari: kronolojik).
    Append,
    /// Kapsayicinin basina ekle (akislar: en yeni ustte).
    Prepend,
}

/// Tel uzerinde tasinan HTML parcasi.
///
/// Uygulama kurali (istemci betigiyle birebir): `target` DOM'da varsa
/// `outerHTML` ile degistirilir — upsert idempotenttir. Yoksa `container`
/// icine `swap` konumundan eklenir. `container` yoksa parca dusulur.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fragment {
    /// Hedef elemanin DOM kimligi.
    pub target: String,
    /// Hedef yoksa eklenecegi kapsayici kimligi.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Ekleme konumu.
    pub swap: Swap,
    /// Sunucuda uretilmis HTML.
    pub html: String,
}

impl Fragment {
    /// Yalnizca yerine gecen parca (kapsayici yok: eleman zaten sayfada).
    #[must_use]
    pub fn replace(target: impl Into<String>, html: String) -> Self {
        Self {
            target: target.into(),
            container: None,
            swap: Swap::Append,
            html,
        }
    }

    /// Satir upsert'i: varsa degistir, yoksa kapsayicinin sonuna ekle.
    #[must_use]
    pub fn upsert(target: impl Into<String>, container: &str, html: String) -> Self {
        Self {
            target: target.into(),
            container: Some(container.to_string()),
            swap: Swap::Append,
            html,
        }
    }

    /// Akis kaydi: en yeni ustte.
    #[must_use]
    pub fn feed(target: impl Into<String>, container: &str, html: String) -> Self {
        Self {
            target: target.into(),
            container: Some(container.to_string()),
            swap: Swap::Prepend,
            html,
        }
    }

    /// Tel govdesi. Tek satirdir: maud ciktisi ve JSON kacislama satir sonu
    /// uretmez, bu yuzden SSE `data:` alanina dogrudan yazilabilir.
    ///
    /// # Errors
    /// Serilestirme basarisiz olursa [`serde_json::Error`] doner.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Tum panoyu tasiyan parca — ilk baglantida ve bosluk sonrasi kullanilir.
#[must_use]
pub fn board_fragment(snapshot: &SystemSnapshot) -> Fragment {
    Fragment::replace(ids::BOARD, view::board(snapshot).into_string())
}

/// Tek bir `StateEvent`'i HTML parcasina cevirir.
///
/// Yedi varyantin hepsi kapsanir; yeni bir varyant eklendiginde derleme burada
/// kirilir — sessiz kayip olmaz.
#[must_use]
pub fn fragment_for(event: &StateEvent) -> Fragment {
    match event {
        StateEvent::AgentUpserted(agent) => Fragment::upsert(
            view::agent_dom_id(agent.id),
            ids::AGENTS,
            view::agent_row(agent).into_string(),
        ),
        StateEvent::TaskUpserted(task) => Fragment::upsert(
            view::task_dom_id(task.id),
            ids::TASKS,
            view::task_row(task).into_string(),
        ),
        StateEvent::FileTouched(touch) => Fragment::feed(
            view::feed_dom_id("touch", touch.id, touch.ts),
            ids::TOUCHES,
            view::touch_item(touch).into_string(),
        ),
        StateEvent::ToolCall(call) => Fragment::feed(
            view::feed_dom_id("tool", call.id, call.ts),
            ids::TOOLS,
            view::tool_item(call).into_string(),
        ),
        StateEvent::Interrupt(interrupt) => Fragment::feed(
            view::feed_dom_id("interrupt", interrupt.id, interrupt.ts),
            ids::INTERRUPTS,
            view::interrupt_item(interrupt).into_string(),
        ),
        StateEvent::Notice(notice) => Fragment::feed(
            view::feed_dom_id("notice", None, notice.ts),
            ids::NOTICES,
            view::notice_item(notice).into_string(),
        ),
        StateEvent::ResourceTick(gauge) => {
            Fragment::replace(ids::RESOURCE, view::resource_panel(gauge).into_string())
        }
    }
}

/// Parcayi SSE olayina sarar.
fn sse_event(fragment: &Fragment) -> Option<Event> {
    match fragment.to_json() {
        Ok(body) => Some(Event::default().event(EVENT_FRAGMENT).data(body)),
        Err(err) => {
            tracing::error!(error = %err, "HTML parcasi SSE'ye serilestirilemedi, atlandi");
            None
        }
    }
}

/// **SSE ucu (birincil).** Once tum pano, sonra kesintisiz parca akisi.
///
/// Abonelik anlik goruntuden **once** acilir: sayfa render'i ile akisin
/// baslangici arasindaki olaylar kaybolmaz.
///
/// # Errors
/// Cekirdek anlik goruntu veremezse ya da ilk parca serilestirilemezse
/// [`ControlError`] doner.
pub async fn sse_handler<S: ControlPlane>(
    State(state): State<S>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>> + Send + 'static>, ControlError> {
    let receiver = state.events().subscribe();
    let snapshot = state.snapshot().await?;

    let first = sse_event(&board_fragment(&snapshot))
        .ok_or_else(|| ControlError::Serialization("pano parcasi serilestirilemedi".to_string()))?;

    let tail = BroadcastStream::new(receiver).filter_map(|item| async move {
        match item {
            Ok(event) => sse_event(&fragment_for(event.as_ref())).map(Ok),
            Err(BroadcastStreamRecvError::Lagged(skipped)) => {
                tracing::warn!(
                    skipped,
                    "WebUI abonesi geride kaldi; sayfa yeniden yuklenecek"
                );
                Some(Ok(Event::default().event(EVENT_RELOAD).data("{}")))
            }
        }
    });

    let stream = futures::stream::once(async move { Ok(first) }).chain(tail);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{
        AgentState, AgentTier, AgentView, FileTouch, InterruptView, NoticeLevel, NoticeView,
        ResourceGauge, TaskView, ToolCallView,
    };

    fn agent() -> AgentView {
        AgentView {
            id: 5,
            persona: "executor".into(),
            tier: AgentTier::Queued,
            task_id: 2,
            parent_id: Some(1),
            state: AgentState::AwaitingModel,
            rss_kb: 1024,
            tokens_in: 10,
            tokens_out: 5,
            cost: 0.5,
            trust: 1.0,
            depth: 2,
            last_event_seq: 3,
        }
    }

    #[test]
    fn ajan_olayi_satiri_upsert_eder() {
        let fragment = fragment_for(&StateEvent::AgentUpserted(agent()));
        assert_eq!(fragment.target, "agent-5");
        assert_eq!(fragment.container.as_deref(), Some(ids::AGENTS));
        assert_eq!(fragment.swap, Swap::Append);
        assert!(fragment.html.contains("id=\"agent-5\""));
    }

    #[test]
    fn gorev_olayi_satiri_upsert_eder() {
        let task = TaskView {
            id: 2,
            parent_id: Some(1),
            root_id: 1,
            title: "alt gorev".into(),
            mode: "autonomous".into(),
            status: "queued".into(),
            depth: 1,
            budget_allocated: None,
            budget_spent: None,
            duration_target: None,
            created_at: omni_proto::now(),
            closed_at: None,
        };
        let fragment = fragment_for(&StateEvent::TaskUpserted(task));
        assert_eq!(fragment.target, "task-2");
        assert_eq!(fragment.container.as_deref(), Some(ids::TASKS));
    }

    #[test]
    fn akis_olaylari_basa_eklenir() {
        let ts = omni_proto::now();
        let touch = StateEvent::FileTouched(FileTouch {
            id: Some(1),
            agent_id: 1,
            path: "src/lib.rs".into(),
            outside_workspace: false,
            added: 1,
            removed: 0,
            pre_ref: None,
            post_ref: None,
            ts,
        });
        let call = StateEvent::ToolCall(ToolCallView {
            id: Some(2),
            agent_id: 1,
            tool: "read_file".into(),
            args: None,
            result_ref: None,
            status: "ok".into(),
            capability_ok: Some(true),
            ts,
        });
        let interrupt = StateEvent::Interrupt(InterruptView {
            id: Some(3),
            agent_id: 1,
            kind: "pause".into(),
            source: "tui".into(),
            reason: None,
            ts,
            resolved_at: None,
        });
        let notice = StateEvent::Notice(NoticeView::new(
            NoticeLevel::Info,
            "spawned",
            "ajan acildi",
            ts,
        ));

        for (event, container) in [
            (touch, ids::TOUCHES),
            (call, ids::TOOLS),
            (interrupt, ids::INTERRUPTS),
            (notice, ids::NOTICES),
        ] {
            let fragment = fragment_for(&event);
            assert_eq!(fragment.swap, Swap::Prepend);
            assert_eq!(fragment.container.as_deref(), Some(container));
        }
    }

    #[test]
    fn kaynak_olayi_paneli_degistirir() {
        let fragment = fragment_for(&StateEvent::ResourceTick(ResourceGauge::default()));
        assert_eq!(fragment.target, ids::RESOURCE);
        assert!(fragment.container.is_none());
    }

    #[test]
    fn parca_tek_satir_json_uretir() {
        let mut task = TaskView {
            id: 1,
            parent_id: None,
            root_id: 1,
            title: "satir\nsonu".into(),
            mode: "m".into(),
            status: "s".into(),
            depth: 0,
            budget_allocated: None,
            budget_spent: None,
            duration_target: None,
            created_at: omni_proto::now(),
            closed_at: None,
        };
        task.title.push_str(" <b>");
        let json = fragment_for(&StateEvent::TaskUpserted(task))
            .to_json()
            .expect("kodlama");
        assert!(!json.contains('\n'));
        assert!(json.contains("&lt;b&gt;"));
    }

    #[test]
    fn pano_parcasi_tum_panoyu_tasir() {
        let fragment = board_fragment(&SystemSnapshot::empty(omni_proto::now()));
        assert_eq!(fragment.target, ids::BOARD);
        assert!(fragment.html.contains("id=\"agents\""));
    }

    #[test]
    fn swap_adlari_tel_uzerinde_kararli() {
        assert_eq!(
            serde_json::to_value(Swap::Prepend).expect("kodlama"),
            "prepend"
        );
        assert_eq!(
            serde_json::to_value(Swap::Append).expect("kodlama"),
            "append"
        );
    }
}
