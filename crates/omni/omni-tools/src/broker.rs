//! K3 yetki broker'i — tool cagrilarinin TEK zorlama noktasi (MASTER-PLAN 9.1).
//!
//! Model katmaninda token engelleme mumkun degildir; zorlama tool katmanindadir.
//! Personanin tasidigi izin/red listesi burada `AgentBuilder::with_tools` ve
//! `AgentBuilder::with_disallowed_tools` cagrilarina donusur. Expose edilmeyen
//! tool = model ne uretirse uretsin calismaz.
//!
//! Karar burada verilir, baska hicbir yerde tekrarlanmaz. Her karar
//! `capability_audit` satirina, her cagri `tool_calls` satirina (migrations/0004)
//! donusecek sekilde gerekcelendirilir. DB yazimi omni-storage'in isidir; bu
//! modul kayitlari uretip mpsc ile yayinlar.

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::error::ToolsError;

/// `capability_audit.capability` icin tool acilim yetkisinin adi.
pub const CAPABILITY_TOOL: &str = "tool";

/// Broker karari.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Cagri izin listesine uyuyor.
    Allow,
    /// Cagri reddedildi; metin denetim kaydina gider.
    Deny(String),
}

impl Decision {
    /// Kararin izin verip vermedigi.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Decision::Allow)
    }

    /// `capability_audit.decision` sutununa yazilan sabit etiket.
    pub fn label(&self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Deny(_) => "deny",
        }
    }

    /// Red gerekcesi; izinli kararda `None`.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Decision::Allow => None,
            Decision::Deny(reason) => Some(reason.as_str()),
        }
    }
}

/// `tool_calls.status` sutununun kapali deger kumesi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    /// Cagri calisti ve sonuc dondu.
    Ok,
    /// Broker reddetti; cagri hic calismadi.
    Denied,
    /// Cagri calisti, tool hata dondurdu.
    Failed,
    /// Tool panikledi; supervisor yakaladi, surec ayakta (MASTER-PLAN 8.x).
    Panicked,
}

impl ToolCallStatus {
    /// DB'ye yazilan sabit metin.
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolCallStatus::Ok => "ok",
            ToolCallStatus::Denied => "denied",
            ToolCallStatus::Failed => "failed",
            ToolCallStatus::Panicked => "panicked",
        }
    }
}

/// Persona basina kurulan izin/red listesi.
///
/// Listeler kurulusta normalize edilir: bosluklar kirpilir, bos adlar ve
/// tekrarlar atilir, sira korunur. Red listesindeki bir ad izin listesinden de
/// silinir — boylece `allowed_tools()` ciktisi ajana ASLA yasakli bir tool
/// acmaz (9.1 tek zorlama noktasi).
#[derive(Debug, Clone, Default)]
pub struct ToolBroker {
    allowed: Vec<String>,
    disallowed: Vec<String>,
}

impl ToolBroker {
    /// Izin ve red listelerinden broker kurar.
    ///
    /// Bos izin listesi "persona acik bir allowlist tasimiyor" demektir; bu
    /// durumda yalnizca red listesi zorlanir.
    pub fn new(allowed: Vec<String>, disallowed: Vec<String>) -> Self {
        let disallowed = normalize(disallowed);
        let allowed = normalize(allowed)
            .into_iter()
            .filter(|tool| !disallowed.iter().any(|denied| denied == tool))
            .collect();
        Self {
            allowed,
            disallowed,
        }
    }

    /// Hicbir kisitlama tasimayan broker.
    pub fn permissive() -> Self {
        Self::default()
    }

    /// `AgentBuilder::with_tools` icin acik tool adlari.
    pub fn allowed_tools(&self) -> &[String] {
        &self.allowed
    }

    /// `AgentBuilder::with_disallowed_tools` icin kapali tool adlari.
    pub fn disallowed_tools(&self) -> &[String] {
        &self.disallowed
    }

    /// Persona acik bir allowlist tasiyor mu?
    pub fn has_allowlist(&self) -> bool {
        !self.allowed.is_empty()
    }

    /// Tek karar noktasi: tool cagrilabilir mi?
    pub fn is_permitted(&self, tool: &str) -> bool {
        self.decide(tool).is_allowed()
    }

    /// Gerekceli karar; denetim kaydi bu ciktidan uretilir.
    pub fn decide(&self, tool: &str) -> Decision {
        let tool = tool.trim();
        if tool.is_empty() {
            return Decision::Deny("tool adi bos".to_string());
        }
        if self.disallowed.iter().any(|denied| denied == tool) {
            return Decision::Deny(format!("'{tool}' red listesinde"));
        }
        if self.allowed.is_empty() || self.allowed.iter().any(|allowed| allowed == tool) {
            Decision::Allow
        } else {
            Decision::Deny(format!("'{tool}' izin listesinde degil"))
        }
    }

    /// Verilen tool setini izin/red listesine gore suzer — kayit katalogundan
    /// gelen id'leri personaya daraltmak icin.
    pub fn filter<'a, I>(&self, tools: I) -> Vec<String>
    where
        I: IntoIterator<Item = &'a str>,
    {
        tools
            .into_iter()
            .filter(|tool| self.is_permitted(tool))
            .map(str::to_string)
            .collect()
    }

    /// Bir tool cagrisi icin yetki denetim satiri uretir.
    pub fn capability_decision(&self, agent_id: i64, tool: &str) -> CapabilityDecision {
        CapabilityDecision::new(agent_id, CAPABILITY_TOOL, tool, self.decide(tool))
    }
}

/// Bosluklari kirpar, bos adlari ve tekrarlari atar, sirayi korur.
fn normalize(tools: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tools.len());
    for tool in tools {
        let trimmed = tool.trim();
        if trimmed.is_empty() || out.iter().any(|existing| existing == trimmed) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

/// `tool_calls` satirinin bellek karsiligi (migrations/0004).
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    /// Cagriyi yapan ajan.
    pub agent_id: i64,
    /// Cagrilan tool adi.
    pub tool: String,
    /// Cagri argumanlari (JSON metni); yoksa `None`.
    pub args_json: Option<String>,
    /// Sonuc icerigi CAS atfi; yoksa `None`.
    pub result_ref: Option<String>,
    /// Cagrinin akibeti.
    pub status: ToolCallStatus,
    /// Broker cagriya izin verdi mi?
    pub capability_ok: bool,
    /// Kaydin zaman damgasi (RFC 3339).
    pub ts: String,
}

impl ToolCallRecord {
    /// Yeni bir cagri kaydi; zaman damgasi uretim aninda konur.
    pub fn new(
        agent_id: i64,
        tool: impl Into<String>,
        status: ToolCallStatus,
        capability_ok: bool,
    ) -> Self {
        Self {
            agent_id,
            tool: tool.into(),
            args_json: None,
            result_ref: None,
            status,
            capability_ok,
            ts: now_rfc3339(),
        }
    }

    /// Broker'in reddettigi cagri: hic calismadi, `capability_ok = false`.
    pub fn denied(agent_id: i64, tool: impl Into<String>) -> Self {
        Self::new(agent_id, tool, ToolCallStatus::Denied, false)
    }

    /// Arguman metnini baglar.
    pub fn with_args(mut self, args_json: Option<String>) -> Self {
        self.args_json = args_json;
        self
    }

    /// Sonuc atfini baglar (omni-storage CAS yazimindan sonra).
    pub fn with_result_ref(mut self, result_ref: Option<String>) -> Self {
        self.result_ref = result_ref;
        self
    }
}

/// `capability_audit` satirinin bellek karsiligi (migrations/0004).
#[derive(Debug, Clone)]
pub struct CapabilityDecision {
    /// Karara konu olan ajan.
    pub agent_id: i64,
    /// Yetki adi (tool acilimi icin [`CAPABILITY_TOOL`]).
    pub capability: String,
    /// Yetkinin hedefi — tool adi, yol vb.
    pub target: String,
    /// Broker karari; DB'ye [`Decision::label`] ile yazilir.
    pub decision: Decision,
    /// Karari onaylayan merci; otomatik kararlarda `None`.
    pub approver: Option<String>,
    /// Kaydin zaman damgasi (RFC 3339).
    pub ts: String,
}

impl CapabilityDecision {
    /// Otomatik (onaysiz) broker karari.
    pub fn new(
        agent_id: i64,
        capability: impl Into<String>,
        target: impl Into<String>,
        decision: Decision,
    ) -> Self {
        Self {
            agent_id,
            capability: capability.into(),
            target: target.into(),
            decision,
            approver: None,
            ts: now_rfc3339(),
        }
    }

    /// Karari onaylayan merciyi baglar.
    pub fn with_approver(mut self, approver: impl Into<String>) -> Self {
        self.approver = Some(approver.into());
        self
    }

    /// Karar izin veriyor mu?
    pub fn is_allowed(&self) -> bool {
        self.decision.is_allowed()
    }
}

/// Denetim akisinda tasinan kayit turleri.
#[derive(Debug, Clone)]
pub enum AuditEvent {
    /// `tool_calls` satiri.
    ToolCall(Box<ToolCallRecord>),
    /// `capability_audit` satiri.
    Capability(Box<CapabilityDecision>),
}

/// Denetim akisinin gonderici ucu.
pub type AuditSink = UnboundedSender<AuditEvent>;

/// Denetim akisinin alici ucu; omni-storage bunu tuketip DB'ye yazar.
pub type AuditStream = UnboundedReceiver<AuditEvent>;

/// Yeni bir denetim akisi kanali.
pub fn audit_channel() -> (AuditSink, AuditStream) {
    unbounded_channel()
}

/// Kayit gecidi: broker karari + denetim kaydi tek cagrida uretilir.
///
/// Alici dusmusse kayitlar sessizce duser; denetim akisinin kopmasi tool
/// yurutmesini engellemez, yalnizca izlenir (I6: panik yok).
#[derive(Debug, Clone)]
pub struct AuditGate {
    sink: AuditSink,
}

impl AuditGate {
    /// Verilen akisa yazan gecit.
    pub fn new(sink: AuditSink) -> Self {
        Self { sink }
    }

    /// Bir `tool_calls` satiri yayinlar.
    pub fn emit_tool_call(&self, record: ToolCallRecord) {
        self.emit(AuditEvent::ToolCall(Box::new(record)));
    }

    /// Bir `capability_audit` satiri yayinlar.
    pub fn emit_capability(&self, decision: CapabilityDecision) {
        self.emit(AuditEvent::Capability(Box::new(decision)));
    }

    /// TEK zorlama noktasi: broker'a sorar, karari denetime yazar, reddi
    /// `ToolsError::Denied` olarak dondurur.
    ///
    /// Red durumunda hem `capability_audit` hem `tool_calls` satiri uretilir —
    /// denenen ve reddedilen her cagri loglanir (K3 kabul kapisi).
    pub fn authorize_tool(
        &self,
        broker: &ToolBroker,
        agent_id: i64,
        tool: &str,
        args_json: Option<String>,
    ) -> Result<(), ToolsError> {
        let decision = broker.decide(tool);
        let denial = decision.reason().map(str::to_string);
        self.emit_capability(CapabilityDecision::new(
            agent_id,
            CAPABILITY_TOOL,
            tool,
            decision,
        ));

        match denial {
            None => Ok(()),
            Some(reason) => {
                tracing::warn!(agent_id, tool, reason = %reason, "tool cagrisi reddedildi");
                self.emit_tool_call(ToolCallRecord::denied(agent_id, tool).with_args(args_json));
                Err(ToolsError::Denied(reason))
            }
        }
    }

    /// Yurutulmus bir cagriyi kaydeder; yetki zaten [`Self::authorize_tool`]
    /// ile verilmistir.
    pub fn complete_tool_call(
        &self,
        agent_id: i64,
        tool: &str,
        status: ToolCallStatus,
        args_json: Option<String>,
        result_ref: Option<String>,
    ) {
        self.emit_tool_call(
            ToolCallRecord::new(agent_id, tool, status, true)
                .with_args(args_json)
                .with_result_ref(result_ref),
        );
    }

    fn emit(&self, event: AuditEvent) {
        if self.sink.send(event).is_err() {
            tracing::debug!("denetim akisi alicisi kapali, kayit dusuruldu");
        }
    }
}

/// Kayitlarin zaman damgasi. `tool_calls.ts` / `capability_audit.ts` metin
/// sutunudur; RFC 3339 sirali karsilastirilabilir kalir.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_listesi_izne_baskin_gelir() {
        let broker = ToolBroker::new(
            vec!["read_file".to_string(), "bash".to_string()],
            vec!["bash".to_string()],
        );
        assert!(broker.is_permitted("read_file"));
        assert!(!broker.is_permitted("bash"));
        // Yasakli ad ajana ACILMAZ.
        assert_eq!(broker.allowed_tools(), ["read_file".to_string()]);
        assert_eq!(broker.disallowed_tools(), ["bash".to_string()]);
    }

    #[test]
    fn listeler_normalize_edilir() {
        let broker = ToolBroker::new(
            vec![
                "  grep ".to_string(),
                "grep".to_string(),
                String::new(),
                "read_file".to_string(),
            ],
            vec!["  ".to_string()],
        );
        assert_eq!(
            broker.allowed_tools(),
            ["grep".to_string(), "read_file".to_string()]
        );
        assert!(broker.disallowed_tools().is_empty());
    }

    #[test]
    fn allowlist_disi_tool_reddedilir() {
        let broker = ToolBroker::new(vec!["read_file".to_string()], Vec::new());
        match broker.decide("write_file") {
            Decision::Deny(reason) => assert!(reason.contains("write_file")),
            Decision::Allow => panic!("allowlist disi tool izinli sayildi"),
        }
    }

    #[test]
    fn bos_allowlist_yalniz_red_listesini_zorlar() {
        let broker = ToolBroker::new(Vec::new(), vec!["bash".to_string()]);
        assert!(!broker.has_allowlist());
        assert!(broker.is_permitted("read_file"));
        assert!(!broker.is_permitted("bash"));
    }

    #[test]
    fn bos_tool_adi_reddedilir() {
        let broker = ToolBroker::permissive();
        assert!(!broker.is_permitted("   "));
    }

    #[test]
    fn filtre_izinli_alt_kumeyi_dondurur() {
        let broker = ToolBroker::new(
            vec!["read_file".to_string(), "grep".to_string()],
            Vec::new(),
        );
        let filtered = broker.filter(["grep", "bash", "read_file"]);
        assert_eq!(filtered, ["grep".to_string(), "read_file".to_string()]);
    }

    #[test]
    fn yetki_karari_broker_uzerinden_uretilir() {
        let broker = ToolBroker::new(vec!["read_file".to_string()], Vec::new());
        let decision = broker.capability_decision(4, "bash");
        assert_eq!(decision.agent_id, 4);
        assert_eq!(decision.capability, CAPABILITY_TOOL);
        assert_eq!(decision.target, "bash");
        assert!(!decision.is_allowed());
    }

    #[test]
    fn red_hem_yetki_hem_cagri_satiri_uretir() {
        let broker = ToolBroker::new(vec!["read_file".to_string()], Vec::new());
        let (sink, mut stream) = audit_channel();
        let gate = AuditGate::new(sink);

        let result = gate.authorize_tool(&broker, 7, "bash", Some("{}".to_string()));
        assert!(matches!(result, Err(ToolsError::Denied(_))));

        match stream.try_recv() {
            Ok(AuditEvent::Capability(decision)) => {
                assert_eq!(decision.agent_id, 7);
                assert_eq!(decision.capability, CAPABILITY_TOOL);
                assert_eq!(decision.target, "bash");
                assert_eq!(decision.decision.label(), "deny");
                assert!(decision.approver.is_none());
            }
            other => panic!("yetki kaydi bekleniyordu: {other:?}"),
        }

        match stream.try_recv() {
            Ok(AuditEvent::ToolCall(record)) => {
                assert_eq!(record.status, ToolCallStatus::Denied);
                assert_eq!(record.status.as_str(), "denied");
                assert!(!record.capability_ok);
                assert_eq!(record.args_json.as_deref(), Some("{}"));
                assert!(record.result_ref.is_none());
                assert!(!record.ts.is_empty());
            }
            other => panic!("cagri kaydi bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn izin_yalniz_yetki_satiri_uretir() {
        let broker = ToolBroker::new(vec!["read_file".to_string()], Vec::new());
        let (sink, mut stream) = audit_channel();
        let gate = AuditGate::new(sink);

        assert!(gate.authorize_tool(&broker, 3, "read_file", None).is_ok());

        match stream.try_recv() {
            Ok(AuditEvent::Capability(decision)) => {
                assert!(decision.is_allowed());
                assert_eq!(decision.decision.label(), "allow");
            }
            other => panic!("yetki kaydi bekleniyordu: {other:?}"),
        }
        assert!(stream.try_recv().is_err());
    }

    #[test]
    fn tamamlanan_cagri_kaydi_yayinlanir() {
        let (sink, mut stream) = audit_channel();
        let gate = AuditGate::new(sink);
        gate.complete_tool_call(
            11,
            "read_file",
            ToolCallStatus::Ok,
            Some("{\"path\":\"a\"}".to_string()),
            Some("cas:abc".to_string()),
        );

        match stream.try_recv() {
            Ok(AuditEvent::ToolCall(record)) => {
                assert_eq!(record.agent_id, 11);
                assert_eq!(record.tool, "read_file");
                assert!(record.capability_ok);
                assert_eq!(record.status.as_str(), "ok");
                assert_eq!(record.result_ref.as_deref(), Some("cas:abc"));
            }
            other => panic!("cagri kaydi bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn onaylayan_merci_baglanir() {
        let decision = CapabilityDecision::new(1, "self_modify", "src/main.rs", Decision::Allow)
            .with_approver("judge");
        assert_eq!(decision.approver.as_deref(), Some("judge"));
        assert_eq!(decision.decision.label(), "allow");
        assert!(decision.decision.reason().is_none());
    }

    #[test]
    fn alici_dusunce_kayit_sessizce_duser() {
        let broker = ToolBroker::permissive();
        let (sink, stream) = audit_channel();
        drop(stream);
        let gate = AuditGate::new(sink);
        assert!(gate.authorize_tool(&broker, 1, "read_file", None).is_ok());
    }

    #[test]
    fn calisma_akibeti_statuse_yansir() {
        let record = ToolCallRecord::new(2, "grep", ToolCallStatus::Panicked, true);
        assert_eq!(record.status.as_str(), "panicked");
        let failed = ToolCallRecord::new(2, "grep", ToolCallStatus::Failed, true);
        assert_eq!(failed.status.as_str(), "failed");
    }
}
