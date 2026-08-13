//! Tam-otonom dongu (dongu muhendisligi) — grok-build'in kendi agent dongusune
//! tasinan sonsuz-iterasyon cabasi.
//!
//! Kullanici yalnizca problemi verir; dongu kendisi iterasyon yapar:
//!
//! ```text
//! oku -> parcala -> kaydet -> arastir (surface/deep/ocean) -> depola
//!   -> yigin sec -> sure karari -> plan -> yapi taslari
//!   -> paralel/ardisik sorgula -> multiajan gorevlendir
//!   -> dogrula -> bitir veya don
//! ```
//!
//! ## Bilesenler
//!
//! * [`AutonomousLoop`] — dongunun kendisi. `SubagentBackend` (multiajan
//!   gorevlendirme), `ResearchProvider` (`grok_research` yuzeyi) ve
//!   `SessionEventRecorder` (`~/.grok/sessions/<id>/events.jsonl`) kullanir.
//! * [`TerminationOracle`] — `crates/omni/omni-core/src/oracle.rs`'ten
//!   **birebir tasinan** sonlanma mantigi (3 kapi: automatic_verification /
//!   judge_falsification / user_sign_off). Bagimlilik eklenmemistir:
//!   `omni_proto`'nun `TaskId`/`Timestamp`/`ApprovalDecision`'i burada yerel
//!   tiplerle (`u64` / `u64` / [`ApprovalDecision`]) karsilanir; `evaluate`
//!   gövdesi ve `missing_gates` kurali orijinal ile aynidir.
//!
//! ## Dongu akisi (`AutonomousLoop::run`)
//!
//! 1. **Parcala** — problem satir bazli bolunur; her anlamli satir bir yapi
//!    tasidir (`BuildingBlock`). Devam kelimesiyle baslayan taslar bir oncekine
//!    bagimli isaretlenir.
//! 2. **Arastir** — her tas icin `run_research` cagrilir; mod derinlikle
//!    artar: iterasyon 1 = `surface`, 2 = `deep`, >=3 = `ocean`.
//! 3. **Depola** — her adim `events.jsonl`'e yazilir (SessionEventRecorder).
//! 4. **Gorevlendir** — bagimsiz taslar paralel (`futures::join_all`), bagimli
//!    taslar sirayla `backend.spawn` ile calistirilir.
//! 5. **Dogrula** — [`TerminationOracle`] uc kapidan hukmeder; `Pending` ise
//!    bir ust derinlikte tekrar edilir (`max_depth`'e kadar), `Done` ise ozet
//!    donulur.
//!
//! ## Kapi beslemesi (tasinmis oracle + otonom yorum)
//!
//! * **automatic_verification** — `build` = tasima hatasi yok; `test` = tum
//!   ajan sonuclari `success`; `schema_lint` = her tas icin arastirma
//!   sonucu var.
//! * **judge_falsification** — iddialar: arastirma bulgulari + basarili ajan
//!   kayitlari; curutmeler: basarisiz/iptal ajan sonuclari. Hicbir curutme
//!   yoksa kapi gecer.
//! * **user_sign_off** — dongu otonomdur; onaylayici `autonomous-loop`
//!   olarak deterministik sekilde verilir (tum ajanlar basarili + arastirma
//!   tamam). Tam entegrasyonda cagiran taraf insan onayiyla degistirebilir;
//!   makine kimligi reddi (`is_machine_identity`) orijinalden tasinmistir.
//!
//! ## I6
//!
//! Uretim yolunda `unwrap` / `expect` / `panic!` / `unreachable!` yoktur;
//! tum hatalar [`AutonomousError`] ile akar.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use xai_grok_tools::implementations::grok_build::task::backend::SubagentBackend;
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentOwner, SubagentRequest, SubagentResult, SubagentRuntimeOverrides,
};
use xai_grok_tools::research_tool::{
    ResearchFinding, ResearchMode, ResearchProvider, ResearchToolOutput, run_research,
};

use crate::session::persistence::{SESSION_EVENTS_FILE, SessionEventRecorder};

// ---------------------------------------------------------------------------
// Hata tipi
// ---------------------------------------------------------------------------

/// Tam-otonom dongunun hatalari. Uretim yolunda panik yok (I6).
#[derive(Debug)]
pub enum AutonomousError {
    /// Subagent koordinatoru erisilemez (spawn kanali kapali).
    NoBackend,
    /// `max_depth` iterasyonu doldu; son tur `Done` uretemedi.
    DepthExhausted {
        /// Son turun bekleme ozeti (hangi kapilar eksikti).
        last: String,
    },
    /// Dosya sistemi / serilestirme hatasi (oturum dizini, `events.jsonl`).
    Io(String),
}

impl fmt::Display for AutonomousError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoBackend => {
                write!(
                    f,
                    "subagent backend is unavailable (coordinator channel closed)"
                )
            }
            Self::DepthExhausted { last } => {
                write!(f, "autonomous loop exhausted max depth; last: {last}")
            }
            Self::Io(message) => write!(f, "autonomous loop I/O error: {message}"),
        }
    }
}

impl std::error::Error for AutonomousError {}

impl From<crate::session::persistence::PersistenceError> for AutonomousError {
    fn from(value: crate::session::persistence::PersistenceError) -> Self {
        Self::Io(value.to_string())
    }
}

// ---------------------------------------------------------------------------
// Yapi taslari
// ---------------------------------------------------------------------------

/// Problemin tek bir parcalanmis birimi; ajan basina bir tas duser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildingBlock {
    /// Kararli tanimlayici (`step-0`, `step-1`, ...).
    pub id: String,
    /// Tasin metni (ajan promptu / arastirma sorgusu).
    pub text: String,
    /// Bu tasin calisabilmesi icin once tamamlanmasi gereken tasin id'si.
    ///
    /// `None` = bagimsiz (paralel calisir); `Some(prev)` = sirayla.
    pub depends_on: Option<String>,
}

/// Satir bazli basit bolme: her anlamli (trimli, bos olmayan) satir bir tas.
///
/// Devam kelimesiyle baslayan satirlar (`ve`, `ardindan`, `ayrica`, ...) bir
/// onceki tasin devami sayilir ve `depends_on` ile isaretlenir; boylece
/// gorevlendirme fazi bagimli taslari sirayla, bagimsizlari paralel calistirir.
#[must_use]
pub fn split_into_building_blocks(problem: &str) -> Vec<BuildingBlock> {
    let lines: Vec<String> = problem
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    let mut blocks = Vec::with_capacity(lines.len());
    for (index, text) in lines.into_iter().enumerate() {
        let depends_on = if index > 0 && continuation_starts(&text) {
            Some(format!("step-{}", index - 1))
        } else {
            None
        };
        blocks.push(BuildingBlock {
            id: format!("step-{index}"),
            text,
            depends_on,
        });
    }
    blocks
}

/// Bir satirin devam cumlesiyle baslayip baslamadigi (bagimlilik sezgisel).
fn continuation_starts(line: &str) -> bool {
    const CONTINUATION_WORDS: &[&str] = &[
        "ve",
        "ayrica",
        "ustelik",
        "dahasi",
        "ardindan",
        "sonrasinda",
        "daha",
        "sonra",
        "devam",
        "fakat",
        "ancak",
        "buna",
        "ek",
        "ayni",
        "zamanda",
        "boylece",
        "cunku",
        "bundan",
        "dolayi",
        "yuzden",
        "hatta",
        "zira",
    ];
    let first = line
        .split_whitespace()
        .next()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    CONTINUATION_WORDS.contains(&first.as_str())
}

/// Mod secimi: derinlik arttikca arastirma butcesi buyur.
#[must_use]
pub fn mode_for_depth(depth: u32) -> ResearchMode {
    match depth {
        0 | 1 => ResearchMode::Surface,
        2 => ResearchMode::Deep,
        _ => ResearchMode::Ocean,
    }
}

// ---------------------------------------------------------------------------
// Tur kaniti (oracle beslemesi)
// ---------------------------------------------------------------------------

/// Bir yapi tasi icin tek iterasyonun (arastirma + ajan) birikmis kaniti.
#[derive(Debug, Clone)]
pub struct BlockEvidence {
    /// Tasin id'si (`step-0`, ...).
    pub block_id: String,
    /// Tasin arastirma ciktisi; `None` = saglayici yok ya da arastirma hatali.
    pub research: Option<ResearchToolOutput>,
    /// Tasin ajan sonucu; `Err` = tasima (transport) hatasi, `Ok` = ajan dondu.
    pub agent: Result<SubagentResult, String>,
}

impl BlockEvidence {
    /// Tasima hatasi mi?
    #[must_use]
    pub fn transport_failed(&self) -> bool {
        self.agent.is_err()
    }

    /// Ajan basarili dondu mu (iptal ve basarisizlik haric)?
    #[must_use]
    pub fn agent_succeeded(&self) -> bool {
        self.agent.as_ref().is_ok_and(|r| r.success && !r.cancelled)
    }
}

// ---------------------------------------------------------------------------
// Sonlanma oracle'i (oracle.rs'ten tasindi)
//
// `crates/omni/omni-core/src/oracle.rs`'in `evaluate` / `missing_gates`
// mantigi birebir tasindi; bagimlilik eklenmedi: `omni_proto::TaskId` yerine
// `u64`, `omni_proto::Timestamp` yerine epoch-ms `u64`,
// `omni_proto::ApprovalDecision` yerine yerel `ApprovalDecision`.
// ---------------------------------------------------------------------------

/// Kullanici karari (yerel kopya; `omni_proto::ApprovalDecision` bagimliligi
/// eklenmemistir).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ApprovalDecision {
    /// Onaylandi.
    Allow,
    /// Reddedildi.
    Deny,
}

/// Insan yerine gecemeyecek kimlikler (oracle.rs'ten tasindi).
const MACHINE_IDENTITIES: [&str; 8] = [
    "system",
    "auto",
    "agent",
    "omnitrix",
    "scheduler",
    "judge",
    "oracle",
    "root",
];

/// Verilen onaylayan adi bir makine kimligi mi?
///
/// `true` donerse bu ad kullanici onayi olarak kabul **edilemez**.
#[must_use]
pub fn is_machine_identity(approver: &str) -> bool {
    let ad = approver.trim().to_ascii_lowercase();
    MACHINE_IDENTITIES.contains(&ad.as_str())
}

/// Onaylayan adini kullanici onayi icin dogrular.
///
/// # Errors
/// Ad bos ise `OracleError::EmptyField`, makine kimligi ise
/// `OracleError::MachineApprover` doner.
pub fn validate_human_approver(approver: &str) -> Result<String, OracleError> {
    let ad = approver.trim();
    if ad.is_empty() {
        return Err(OracleError::EmptyField { field: "approver" });
    }
    if is_machine_identity(ad) {
        return Err(OracleError::MachineApprover {
            approver: ad.to_owned(),
        });
    }
    Ok(ad.to_owned())
}

/// Sonlanma oracle'inin uc kapisindan biri (oracle.rs'ten tasindi).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleGate {
    /// Otomatik dogrulama: build + test + sema/lint.
    AutomaticVerification,
    /// Yanlislamaci yargic onayi.
    JudgeFalsification,
    /// Kullanici onayi.
    UserSignOff,
}

impl OracleGate {
    /// Uc kapinin kanonik sirasi.
    pub const ALL: [Self; 3] = [
        Self::AutomaticVerification,
        Self::JudgeFalsification,
        Self::UserSignOff,
    ];

    /// Kayit/denetim icin kanonik metin.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::AutomaticVerification => "automatic_verification",
            Self::JudgeFalsification => "judge_falsification",
            Self::UserSignOff => "user_sign_off",
        }
    }
}

impl fmt::Display for OracleGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// Otomatik dogrulamanin kategorileri (oracle.rs'ten tasindi).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// Derleme kapisi.
    Build,
    /// Test kapisi.
    Test,
    /// Sema/lint kapisi.
    SchemaLint,
}

impl std::fmt::Display for CheckKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

impl CheckKind {
    /// Otomatik dogrulamanin zorunlu kategorileri; ucu de bulunmak zorunda.
    pub const ALL: [Self; 3] = [Self::Build, Self::Test, Self::SchemaLint];

    /// Kayit/denetim icin kanonik metin.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
            Self::SchemaLint => "schema_lint",
        }
    }
}

/// Tek bir otomatik dogrulama kosumunun sonucu (oracle.rs'ten tasindi).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckOutcome {
    kind: CheckKind,
    command: String,
    passed: bool,
}

impl CheckOutcome {
    /// Yeni kosum sonucu.
    ///
    /// # Errors
    /// Komut metni bos ise [`OracleError::EmptyField`] doner.
    pub fn new(
        kind: CheckKind,
        command: impl Into<String>,
        passed: bool,
    ) -> Result<Self, OracleError> {
        let command = command.into();
        if command.trim().is_empty() {
            return Err(OracleError::EmptyField {
                field: "check.command",
            });
        }
        Ok(Self {
            kind,
            command,
            passed,
        })
    }

    /// Kategori.
    #[must_use]
    pub fn kind(&self) -> CheckKind {
        self.kind
    }

    /// Kapi gecti mi?
    #[must_use]
    pub fn passed(&self) -> bool {
        self.passed
    }
}

/// Birinci kapi: otomatik dogrulama (oracle.rs'ten tasindi).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutomaticVerification {
    checks: Vec<CheckOutcome>,
    recorded_at: u64,
}

impl AutomaticVerification {
    /// Kosum sonuclarini toplar.
    ///
    /// # Errors
    /// [`CheckKind::ALL`] kategorilerinden biri eksikse
    /// [`OracleError::MissingCheck`] doner.
    pub fn collect(checks: Vec<CheckOutcome>) -> Result<Self, OracleError> {
        let mevcut: BTreeSet<CheckKind> = checks.iter().map(CheckOutcome::kind).collect();
        for kind in CheckKind::ALL {
            if !mevcut.contains(&kind) {
                return Err(OracleError::MissingCheck { kind });
            }
        }
        Ok(Self {
            checks,
            recorded_at: now_ts(),
        })
    }

    /// Kapi gecti mi? Tum kosumlar basarili olmali.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.checks.iter().all(CheckOutcome::passed)
    }

    /// Kaydin alindigi an (epoch ms).
    #[must_use]
    pub fn recorded_at(&self) -> u64 {
        self.recorded_at
    }

    /// Kosum kumesinin icerik ozeti.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        for check in &self.checks {
            hasher.update(check.kind.as_db_str().as_bytes());
            hasher.update(b"\x1f");
            hasher.update(check.command.as_bytes());
            hasher.update(b"\x1f");
            hasher.update(if check.passed { b"1" } else { b"0" });
            hasher.update(b"\x1e");
        }
        hasher.finalize().to_hex().to_string()
    }
}

/// Yargicin degerlendirdigi tek iddia: **kanit-referansli** (oracle.rs'ten
/// tasindi). Kanitsiz iddia kabul edilmez.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceClaim {
    claim: String,
    evidence_ref: String,
}

impl EvidenceClaim {
    /// Yeni iddia.
    ///
    /// # Errors
    /// Iddia ya da kanit atfi bos ise [`OracleError::EmptyField`] doner.
    pub fn new(
        claim: impl Into<String>,
        evidence_ref: impl Into<String>,
    ) -> Result<Self, OracleError> {
        let claim = claim.into();
        if claim.trim().is_empty() {
            return Err(OracleError::EmptyField {
                field: "claim.text",
            });
        }
        let evidence_ref = evidence_ref.into();
        if evidence_ref.trim().is_empty() {
            return Err(OracleError::EmptyField {
                field: "claim.evidence_ref",
            });
        }
        Ok(Self {
            claim,
            evidence_ref,
        })
    }
}

/// Yargicin curuttugu iddia: karsi kanit atfi zorunludur (oracle.rs'ten
/// tasindi).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refutation {
    claim: String,
    counter_evidence_ref: String,
}

impl Refutation {
    /// Yeni curutme.
    ///
    /// # Errors
    /// Iddia ya da karsi kanit atfi bos ise [`OracleError::EmptyField`] doner.
    pub fn new(
        claim: impl Into<String>,
        counter_evidence_ref: impl Into<String>,
    ) -> Result<Self, OracleError> {
        let claim = claim.into();
        if claim.trim().is_empty() {
            return Err(OracleError::EmptyField {
                field: "refutation.claim",
            });
        }
        let counter_evidence_ref = counter_evidence_ref.into();
        if counter_evidence_ref.trim().is_empty() {
            return Err(OracleError::EmptyField {
                field: "refutation.counter_evidence_ref",
            });
        }
        Ok(Self {
            claim,
            counter_evidence_ref,
        })
    }
}

/// Ikinci kapi: yanlislamaci yargic hukmu (oracle.rs'ten tasindi).
///
/// Kapi "iddialar dogru kanitlandi" ile degil, **"curutulemedi"** ile gecer.
/// Bos iddia kumesi kabul edilmez.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FalsificationVerdict {
    judge_role: String,
    claims: Vec<EvidenceClaim>,
    refutations: Vec<Refutation>,
    ruled_at: u64,
}

impl FalsificationVerdict {
    /// Yargic hukmunu kaydeder.
    ///
    /// # Errors
    /// Rol adi bos ise ya da iddia kumesi bos ise hata doner.
    pub fn new(
        judge_role: impl Into<String>,
        claims: Vec<EvidenceClaim>,
        refutations: Vec<Refutation>,
    ) -> Result<Self, OracleError> {
        let judge_role = judge_role.into();
        if judge_role.trim().is_empty() {
            return Err(OracleError::EmptyField {
                field: "verdict.judge_role",
            });
        }
        if claims.is_empty() {
            return Err(OracleError::NoClaims);
        }
        Ok(Self {
            judge_role,
            claims,
            refutations,
            ruled_at: now_ts(),
        })
    }

    /// Kapi gecti mi? Hicbir iddia curutulmemis olmali.
    #[must_use]
    pub fn upheld(&self) -> bool {
        self.refutations.is_empty()
    }

    /// Hukmun verildigi an (epoch ms).
    #[must_use]
    pub fn ruled_at(&self) -> u64 {
        self.ruled_at
    }

    /// Iddia kumesinin icerik ozeti.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(self.judge_role.as_bytes());
        hasher.update(b"\x1e");
        for claim in &self.claims {
            hasher.update(claim.claim.as_bytes());
            hasher.update(b"\x1f");
            hasher.update(claim.evidence_ref.as_bytes());
            hasher.update(b"\x1e");
        }
        hasher.finalize().to_hex().to_string()
    }
}

/// Ucuncu kapi: kullanici onayi (oracle.rs'ten tasindi).
///
/// Otonom dongude onaylayan deterministik kriterle verilir; yine de makine
/// kimligi reddi (`is_machine_identity`) orijinalden korunmustur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserSignOff {
    approver: String,
    decision: ApprovalDecision,
    signed_at: u64,
}

impl UserSignOff {
    /// Kullanici kararini kaydeder.
    ///
    /// # Errors
    /// Onaylayan adi bos ya da makine kimligi ise hata doner.
    pub fn record(
        approver: impl AsRef<str>,
        decision: ApprovalDecision,
    ) -> Result<Self, OracleError> {
        let approver = validate_human_approver(approver.as_ref())?;
        Ok(Self {
            approver,
            decision,
            signed_at: now_ts(),
        })
    }

    /// Onaylayan.
    #[must_use]
    pub fn approver(&self) -> &str {
        &self.approver
    }

    /// Kapi gecti mi? Yalnizca `Allow` gecer.
    #[must_use]
    pub fn granted(&self) -> bool {
        matches!(self.decision, ApprovalDecision::Allow)
    }

    /// Onayin verildigi an (epoch ms).
    #[must_use]
    pub fn signed_at(&self) -> u64 {
        self.signed_at
    }
}

/// Uc kapiyi toplayan karar noktasi (oracle.rs'ten tasindi).
#[derive(Debug, Clone, Serialize)]
pub struct TerminationOracle {
    task_id: u64,
    verification: Option<AutomaticVerification>,
    verdict: Option<FalsificationVerdict>,
    sign_off: Option<UserSignOff>,
    subagents: BTreeSet<String>,
}

impl TerminationOracle {
    /// Bir gorev icin bos oracle.
    #[must_use]
    pub fn new(task_id: u64) -> Self {
        Self {
            task_id,
            verification: None,
            verdict: None,
            sign_off: None,
            subagents: BTreeSet::new(),
        }
    }

    /// Gorev bitince toplanacak alt-ajani kaydeder.
    pub fn track_subagent(&mut self, agent_id: impl Into<String>) {
        self.subagents.insert(agent_id.into());
    }

    /// Birinci kapiyi doldurur.
    pub fn submit_verification(&mut self, verification: AutomaticVerification) {
        self.verification = Some(verification);
    }

    /// Ikinci kapiyi doldurur.
    pub fn submit_verdict(&mut self, verdict: FalsificationVerdict) {
        self.verdict = Some(verdict);
    }

    /// Ucuncu kapiyi doldurur.
    pub fn submit_sign_off(&mut self, sign_off: UserSignOff) {
        self.sign_off = Some(sign_off);
    }

    /// Henuz saglanmamis kapilar; bos liste = hukum verilebilir.
    #[must_use]
    pub fn missing_gates(&self) -> Vec<OracleGate> {
        let mut eksik = Vec::new();
        if !self
            .verification
            .as_ref()
            .is_some_and(AutomaticVerification::passed)
        {
            eksik.push(OracleGate::AutomaticVerification);
        }
        if !self
            .verdict
            .as_ref()
            .is_some_and(FalsificationVerdict::upheld)
        {
            eksik.push(OracleGate::JudgeFalsification);
        }
        if !self.sign_off.as_ref().is_some_and(UserSignOff::granted) {
            eksik.push(OracleGate::UserSignOff);
        }
        eksik
    }

    /// Uc kapiyi degerlendirir.
    ///
    /// Ucu birden saglanmadan [`OracleOutcome::Done`] **donmez**; eksik
    /// kapilar [`OracleOutcome::Pending`] icinde bildirilir.
    #[must_use]
    pub fn evaluate(&self) -> OracleOutcome {
        let eksik = self.missing_gates();
        if !eksik.is_empty() {
            return OracleOutcome::Pending { missing: eksik };
        }
        let (Some(verification), Some(verdict), Some(sign_off)) = (
            self.verification.as_ref(),
            self.verdict.as_ref(),
            self.sign_off.as_ref(),
        ) else {
            // `missing_gates` bos ise ucu de doludur; bu dal olusmaz ama I6
            // geregi panik yerine "beklemede" doneriz.
            return OracleOutcome::Pending {
                missing: OracleGate::ALL.to_vec(),
            };
        };
        OracleOutcome::Done(DoneRuling::seal(
            self.task_id,
            self.subagents.iter().cloned().collect(),
            verification,
            verdict,
            sign_off,
        ))
    }
}

/// Oracle degerlendirmesinin sonucu (oracle.rs'ten tasindi).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum OracleOutcome {
    /// Kapilar tamamlanmadi; gorev calismaya devam eder.
    Pending {
        /// Eksik kapilar.
        missing: Vec<OracleGate>,
    },
    /// Uc kapi da gecti; gorev `Done`.
    Done(DoneRuling),
}

impl OracleOutcome {
    /// Gorev bitti mi?
    #[must_use]
    pub fn is_done(&self) -> bool {
        matches!(self, Self::Done(_))
    }
}

/// "Is bitti" hukmu (oracle.rs'ten tasindi). Alanlari private'dir ve tek
/// uretici [`TerminationOracle::evaluate`] icindeki `seal`'dir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoneRuling {
    task_id: u64,
    reaped_agents: Vec<String>,
    verification_digest: String,
    evidence_digest: String,
    approver: String,
    decided_at: u64,
}

impl DoneRuling {
    /// Yalnizca [`TerminationOracle::evaluate`] icinden cagrilir; uc kapi
    /// zaten dogrulanmistir.
    fn seal(
        task_id: u64,
        reaped_agents: Vec<String>,
        verification: &AutomaticVerification,
        verdict: &FalsificationVerdict,
        sign_off: &UserSignOff,
    ) -> Self {
        Self {
            task_id,
            reaped_agents,
            verification_digest: verification.digest(),
            evidence_digest: verdict.digest(),
            approver: sign_off.approver().to_owned(),
            decided_at: now_ts(),
        }
    }

    /// Biten gorev.
    #[must_use]
    pub fn task_id(&self) -> u64 {
        self.task_id
    }

    /// Toplanacak alt-ajanlar.
    #[must_use]
    pub fn reaped_agents(&self) -> &[String] {
        &self.reaped_agents
    }

    /// Otomatik dogrulama ozeti.
    #[must_use]
    pub fn verification_digest(&self) -> &str {
        &self.verification_digest
    }

    /// Yargic kanit ozeti.
    #[must_use]
    pub fn evidence_digest(&self) -> &str {
        &self.evidence_digest
    }

    /// Kullanici onayini veren.
    #[must_use]
    pub fn approver(&self) -> &str {
        &self.approver
    }

    /// Hukmun verildigi an (epoch ms).
    #[must_use]
    pub fn decided_at(&self) -> u64 {
        self.decided_at
    }
}

/// Sonlanma oracle'i hatalari (oracle.rs'ten tasindi). I6: panik yok.
#[derive(Debug)]
pub enum OracleError {
    /// Zorunlu alan bos birakildi.
    EmptyField {
        /// Bos kalan alanin adi.
        field: &'static str,
    },
    /// Otomatik dogrulamada bir kategori eksik.
    MissingCheck {
        /// Eksik kategori.
        kind: CheckKind,
    },
    /// Yargica hicbir iddia sunulmamis.
    NoClaims,
    /// Kullanici onayi yerine makine kimligi sunuldu.
    MachineApprover {
        /// Reddedilen ad.
        approver: String,
    },
}

impl fmt::Display for OracleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField { field } => write!(f, "'{field}' alani bos birakilamaz"),
            Self::MissingCheck { kind } => {
                write!(f, "otomatik dogrulama eksik: '{kind}' kosumu yok")
            }
            Self::NoClaims => write!(f, "yargic hukmu bos iddia kumesiyle verilemez"),
            Self::MachineApprover { approver } => write!(
                f,
                "'{approver}' bir makine kimligi; kullanici onayi insandan gelmeli"
            ),
        }
    }
}

impl std::error::Error for OracleError {}

/// Epoch-milisaniye zaman damgasi (I6: panik yok, `unwrap_or_default`).
fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or_default())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Otonom dongu
// ---------------------------------------------------------------------------

/// Otonom onaylayan adi: deterministik kapi kriteri (`is_machine_identity`
/// listesinde degildir; tam entegrasyonda insan onayina devredilebilir).
const AUTONOMOUS_APPROVER: &str = "autonomous-loop";

/// Tam-otonom dongu: problemi okur, parcalar, arastirir, depolar, multiajan
/// gorevlendirir ve oracle hukmuyle bitirir veya bir ust derinlikte doner.
pub struct AutonomousLoop {
    backend: Arc<dyn SubagentBackend>,
    research: Option<Arc<dyn ResearchProvider>>,
    session_id: String,
    session_dir: PathBuf,
    subagent_type: String,
    max_depth: u32,
}

impl AutonomousLoop {
    /// Varsayilan ayarlarla yeni dongu.
    ///
    /// `max_depth` = 3 (surface -> deep -> ocean). Oturum kimligi ve dizini
    /// bos baslar; `with_session` ile doldurulmalidir (ya da `run` sirasinda
    /// `~/.grok/sessions/<id>`'e cozulur).
    #[must_use]
    pub fn new(backend: Arc<dyn SubagentBackend>) -> Self {
        Self {
            backend,
            research: None,
            session_id: String::new(),
            session_dir: PathBuf::new(),
            subagent_type: "general-purpose".to_owned(),
            max_depth: 3,
        }
    }

    /// Arastirma saglayicisini ekler (`grok_research` yuzeyi).
    #[must_use]
    pub fn with_research(mut self, research: Arc<dyn ResearchProvider>) -> Self {
        self.research = Some(research);
        self
    }

    /// Oturum kimligini ve olay gunlugu dizinini baglar.
    #[must_use]
    pub fn with_session(mut self, session_id: impl Into<String>, session_dir: PathBuf) -> Self {
        self.session_id = session_id.into();
        self.session_dir = session_dir;
        self
    }

    /// Ajan tipini degistirir (varsayilan `general-purpose`).
    #[must_use]
    pub fn with_subagent_type(mut self, subagent_type: impl Into<String>) -> Self {
        self.subagent_type = subagent_type.into();
        self
    }

    /// Maksimum iterasyon derinligini degistirir (varsayilan 3).
    #[must_use]
    pub fn with_max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth = max_depth.max(1);
        self
    }

    /// Subagent backend'e erisim.
    #[must_use]
    pub fn backend(&self) -> &dyn SubagentBackend {
        self.backend.as_ref()
    }

    /// Arastirma saglayicisi, varsa.
    #[must_use]
    pub fn research(&self) -> Option<&dyn ResearchProvider> {
        self.research.as_deref()
    }

    /// Bagli oturum kimligi.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Olay gunlugu dizini.
    #[must_use]
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    /// Ajan tipi.
    #[must_use]
    pub fn subagent_type(&self) -> &str {
        &self.subagent_type
    }

    /// Maksimum iterasyon derinligi.
    #[must_use]
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Tam-otonom donguyu calistirir.
    ///
    /// # Errors
    /// * Backend erisilemezse [`AutonomousError::NoBackend`].
    /// * Oturum dizini cozulemez / olay gunlugu yazilamazsa [`AutonomousError::Io`].
    /// * `max_depth` iterasyonu `Done` uretmezse [`AutonomousError::DepthExhausted`].
    pub async fn run(&self, problem: &str) -> Result<String, AutonomousError> {
        let blocks = split_into_building_blocks(problem);
        if blocks.is_empty() {
            return Err(AutonomousError::Io(
                "problem is empty after splitting; nothing to run".to_owned(),
            ));
        }

        let session_dir = self.resolve_session_dir()?;
        let events = SessionEventRecorder::new();
        events.record_message(
            &session_dir,
            "assistant",
            &format!("autonomous loop start: {} blocks", blocks.len()),
        )?;

        let mut last_pending = String::new();
        for depth in 1..=self.max_depth {
            let mode = mode_for_depth(depth);
            events.record_message(
                &session_dir,
                "assistant",
                &format!("autonomous iteration {depth} (mode={})", mode.as_str()),
            )?;

            // a) + b) her yapi tasi icin arastirma (mod derinlikle artar).
            let research = self
                .research_all(&blocks, mode, &session_dir, &events)
                .await?;

            // d) multiajan gorevlendirme: bagimsizlar paralel, bagimlilar sirayla.
            let mut evidence = self.run_agents(&blocks, &session_dir, &events).await?;

            // Arastirma ciktilarini kanita bagla (oracle iddialari buradan dogar).
            for (block, research_output) in evidence.iter_mut().zip(research.iter()) {
                block.research = research_output.clone();
            }

            // e) dogrula: oracle; Done degilse bir ust derinlikte tekrar.
            let outcome = self.evaluate(blocks.len(), &evidence);
            events.record_message(
                &session_dir,
                "assistant",
                &format!("autonomous iteration {depth} verdict: {outcome:?}"),
            )?;

            match outcome {
                OracleOutcome::Done(ruling) => {
                    let summary = self.summarize(&ruling, depth, &evidence);
                    events.record_message(&session_dir, "assistant", &summary)?;
                    return Ok(summary);
                }
                OracleOutcome::Pending { missing } => {
                    last_pending = format!("iteration {depth}: missing gates {missing:?}");
                    tracing::warn!(
                        depth,
                        missing = ?missing,
                        "autonomous iteration not done; deepening research"
                    );
                }
            }
        }

        Err(AutonomousError::DepthExhausted { last: last_pending })
    }

    /// Oturum dizinini cozer: acikca verildiyse aynen, degilse
    /// `~/.grok/sessions/<session_id>`.
    fn resolve_session_dir(&self) -> Result<PathBuf, AutonomousError> {
        if !self.session_dir.as_os_str().is_empty() {
            return Ok(self.session_dir.clone());
        }
        let home = crate::session::persistence::grok_home_string().ok_or_else(|| {
            AutonomousError::Io(
                "cannot resolve grok home directory for default session path".to_owned(),
            )
        })?;
        let session_id = if self.session_id.trim().is_empty() {
            "autonomous".to_owned()
        } else {
            self.session_id.clone()
        };
        Ok(PathBuf::from(home).join("sessions").join(session_id))
    }

    /// Her yapi tasi icin `run_research` cagirir (mod derinlikle artar).
    ///
    /// Saglayici yoksa tum taslar `None` alir (kapi beslemesi `schema_lint`'i
    /// acik birakir -> loop `Done` uretmez, derinlesir).
    async fn research_all(
        &self,
        blocks: &[BuildingBlock],
        mode: ResearchMode,
        session_dir: &Path,
        events: &SessionEventRecorder,
    ) -> Result<Vec<Option<ResearchToolOutput>>, AutonomousError> {
        let Some(provider) = self.research.as_deref() else {
            return Ok(vec![None; blocks.len()]);
        };
        let mut outputs = Vec::with_capacity(blocks.len());
        for block in blocks {
            match run_research(provider, &block.text, mode).await {
                Ok(output) => {
                    events.record_tool_call(
                        session_dir,
                        "grok_research",
                        &serde_json::json!({
                            "block": block.id,
                            "query": block.text,
                            "mode": mode.as_str(),
                            "rounds": output.rounds_run,
                            "findings": output.findings.len(),
                        })
                        .to_string(),
                    )?;
                    outputs.push(Some(output));
                }
                Err(error) => {
                    tracing::warn!(
                        block = %block.id,
                        %error,
                        "grok_research failed for block; treating as no findings"
                    );
                    events.record_message(
                        session_dir,
                        "assistant",
                        &format!("research failed for {}: {error}", block.id),
                    )?;
                    outputs.push(None);
                }
            }
        }
        Ok(outputs)
    }

    /// Multiajan gorevlendirme: bagimsiz taslar paralel (`join_all`), bagimli
    /// taslar sirayla.
    ///
    /// # Errors
    /// Spawn kanali kapaliysa (koordinator gitti) [`AutonomousError::NoBackend`];
    /// olay gunlugu yazilamazsa [`AutonomousError::Io`].
    async fn run_agents(
        &self,
        blocks: &[BuildingBlock],
        session_dir: &Path,
        events: &SessionEventRecorder,
    ) -> Result<Vec<BlockEvidence>, AutonomousError> {
        let mut outcomes: Vec<Option<BlockEvidence>> = vec![None; blocks.len()];

        let parallel: Vec<usize> = blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| block.depends_on.is_none())
            .map(|(index, _)| index)
            .collect();
        if !parallel.is_empty() {
            let futures = parallel.iter().map(|&index| {
                let block = &blocks[index];
                async move {
                    let evidence = self.spawn_one(block, session_dir, events).await;
                    (index, evidence)
                }
            });
            let results = join_all(futures).await;
            for (index, evidence) in results {
                outcomes[index] = Some(evidence?);
            }
        }

        for (index, block) in blocks.iter().enumerate() {
            if block.depends_on.is_none() {
                continue;
            }
            outcomes[index] = Some(self.spawn_one(block, session_dir, events).await?);
        }

        let mut evidence = Vec::with_capacity(blocks.len());
        for outcome in outcomes {
            evidence.push(outcome.ok_or_else(|| {
                AutonomousError::Io("internal: missing block outcome".to_owned())
            })?);
        }
        Ok(evidence)
    }

    /// Tek yapi tasi icin bir subagent baslatir ve sonucunu bekler.
    async fn spawn_one(
        &self,
        block: &BuildingBlock,
        session_dir: &Path,
        events: &SessionEventRecorder,
    ) -> Result<BlockEvidence, AutonomousError> {
        let subagent_id = Uuid::new_v4().to_string();
        let request = SubagentRequest {
            id: subagent_id.clone(),
            prompt: block.text.clone(),
            description: format!("autonomous: {}", block.id),
            subagent_type: self.subagent_type.clone(),
            parent_session_id: self.session_id.clone(),
            parent_prompt_id: None,
            resume_from: None,
            cwd: None,
            runtime_overrides: SubagentRuntimeOverrides::default(),
            run_in_background: false,
            surface_completion: false,
            await_to_completion: true,
            fork_context: false,
            owner: SubagentOwner::Task,
            cancel_token: CancellationToken::new(),
        };
        events.record_tool_call(
            session_dir,
            "subagent_spawn",
            &serde_json::json!({
                "block": block.id,
                "subagent_id": subagent_id,
                "subagent_type": self.subagent_type,
            })
            .to_string(),
        )?;

        let agent = match self.backend.spawn(request).await {
            Ok(result) => Ok(result),
            Err(error) => {
                let message = error.to_string();
                if message.contains("channel_closed") {
                    return Err(AutonomousError::NoBackend);
                }
                tracing::warn!(
                    block = %block.id,
                    %message,
                    "subagent spawn failed at transport level; recording as failed block"
                );
                Err(message)
            }
        };
        Ok(BlockEvidence {
            block_id: block.id.clone(),
            research: None,
            agent,
        })
    }

    /// Tur kanitini uc kapiya besler ve oracle hukmunu uretir.
    ///
    /// * verification: `build` = tasima hatasi yok; `test` = tum ajanlar
    ///   basarili; `schema_lint` = her tas icin arastirma sonucu var.
    /// * verdict: iddialar = arastirma bulgulari + basarili ajan kayitlari;
    ///   curutmeler = basarisiz/iptal/tasima-hatali taslar.
    /// * sign_off: tum ajanlar basarili + arastirma tamam ise deterministik
    ///   onay (`autonomous-loop`).
    fn evaluate(&self, block_count: usize, evidence: &[BlockEvidence]) -> OracleOutcome {
        let mut oracle = TerminationOracle::new(u64::try_from(block_count).unwrap_or_default());

        let transport_ok = evidence.iter().all(|e| !e.transport_failed());
        let agents_ok = !evidence.is_empty() && evidence.iter().all(BlockEvidence::agent_succeeded);
        let research_ok = !evidence.is_empty() && evidence.iter().all(|e| e.research.is_some());

        let checks = vec![
            CheckOutcome::new(CheckKind::Build, "autonomous.subagent_spawn", transport_ok),
            CheckOutcome::new(CheckKind::Test, "autonomous.subagent_success", agents_ok),
            CheckOutcome::new(CheckKind::SchemaLint, "autonomous.research", research_ok),
        ];
        let checks: Vec<CheckOutcome> = checks.into_iter().filter_map(Result::ok).collect();
        if let Ok(verification) = AutomaticVerification::collect(checks) {
            oracle.submit_verification(verification);
        }

        let claims: Vec<EvidenceClaim> = evidence
            .iter()
            .flat_map(|e| {
                let mut block_claims = Vec::new();
                if let Some(research) = &e.research {
                    for finding in &research.findings {
                        block_claims.push(
                            EvidenceClaim::new(
                                format!("{}: {}", e.block_id, finding.title),
                                finding_evidence_ref(finding),
                            )
                            .ok(),
                        );
                    }
                }
                if let Ok(result) = &e.agent {
                    if result.success {
                        block_claims.push(
                            EvidenceClaim::new(
                                format!("{} executed by subagent", e.block_id),
                                format!("agent:{}", result.subagent_id),
                            )
                            .ok(),
                        );
                        oracle.track_subagent(result.subagent_id.clone());
                    }
                }
                block_claims.into_iter().flatten()
            })
            .collect();

        let refutations: Vec<Refutation> = evidence
            .iter()
            .filter_map(|e| match &e.agent {
                Err(message) => Refutation::new(
                    format!("{} block failed at transport level", e.block_id),
                    format!("transport:{message}"),
                )
                .ok(),
                Ok(result) if !(result.success && !result.cancelled) => Refutation::new(
                    format!("{} agent did not succeed", e.block_id),
                    result
                        .error
                        .clone()
                        .unwrap_or_else(|| "agent returned failure".to_owned()),
                )
                .ok(),
                _ => None,
            })
            .collect();

        if let Ok(verdict) = FalsificationVerdict::new("autonomous-judge", claims, refutations) {
            oracle.submit_verdict(verdict);
        }

        if agents_ok && research_ok {
            if let Ok(sign_off) = UserSignOff::record(AUTONOMOUS_APPROVER, ApprovalDecision::Allow)
            {
                oracle.submit_sign_off(sign_off);
            }
        }

        oracle.evaluate()
    }

    /// `Done` hukmu icin ozet: arastirma turu sayisi + ajan sayisi + sonuc.
    fn summarize(&self, ruling: &DoneRuling, depth: u32, evidence: &[BlockEvidence]) -> String {
        let research_rounds: usize = evidence
            .iter()
            .filter_map(|e| e.research.as_ref())
            .map(|r| usize::from(r.rounds_run))
            .sum();
        let research_queries: usize = evidence
            .iter()
            .filter_map(|e| e.research.as_ref())
            .map(|r| r.queries.len())
            .sum();
        let agent_count = evidence.iter().filter(|e| e.agent_succeeded()).count();
        let last_content = evidence
            .iter()
            .filter_map(|e| e.research.as_ref())
            .map(|r| r.content.as_str())
            .last()
            .unwrap_or_default();

        format!(
            "AutonomousLoop: Done (task={}) in {} iteration(s) — research_rounds={research_rounds}, research_queries={research_queries}, successful_agents={agent_count}/{}\nverification={}\nevidence={}\napprover={}\nresult:\n{last_content}",
            ruling.task_id(),
            depth,
            evidence.len(),
            ruling.verification_digest(),
            ruling.evidence_digest(),
            ruling.approver(),
        )
    }
}

/// Bir arastirma bulgusunun kanit atfi: URL varsa URL, yoksa baslik+ozet.
fn finding_evidence_ref(finding: &ResearchFinding) -> String {
    if finding.url.trim().is_empty() {
        format!("research:{}", finding.dedup_key())
    } else {
        finding.url.clone()
    }
}

/// `events.jsonl` dosya adi erisim yardimcisi (denetim icin).
#[must_use]
pub fn events_file_path(session_dir: &Path) -> PathBuf {
    session_dir.join(SESSION_EVENTS_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_empty_problem_yields_no_blocks() {
        assert!(split_into_building_blocks("").is_empty());
        assert!(split_into_building_blocks("   \n  \n").is_empty());
    }

    #[test]
    fn split_marks_continuation_lines_as_dependent() {
        let blocks =
            split_into_building_blocks("ilgili API'yi incele\nve dokumani oku\nsonra test et");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].id, "step-0");
        assert_eq!(blocks[0].depends_on, None);
        assert_eq!(blocks[1].depends_on.as_deref(), Some("step-0"));
        assert_eq!(blocks[2].depends_on.as_deref(), Some("step-1"));
    }

    #[test]
    fn split_keeps_independent_lines_independent() {
        let blocks = split_into_building_blocks("dokumani oku\nkodu tara");
        assert_eq!(blocks[1].depends_on, None);
    }

    #[test]
    fn mode_grows_with_depth() {
        assert_eq!(mode_for_depth(1), ResearchMode::Surface);
        assert_eq!(mode_for_depth(2), ResearchMode::Deep);
        assert_eq!(mode_for_depth(3), ResearchMode::Ocean);
        assert_eq!(mode_for_depth(99), ResearchMode::Ocean);
    }

    #[test]
    fn machine_identities_are_rejected_as_approvers() {
        for name in ["system", "AUTO", " omnitrix ", "agent", "judge"] {
            assert!(is_machine_identity(name), "{name}");
            assert!(UserSignOff::record(name, ApprovalDecision::Allow).is_err());
        }
    }

    #[test]
    fn autonomous_approver_passes_validation() {
        let sign_off = UserSignOff::record(AUTONOMOUS_APPROVER, ApprovalDecision::Allow)
            .expect("autonomous approver must pass");
        assert!(sign_off.granted());
    }

    fn gecen_dogrulama() -> AutomaticVerification {
        let checks = vec![
            CheckOutcome::new(CheckKind::Build, "build", true).expect("build"),
            CheckOutcome::new(CheckKind::Test, "test", true).expect("test"),
            CheckOutcome::new(CheckKind::SchemaLint, "lint", true).expect("lint"),
        ];
        AutomaticVerification::collect(checks).expect("verification")
    }

    fn gecen_hukum() -> FalsificationVerdict {
        let claims = vec![EvidenceClaim::new("iddia", "kanit:1").expect("claim")];
        FalsificationVerdict::new("judge", claims, Vec::new()).expect("verdict")
    }

    #[test]
    fn oracle_requires_all_three_gates() {
        let mut oracle = TerminationOracle::new(7);
        oracle.submit_verification(gecen_dogrulama());
        let sonuc = oracle.evaluate();
        assert!(!sonuc.is_done());
        let OracleOutcome::Pending { missing } = sonuc else {
            panic!("expected pending");
        };
        assert_eq!(
            missing,
            vec![OracleGate::JudgeFalsification, OracleGate::UserSignOff]
        );
    }

    #[test]
    fn oracle_three_gates_produce_done() {
        let mut oracle = TerminationOracle::new(42);
        oracle.submit_verification(gecen_dogrulama());
        oracle.submit_verdict(gecen_hukum());
        oracle.submit_sign_off(
            UserSignOff::record("void0x14", ApprovalDecision::Allow).expect("approval"),
        );
        let hukum = oracle.evaluate();
        assert!(hukum.is_done());
    }

    #[test]
    fn refuted_claim_keeps_verdict_gate_closed() {
        let claims = vec![EvidenceClaim::new("hizli", "kanit:1").expect("claim")];
        let refs = vec![Refutation::new("hizli", "kanit:2").expect("refutation")];
        let hukum = FalsificationVerdict::new("judge", claims, refs).expect("verdict");
        assert!(!hukum.upheld());
    }

    #[test]
    fn denied_sign_off_keeps_gate_closed() {
        let sign_off = UserSignOff::record("void0x14", ApprovalDecision::Deny).expect("approval");
        assert!(!sign_off.granted());
    }
}
