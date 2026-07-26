//! Kontrol duzlemi akis istemcisi (MASTER-PLAN 6.2 okuma sozlesmesi + Bolum 5).
//!
//! `omni-control` iki tasima sunar ve **ikisi de ayni cerceveleri** tasir:
//! SSE'de cerceve adi `event:` alanidir, WebSocket'te govde
//! `{ "kind": ..., "payload": ... }`'dir. Sozlesme her iki uctа da aynidir:
//! once bir [`SystemSnapshot`], sonrasinda kesintisiz [`StateEvent`] akisi.
//!
//! WebUI ayni akisi tuketir; farkli olan yalnizca render'dir (K7). Bu yuzden
//! cerceve adlari burada **yeniden tanimlanmaz**, `omni-control`'den alinir —
//! tek kaynak kurali (I3/B5).
//!
//! Onemli ayrim: yayin ucu `StateEvent`'i **sira numarasiz** tasir; bosluk
//! tespiti sunucu tarafinda yapilir ve istemciye [`FRAME_LAGGED`] cercevesi
//! olarak bildirilir. Bu yuzden akistan gelen olay [`UiState::apply_event`] ile
//! uygulanir; `seq` ureten [`omni_proto::StateFrame`] yolu (yerel/test surucusu)
//! ayri durur.
//!
//! # Ornek
//! ```
//! use omni_tui::{ControlFrame, ControlOutcome, UiState};
//!
//! let mut state = UiState::new();
//! let frame = ControlFrame::from_ws_text(
//!     r#"{"kind":"lagged","payload":{"skipped":7}}"#,
//! )
//! .expect("cerceve");
//! assert!(matches!(state.apply_control(frame), ControlOutcome::Lagged { skipped: 7 }));
//! assert!(state.needs_resync());
//! ```

use omni_control::stream::{FRAME_ERROR, FRAME_LAGGED, FRAME_SNAPSHOT, FRAME_STATE};
use omni_proto::{StateEvent, SystemSnapshot};
use serde_json::Value;

use crate::TuiError;
use crate::state::UiState;

/// WebSocket govdesindeki cerceve adi alani.
const FIELD_KIND: &str = "kind";

/// WebSocket govdesindeki yuk alani.
const FIELD_PAYLOAD: &str = "payload";

/// Kontrol duzleminden inen tek cerceve.
///
/// Buyuk yukler kutulanir: enum'un tum varyantlari en buyugu kadar yer
/// kapladigindan, akis boyunca tasinan cerceve kucuk kalir.
#[derive(Debug, Clone)]
pub enum ControlFrame {
    /// Ilk yukleme goruntusu ([`FRAME_SNAPSHOT`]).
    Snapshot(Box<SystemSnapshot>),
    /// Durum olayi ([`FRAME_STATE`]).
    State(Box<StateEvent>),
    /// Abone geride kaldi ([`FRAME_LAGGED`]); snapshot yeniden cekilmeli.
    Lagged {
        /// Atlanan olay sayisi.
        skipped: u64,
    },
    /// Kontrol duzlemi hata bildirdi ([`FRAME_ERROR`]).
    Error {
        /// Makine tarafinda eslenebilir kisa kod.
        code: String,
        /// Insan okuyacagi ayrinti.
        detail: String,
    },
}

impl ControlFrame {
    /// Cercevenin kanonik adi.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Snapshot(_) => FRAME_SNAPSHOT,
            Self::State(_) => FRAME_STATE,
            Self::Lagged { .. } => FRAME_LAGGED,
            Self::Error { .. } => FRAME_ERROR,
        }
    }

    /// Cerceve adi ve cozulmus yukten cerceve kurar.
    ///
    /// # Errors
    /// Ad taninmiyorsa ya da yuk beklenen sekilde degilse
    /// [`TuiError::BadFrame`] doner.
    pub fn from_parts(kind: &str, payload: Value) -> Result<Self, TuiError> {
        match kind {
            FRAME_SNAPSHOT => serde_json::from_value(payload)
                .map(|snapshot| Self::Snapshot(Box::new(snapshot)))
                .map_err(|err| TuiError::BadFrame(err.to_string())),
            FRAME_STATE => serde_json::from_value(payload)
                .map(|event| Self::State(Box::new(event)))
                .map_err(|err| TuiError::BadFrame(err.to_string())),
            FRAME_LAGGED => Ok(Self::Lagged {
                // Sayac okunamazsa cerceveyi dusurmeyiz: "geride kalindi"
                // bilgisi sayidan daha degerlidir.
                skipped: payload.get("skipped").and_then(Value::as_u64).unwrap_or(0),
            }),
            FRAME_ERROR => Ok(Self::Error {
                code: metin(&payload, "code").unwrap_or_else(|| "unknown".to_string()),
                detail: metin(&payload, "detail").unwrap_or_default(),
            }),
            other => Err(TuiError::BadFrame(format!("bilinmeyen cerceve: {other}"))),
        }
    }

    /// SSE cercevesini cozer: `event:` adi + `data:` govdesi.
    ///
    /// # Errors
    /// Govde gecerli JSON degilse ya da ad taninmiyorsa [`TuiError::BadFrame`]
    /// doner.
    pub fn from_sse(event: &str, data: &str) -> Result<Self, TuiError> {
        let payload: Value =
            serde_json::from_str(data).map_err(|err| TuiError::BadFrame(err.to_string()))?;
        Self::from_parts(event, payload)
    }

    /// WebSocket metin cercevesini cozer: `{ "kind": ..., "payload": ... }`.
    ///
    /// # Errors
    /// Govde gecerli JSON degilse, `kind` alani yoksa ya da ad taninmiyorsa
    /// [`TuiError::BadFrame`] doner.
    pub fn from_ws_text(raw: &str) -> Result<Self, TuiError> {
        let mut govde: Value =
            serde_json::from_str(raw).map_err(|err| TuiError::BadFrame(err.to_string()))?;
        let kind = metin(&govde, FIELD_KIND)
            .ok_or_else(|| TuiError::BadFrame(format!("'{FIELD_KIND}' alani yok")))?;
        let payload = govde
            .get_mut(FIELD_PAYLOAD)
            .map_or(Value::Null, Value::take);
        Self::from_parts(&kind, payload)
    }

    /// Bu cerceve snapshot tazelemesi gerektiriyor mu?
    pub fn needs_resync(&self) -> bool {
        matches!(self, Self::Lagged { .. })
    }
}

/// Bir cercevenin uygulanma sonucu. Surucu buna bakarak snapshot'i yeniler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlOutcome {
    /// Ilk yukleme yerlesti; turetilmis gorunum yeniden kuruldu.
    Loaded,
    /// Olay uygulandi.
    Applied,
    /// Yayin tamponu tasti; `SystemSnapshot` yeniden cekilmeli (Bolum 6.2).
    Lagged {
        /// Atlanan olay sayisi.
        skipped: u64,
    },
    /// Kontrol duzlemi hata bildirdi; akis acik kalir.
    Failed {
        /// Makine tarafinda eslenebilir kisa kod.
        code: String,
        /// Insan okuyacagi ayrinti.
        detail: String,
    },
}

impl ControlOutcome {
    /// Snapshot yeniden cekilmeli mi?
    pub fn needs_resync(&self) -> bool {
        matches!(self, Self::Lagged { .. })
    }

    /// Hata cercevesi mi?
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

impl UiState {
    /// Kontrol duzlemi cercevesini uygular (6.2 okuma sozlesmesi).
    ///
    /// Yayin ucu sira numarasi tasimadigi icin olaylar `apply_event` ile
    /// uygulanir; bosluk bilgisi [`ControlFrame::Lagged`] cercevesinden gelir.
    pub fn apply_control(&mut self, frame: ControlFrame) -> ControlOutcome {
        match frame {
            ControlFrame::Snapshot(snapshot) => {
                self.load(*snapshot);
                ControlOutcome::Loaded
            }
            ControlFrame::State(event) => {
                self.apply_event(*event);
                ControlOutcome::Applied
            }
            ControlFrame::Lagged { skipped } => {
                self.mark_resync();
                tracing::warn!(skipped, "yayin tamponu tasti; snapshot tazelenmeli");
                ControlOutcome::Lagged { skipped }
            }
            ControlFrame::Error { code, detail } => {
                tracing::warn!(%code, %detail, "kontrol duzlemi hata cercevesi gonderdi");
                ControlOutcome::Failed { code, detail }
            }
        }
    }
}

/// JSON nesnesinden metin alan okur.
fn metin(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{AgentState, AgentTier, AgentView, FileTouch};

    fn ajan(id: i64) -> AgentView {
        AgentView {
            id,
            persona: "planner".into(),
            tier: AgentTier::Active,
            task_id: 1,
            parent_id: None,
            state: AgentState::Planning,
            rss_kb: 512,
            tokens_in: 1,
            tokens_out: 1,
            cost: 0.0,
            trust: 1.0,
            depth: 0,
            last_event_seq: 0,
        }
    }

    /// `omni-control` WS ucunun urettigi cerceveyi birebir taklit eder.
    fn ws_frame(kind: &str, payload: Value) -> String {
        serde_json::json!({ "kind": kind, "payload": payload }).to_string()
    }

    #[test]
    fn snapshot_cercevesi_yuklenir() {
        let mut snap = SystemSnapshot::empty(omni_proto::now());
        snap.agents.push(ajan(4));
        let ham = ws_frame(
            FRAME_SNAPSHOT,
            serde_json::to_value(&snap).expect("kodlama"),
        );

        let frame = ControlFrame::from_ws_text(&ham).expect("cerceve");
        assert_eq!(frame.kind(), FRAME_SNAPSHOT);

        let mut state = UiState::new();
        assert_eq!(state.apply_control(frame), ControlOutcome::Loaded);
        assert_eq!(state.agents().len(), 1);
    }

    #[test]
    fn state_cercevesi_olayi_uygular() {
        let olay = StateEvent::AgentUpserted(ajan(9));
        let ham = ws_frame(FRAME_STATE, serde_json::to_value(&olay).expect("kodlama"));

        let mut state = UiState::new();
        assert_eq!(
            state.apply_control(ControlFrame::from_ws_text(&ham).expect("cerceve")),
            ControlOutcome::Applied
        );
        assert!(state.agent(9).is_some());
    }

    #[test]
    fn sse_ve_ws_ayni_sonucu_verir() {
        // K7: iki tasima da ayni cerceveleri tasir.
        let olay = StateEvent::AgentUpserted(ajan(3));
        let govde = serde_json::to_string(&olay).expect("kodlama");

        let sse = ControlFrame::from_sse(FRAME_STATE, &govde).expect("sse");
        let ws = ControlFrame::from_ws_text(&ws_frame(
            FRAME_STATE,
            serde_json::to_value(&olay).expect("kodlama"),
        ))
        .expect("ws");

        let mut a = UiState::new();
        let mut b = UiState::new();
        assert_eq!(a.apply_control(sse), b.apply_control(ws));
        assert_eq!(a.agents().len(), b.agents().len());
    }

    #[test]
    fn lagged_cercevesi_resync_ister() {
        let ham = ws_frame(FRAME_LAGGED, serde_json::json!({ "skipped": 12 }));
        let frame = ControlFrame::from_ws_text(&ham).expect("cerceve");
        assert!(frame.needs_resync());

        let mut state = UiState::new();
        let sonuc = state.apply_control(frame);
        assert_eq!(sonuc, ControlOutcome::Lagged { skipped: 12 });
        assert!(sonuc.needs_resync());
        assert!(state.needs_resync());

        // Snapshot tazelenince bayrak duser.
        state.load(SystemSnapshot::empty(omni_proto::now()));
        assert!(!state.needs_resync());
    }

    #[test]
    fn sayacsiz_lagged_dusurulmez() {
        let frame =
            ControlFrame::from_ws_text(&ws_frame(FRAME_LAGGED, Value::Null)).expect("cerceve");
        assert_eq!(frame.kind(), FRAME_LAGGED);
        let mut state = UiState::new();
        assert_eq!(
            state.apply_control(frame),
            ControlOutcome::Lagged { skipped: 0 }
        );
    }

    #[test]
    fn hata_cercevesi_akisi_kapatmaz() {
        let ham = ws_frame(
            FRAME_ERROR,
            serde_json::json!({ "code": "command_failed", "detail": "cekirdek reddetti" }),
        );
        let mut state = UiState::new();
        let sonuc = state.apply_control(ControlFrame::from_ws_text(&ham).expect("cerceve"));
        assert!(sonuc.is_failure());
        assert!(!sonuc.needs_resync());
        match sonuc {
            ControlOutcome::Failed { code, detail } => {
                assert_eq!(code, "command_failed");
                assert_eq!(detail, "cekirdek reddetti");
            }
            other => panic!("beklenmeyen sonuc: {other:?}"),
        }
    }

    #[test]
    fn bozuk_cerceveler_hata_verir() {
        assert!(matches!(
            ControlFrame::from_ws_text("{ bozuk"),
            Err(TuiError::BadFrame(_))
        ));
        assert!(matches!(
            ControlFrame::from_ws_text(r#"{"payload":{}}"#),
            Err(TuiError::BadFrame(_))
        ));
        assert!(matches!(
            ControlFrame::from_ws_text(&ws_frame("yok_boyle", Value::Null)),
            Err(TuiError::BadFrame(_))
        ));
        assert!(matches!(
            ControlFrame::from_sse(FRAME_SNAPSHOT, "{ bozuk"),
            Err(TuiError::BadFrame(_))
        ));
        // Ad dogru ama yuk sema disinda.
        assert!(matches!(
            ControlFrame::from_sse(FRAME_STATE, r#"{"type":"yok_boyle"}"#),
            Err(TuiError::BadFrame(_))
        ));
    }

    #[test]
    fn diff_grafigi_akistan_beslenir() {
        // 9.3: dokunulan dosyalarin +/- grafigi FileTouched cercevesinden turer.
        let touch = StateEvent::FileTouched(FileTouch {
            id: None,
            agent_id: 2,
            path: "src/lib.rs".into(),
            outside_workspace: false,
            added: 30,
            removed: 4,
            pre_ref: None,
            post_ref: None,
            ts: omni_proto::now(),
        });
        let ham = ws_frame(FRAME_STATE, serde_json::to_value(&touch).expect("kodlama"));

        let mut state = UiState::new();
        let _ = state.apply_control(ControlFrame::from_ws_text(&ham).expect("cerceve"));

        let stat = state.diff(2).expect("diff");
        assert_eq!(stat.added, 30);
        assert_eq!(stat.removed, 4);
        assert_eq!(stat.file_count(), 1);
    }
}
