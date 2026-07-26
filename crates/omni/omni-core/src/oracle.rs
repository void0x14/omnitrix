//! Sonlanma oracle'i — "is bitti" karari (MASTER-PLAN 7.5, K10, R6).
//!
//! Bir gorev **ancak** su ucu birden saglaninca `Done` olur:
//!
//! 1. [`AutomaticVerification`] — build + test + sema/lint kapisi gecer.
//! 2. [`FalsificationVerdict`] — yanlislamaci yargic (3.4) onayi: her iddia
//!    kanit-referanslidir ve hicbiri curutulememistir.
//! 3. [`UserSignOff`] — kullanici onayi (K10 hibrit karar).
//!
//! Uc kapi **ayri ayri** temsil edilir; hicbiri digerinin yerine gecmez.
//! [`TerminationOracle::evaluate`] ucu birden saglanmadan [`DoneRuling`]
//! uretmez — ve `DoneRuling` uretilmeden butce dondurulamaz, alt-ajanlar
//! toplanamaz. Boylece "sonsuz iterasyon ama bitince dur" (K11) celiskisi tip
//! duzeyinde cozulur: bitmis ise kaynak yakan bir kod yolu yoktur (R6).
//!
//! Bu dosyada I/O yoktur: karar saf, senkron ve deterministiktir. Komutlari
//! kim kosturur, kullaniciya kim sorar — cagiranin isidir; oracle yalnizca
//! sonuclari toplar ve hukum verir.

use std::collections::BTreeSet;
use std::fmt;

use omni_proto::{AgentId, ApprovalDecision, TaskId, Timestamp, now};
use serde::{Deserialize, Serialize};

/// Insan yerine gecemeyecek kimlikler.
///
/// Kullanici onayi (K10) **insandan** gelir; bir ajanin ya da zamanlayicinin
/// kendini onaylamasi kapiyi gecersiz kilar. Karsilastirma kirpilmis ve
/// buyuk/kucuk harf duyarsizdir.
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
/// Ad bos ise [`OracleError::EmptyField`], makine kimligi ise
/// [`OracleError::MachineApprover`] doner.
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

/// Sonlanma oracle'inin uc kapisindan biri (7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleGate {
    /// Otomatik dogrulama: build + test + sema/lint.
    AutomaticVerification,
    /// Yanlislamaci yargic onayi (3.4).
    JudgeFalsification,
    /// Kullanici onayi (K10).
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

/// Otomatik dogrulamanin kategorileri (faz kapisi komutlari, 7.5/1).
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

impl fmt::Display for CheckKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// Tek bir otomatik dogrulama kosumunun sonucu.
///
/// `command` denetlenebilirlik icin tutulur: hangi komutun hangi sonucu
/// urettigi kayittan okunabilmelidir. Cikti govdesi DB'ye degil CAS'a gider;
/// burada yalnizca atif (`detail_ref`) durur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckOutcome {
    kind: CheckKind,
    command: String,
    passed: bool,
    detail_ref: Option<String>,
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
            detail_ref: None,
        })
    }

    /// Cikti govdesinin CAS atfini ekler.
    #[must_use]
    pub fn with_detail_ref(mut self, detail_ref: impl Into<String>) -> Self {
        self.detail_ref = Some(detail_ref.into());
        self
    }

    /// Kategori.
    #[must_use]
    pub fn kind(&self) -> CheckKind {
        self.kind
    }

    /// Kosulan komut.
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Kapi gecti mi?
    #[must_use]
    pub fn passed(&self) -> bool {
        self.passed
    }

    /// Cikti govdesinin CAS atfi.
    #[must_use]
    pub fn detail_ref(&self) -> Option<&str> {
        self.detail_ref.as_deref()
    }
}

/// Birinci kapi: otomatik dogrulama (build + test + sema/lint).
///
/// [`AutomaticVerification::collect`] uc kategorinin **hepsinin** sonucunu
/// ister; eksik kategori "gecti" sayilmaz, hata doner. Boylece testi
/// atlayarak `Done`'a ulasan bir yol kalmaz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutomaticVerification {
    checks: Vec<CheckOutcome>,
    recorded_at: Timestamp,
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
            recorded_at: now(),
        })
    }

    /// Tum kosumlar.
    #[must_use]
    pub fn checks(&self) -> &[CheckOutcome] {
        &self.checks
    }

    /// Kaydin alindigi an.
    #[must_use]
    pub fn recorded_at(&self) -> Timestamp {
        self.recorded_at
    }

    /// Basarisiz kosumlar.
    pub fn failures(&self) -> impl Iterator<Item = &CheckOutcome> {
        self.checks.iter().filter(|c| !c.passed)
    }

    /// Kapi gecti mi? Tum kosumlar basarili olmali.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.checks.iter().all(CheckOutcome::passed)
    }

    /// Kosum kumesinin icerik ozeti (hukum ozetinde kullanilir).
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

/// Yargicin degerlendirdigi tek iddia: **kanit-referansli** olmak zorunda
/// (3.4). Kanitsiz iddia kabul edilmez.
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
    pub fn new(claim: impl Into<String>, evidence_ref: impl Into<String>) -> Result<Self, OracleError> {
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

    /// Iddia metni.
    #[must_use]
    pub fn claim(&self) -> &str {
        &self.claim
    }

    /// Kanitin atfi (CAS ya da hashline capasi).
    #[must_use]
    pub fn evidence_ref(&self) -> &str {
        &self.evidence_ref
    }
}

/// Yargicin curuttugu iddia: karsi kanit atfi zorunludur.
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

    /// Curutulen iddia.
    #[must_use]
    pub fn claim(&self) -> &str {
        &self.claim
    }

    /// Karsi kanitin atfi.
    #[must_use]
    pub fn counter_evidence_ref(&self) -> &str {
        &self.counter_evidence_ref
    }
}

/// Ikinci kapi: yanlislamaci yargic hukmu (3.4).
///
/// Kapi "iddialar dogru kanitlandi" ile degil, **"curutulemedi"** ile gecer.
/// Bos iddia kumesi kabul edilmez: curutulecek bir sey yoksa yargic
/// calismamis demektir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FalsificationVerdict {
    judge_role: String,
    claims: Vec<EvidenceClaim>,
    refutations: Vec<Refutation>,
    ruled_at: Timestamp,
}

impl FalsificationVerdict {
    /// Yargic hukmunu kaydeder.
    ///
    /// `judge_role` bir MODEL adi degil, yonlendirme rolunun adidir (I5);
    /// rol->model esleme `config/routing.toml`'da yasar.
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
            ruled_at: now(),
        })
    }

    /// Yargicin yonlendirme rolu.
    #[must_use]
    pub fn judge_role(&self) -> &str {
        &self.judge_role
    }

    /// Degerlendirilen iddialar.
    #[must_use]
    pub fn claims(&self) -> &[EvidenceClaim] {
        &self.claims
    }

    /// Curutulen iddialar.
    #[must_use]
    pub fn refutations(&self) -> &[Refutation] {
        &self.refutations
    }

    /// Hukmun verildigi an.
    #[must_use]
    pub fn ruled_at(&self) -> Timestamp {
        self.ruled_at
    }

    /// Kapi gecti mi? Hicbir iddia curutulmemis olmali.
    #[must_use]
    pub fn upheld(&self) -> bool {
        self.refutations.is_empty()
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

/// Ucuncu kapi: kullanici onayi (K10 hibrit karar).
///
/// `capability_audit`/`Command::Approve` yolundan gelir. Onaylayan **insan**
/// olmak zorundadir: makine kimligi ([`is_machine_identity`]) reddedilir ki
/// sistem kendi kendini onaylayamasin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserSignOff {
    approver: String,
    decision: ApprovalDecision,
    note: Option<String>,
    signed_at: Timestamp,
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
            note: None,
            signed_at: now(),
        })
    }

    /// Serbest metin gerekce ekler.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Onaylayan.
    #[must_use]
    pub fn approver(&self) -> &str {
        &self.approver
    }

    /// Karar.
    #[must_use]
    pub fn decision(&self) -> ApprovalDecision {
        self.decision
    }

    /// Gerekce.
    #[must_use]
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// Kararin verildigi an.
    #[must_use]
    pub fn signed_at(&self) -> Timestamp {
        self.signed_at
    }

    /// Kapi gecti mi? Yalnizca `Allow` gecer.
    #[must_use]
    pub fn granted(&self) -> bool {
        matches!(self.decision, ApprovalDecision::Allow)
    }
}

/// Uc kapiyi toplayan karar noktasi (7.5).
///
/// Kapilar bagimsizdir ve herhangi bir sirada doldurulabilir; onemli olan
/// [`TerminationOracle::evaluate`] aninda ucunun de saglanmis olmasidir.
#[derive(Debug, Clone, Serialize)]
pub struct TerminationOracle {
    task_id: TaskId,
    verification: Option<AutomaticVerification>,
    verdict: Option<FalsificationVerdict>,
    sign_off: Option<UserSignOff>,
    subagents: BTreeSet<AgentId>,
}

impl TerminationOracle {
    /// Bir gorev icin bos oracle.
    #[must_use]
    pub fn new(task_id: TaskId) -> Self {
        Self {
            task_id,
            verification: None,
            verdict: None,
            sign_off: None,
            subagents: BTreeSet::new(),
        }
    }

    /// Gorev bitince toplanacak alt-ajani kaydeder (K11/R6).
    pub fn track_subagent(&mut self, agent_id: AgentId) {
        self.subagents.insert(agent_id);
    }

    /// Zincirlenebilir [`TerminationOracle::track_subagent`].
    #[must_use]
    pub fn with_subagent(mut self, agent_id: AgentId) -> Self {
        self.track_subagent(agent_id);
        self
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

    /// Hukum verilen gorev.
    #[must_use]
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    /// Izlenen alt-ajanlar.
    #[must_use]
    pub fn subagents(&self) -> Vec<AgentId> {
        self.subagents.iter().copied().collect()
    }

    /// Birinci kapi.
    #[must_use]
    pub fn verification(&self) -> Option<&AutomaticVerification> {
        self.verification.as_ref()
    }

    /// Ikinci kapi.
    #[must_use]
    pub fn verdict(&self) -> Option<&FalsificationVerdict> {
        self.verdict.as_ref()
    }

    /// Ucuncu kapi.
    #[must_use]
    pub fn sign_off(&self) -> Option<&UserSignOff> {
        self.sign_off.as_ref()
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
        if !self.verdict.as_ref().is_some_and(FalsificationVerdict::upheld) {
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
            self.subagents(),
            verification,
            verdict,
            sign_off,
        ))
    }

    /// [`TerminationOracle::evaluate`]'in `Result` donen hali.
    ///
    /// # Errors
    /// Kapilardan biri eksikse [`OracleError::GatesNotSatisfied`] doner.
    pub fn rule(&self) -> Result<DoneRuling, OracleError> {
        match self.evaluate() {
            OracleOutcome::Done(ruling) => Ok(ruling),
            OracleOutcome::Pending { missing } => Err(OracleError::GatesNotSatisfied { missing }),
        }
    }
}

/// Oracle degerlendirmesinin sonucu.
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

    /// Hukum, verildiyse.
    #[must_use]
    pub fn ruling(&self) -> Option<&DoneRuling> {
        match self {
            Self::Done(ruling) => Some(ruling),
            Self::Pending { .. } => None,
        }
    }
}

/// "Is bitti" hukmu (7.5).
///
/// Alanlari **private**'dir ve tek uretici [`TerminationOracle::evaluate`]
/// icindeki `seal`'dir; `Deserialize` bilerek turetilmez ki disaridan JSON ile
/// sahte hukum uretilemesin. Hukum olmadan ne alt-ajanlar toplanir ne de
/// butce kesilir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoneRuling {
    task_id: TaskId,
    reaped_agents: Vec<AgentId>,
    verification_digest: String,
    evidence_digest: String,
    approver: String,
    decided_at: Timestamp,
}

impl DoneRuling {
    /// Yalnizca modul icinden cagrilir; uc kapi zaten dogrulanmistir.
    fn seal(
        task_id: TaskId,
        reaped_agents: Vec<AgentId>,
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
            decided_at: now(),
        }
    }

    /// Biten gorev.
    #[must_use]
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    /// Toplanacak alt-ajanlar.
    #[must_use]
    pub fn reaped_agents(&self) -> &[AgentId] {
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

    /// Hukmun verildigi an.
    #[must_use]
    pub fn decided_at(&self) -> Timestamp {
        self.decided_at
    }

    /// Scheduler'a verilecek sonlanma plani: alt-ajanlari topla, butceyi kes.
    #[must_use]
    pub fn termination_plan(&self) -> TerminationPlan {
        TerminationPlan {
            task_id: self.task_id,
            reaped_agents: self.reaped_agents.clone(),
            freeze: BudgetFreeze {
                task_id: self.task_id,
                frozen_at: self.decided_at,
            },
            decided_at: self.decided_at,
        }
    }
}

/// Bitmis gorev icin butce kesimi (K11/R6).
///
/// Yalnizca [`DoneRuling::termination_plan`] uretir. Elinde bir `BudgetFreeze`
/// olan kod, harcamanin kesildigini **kanitlanmis** olarak bilir; sahte
/// uretilemez (private alanlar, `Deserialize` yok).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetFreeze {
    task_id: TaskId,
    frozen_at: Timestamp,
}

impl BudgetFreeze {
    /// Butcesi kesilen gorev.
    #[must_use]
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    /// Kesimin ani.
    #[must_use]
    pub fn frozen_at(&self) -> Timestamp {
        self.frozen_at
    }

    /// Bitmis ise kaynak yakmak yasak: her zaman `false`.
    #[must_use]
    pub fn allows_spend(&self) -> bool {
        false
    }
}

/// Sonlanma plani: scheduler bunu uygular (7.5 son paragraf).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminationPlan {
    /// `Done`'a alinacak gorev.
    pub task_id: TaskId,
    /// Toplanacak alt-ajanlar.
    pub reaped_agents: Vec<AgentId>,
    /// Butce kesimi.
    pub freeze: BudgetFreeze,
    /// Hukmun ani.
    pub decided_at: Timestamp,
}

/// Sonlanma oracle'i hatalari. Uretim yolunda panik yok (I6).
#[derive(Debug, thiserror::Error)]
pub enum OracleError {
    /// Zorunlu alan bos birakildi.
    #[error("'{field}' alani bos birakilamaz")]
    EmptyField {
        /// Bos kalan alanin adi.
        field: &'static str,
    },

    /// Otomatik dogrulamada bir kategori eksik.
    #[error("otomatik dogrulama eksik: '{kind}' kosumu yok")]
    MissingCheck {
        /// Eksik kategori.
        kind: CheckKind,
    },

    /// Yargica hicbir iddia sunulmamis.
    #[error("yargic hukmu bos iddia kumesiyle verilemez (3.4)")]
    NoClaims,

    /// Kullanici onayi yerine makine kimligi sunuldu.
    #[error("'{approver}' bir makine kimligi; kullanici onayi insandan gelmeli (K10)")]
    MachineApprover {
        /// Reddedilen ad.
        approver: String,
    },

    /// Uc kapinin hepsi saglanmadan hukum istendi.
    #[error("sonlanma kapilari eksik: {missing:?}")]
    GatesNotSatisfied {
        /// Eksik kapilar.
        missing: Vec<OracleGate>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gecen_dogrulama() -> AutomaticVerification {
        let checks = vec![
            CheckOutcome::new(CheckKind::Build, "cargo check -p omni-core", true)
                .expect("build kosumu"),
            CheckOutcome::new(CheckKind::Test, "cargo test -p omni-core", true).expect("test"),
            CheckOutcome::new(CheckKind::SchemaLint, "cargo clippy -p omni-core", true)
                .expect("lint"),
        ];
        AutomaticVerification::collect(checks).expect("dogrulama")
    }

    fn gecen_hukum() -> FalsificationVerdict {
        let claims = vec![
            EvidenceClaim::new("kapi testleri yesil", "cas:deadbeef").expect("iddia"),
        ];
        FalsificationVerdict::new("judge", claims, Vec::new()).expect("hukum")
    }

    fn kullanici_onayi() -> UserSignOff {
        UserSignOff::record("void0x14", ApprovalDecision::Allow).expect("onay")
    }

    #[test]
    fn eksik_kategori_dogrulamayi_reddeder() {
        let checks = vec![
            CheckOutcome::new(CheckKind::Build, "cargo check", true).expect("build"),
            CheckOutcome::new(CheckKind::Test, "cargo test", true).expect("test"),
        ];
        let sonuc = AutomaticVerification::collect(checks);
        assert!(matches!(
            sonuc,
            Err(OracleError::MissingCheck {
                kind: CheckKind::SchemaLint
            })
        ));
    }

    #[test]
    fn kanitsiz_iddia_reddedilir() {
        assert!(EvidenceClaim::new("bitti", "   ").is_err());
        assert!(EvidenceClaim::new("  ", "cas:1").is_err());
    }

    #[test]
    fn bos_iddia_kumesi_yargic_hukmu_sayilmaz() {
        let sonuc = FalsificationVerdict::new("judge", Vec::new(), Vec::new());
        assert!(matches!(sonuc, Err(OracleError::NoClaims)));
    }

    #[test]
    fn curutulen_iddia_kapiyi_kapatir() {
        let claims = vec![EvidenceClaim::new("hizli", "cas:1").expect("iddia")];
        let refs = vec![Refutation::new("hizli", "cas:2").expect("curutme")];
        let hukum = FalsificationVerdict::new("judge", claims, refs).expect("hukum");
        assert!(!hukum.upheld());
    }

    #[test]
    fn makine_kendini_onaylayamaz() {
        for ad in ["system", "AUTO", " omnitrix "] {
            let sonuc = UserSignOff::record(ad, ApprovalDecision::Allow);
            assert!(matches!(sonuc, Err(OracleError::MachineApprover { .. })), "{ad}");
        }
    }

    #[test]
    fn tek_kapi_done_uretmez() {
        let mut oracle = TerminationOracle::new(7);
        oracle.submit_verification(gecen_dogrulama());
        let sonuc = oracle.evaluate();
        assert!(!sonuc.is_done());
        let OracleOutcome::Pending { missing } = sonuc else {
            unreachable!("beklemede olmali")
        };
        assert_eq!(
            missing,
            vec![OracleGate::JudgeFalsification, OracleGate::UserSignOff]
        );
    }

    #[test]
    fn iki_kapi_done_uretmez() {
        let mut oracle = TerminationOracle::new(7);
        oracle.submit_verification(gecen_dogrulama());
        oracle.submit_verdict(gecen_hukum());
        assert!(!oracle.evaluate().is_done());
        assert!(oracle.rule().is_err());
    }

    #[test]
    fn basarisiz_build_kullanici_onayina_ragmen_kapali() {
        let checks = vec![
            CheckOutcome::new(CheckKind::Build, "cargo check", false).expect("build"),
            CheckOutcome::new(CheckKind::Test, "cargo test", true).expect("test"),
            CheckOutcome::new(CheckKind::SchemaLint, "cargo clippy", true).expect("lint"),
        ];
        let mut oracle = TerminationOracle::new(1);
        oracle.submit_verification(AutomaticVerification::collect(checks).expect("dogrulama"));
        oracle.submit_verdict(gecen_hukum());
        oracle.submit_sign_off(kullanici_onayi());
        assert_eq!(
            oracle.missing_gates(),
            vec![OracleGate::AutomaticVerification]
        );
        assert!(!oracle.evaluate().is_done());
    }

    #[test]
    fn kullanici_reddi_done_uretmez() {
        let mut oracle = TerminationOracle::new(1);
        oracle.submit_verification(gecen_dogrulama());
        oracle.submit_verdict(gecen_hukum());
        oracle.submit_sign_off(
            UserSignOff::record("void0x14", ApprovalDecision::Deny).expect("onay"),
        );
        assert_eq!(oracle.missing_gates(), vec![OracleGate::UserSignOff]);
    }

    #[test]
    fn uc_kapi_done_ve_butce_kesimi_uretir() {
        let mut oracle = TerminationOracle::new(42).with_subagent(3).with_subagent(5);
        oracle.submit_verification(gecen_dogrulama());
        oracle.submit_verdict(gecen_hukum());
        oracle.submit_sign_off(kullanici_onayi());

        let hukum = oracle.rule().expect("hukum");
        assert_eq!(hukum.task_id(), 42);
        assert_eq!(hukum.reaped_agents(), &[3, 5]);
        assert_eq!(hukum.approver(), "void0x14");
        assert!(!hukum.evidence_digest().is_empty());

        let plan = hukum.termination_plan();
        assert_eq!(plan.task_id, 42);
        assert_eq!(plan.reaped_agents, vec![3, 5]);
        assert!(!plan.freeze.allows_spend());
        assert_eq!(plan.freeze.task_id(), 42);
    }

    #[test]
    fn ozet_icerige_duyarli() {
        let a = gecen_dogrulama().digest();
        let checks = vec![
            CheckOutcome::new(CheckKind::Build, "cargo check -p omni-core", false).expect("build"),
            CheckOutcome::new(CheckKind::Test, "cargo test -p omni-core", true).expect("test"),
            CheckOutcome::new(CheckKind::SchemaLint, "cargo clippy -p omni-core", true)
                .expect("lint"),
        ];
        let b = AutomaticVerification::collect(checks)
            .expect("dogrulama")
            .digest();
        assert_ne!(a, b);
    }
}
