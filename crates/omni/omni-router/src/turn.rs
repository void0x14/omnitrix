//! Tur dongusu — Faz 1 dikey diliminin kalbi (MASTER-PLAN Bolum 2, B3 boslugu).
//!
//! Vendored `xai_grok_agent::Agent` tipinin `run`/`turn`/`step` metodu YOKTUR;
//! o tip yalnizca "render edilmis sistem promptu + `ToolBridge` + politika"
//! demetidir. Modeli surekli konusturan, tool cagrilarini yurutup sonucu
//! konusmaya geri besleyen dongu BURADADIR.
//!
//! Bir turun anatomisi:
//!
//! 1. Gorev metni konusma kaydina kullanici mesaji olarak girer.
//! 2. Kayit [`SamplerLayer::one_turn`] ile modele gonderilir.
//! 3. Yanitta tool cagrisi varsa: K3 broker'i (9.1) karar verir, izinliyse
//!    `agent.tool_bridge().call(..)` calisir, sonuc konusmaya `ToolResult`
//!    olarak eklenir ve 2'ye donulur.
//! 4. Tool cagrisi yoksa dongu biter.
//! 5. Her adim `omni_storage::events` uzerinden `agent_events` + `messages` +
//!    `tool_calls` satirlarina yazilir.
//!
//! Sonsuz dongu korumasi tur tavanidir ([`TurnLoop::with_max_turns`]). Gercek
//! sonlanma oracle'i (7.5) Faz 4/10 kapsamindadir; Faz 1'de sabit tavan yeter.
//!
//! I6: uretim yolunda panik yok — tool hatalari `Err` olarak yukari degil,
//! MODELE geri gider (ajan dongusunun dogru davranisi budur). Yukari cikan tek
//! hata sinifi modele ulasamamaktir ([`RouterError::Sampling`]).
//!
//! I5: bu dosyada literal model adi/fiyati yoktur. Model kimligi yanittan
//! (`AssistantItem.model_id`) ya da cagirandan gelir.

use std::sync::Arc;

use serde_json::json;
use tracing::{debug, info, warn};

use omni_agent::AgentSession;
use omni_storage::events::{AgentEventRecord, EventWriter, MessageRecord, ToolCallRecord};
use omni_tools::broker::{AuditGate, ToolBroker, ToolCallStatus};
use xai_grok_sampling_types::{ConversationItem, ToolCall, ToolResultItem};

use crate::sampler::SamplerLayer;
use crate::strategies::RouterError;

/// Tur tavaninin varsayilani. Sonlanma oracle'i gelene kadar tek korumadir.
pub const DEFAULT_MAX_TURNS: u32 = 12;

// ---------------------------------------------------------------------------
// Olay-log ucu
// ---------------------------------------------------------------------------

/// Dongunun her adimini `omni-storage`'a dokan uc.
///
/// Dayaniklilik hatasi dongunun akisini KESMEZ: yazim basarisiz olursa kayit
/// duser, sayaci artar ve [`TurnOutcome::dropped_events`] ile disari bildirilir.
/// (Katili kapatma icin `RouterError`'in bir `Storage` varyantina ihtiyaci var;
/// o tip bu dosyanin kapsaminda degil.)
pub struct EventSink {
    writer: Arc<EventWriter>,
    agent_id: i64,
    dropped: u64,
}

impl EventSink {
    /// Verilen yazici uzerine, tek ajan icin uc kurar.
    pub fn new(writer: Arc<EventWriter>, agent_id: i64) -> Self {
        Self {
            writer,
            agent_id,
            dropped: 0,
        }
    }

    /// Kayitlarin sahibi ajan.
    pub fn agent_id(&self) -> i64 {
        self.agent_id
    }

    /// Yazilamadigi icin dusen kayit sayisi.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// `agent_events` satiri.
    pub async fn record_event(&mut self, kind: &str, payload: Option<serde_json::Value>) {
        let record = AgentEventRecord {
            agent_id: self.agent_id,
            kind: kind.to_owned(),
            payload_json: payload.and_then(|v| serde_json::to_string(&v).ok()),
        };
        if let Err(err) = self.writer.record_event(record).await {
            self.drop_record("agent_events", kind, &err.to_string());
        }
    }

    /// `messages` satiri; icerik CAS'a gider.
    pub async fn record_message(
        &mut self,
        role: &str,
        content: &[u8],
        provider_model: Option<String>,
        tokens_in: Option<i64>,
        tokens_out: Option<i64>,
    ) {
        let record = MessageRecord {
            agent_id: self.agent_id,
            role: role.to_owned(),
            provider_model,
            content: content.to_vec(),
            tokens_in,
            tokens_out,
            // Fiyat cevrimi burada YAPILMAZ (I5): tarife bilgisi omni-provider'in.
            cost: None,
        };
        if let Err(err) = self.writer.record_message(record).await {
            self.drop_record("messages", role, &err.to_string());
        }
    }

    /// `tool_calls` satiri; sonuc metni CAS'a gider.
    pub async fn record_tool_call(
        &mut self,
        tool: &str,
        args_json: Option<String>,
        result: Option<Vec<u8>>,
        status: ToolCallStatus,
        capability_ok: bool,
    ) {
        let record = ToolCallRecord {
            agent_id: self.agent_id,
            tool: tool.to_owned(),
            args_json,
            result,
            status: status.as_str().to_owned(),
            capability_ok: Some(capability_ok),
        };
        if let Err(err) = self.writer.record_tool_call(record).await {
            self.drop_record("tool_calls", tool, &err.to_string());
        }
    }

    fn drop_record(&mut self, table: &str, label: &str, err: &str) {
        self.dropped += 1;
        warn!(
            agent_id = self.agent_id,
            table, label, error = err, "olay-log satiri yazilamadi, kayit dustu"
        );
    }
}

impl std::fmt::Debug for EventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventSink")
            .field("agent_id", &self.agent_id)
            .field("dropped", &self.dropped)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Konusma kaydi
// ---------------------------------------------------------------------------

/// Konusma kaydinin tek girdisi.
#[derive(Debug, Clone)]
pub enum TranscriptEntry {
    /// Kullanici (ya da gorev) metni.
    User(String),
    /// Modelin duz metin yaniti.
    Assistant(String),
    /// Modelin istedigi tool cagrilari.
    ToolCalls(Vec<ToolCall>),
    /// Yurutulmus (ya da reddedilmis) bir cagrinin modele donen sonucu.
    ToolResult {
        /// Cagriyi eslestiren kimlik.
        call_id: String,
        /// Cagrilan tool adi.
        tool: String,
        /// Modele giden metin — `ToolRunResult::prompt_text` (kirli, ama
        /// `<system-reminder>` bloklarini tasidigi icin MODELE bu gider).
        content: String,
    },
}

/// Dongunun tuttugu konusma kaydi.
///
/// Iki cikti uretir:
///   * [`Transcript::to_items`] — kanonik `ConversationItem` listesi. Sampler
///     katmani istek duzeyinde bir uc actiginda dogrudan bu kullanilir.
///   * [`Transcript::render`] — duz metin render'i. Bugun kullanilan yol budur,
///     cunku [`SamplerLayer::one_turn`] tek bir `String` prompt alir.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
}

impl Transcript {
    /// Bos kayit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Girdi sayisi.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Kayit bos mu?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Girdilere salt okuma erisim.
    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    /// Kullanici metni ekler.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.entries.push(TranscriptEntry::User(text.into()));
    }

    /// Model metni ekler.
    pub fn push_assistant(&mut self, text: impl Into<String>) {
        self.entries.push(TranscriptEntry::Assistant(text.into()));
    }

    /// Modelin istedigi cagrilari ekler; bos liste yok sayilir.
    pub fn push_tool_calls(&mut self, calls: Vec<ToolCall>) {
        if calls.is_empty() {
            return;
        }
        self.entries.push(TranscriptEntry::ToolCalls(calls));
    }

    /// Bir cagrinin sonucunu ekler.
    pub fn push_tool_result(
        &mut self,
        call_id: impl Into<String>,
        tool: impl Into<String>,
        content: impl Into<String>,
    ) {
        self.entries.push(TranscriptEntry::ToolResult {
            call_id: call_id.into(),
            tool: tool.into(),
            content: content.into(),
        });
    }

    /// Kanonik `ConversationItem` listesi (sistem promptu bastadir).
    pub fn to_items(&self, system_prompt: &str) -> Vec<ConversationItem> {
        let mut items = Vec::with_capacity(self.entries.len() + 1);
        if !system_prompt.trim().is_empty() {
            items.push(ConversationItem::system(system_prompt));
        }
        for entry in &self.entries {
            match entry {
                TranscriptEntry::User(text) => items.push(ConversationItem::user(text.clone())),
                TranscriptEntry::Assistant(text) => {
                    items.push(ConversationItem::assistant(text.clone()));
                }
                TranscriptEntry::ToolCalls(calls) => {
                    items.push(ConversationItem::assistant_tool_calls(calls.clone()));
                }
                TranscriptEntry::ToolResult {
                    call_id, content, ..
                } => {
                    items.push(ConversationItem::ToolResult(ToolResultItem {
                        tool_call_id: call_id.clone(),
                        content: Arc::<str>::from(content.as_str()),
                        images: Vec::new(),
                    }));
                }
            }
        }
        items
    }

    /// Duz metin render'i — `one_turn` bir `String` bekledigi icin gerekli.
    ///
    /// Bicim deterministiktir; ayni kayit her zaman ayni metni uretir.
    pub fn render(&self, system_prompt: &str) -> String {
        let mut out = String::new();
        let trimmed_prompt = system_prompt.trim();
        if !trimmed_prompt.is_empty() {
            out.push_str(trimmed_prompt);
            out.push_str("\n\n");
        }
        for entry in &self.entries {
            match entry {
                TranscriptEntry::User(text) => {
                    out.push_str("[user]\n");
                    out.push_str(text);
                    out.push_str("\n\n");
                }
                TranscriptEntry::Assistant(text) => {
                    out.push_str("[assistant]\n");
                    out.push_str(text);
                    out.push_str("\n\n");
                }
                TranscriptEntry::ToolCalls(calls) => {
                    for call in calls {
                        out.push_str("[assistant tool-call] ");
                        out.push_str(&call.name);
                        out.push(' ');
                        out.push_str(&call.arguments);
                        out.push('\n');
                    }
                    out.push('\n');
                }
                TranscriptEntry::ToolResult { tool, content, .. } => {
                    out.push_str("[tool-result ");
                    out.push_str(tool);
                    out.push_str("]\n");
                    out.push_str(content);
                    out.push_str("\n\n");
                }
            }
        }
        out.push_str("[assistant]\n");
        out
    }
}

// ---------------------------------------------------------------------------
// Dongu ciktisi
// ---------------------------------------------------------------------------

/// Dongunun neden durdugu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStop {
    /// Model tool istemedi; yanit nihai.
    Completed,
    /// Tur tavani doldu — sonsuz dongu korumasi devreye girdi.
    MaxTurns,
    /// Gorev metni bostu; modele hic gidilmedi.
    EmptyTask,
}

impl TurnStop {
    /// Olay-log'a yazilan sabit etiket.
    pub fn as_str(&self) -> &'static str {
        match self {
            TurnStop::Completed => "completed",
            TurnStop::MaxTurns => "max_turns",
            TurnStop::EmptyTask => "empty_task",
        }
    }
}

/// Bir gorevin dongu sonucu.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    /// Modele kac kez gidildi.
    pub turns: u32,
    /// Yurutulen (izinli) tool cagrisi sayisi.
    pub tool_calls: u32,
    /// Broker'in reddettigi cagri sayisi.
    pub denied_tool_calls: u32,
    /// Tool'un hata dondurdugu cagri sayisi.
    pub failed_tool_calls: u32,
    /// Son model metni.
    pub final_text: String,
    /// Durma nedeni.
    pub stop: TurnStop,
    /// Toplam prompt token'i (yanit bildirdigi kadariyla).
    pub tokens_in: u64,
    /// Toplam uretim token'i.
    pub tokens_out: u64,
    /// Yazilamayan olay-log satiri sayisi.
    pub dropped_events: u64,
}

impl TurnOutcome {
    fn empty(stop: TurnStop) -> Self {
        Self {
            turns: 0,
            tool_calls: 0,
            denied_tool_calls: 0,
            failed_tool_calls: 0,
            final_text: String::new(),
            stop,
            tokens_in: 0,
            tokens_out: 0,
            dropped_events: 0,
        }
    }

    /// Dongu tool'a hic dokunmadan bitti mi?
    pub fn touched_tools(&self) -> bool {
        self.tool_calls + self.denied_tool_calls > 0
    }
}

// ---------------------------------------------------------------------------
// Dongu
// ---------------------------------------------------------------------------

/// Tek ajan icin tur dongusu.
///
/// Uc parca: modele giden uc ([`SamplerLayer`]), ajanin kendisi
/// ([`AgentSession`] — sistem promptu + `ToolBridge`) ve olay-log ucu
/// ([`EventSink`]).
pub struct TurnLoop {
    sampler: SamplerLayer,
    session: AgentSession,
    events: EventSink,
    broker: ToolBroker,
    audit: Option<AuditGate>,
    max_turns: u32,
}

impl TurnLoop {
    /// Dongoyu kurar; tur tavani [`DEFAULT_MAX_TURNS`], broker izin verici.
    pub fn new(sampler: SamplerLayer, session: AgentSession, events: EventSink) -> Self {
        Self {
            sampler,
            session,
            events,
            broker: ToolBroker::permissive(),
            audit: None,
            max_turns: DEFAULT_MAX_TURNS,
        }
    }

    /// Sonsuz dongu korumasi: tur tavani. 0 verilirse 1'e yukseltilir.
    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns.max(1);
        self
    }

    /// K3 yetki broker'ini baglar (9.1). Verilmezse tum tool'lar aciktir.
    pub fn with_broker(mut self, broker: ToolBroker) -> Self {
        self.broker = broker;
        self
    }

    /// `capability_audit` satirlarinin gidecegi denetim gecidini baglar.
    pub fn with_audit_gate(mut self, gate: AuditGate) -> Self {
        self.audit = Some(gate);
        self
    }

    /// Yururlukteki tur tavani.
    pub fn max_turns(&self) -> u32 {
        self.max_turns
    }

    /// Sarmalanan oturum.
    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    /// Olay-log ucu.
    pub fn events(&self) -> &EventSink {
        &self.events
    }

    /// Bir gorevi bastan sona kosturur.
    ///
    /// Yukari cikan tek hata modele ulasamamaktir. Tool hatalari ve broker
    /// redleri konusmaya geri beslenir; ajan dongusunun dogru davranisi budur.
    pub async fn run_task(&mut self, task: &str) -> Result<TurnOutcome, RouterError> {
        let task = task.trim();
        if task.is_empty() {
            warn!("bos gorev metni; dongu baslatilmadi");
            self.events.record_event("task_rejected", None).await;
            let mut outcome = TurnOutcome::empty(TurnStop::EmptyTask);
            outcome.dropped_events = self.events.dropped();
            return Ok(outcome);
        }

        let agent_id = self.events.agent_id();
        let system_prompt = self.session.system_prompt().to_owned();
        let max_turns = self.max_turns;

        self.events
            .record_event(
                "task_started",
                Some(json!({ "max_turns": max_turns, "task_len": task.len() })),
            )
            .await;
        self.events
            .record_message("user", task.as_bytes(), None, None, None)
            .await;

        let mut transcript = Transcript::new();
        transcript.push_user(task);

        let mut outcome = TurnOutcome::empty(TurnStop::MaxTurns);

        for turn in 1..=max_turns {
            outcome.turns = turn;
            self.events
                .record_event("turn_started", Some(json!({ "turn": turn })))
                .await;

            // --- 2) modele git -------------------------------------------------
            let prompt = transcript.render(&system_prompt);
            let (response, metrics) = self.sampler.one_turn(prompt).await?;

            let assistant_text = response.assistant_text();
            let calls: Vec<ToolCall> = response
                .assistant()
                .map(|item| item.tool_calls.clone())
                .unwrap_or_default();
            let model_id = response.assistant().and_then(|item| item.model_id.clone());
            let tokens_in = response
                .usage
                .as_ref()
                .map(|usage| i64::from(usage.prompt_tokens));
            let tokens_out = response
                .usage
                .as_ref()
                .map(|usage| i64::from(usage.completion_tokens));
            let stop_reason = response
                .stop_reason
                .as_ref()
                .map(|reason| format!("{reason:?}"));

            outcome.tokens_in += tokens_in.and_then(|v| u64::try_from(v).ok()).unwrap_or(0);
            outcome.tokens_out += tokens_out.and_then(|v| u64::try_from(v).ok()).unwrap_or(0);

            // --- 5) event-log: model mesaji ------------------------------------
            self.events
                .record_message(
                    "assistant",
                    assistant_text.as_bytes(),
                    model_id.clone(),
                    tokens_in,
                    tokens_out,
                )
                .await;
            self.events
                .record_event(
                    "assistant_message",
                    Some(json!({
                        "turn": turn,
                        "tool_calls": calls.len(),
                        "stop_reason": stop_reason,
                        "cost_usd_ticks": response.cost_usd_ticks,
                        "latency": format!("{metrics:?}"),
                    })),
                )
                .await;

            if !assistant_text.is_empty() {
                transcript.push_assistant(assistant_text.clone());
                outcome.final_text = assistant_text;
            }

            // --- 4) tool cagrisi yoksa bitir -----------------------------------
            if calls.is_empty() {
                outcome.stop = TurnStop::Completed;
                debug!(agent_id, turn, "tool cagrisi yok; dongu tamamlandi");
                break;
            }

            // --- 3) tool cagrilari: broker -> ToolBridge -> konusma -------------
            transcript.push_tool_calls(calls.clone());
            for call in &calls {
                let status = self.dispatch_tool(&mut transcript, call).await;
                match status {
                    ToolCallStatus::Ok => outcome.tool_calls += 1,
                    ToolCallStatus::Denied => outcome.denied_tool_calls += 1,
                    ToolCallStatus::Failed | ToolCallStatus::Panicked => {
                        outcome.tool_calls += 1;
                        outcome.failed_tool_calls += 1;
                    }
                }
            }
        }

        if outcome.stop == TurnStop::MaxTurns {
            warn!(
                agent_id,
                max_turns, "tur tavani doldu; dongu zorla sonlandirildi"
            );
        }

        self.events
            .record_event(
                "task_finished",
                Some(json!({
                    "stop": outcome.stop.as_str(),
                    "turns": outcome.turns,
                    "tool_calls": outcome.tool_calls,
                    "denied_tool_calls": outcome.denied_tool_calls,
                    "failed_tool_calls": outcome.failed_tool_calls,
                    "tokens_in": outcome.tokens_in,
                    "tokens_out": outcome.tokens_out,
                })),
            )
            .await;

        outcome.dropped_events = self.events.dropped();
        info!(
            agent_id,
            turns = outcome.turns,
            stop = outcome.stop.as_str(),
            tool_calls = outcome.tool_calls,
            "gorev dongusu bitti"
        );
        Ok(outcome)
    }

    /// Tek bir tool cagrisini yurutur ve sonucu konusmaya ekler.
    ///
    /// Bu fonksiyon HICBIR kosulda `Err` dondurmez: her akibet hem `tool_calls`
    /// satirina hem de modele donen `ToolResult` metnine cevrilir.
    async fn dispatch_tool(
        &mut self,
        transcript: &mut Transcript,
        call: &ToolCall,
    ) -> ToolCallStatus {
        let agent_id = self.events.agent_id();
        let tool = call.name.trim().to_owned();
        let call_id = call.id.to_string();
        let args_text = call.arguments.trim().to_owned();

        // --- 9.1: TEK zorlama noktasi -----------------------------------------
        let decision = self.broker.decide(&tool);
        if let Some(gate) = &self.audit {
            gate.emit_capability(self.broker.capability_decision(agent_id, &tool));
        }

        if let Some(reason) = decision.reason() {
            let message = format!("tool cagrisi reddedildi: {reason}");
            warn!(agent_id, tool, reason, "broker cagriyi reddetti");
            self.events
                .record_tool_call(
                    &tool,
                    non_empty(&args_text),
                    Some(message.as_bytes().to_vec()),
                    ToolCallStatus::Denied,
                    false,
                )
                .await;
            self.events
                .record_event(
                    "tool_denied",
                    Some(json!({ "tool": tool, "call_id": call_id, "reason": reason })),
                )
                .await;
            transcript.push_tool_result(call_id, tool, message);
            return ToolCallStatus::Denied;
        }

        // --- argumanlari coz ---------------------------------------------------
        let params = if args_text.is_empty() {
            json!({})
        } else {
            match serde_json::from_str::<serde_json::Value>(&args_text) {
                Ok(value) => value,
                Err(err) => {
                    let message = format!("tool argumanlari cozulemedi: {err}");
                    warn!(agent_id, tool, error = %err, "tool argumani gecersiz JSON");
                    self.events
                        .record_tool_call(
                            &tool,
                            non_empty(&args_text),
                            Some(message.as_bytes().to_vec()),
                            ToolCallStatus::Failed,
                            true,
                        )
                        .await;
                    transcript.push_tool_result(call_id, tool, message);
                    return ToolCallStatus::Failed;
                }
            }
        };

        // ToolBridge'i Arc olarak kopyalayip oturum oduncunu birakiyoruz;
        // asagida `self.events` degistirilebilir olmali.
        let bridge = Arc::clone(self.session.agent().tool_bridge());

        self.events
            .record_event(
                "tool_started",
                Some(json!({ "tool": tool, "call_id": call_id })),
            )
            .await;

        // --- yurutme -----------------------------------------------------------
        match bridge.call(&tool, params, call_id.as_str()).await {
            Ok(run) => {
                // Mantiksal basarisizlik `Err` DEGIL, `Ok(ToolOutput::…)` icinde
                // gelir — daima `is_error()` bakilir.
                let is_error = run.output.is_error();
                let effective = run.effective_tool_name.clone();
                let prompt_text = run.prompt_text;
                let status = if is_error {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Ok
                };

                self.events
                    .record_tool_call(
                        &tool,
                        non_empty(&args_text),
                        Some(prompt_text.as_bytes().to_vec()),
                        status,
                        true,
                    )
                    .await;
                self.events
                    .record_event(
                        "tool_finished",
                        Some(json!({
                            "tool": tool,
                            "call_id": call_id,
                            "status": status.as_str(),
                            "effective_tool": effective,
                            "result_len": prompt_text.len(),
                        })),
                    )
                    .await;

                debug!(agent_id, tool, status = status.as_str(), "tool cagrisi bitti");
                // MODELE `prompt_text` gider (system-reminder bloklarini tasir).
                transcript.push_tool_result(call_id, tool, prompt_text);
                status
            }
            Err(err) => {
                let message = format!("tool calisma zamani hatasi: {err}");
                warn!(agent_id, tool, error = %err, "tool cagrisi basarisiz");
                self.events
                    .record_tool_call(
                        &tool,
                        non_empty(&args_text),
                        Some(message.as_bytes().to_vec()),
                        ToolCallStatus::Failed,
                        true,
                    )
                    .await;
                self.events
                    .record_event(
                        "tool_failed",
                        Some(json!({ "tool": tool, "call_id": call_id })),
                    )
                    .await;
                transcript.push_tool_result(call_id, tool, message);
                ToolCallStatus::Failed
            }
        }
    }
}

impl std::fmt::Debug for TurnLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnLoop")
            .field("max_turns", &self.max_turns)
            .field("agent", &self.session.name())
            .field("events", &self.events)
            .finish_non_exhaustive()
    }
}

/// Bos metni `None`'a cevirir — `args_json` sutununda bos dize istemiyoruz.
fn non_empty(text: &str) -> Option<String> {
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: Arc::<str>::from(id),
            name: name.to_owned(),
            arguments: Arc::<str>::from(arguments),
        }
    }

    #[test]
    fn bos_kayit_yalniz_sistem_promptunu_tasir() {
        let transcript = Transcript::new();
        assert!(transcript.is_empty());
        assert_eq!(transcript.len(), 0);
        let items = transcript.to_items("sistem");
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn bos_sistem_promptu_item_uretmez() {
        let transcript = Transcript::new();
        assert!(transcript.to_items("   ").is_empty());
    }

    #[test]
    fn kayit_kanonik_item_listesine_cevrilir() {
        let mut transcript = Transcript::new();
        transcript.push_user("gorev");
        transcript.push_tool_calls(vec![tool_call("c1", "read_file", "{}")]);
        transcript.push_tool_result("c1", "read_file", "icerik");
        transcript.push_assistant("bitti");

        let items = transcript.to_items("sistem");
        // system + user + assistant_tool_calls + tool_result + assistant
        assert_eq!(items.len(), 5);
        assert!(matches!(items[0], ConversationItem::System(_)));
        assert!(matches!(items[1], ConversationItem::User(_)));
        assert!(matches!(items[2], ConversationItem::Assistant(_)));
        match &items[3] {
            ConversationItem::ToolResult(result) => {
                assert_eq!(result.tool_call_id, "c1");
                assert_eq!(result.content.as_ref(), "icerik");
                assert!(result.images.is_empty());
            }
            other => panic!("tool sonucu bekleniyordu: {other:?}"),
        }
        assert!(matches!(items[4], ConversationItem::Assistant(_)));
    }

    #[test]
    fn bos_cagri_listesi_kayda_girmez() {
        let mut transcript = Transcript::new();
        transcript.push_tool_calls(Vec::new());
        assert!(transcript.is_empty());
    }

    #[test]
    fn render_deterministiktir_ve_her_parcayi_tasir() {
        let mut transcript = Transcript::new();
        transcript.push_user("gorev metni");
        transcript.push_tool_calls(vec![tool_call("c1", "read_file", "{\"p\":1}")]);
        transcript.push_tool_result("c1", "read_file", "dosya icerigi");

        let first = transcript.render("sistem promptu");
        let second = transcript.render("sistem promptu");
        assert_eq!(first, second);

        assert!(first.starts_with("sistem promptu"));
        assert!(first.contains("[user]\ngorev metni"));
        assert!(first.contains("[assistant tool-call] read_file {\"p\":1}"));
        assert!(first.contains("[tool-result read_file]\ndosya icerigi"));
        assert!(first.ends_with("[assistant]\n"));
    }

    #[test]
    fn render_bos_sistem_promptunu_atlar() {
        let mut transcript = Transcript::new();
        transcript.push_user("x");
        let rendered = transcript.render("  ");
        assert!(rendered.starts_with("[user]\nx"));
    }

    #[test]
    fn girdiler_okunabilir() {
        let mut transcript = Transcript::new();
        transcript.push_user("a");
        transcript.push_assistant("b");
        assert_eq!(transcript.entries().len(), 2);
        assert!(matches!(
            transcript.entries()[0],
            TranscriptEntry::User(ref t) if t == "a"
        ));
    }

    #[test]
    fn durma_etiketleri_sabittir() {
        assert_eq!(TurnStop::Completed.as_str(), "completed");
        assert_eq!(TurnStop::MaxTurns.as_str(), "max_turns");
        assert_eq!(TurnStop::EmptyTask.as_str(), "empty_task");
    }

    #[test]
    fn bos_cikti_sifirlarla_baslar() {
        let outcome = TurnOutcome::empty(TurnStop::EmptyTask);
        assert_eq!(outcome.turns, 0);
        assert_eq!(outcome.tokens_in, 0);
        assert!(!outcome.touched_tools());
        assert!(outcome.final_text.is_empty());
    }

    #[test]
    fn tool_dokunusu_sayilir() {
        let mut outcome = TurnOutcome::empty(TurnStop::Completed);
        outcome.denied_tool_calls = 1;
        assert!(outcome.touched_tools());
    }

    #[test]
    fn bos_arguman_metni_none_olur() {
        assert!(non_empty("").is_none());
        assert_eq!(non_empty("{}").as_deref(), Some("{}"));
    }

    #[test]
    fn broker_karari_red_gerekcesi_tasir() {
        // Dongunun 9.1 dali bu karara bakar; broker'in kendisi omni-tools'ta
        // test edilir, burada yalniz dikis dogrulanir.
        let broker = ToolBroker::new(vec!["read_file".to_owned()], Vec::new());
        assert!(broker.decide("read_file").reason().is_none());
        assert!(broker.decide("bash").reason().is_some());
    }
}

// ---------------------------------------------------------------------------
// Faz 1 kapisi — uctan uca dongu testleri
// ---------------------------------------------------------------------------
//
// Bu modul MASTER-PLAN Faz 1 kapisinin dort maddesini TEK bir kosuda dogrular:
//
//   1. Tek ajan gercek bir LLM ucuna baglanir — sahte sampler DEGIL, gercek
//      `SamplerLayer` + gercek HTTP + gercek SSE cozumleme. Sunucu yereldir
//      (`mockito`) ama tel uzerindeki protokol gercektir.
//   2. Gercek tool calisir — vendored `ToolBridge` uzerinden `read_file`
//      cagrilir ve diskteki gercek dosyayi okur.
//   3. Tool sonucu konusmaya geri beslenir — ikinci istegin GOVDESI icinde
//      dosya iceriginin gecmesi bunun kaniti.
//   4. Her adim olay-log'a yazilir — SQLite'taki `agent_events` / `messages` /
//      `tool_calls` satirlari sayilarak dogrulanir.
//
// Ayrica sonsuz dongu korumasi ayri bir testle kanitlanir: model her turda
// tool istemeye devam ederse dongu tur tavaninda KESILIR.
//
// I5: burada da literal saglayici model adi yoktur; tel uzerinde donen kimlik
// notr bir test etiketidir.
#[cfg(test)]
mod faz1_gate_tests {
    use super::*;

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use omni_agent::{AgentSpec, PersonaSpec};
    use omni_storage::events::EventWriter;
    use omni_storage::sqlite_schema::SchemaManager;
    use omni_tools::ToolNotificationHandle;
    use tempfile::TempDir;
    use xai_grok_sampler::{RetryPolicy, SamplerConfig};

    /// Tel uzerinde donen notr model etiketi (I5: saglayici adi degil).
    const WIRE_MODEL: &str = "omni-test-wire-model";
    /// Okunan dosyanin icinde arayacagimiz iz.
    const FILE_MARKER: &str = "OMNI-FAZ1-DOSYA-IZI";
    /// Modelin son turda dondurdugu iz.
    const FINAL_MARKER: &str = "OMNI-FAZ1-BITTI";
    /// Faz 1 tool setinin okuma ucu; vendored ad budur.
    const READ_TOOL: &str = "read_file";

    /// Tek bir SSE `data:` olayi uretir.
    ///
    /// Alanlarin tamami ACIKCA yazilir: `ChatChunkDelta::reasoning_content` ve
    /// `ChatCompletionChunk::usage` gibi alanlarda `#[serde(default)]` YOKTUR,
    /// eksik birakilirsa cozumleme hata verir.
    fn sse_event(delta: serde_json::Value, finish: Option<&str>, usage: bool) -> String {
        let chunk = json!({
            "id": "omni-test-chunk",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": WIRE_MODEL,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish,
            }],
            "usage": if usage {
                json!({ "prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18 })
            } else {
                serde_json::Value::Null
            },
            "system_fingerprint": null,
        });
        format!("data: {chunk}\n\n")
    }

    /// Modelin bir tool cagirdigi akis.
    fn sse_tool_call(call_id: &str, tool: &str, arguments: &str) -> String {
        let delta = json!({
            "role": "assistant",
            "content": null,
            "reasoning_content": null,
            "tool_calls": [{
                "index": 0,
                "id": call_id,
                "type": "function",
                "function": { "name": tool, "arguments": arguments },
            }],
            "tool_call_id": null,
        });
        format!(
            "{}data: [DONE]\n\n",
            sse_event(delta, Some("tool_calls"), true)
        )
    }

    /// Modelin duz metinle bitirdigi akis.
    fn sse_text(text: &str) -> String {
        let delta = json!({
            "role": "assistant",
            "content": text,
            "reasoning_content": null,
            "tool_calls": [],
            "tool_call_id": null,
        });
        format!("{}data: [DONE]\n\n", sse_event(delta, Some("stop"), true))
    }

    /// Testin butun parcalarini bir arada tutar; `TempDir` erken dusmemeli.
    struct Harness {
        _workspace: TempDir,
        loop_: TurnLoop,
        writer: Arc<EventWriter>,
        agent_id: i64,
        target_file: PathBuf,
    }

    /// Olay-log'daki satirlari sayar. Yazimlar `WriterActor` uzerinden toplu
    /// gittigi icin cagrilmadan once kisa bir bekleme gerekir.
    async fn count_rows(writer: &EventWriter, table: &str, agent_id: i64) -> i64 {
        // Toplu yazicinin kuyrugu bosalsin.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let conn = rusqlite_open(writer.db_path());
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE agent_id = ?1");
        conn.query_row(&sql, [agent_id], |row| row.get::<_, i64>(0))
            .unwrap_or(-1)
    }

    /// Testte okuma icin ayri bir baglanti; `EventWriter` kendi baglantisini
    /// disari vermez.
    fn rusqlite_open(path: &Path) -> rusqlite::Connection {
        rusqlite::Connection::open(path).expect("test veritabani acilmali")
    }

    /// `agent_events` / `messages` / `tool_calls` satirlarinin HEPSI
    /// `agents(id)`'ye YABANCI ANAHTAR ile baglidir (goc 0003). Ajan satiri
    /// yoksa yazicinin her yazimi "FOREIGN KEY constraint failed" ile duser ve
    /// dongu bunu sessizce `dropped_events` sayacina yazar. Kapi testinin
    /// olay-log adimini gercekten olcebilmesi icin once gorev + ajan satirlari
    /// acilir.
    fn seed_agent_row(db_path: &Path, agent_id: i64) {
        let conn = rusqlite_open(db_path);
        conn.execute_batch(&format!(
            "INSERT INTO tasks (id, parent_id, root_id, title, mode, status, depth)
                 VALUES (1, NULL, 1, 'faz1 kapi testi', 'single', 'running', 0);
             INSERT INTO agents (id, task_id, persona, parent_agent_id, state)
                 VALUES ({agent_id}, 1, 'omni-faz1', NULL, 'running');"
        ))
        .expect("gorev + ajan satirlari acilmali");
    }

    /// Ajan + sampler + olay-log ucunu kurar.
    ///
    /// `base_url` mockito sunucusunun adresidir; `SamplerConfig` disaridan
    /// geldigi icin dongu gercek HTTP konusur.
    async fn harness(base_url: String, max_turns: u32) -> Harness {
        let workspace = TempDir::new().expect("gecici calisma dizini");
        let root = workspace.path().to_path_buf();

        // Ajanin okuyacagi GERCEK dosya.
        let target_file = root.join("hedef.txt");
        std::fs::write(&target_file, format!("birinci satir\n{FILE_MARKER}\n"))
            .expect("hedef dosya yazilmali");

        // --- olay-log: gercek SQLite + gercek CAS -----------------------------
        let db_path = root.join("omni.db");
        SchemaManager::new(&db_path)
            .expect("sema yoneticisi")
            .run_migrations()
            .expect("gocler uygulanmali");
        let agent_id = 4242_i64;
        seed_agent_row(&db_path, agent_id);
        let writer = Arc::new(
            EventWriter::open(&db_path, &root.join("cas")).expect("olay yazicisi acilmali"),
        );

        // --- ajan: gercek ToolBridge -----------------------------------------
        let definition = PersonaSpec::new("omni-faz1", "Faz 1 dikey dilim ajani").to_definition();
        let session = AgentSpec::new(definition, root.clone(), root.join("resources_state.json"))
            .build(omni_tools::local_terminal(), ToolNotificationHandle::noop())
            .await
            .expect("ajan kurulmali");

        // --- sampler: gercek aktor, gercek HTTP -------------------------------
        let config = SamplerConfig {
            api_key: Some("test-anahtari".to_owned()),
            base_url,
            model: WIRE_MODEL.to_owned(),
            max_completion_tokens: Some(256),
            idle_timeout_secs: Some(20),
            ..Default::default()
        };
        let retry = RetryPolicy {
            // Testte yeniden deneme istemiyoruz: istek sayaci anlamli kalsin.
            max_retries: 0,
            ..Default::default()
        };
        let sampler = SamplerLayer::spawn(config, retry);

        let events = EventSink::new(Arc::clone(&writer), agent_id);
        let loop_ = TurnLoop::new(sampler, session, events)
            .with_max_turns(max_turns)
            .with_broker(ToolBroker::new(vec![READ_TOOL.to_owned()], Vec::new()));

        Harness {
            _workspace: workspace,
            loop_,
            writer,
            agent_id,
            target_file,
        }
    }

    /// KAPI 1+2+3+4: gercek uc, gercek tool, geri besleme, olay-log.
    #[tokio::test(flavor = "multi_thread")]
    async fn faz1_kapisi_uctan_uca_karsilanir() {
        let mut server = mockito::Server::new_async().await;

        // Hedef dosyanin yolu ajan kurulmadan once bilinemez; bu yuzden once
        // gecici dizini kurup sonra mock'u yaziyoruz. Harness'i iki adima
        // bolmemek icin dosya yolunu mock govdesine calistirma aninda,
        // istek govdesine bakarak seciyoruz.
        let hits = Arc::new(AtomicUsize::new(0));
        let bodies: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        // Yol, harness kurulduktan sonra dolduruluyor; kapanis onu okuyor.
        let path_slot: Arc<std::sync::Mutex<String>> =
            Arc::new(std::sync::Mutex::new(String::new()));

        let hits_cl = Arc::clone(&hits);
        let bodies_cl = Arc::clone(&bodies);
        let path_cl = Arc::clone(&path_slot);

        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body_from_request(move |request| {
                let body = request
                    .body()
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .unwrap_or_default();
                let nth = hits_cl.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut sink) = bodies_cl.lock() {
                    sink.push(body);
                }
                let target = path_cl
                    .lock()
                    .map(|guard| guard.clone())
                    .unwrap_or_default();
                if nth == 0 {
                    // Ilk tur: gercek dosyayi okumasi icin tool cagrisi.
                    let args = json!({ "target_file": target }).to_string();
                    sse_tool_call("cagri-1", READ_TOOL, &args).into_bytes()
                } else {
                    // Ikinci tur: tool sonucunu gordu, duz metinle bitiriyor.
                    sse_text(&format!("{FINAL_MARKER} tamam")).into_bytes()
                }
            })
            .expect_at_least(2)
            .create_async()
            .await;

        let mut h = harness(server.url(), 6).await;
        if let Ok(mut slot) = path_slot.lock() {
            *slot = h.target_file.to_string_lossy().into_owned();
        }

        let outcome = h
            .loop_
            .run_task("hedef.txt dosyasini oku ve iceriginde ne yaziyorsa bildir")
            .await
            .expect("dongu modele ulasabilmeli");

        // --- KAPI 1: gercek uce iki kez gidildi -------------------------------
        assert_eq!(hits.load(Ordering::SeqCst), 2, "tam iki tur beklenir");
        assert_eq!(outcome.turns, 2);
        assert_eq!(outcome.stop, TurnStop::Completed);
        assert!(
            outcome.final_text.contains(FINAL_MARKER),
            "son metin modelden gelmeli: {}",
            outcome.final_text
        );
        // Kullanim bilgisi tel uzerinden geldi.
        assert!(outcome.tokens_in > 0 && outcome.tokens_out > 0);

        // --- KAPI 2: gercek tool calisti -------------------------------------
        assert_eq!(outcome.tool_calls, 1, "bir tool cagrisi yurutulmeli");
        assert_eq!(outcome.denied_tool_calls, 0, "broker izin vermeliydi");
        assert_eq!(
            outcome.failed_tool_calls, 0,
            "gercek dosya okumasi basarili olmali"
        );

        // --- KAPI 3: sonuc konusmaya geri beslendi ----------------------------
        let captured = bodies.lock().map(|b| b.clone()).unwrap_or_default();
        assert_eq!(captured.len(), 2);
        assert!(
            !captured[0].contains(FILE_MARKER),
            "ilk istekte dosya icerigi henuz olmamali"
        );
        assert!(
            captured[1].contains(FILE_MARKER),
            "ikinci istek tool sonucunu tasimali; govde: {}",
            captured[1]
        );
        assert!(
            captured[1].contains(READ_TOOL),
            "ikinci istek tool cagrisini da tasimali"
        );

        // --- KAPI 4: her adim olay-log'a yazildi ------------------------------
        assert_eq!(outcome.dropped_events, 0, "hicbir kayit dusmemeli");
        let events = count_rows(&h.writer, "agent_events", h.agent_id).await;
        let messages = count_rows(&h.writer, "messages", h.agent_id).await;
        let tool_calls = count_rows(&h.writer, "tool_calls", h.agent_id).await;

        // task_started + 2x turn_started + 2x assistant_message + tool_started
        // + tool_finished + task_finished = 8
        assert_eq!(events, 8, "agent_events satir sayisi");
        // 1 kullanici + 2 model mesaji
        assert_eq!(messages, 3, "messages satir sayisi");
        assert_eq!(tool_calls, 1, "tool_calls satir sayisi");
    }

    /// KAPI 5: sonsuz dongu korumasi — model hep tool isterse tavan keser.
    #[tokio::test(flavor = "multi_thread")]
    async fn tur_tavani_sonsuz_donguyu_keser() {
        let mut server = mockito::Server::new_async().await;
        let hits = Arc::new(AtomicUsize::new(0));
        let path_slot: Arc<std::sync::Mutex<String>> =
            Arc::new(std::sync::Mutex::new(String::new()));

        let hits_cl = Arc::clone(&hits);
        let path_cl = Arc::clone(&path_slot);

        // Model HER turda ayni tool'u istiyor; tek durduran sey tavandir.
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body_from_request(move |_request| {
                let nth = hits_cl.fetch_add(1, Ordering::SeqCst);
                let target = path_cl
                    .lock()
                    .map(|guard| guard.clone())
                    .unwrap_or_default();
                let args = json!({ "target_file": target }).to_string();
                sse_tool_call(&format!("cagri-{nth}"), READ_TOOL, &args).into_bytes()
            })
            .expect_at_least(3)
            .create_async()
            .await;

        let mut h = harness(server.url(), 3).await;
        if let Ok(mut slot) = path_slot.lock() {
            *slot = h.target_file.to_string_lossy().into_owned();
        }

        let outcome = h
            .loop_
            .run_task("dosyayi surekli oku")
            .await
            .expect("dongu modele ulasabilmeli");

        assert_eq!(outcome.stop, TurnStop::MaxTurns, "tavan devreye girmeli");
        assert_eq!(outcome.turns, 3, "tavan kadar tur atilmali");
        assert_eq!(hits.load(Ordering::SeqCst), 3, "tavandan fazla istek yok");
        assert_eq!(outcome.tool_calls, 3);
    }

    /// Broker reddi dongoyu KESMEZ; red modele metin olarak geri doner.
    #[tokio::test(flavor = "multi_thread")]
    async fn broker_reddi_modele_geri_beslenir() {
        let mut server = mockito::Server::new_async().await;
        let hits = Arc::new(AtomicUsize::new(0));
        let bodies: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let hits_cl = Arc::clone(&hits);
        let bodies_cl = Arc::clone(&bodies);

        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body_from_request(move |request| {
                let body = request
                    .body()
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .unwrap_or_default();
                let nth = hits_cl.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut sink) = bodies_cl.lock() {
                    sink.push(body);
                }
                if nth == 0 {
                    // Izin listesinde OLMAYAN bir tool.
                    sse_tool_call("cagri-red", "bash", "{\"command\":\"ls\"}").into_bytes()
                } else {
                    sse_text(&format!("{FINAL_MARKER} anlasildi")).into_bytes()
                }
            })
            .expect_at_least(2)
            .create_async()
            .await;

        let mut h = harness(server.url(), 5).await;
        let outcome = h
            .loop_
            .run_task("kabuk komutu calistir")
            .await
            .expect("dongu modele ulasabilmeli");

        assert_eq!(outcome.stop, TurnStop::Completed);
        assert_eq!(outcome.denied_tool_calls, 1, "broker reddetmeliydi");
        assert_eq!(outcome.tool_calls, 0, "reddedilen cagri yurutulmemeli");

        let captured = bodies.lock().map(|b| b.clone()).unwrap_or_default();
        assert_eq!(captured.len(), 2);
        assert!(
            captured[1].contains("reddedildi"),
            "red gerekcesi modele donmeli; govde: {}",
            captured[1]
        );
    }

    /// Bos gorev metni modele HIC gitmez.
    #[tokio::test(flavor = "multi_thread")]
    async fn bos_gorev_modele_gitmez() {
        let mut server = mockito::Server::new_async().await;
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_cl = Arc::clone(&hits);

        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_body_from_request(move |_request| {
                hits_cl.fetch_add(1, Ordering::SeqCst);
                sse_text("olmamaliydi").into_bytes()
            })
            .create_async()
            .await;

        let mut h = harness(server.url(), 4).await;
        let outcome = h.loop_.run_task("   \n\t ").await.expect("hata olmamali");

        assert_eq!(outcome.stop, TurnStop::EmptyTask);
        assert_eq!(outcome.turns, 0);
        assert_eq!(hits.load(Ordering::SeqCst), 0, "modele istek gitmemeli");
    }
}

