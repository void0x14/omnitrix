//! Self-hosting kapisi — Omnitrix kendi kodunu gelistirir, **ama kapili**
//! (MASTER-PLAN 17.4, AS11).
//!
//! Kapilar:
//!
//! 1. **Zorunlu gecici git branch** (K5, `xai-fast-worktree`): [`SelfModifyBranch`]
//!    ancak gecici branch adiyla kurulabilir; korumali dal adi (`main`,
//!    `master`, ...) tip duzeyinde reddedilir. Ana agacta dogrudan calisan bir
//!    oturum acilamaz.
//! 2. **Diff-stream** (5.2): her dokunus [`SelfModifySession::record_touch`] ile
//!    akisa duser; hicbir dokunus kaydedilmemisse terfi istenemez.
//! 3. **Yargic kapisi** (3.4): [`oracle::FalsificationVerdict`] curutulmemis
//!    olmali.
//! 4. **`capability_audit` kullanici onayi**: [`CapabilityGrant`] `self_modify`
//!    yetkisini **insan** onaylayanla tasimali (K3/K10).
//! 5. **Aday build**: [`CandidateBuild`] gecici branch'te uretilir ve otomatik
//!    dogrulamasi (build + test + sema/lint) yesildir. Calisan binary'ye
//!    dokunulmaz.
//!
//! **Otomatik merge yoktur.** Terfi emri ([`PromotionOrder`]) uretmenin tek
//! yolu [`SelfModifySession::promote`]'tur; o da yalnizca bir [`UserApproval`]
//! tokeni kabul eder. `UserApproval`'in kurucusu **private**'dir ve tek uretici
//! [`PromotionRequest::approve`]'dur; `PromotionRequest` de yalnizca
//! [`SelfModifySession::request_promotion`] tum kapilar gectiginde uretilir.
//! Zincirin hicbir halkasinda kisayol yoktur: onaysiz merge yapan bir kod yolu
//! **derlenemez**. Token ve emir bilerek `Deserialize` turetmez ki JSON ile
//! sahtesi uretilemesin, `Clone` turetmez ki tek kullanimlik kalsin.
//!
//! Geri-alma = git: gecici branch atilir, ana agac bozulmaz (R7).
//!
//! Bu dosyada I/O yoktur. Git islemleri, build kosumu ve terfi eylemini
//! cagiran katman yapar; burasi yalnizca **kimin neye izni var** sorusunu
//! deterministik olarak cevaplar. Yol dogrulamasi da saf/sozdizimseldir;
//! diskle konusan katman gercek yolu `dunce::canonicalize` ile normalize
//! etmelidir (`Path::canonicalize` kullanilmaz).

use std::fmt;
use std::path::{Component, Path};

use omni_proto::{AgentId, ApprovalDecision, FileTouch, TaskId, Timestamp, now};
use serde::{Deserialize, Serialize};

use crate::oracle::{AutomaticVerification, FalsificationVerdict, OracleError, validate_human_approver};

/// `capability_audit.capability` degeri: kendi kodunu degistirme yetkisi
/// (17.4). Bu yetki her zaman kullanici onayi gerektirir.
pub const CAPABILITY_SELF_MODIFY: &str = "self_modify";

/// Zorunlu gecici branch on eki (K5, `xai-fast-worktree`).
///
/// Self-modify oturumu yalnizca bu on ekle baslayan bir dalda acilabilir.
pub const SELF_MODIFY_BRANCH_PREFIX: &str = "xai-fast-worktree";

/// Asla dogrudan uzerinde calisilmayacak dallar.
const PROTECTED_BRANCHES: [&str; 8] = [
    "main",
    "master",
    "head",
    "trunk",
    "develop",
    "release",
    "stable",
    "masterplan",
];

/// Zorunlu gecici izolasyon branch'i (K5/AS5).
///
/// Kurucu, adi dogrular: [`SELF_MODIFY_BRANCH_PREFIX`] on eki zorunlu, korumali
/// dal adi yasak, yol/kabuk hilesi olusturacak karakterler yasak. Boylece
/// "ana agacta dogrudan self-modify" bir tip hatasi haline gelir.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SelfModifyBranch {
    name: String,
}

impl SelfModifyBranch {
    /// Verilen slug icin gecici branch adi uretir:
    /// `xai-fast-worktree/<slug>`.
    ///
    /// # Errors
    /// Slug bos, korumali ad ya da gecersiz karakter iceriyorsa
    /// [`SelfHostError`] doner.
    pub fn temporary(slug: &str) -> Result<Self, SelfHostError> {
        let slug = slug.trim();
        validate_branch_segment(slug)?;
        Self::adopt(&format!("{SELF_MODIFY_BRANCH_PREFIX}/{slug}"))
    }

    /// Git'ten gelen mevcut bir dal adini gecici branch olarak kabul eder.
    ///
    /// # Errors
    /// Ad on eki tasimiyorsa [`SelfHostError::NotATemporaryBranch`], korumali
    /// dal ise [`SelfHostError::ProtectedBranch`] doner.
    pub fn adopt(name: &str) -> Result<Self, SelfHostError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(SelfHostError::EmptyField {
                field: "branch.name",
            });
        }
        if is_protected_branch(name) {
            return Err(SelfHostError::ProtectedBranch {
                name: name.to_owned(),
            });
        }
        let Some(rest) = name.strip_prefix(SELF_MODIFY_BRANCH_PREFIX) else {
            return Err(SelfHostError::NotATemporaryBranch {
                name: name.to_owned(),
            });
        };
        // On ekten sonrasi ya bos ya da '/' ile ayrilmis olmali; "xai-fast-worktreeX"
        // gibi benzer isimler kabul edilmez.
        let suffix = match rest.strip_prefix('/') {
            Some(suffix) => {
                validate_branch_segment(suffix)?;
                suffix
            }
            None if rest.is_empty() => rest,
            None => {
                return Err(SelfHostError::NotATemporaryBranch {
                    name: name.to_owned(),
                });
            }
        };
        if is_protected_branch(suffix) {
            return Err(SelfHostError::ProtectedBranch {
                name: suffix.to_owned(),
            });
        }
        Ok(Self {
            name: name.to_owned(),
        })
    }

    /// Dal adi.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for SelfModifyBranch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// Dal adi korumali mi?
fn is_protected_branch(name: &str) -> bool {
    let ad = name.trim().trim_matches('/').to_ascii_lowercase();
    PROTECTED_BRANCHES.contains(&ad.as_str())
}

/// Dal adi parcasini dogrular (git ref kurallarinin muhafazakar alt kumesi).
fn validate_branch_segment(segment: &str) -> Result<(), SelfHostError> {
    if segment.is_empty() {
        return Err(SelfHostError::EmptyField {
            field: "branch.segment",
        });
    }
    let gecersiz = segment.starts_with('-')
        || segment.starts_with('/')
        || segment.ends_with('/')
        || segment.ends_with(".lock")
        || segment.contains("..")
        || segment.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\' | '"' | '\'' | '@' | '$')
        });
    if gecersiz {
        return Err(SelfHostError::InvalidBranchName {
            name: segment.to_owned(),
        });
    }
    Ok(())
}

/// Self-modify niyeti: kim, hangi gorev icin, neden, hangi dosyalar.
///
/// Yollar **calisma alanina goreli** olmak zorundadir; mutlak yol ya da `..`
/// iceren yol reddedilir (agactan cikis yok).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfModifyRequest {
    agent_id: AgentId,
    task_id: TaskId,
    rationale: String,
    paths: Vec<String>,
}

impl SelfModifyRequest {
    /// Yeni niyet.
    ///
    /// # Errors
    /// Gerekce bos ise [`SelfHostError::EmptyField`] doner — self-modify
    /// gerekcesiz istenemez (`capability_audit` denetlenebilir olmali).
    pub fn new(
        agent_id: AgentId,
        task_id: TaskId,
        rationale: impl Into<String>,
    ) -> Result<Self, SelfHostError> {
        let rationale = rationale.into();
        if rationale.trim().is_empty() {
            return Err(SelfHostError::EmptyField {
                field: "request.rationale",
            });
        }
        Ok(Self {
            agent_id,
            task_id,
            rationale,
            paths: Vec::new(),
        })
    }

    /// Dokunulacak yolu ekler.
    ///
    /// # Errors
    /// Yol mutlaksa, `..` iceriyorsa ya da bos ise
    /// [`SelfHostError::UnsafePath`] doner.
    pub fn touching(mut self, path: impl AsRef<str>) -> Result<Self, SelfHostError> {
        let path = validate_workspace_path(path.as_ref())?;
        if !self.paths.contains(&path) {
            self.paths.push(path);
        }
        Ok(self)
    }

    /// Istegi acan ajan.
    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    /// Istegin bagli oldugu gorev.
    #[must_use]
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    /// Gerekce.
    #[must_use]
    pub fn rationale(&self) -> &str {
        &self.rationale
    }

    /// Bildirilen yollar.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        &self.paths
    }
}

/// Calisma alanina goreli yol dogrulamasi (saf/sozdizimsel).
///
/// Diske dokunmaz: `Path::canonicalize` yasaktir, gercek normalizasyon I/O
/// katmaninda `dunce::canonicalize` ile yapilir. Burada agactan cikaran
/// sozdizimsel bicimler reddedilir.
fn validate_workspace_path(path: &str) -> Result<String, SelfHostError> {
    let ham = path.trim();
    if ham.is_empty() {
        return Err(SelfHostError::EmptyField {
            field: "request.path",
        });
    }
    if ham.contains('\0') {
        return Err(SelfHostError::UnsafePath {
            path: ham.to_owned(),
            reason: "NUL karakteri",
        });
    }
    let p = Path::new(ham);
    for parca in p.components() {
        match parca {
            Component::Prefix(_) | Component::RootDir => {
                return Err(SelfHostError::UnsafePath {
                    path: ham.to_owned(),
                    reason: "mutlak yol",
                });
            }
            Component::ParentDir => {
                return Err(SelfHostError::UnsafePath {
                    path: ham.to_owned(),
                    reason: "'..' ile agac disina cikis",
                });
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(ham.to_owned())
}

/// `capability_audit` satirinin tip duzeyindeki karsiligi (K3).
///
/// Yalnizca `self_modify` yetkisi icin kurulur ve onaylayan **insan** olmak
/// zorundadir; makine kimligi [`crate::oracle::is_machine_identity`] ile
/// reddedilir, boylece ajan kendi self-modify iznini yazamaz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityGrant {
    agent_id: AgentId,
    capability: String,
    target: String,
    decision: ApprovalDecision,
    approver: String,
    ts: Timestamp,
}

impl CapabilityGrant {
    /// `capability_audit` kaydindan `self_modify` yetkisi kurar.
    ///
    /// # Errors
    /// Hedef bos ise, onaylayan bos ya da makine kimligi ise hata doner.
    pub fn self_modify(
        agent_id: AgentId,
        target: impl Into<String>,
        decision: ApprovalDecision,
        approver: impl AsRef<str>,
    ) -> Result<Self, SelfHostError> {
        let target = target.into();
        if target.trim().is_empty() {
            return Err(SelfHostError::EmptyField {
                field: "grant.target",
            });
        }
        let approver = validate_human_approver(approver.as_ref())?;
        Ok(Self {
            agent_id,
            capability: CAPABILITY_SELF_MODIFY.to_owned(),
            target,
            decision,
            approver,
            ts: now(),
        })
    }

    /// Yetkiyi isteyen ajan.
    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    /// `capability_audit.capability`.
    #[must_use]
    pub fn capability(&self) -> &str {
        &self.capability
    }

    /// `capability_audit.target`.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// `capability_audit.decision`.
    #[must_use]
    pub fn decision(&self) -> ApprovalDecision {
        self.decision
    }

    /// `capability_audit.approver`.
    #[must_use]
    pub fn approver(&self) -> &str {
        &self.approver
    }

    /// Kaydin ani.
    #[must_use]
    pub fn ts(&self) -> Timestamp {
        self.ts
    }

    /// Yetki verildi mi?
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self.decision, ApprovalDecision::Allow)
            && self.capability == CAPABILITY_SELF_MODIFY
    }
}

/// Gecici branch'te uretilen **aday** build (17.4).
///
/// Aday build calisan binary'nin yerine gecmez; yalnizca kullanici terfi
/// ederse devreye girer. Otomatik dogrulamasi yesil degilse terfi istenemez.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateBuild {
    branch: String,
    artifact_ref: String,
    commit: Option<String>,
    verification: AutomaticVerification,
    built_at: Timestamp,
}

impl CandidateBuild {
    /// Aday build kaydi.
    ///
    /// # Errors
    /// Artefakt atfi bos ise hata doner.
    pub fn new(
        branch: &SelfModifyBranch,
        artifact_ref: impl Into<String>,
        verification: AutomaticVerification,
    ) -> Result<Self, SelfHostError> {
        let artifact_ref = artifact_ref.into();
        if artifact_ref.trim().is_empty() {
            return Err(SelfHostError::EmptyField {
                field: "candidate.artifact_ref",
            });
        }
        Ok(Self {
            branch: branch.name().to_owned(),
            artifact_ref,
            commit: None,
            verification,
            built_at: now(),
        })
    }

    /// Aday build'in uzerinde durdugu commit.
    #[must_use]
    pub fn with_commit(mut self, commit: impl Into<String>) -> Self {
        self.commit = Some(commit.into());
        self
    }

    /// Build'in uretildigi gecici dal.
    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Artefakt atfi (CAS ya da dosya yolu).
    #[must_use]
    pub fn artifact_ref(&self) -> &str {
        &self.artifact_ref
    }

    /// Commit kimligi.
    #[must_use]
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    /// Otomatik dogrulama sonucu.
    #[must_use]
    pub fn verification(&self) -> &AutomaticVerification {
        &self.verification
    }

    /// Build'in ani.
    #[must_use]
    pub fn built_at(&self) -> Timestamp {
        self.built_at
    }

    /// Aday terfiye uygun mu? Otomatik dogrulama yesil olmali.
    #[must_use]
    pub fn is_promotable(&self) -> bool {
        self.verification.passed()
    }

    /// Adayin kimligi: dal + artefakt + commit ozeti.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(self.branch.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.artifact_ref.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.commit.as_deref().unwrap_or("").as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.verification.digest().as_bytes());
        hasher.finalize().to_hex().to_string()
    }
}

/// Terfiden once saglanmasi gereken kapilar (17.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfHostGate {
    /// Diff akisina en az bir dokunus dusmus olmali (5.2).
    DiffStream,
    /// Aday build uretilmis ve otomatik dogrulamasi yesil olmali.
    CandidateBuild,
    /// Yanlislamaci yargic hukmu curutulememis olmali (3.4).
    JudgeVerdict,
    /// `capability_audit`'te kullanici onayli `self_modify` yetkisi olmali.
    CapabilityGrant,
}

impl SelfHostGate {
    /// Kapilarin kanonik sirasi.
    pub const ALL: [Self; 4] = [
        Self::DiffStream,
        Self::CandidateBuild,
        Self::JudgeVerdict,
        Self::CapabilityGrant,
    ];

    /// Kayit/denetim icin kanonik metin.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::DiffStream => "diff_stream",
            Self::CandidateBuild => "candidate_build",
            Self::JudgeVerdict => "judge_verdict",
            Self::CapabilityGrant => "capability_grant",
        }
    }
}

impl fmt::Display for SelfHostGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// Kapili self-modify oturumu (17.4).
///
/// Oturum **her zaman** bir gecici branch'e baglidir; ana agac adiyla
/// [`SelfModifyBranch`] kurulamadigi icin ana agacta oturum acmak mumkun
/// degildir.
#[derive(Debug, Clone, Serialize)]
pub struct SelfModifySession {
    id: String,
    request: SelfModifyRequest,
    branch: SelfModifyBranch,
    touches: Vec<FileTouch>,
    verdict: Option<FalsificationVerdict>,
    grant: Option<CapabilityGrant>,
    candidate: Option<CandidateBuild>,
    opened_at: Timestamp,
    promoted: bool,
}

impl SelfModifySession {
    /// Gecici branch uzerinde oturum acar.
    #[must_use]
    pub fn open(request: SelfModifyRequest, branch: SelfModifyBranch) -> Self {
        let opened_at = now();
        let mut hasher = blake3::Hasher::new();
        hasher.update(branch.name().as_bytes());
        hasher.update(b"\x1f");
        hasher.update(request.agent_id.to_string().as_bytes());
        hasher.update(b"\x1f");
        hasher.update(request.task_id.to_string().as_bytes());
        hasher.update(b"\x1f");
        hasher.update(request.rationale.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(opened_at.to_rfc3339().as_bytes());
        let id = hasher.finalize().to_hex().to_string();
        Self {
            id,
            request,
            branch,
            touches: Vec::new(),
            verdict: None,
            grant: None,
            candidate: None,
            opened_at,
            promoted: false,
        }
    }

    /// Oturum kimligi (onay tokeni bu kimlige baglanir).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Niyet.
    #[must_use]
    pub fn request(&self) -> &SelfModifyRequest {
        &self.request
    }

    /// Calisilan gecici dal.
    #[must_use]
    pub fn branch(&self) -> &SelfModifyBranch {
        &self.branch
    }

    /// Diff akisina dusen dokunuslar (5.2).
    #[must_use]
    pub fn touches(&self) -> &[FileTouch] {
        &self.touches
    }

    /// Oturumun acildigi an.
    #[must_use]
    pub fn opened_at(&self) -> Timestamp {
        self.opened_at
    }

    /// Terfi emri daha once uretildi mi?
    #[must_use]
    pub fn is_promoted(&self) -> bool {
        self.promoted
    }

    /// Diff akisina bir dokunus kaydeder (5.2 gorunurluk).
    ///
    /// # Errors
    /// Dokunus baska bir ajana aitse ya da calisma alani disina cikiyorsa
    /// reddedilir; kayit dismissa ugramaz, hata doner.
    pub fn record_touch(&mut self, touch: FileTouch) -> Result<(), SelfHostError> {
        if touch.agent_id != self.request.agent_id {
            return Err(SelfHostError::ForeignTouch {
                expected: self.request.agent_id,
                found: touch.agent_id,
            });
        }
        if touch.outside_workspace {
            return Err(SelfHostError::UnsafePath {
                path: touch.path.clone(),
                reason: "calisma alani disi dokunus",
            });
        }
        validate_workspace_path(&touch.path)?;
        self.touches.push(touch);
        Ok(())
    }

    /// Yargic hukmunu baglar (3.4).
    pub fn attach_verdict(&mut self, verdict: FalsificationVerdict) {
        self.verdict = Some(verdict);
    }

    /// `capability_audit` yetkisini baglar (K3).
    ///
    /// # Errors
    /// Yetki baska bir ajana aitse [`SelfHostError::ForeignGrant`] doner.
    pub fn attach_grant(&mut self, grant: CapabilityGrant) -> Result<(), SelfHostError> {
        if grant.agent_id != self.request.agent_id {
            return Err(SelfHostError::ForeignGrant {
                expected: self.request.agent_id,
                found: grant.agent_id,
            });
        }
        self.grant = Some(grant);
        Ok(())
    }

    /// Aday build'i baglar.
    ///
    /// # Errors
    /// Aday baska bir dalda uretildiyse [`SelfHostError::BranchMismatch`]
    /// doner — ana agacta uretilmis bir aday oturuma giremez.
    pub fn attach_candidate(&mut self, candidate: CandidateBuild) -> Result<(), SelfHostError> {
        if candidate.branch() != self.branch.name() {
            return Err(SelfHostError::BranchMismatch {
                expected: self.branch.name().to_owned(),
                found: candidate.branch().to_owned(),
            });
        }
        self.candidate = Some(candidate);
        Ok(())
    }

    /// Yargic hukmu.
    #[must_use]
    pub fn verdict(&self) -> Option<&FalsificationVerdict> {
        self.verdict.as_ref()
    }

    /// `capability_audit` yetkisi.
    #[must_use]
    pub fn grant(&self) -> Option<&CapabilityGrant> {
        self.grant.as_ref()
    }

    /// Aday build.
    #[must_use]
    pub fn candidate(&self) -> Option<&CandidateBuild> {
        self.candidate.as_ref()
    }

    /// Kullanici onayi istemeden once eksik olan kapilar.
    #[must_use]
    pub fn missing_gates(&self) -> Vec<SelfHostGate> {
        let mut eksik = Vec::new();
        if self.touches.is_empty() {
            eksik.push(SelfHostGate::DiffStream);
        }
        if !self
            .candidate
            .as_ref()
            .is_some_and(CandidateBuild::is_promotable)
        {
            eksik.push(SelfHostGate::CandidateBuild);
        }
        if !self.verdict.as_ref().is_some_and(FalsificationVerdict::upheld) {
            eksik.push(SelfHostGate::JudgeVerdict);
        }
        if !self.grant.as_ref().is_some_and(CapabilityGrant::is_allowed) {
            eksik.push(SelfHostGate::CapabilityGrant);
        }
        eksik
    }

    /// Kullanici onayina sunulacak ozeti uretir.
    ///
    /// Bu, [`UserApproval`] uretmenin **tek** giris kapisidir ve yalnizca tum
    /// kapilar gectiginde acilir.
    ///
    /// # Errors
    /// Kapilardan biri eksikse [`SelfHostError::GatesNotSatisfied`], oturum
    /// zaten terfi ettiyse [`SelfHostError::AlreadyPromoted`] doner.
    pub fn request_promotion(&self) -> Result<PromotionRequest<'_>, SelfHostError> {
        if self.promoted {
            return Err(SelfHostError::AlreadyPromoted {
                session_id: self.id.clone(),
            });
        }
        let eksik = self.missing_gates();
        if !eksik.is_empty() {
            return Err(SelfHostError::GatesNotSatisfied { missing: eksik });
        }
        let Some(candidate) = self.candidate.as_ref() else {
            return Err(SelfHostError::GatesNotSatisfied {
                missing: vec![SelfHostGate::CandidateBuild],
            });
        };
        Ok(PromotionRequest {
            session_id: &self.id,
            branch: &self.branch,
            candidate,
            touches: &self.touches,
        })
    }

    /// Aday build'i terfi ettirir — **yalnizca** kullanici onay tokeni ile.
    ///
    /// Bu fonksiyon [`PromotionOrder`] ureten tek yoldur; emir de calisan
    /// binary'yi degistirmeye yetkili tek belgedir. Token oturuma ve aday
    /// parmak izine baglidir: baska bir oturumun ya da baska bir adayin onayi
    /// buraya gecmez.
    ///
    /// # Errors
    /// Token baska oturuma/adaya aitse, oturum zaten terfi ettiyse ya da bu
    /// arada bir kapi kapandiysa hata doner.
    pub fn promote(&mut self, approval: UserApproval) -> Result<PromotionOrder, SelfHostError> {
        if self.promoted {
            return Err(SelfHostError::AlreadyPromoted {
                session_id: self.id.clone(),
            });
        }
        if approval.session_id != self.id {
            return Err(SelfHostError::ApprovalMismatch {
                expected: self.id.clone(),
                found: approval.session_id,
            });
        }
        let eksik = self.missing_gates();
        if !eksik.is_empty() {
            return Err(SelfHostError::GatesNotSatisfied { missing: eksik });
        }
        let Some(candidate) = self.candidate.as_ref() else {
            return Err(SelfHostError::GatesNotSatisfied {
                missing: vec![SelfHostGate::CandidateBuild],
            });
        };
        let fingerprint = candidate.fingerprint();
        if approval.candidate_fingerprint != fingerprint {
            return Err(SelfHostError::ApprovalMismatch {
                expected: fingerprint,
                found: approval.candidate_fingerprint,
            });
        }
        self.promoted = true;
        Ok(PromotionOrder {
            session_id: self.id.clone(),
            branch: self.branch.name().to_owned(),
            artifact_ref: candidate.artifact_ref().to_owned(),
            commit: candidate.commit().map(str::to_owned),
            candidate_fingerprint: fingerprint,
            approver: approval.approver,
            approved_at: approval.granted_at,
            issued_at: now(),
        })
    }

    /// Oturumu terfi etmeden kapatir: gecici dal atilir, ana agac bozulmaz
    /// (R7 geri-alma).
    #[must_use]
    pub fn abandon(self, reason: impl Into<String>) -> AbandonedSession {
        AbandonedSession {
            session_id: self.id,
            branch: self.branch.name().to_owned(),
            reason: reason.into(),
            abandoned_at: now(),
        }
    }
}

/// Kullaniciya sunulan terfi ozeti.
///
/// Tek isi onay/red almaktir. `approve` disinda [`UserApproval`] ureten baska
/// bir yol yoktur.
#[derive(Debug)]
pub struct PromotionRequest<'a> {
    session_id: &'a str,
    branch: &'a SelfModifyBranch,
    candidate: &'a CandidateBuild,
    touches: &'a [FileTouch],
}

impl PromotionRequest<'_> {
    /// Onay bekleyen oturum.
    #[must_use]
    pub fn session_id(&self) -> &str {
        self.session_id
    }

    /// Gecici dal.
    #[must_use]
    pub fn branch(&self) -> &SelfModifyBranch {
        self.branch
    }

    /// Terfi edilecek aday.
    #[must_use]
    pub fn candidate(&self) -> &CandidateBuild {
        self.candidate
    }

    /// Kullaniciya gosterilecek diff akisi (5.2).
    #[must_use]
    pub fn touches(&self) -> &[FileTouch] {
        self.touches
    }

    /// Diff akisinin net satir degisimi (ozet gosterim icin).
    #[must_use]
    pub fn net_lines(&self) -> i64 {
        self.touches.iter().map(FileTouch::net_lines).sum()
    }

    /// Kullanici onay verir.
    ///
    /// # Errors
    /// Onaylayan bos ya da makine kimligi ise hata doner: sistem kendini
    /// terfi ettiremez (17.4 "kullanici terfi eder").
    pub fn approve(self, approver: impl AsRef<str>) -> Result<UserApproval, SelfHostError> {
        let approver = validate_human_approver(approver.as_ref())?;
        Ok(UserApproval {
            session_id: self.session_id.to_owned(),
            candidate_fingerprint: self.candidate.fingerprint(),
            approver,
            granted_at: now(),
        })
    }

    /// Kullanici reddeder; token uretilmez.
    #[must_use]
    pub fn reject(self, reason: impl Into<String>) -> PromotionRejected {
        PromotionRejected {
            session_id: self.session_id.to_owned(),
            reason: reason.into(),
            rejected_at: now(),
        }
    }
}

/// Kullanici onay tokeni — terfi kapisinin anahtari.
///
/// Alanlari private, kurucusu yok: tek uretici [`PromotionRequest::approve`].
/// `Clone`/`Copy` yok (tek kullanimlik), `Deserialize` yok (JSON ile sahtesi
/// uretilemez). Bu tokeni isteyen tek fonksiyon [`SelfModifySession::promote`]
/// oldugu icin, onaysiz merge yapan bir kod yolu **var olamaz**.
#[derive(Debug, Serialize)]
pub struct UserApproval {
    session_id: String,
    candidate_fingerprint: String,
    approver: String,
    granted_at: Timestamp,
}

impl UserApproval {
    /// Onayin bagli oldugu oturum.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Onaylanan adayin parmak izi.
    #[must_use]
    pub fn candidate_fingerprint(&self) -> &str {
        &self.candidate_fingerprint
    }

    /// Onaylayan kullanici.
    #[must_use]
    pub fn approver(&self) -> &str {
        &self.approver
    }

    /// Onay ani.
    #[must_use]
    pub fn granted_at(&self) -> Timestamp {
        self.granted_at
    }
}

/// Kullanici reddi kaydi (denetim icin).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromotionRejected {
    /// Reddedilen oturum.
    pub session_id: String,
    /// Gerekce.
    pub reason: String,
    /// Reddin ani.
    pub rejected_at: Timestamp,
}

/// Terfi emri: calisan binary'yi degistirmeye yetkili tek belge.
///
/// Yalnizca [`SelfModifySession::promote`] uretir; kurucusu private,
/// `Deserialize` turetilmez. Emri alan katman (bootstrap/omni-tools) git
/// merge + binary takasini yapar; geri-alma yine git'tir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromotionOrder {
    session_id: String,
    branch: String,
    artifact_ref: String,
    commit: Option<String>,
    candidate_fingerprint: String,
    approver: String,
    approved_at: Timestamp,
    issued_at: Timestamp,
}

impl PromotionOrder {
    /// Terfi eden oturum.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Kaynak gecici dal.
    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Terfi edilecek aday artefakt.
    #[must_use]
    pub fn artifact_ref(&self) -> &str {
        &self.artifact_ref
    }

    /// Adayin commit'i.
    #[must_use]
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    /// Adayin parmak izi.
    #[must_use]
    pub fn candidate_fingerprint(&self) -> &str {
        &self.candidate_fingerprint
    }

    /// Terfiyi onaylayan kullanici.
    #[must_use]
    pub fn approver(&self) -> &str {
        &self.approver
    }

    /// Onay ani.
    #[must_use]
    pub fn approved_at(&self) -> Timestamp {
        self.approved_at
    }

    /// Emrin uretildigi an.
    #[must_use]
    pub fn issued_at(&self) -> Timestamp {
        self.issued_at
    }
}

/// Terfi etmeden kapatilan oturum: gecici dal atilir (R7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AbandonedSession {
    /// Kapatilan oturum.
    pub session_id: String,
    /// Atilacak gecici dal.
    pub branch: String,
    /// Gerekce.
    pub reason: String,
    /// Kapanis ani.
    pub abandoned_at: Timestamp,
}

/// Self-hosting kapisi hatalari. Uretim yolunda panik yok (I6).
#[derive(Debug, thiserror::Error)]
pub enum SelfHostError {
    /// Zorunlu alan bos birakildi.
    #[error("'{field}' alani bos birakilamaz")]
    EmptyField {
        /// Bos kalan alanin adi.
        field: &'static str,
    },

    /// Korumali dal uzerinde self-modify denendi.
    #[error("'{name}' korumali dal; self-modify ana agacta calisamaz (K5/17.4)")]
    ProtectedBranch {
        /// Reddedilen dal adi.
        name: String,
    },

    /// Dal adi zorunlu gecici on eki tasimiyor.
    #[error("'{name}' gecici branch degil; 'xai-fast-worktree/' on eki zorunlu (K5)")]
    NotATemporaryBranch {
        /// Reddedilen dal adi.
        name: String,
    },

    /// Dal adi git ref kurallarina uymuyor.
    #[error("gecersiz dal adi: '{name}'")]
    InvalidBranchName {
        /// Reddedilen dal adi.
        name: String,
    },

    /// Calisma alani disina cikan ya da tehlikeli yol.
    #[error("guvensiz yol '{path}': {reason}")]
    UnsafePath {
        /// Reddedilen yol.
        path: String,
        /// Gerekce.
        reason: &'static str,
    },

    /// Dokunus baska bir ajana ait.
    #[error("dokunus ajan {found}'a ait; oturum ajan {expected} icin acildi")]
    ForeignTouch {
        /// Oturumun ajani.
        expected: AgentId,
        /// Dokunusun ajani.
        found: AgentId,
    },

    /// Yetki kaydi baska bir ajana ait.
    #[error("yetki ajan {found}'a ait; oturum ajan {expected} icin acildi")]
    ForeignGrant {
        /// Oturumun ajani.
        expected: AgentId,
        /// Yetkinin ajani.
        found: AgentId,
    },

    /// Aday build baska bir dalda uretilmis.
    #[error("aday build '{found}' dalinda; oturum '{expected}' dalinda")]
    BranchMismatch {
        /// Oturumun dali.
        expected: String,
        /// Adayin dali.
        found: String,
    },

    /// Kapilar tamamlanmadan terfi istendi.
    #[error("self-modify kapilari eksik: {missing:?}")]
    GatesNotSatisfied {
        /// Eksik kapilar.
        missing: Vec<SelfHostGate>,
    },

    /// Onay tokeni bu oturuma/adaya ait degil.
    #[error("onay tokeni eslesmiyor (beklenen '{expected}', gelen '{found}')")]
    ApprovalMismatch {
        /// Beklenen kimlik/parmak izi.
        expected: String,
        /// Tokendeki deger.
        found: String,
    },

    /// Oturum zaten terfi etmis; tekrar terfi yok.
    #[error("oturum '{session_id}' zaten terfi etti")]
    AlreadyPromoted {
        /// Oturum kimligi.
        session_id: String,
    },

    /// Oracle katmanindan gelen dogrulama hatasi (onaylayan kimligi vb.).
    #[error(transparent)]
    Oracle(#[from] OracleError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::{CheckKind, CheckOutcome, EvidenceClaim, Refutation};

    fn dal() -> SelfModifyBranch {
        SelfModifyBranch::temporary("faz10-selfhost").expect("gecici dal")
    }

    fn istek() -> SelfModifyRequest {
        SelfModifyRequest::new(9, 4, "oracle kapisini sikilastir")
            .expect("istek")
            .touching("crates/omni/omni-core/src/oracle.rs")
            .expect("yol")
    }

    fn dogrulama(passed: bool) -> AutomaticVerification {
        let checks = vec![
            CheckOutcome::new(CheckKind::Build, "cargo check -p omni-core", passed)
                .expect("build"),
            CheckOutcome::new(CheckKind::Test, "cargo test -p omni-core", true).expect("test"),
            CheckOutcome::new(CheckKind::SchemaLint, "cargo clippy -p omni-core", true)
                .expect("lint"),
        ];
        AutomaticVerification::collect(checks).expect("dogrulama")
    }

    fn hukum(upheld: bool) -> FalsificationVerdict {
        let claims = vec![EvidenceClaim::new("kapi kapali", "cas:abc").expect("iddia")];
        let refs = if upheld {
            Vec::new()
        } else {
            vec![Refutation::new("kapi kapali", "cas:def").expect("curutme")]
        };
        FalsificationVerdict::new("judge", claims, refs).expect("hukum")
    }

    fn yetki() -> CapabilityGrant {
        CapabilityGrant::self_modify(9, "crates/omni", ApprovalDecision::Allow, "void0x14")
            .expect("yetki")
    }

    fn dokunus() -> FileTouch {
        FileTouch {
            id: None,
            agent_id: 9,
            path: "crates/omni/omni-core/src/oracle.rs".to_owned(),
            outside_workspace: false,
            added: 40,
            removed: 3,
            pre_ref: Some("cas:eski".to_owned()),
            post_ref: Some("cas:yeni".to_owned()),
            ts: now(),
        }
    }

    fn hazir_oturum() -> SelfModifySession {
        let mut oturum = SelfModifySession::open(istek(), dal());
        oturum.record_touch(dokunus()).expect("dokunus");
        oturum.attach_verdict(hukum(true));
        oturum.attach_grant(yetki()).expect("yetki");
        let aday = CandidateBuild::new(oturum.branch(), "cas:aday-binary", dogrulama(true))
            .expect("aday")
            .with_commit("abc123");
        oturum.attach_candidate(aday).expect("aday");
        oturum
    }

    #[test]
    fn ana_agac_dallari_reddedilir() {
        for ad in ["main", "master", "HEAD", "masterplan", "develop"] {
            assert!(
                SelfModifyBranch::adopt(ad).is_err(),
                "'{ad}' kabul edilmemeliydi"
            );
            assert!(SelfModifyBranch::temporary(ad).is_err(), "{ad}");
        }
    }

    #[test]
    fn on_eksiz_dal_reddedilir() {
        assert!(matches!(
            SelfModifyBranch::adopt("feature/x"),
            Err(SelfHostError::NotATemporaryBranch { .. })
        ));
        // On eke benzeyen ama ayrilmamis ad da gecmez.
        assert!(SelfModifyBranch::adopt("xai-fast-worktreeX").is_err());
    }

    #[test]
    fn gecici_dal_kabul_edilir() {
        let b = SelfModifyBranch::temporary("faz10").expect("dal");
        assert_eq!(b.name(), "xai-fast-worktree/faz10");
        assert!(SelfModifyBranch::adopt("xai-fast-worktree/faz10").is_ok());
    }

    #[test]
    fn agac_disi_yol_reddedilir() {
        let istek = SelfModifyRequest::new(1, 1, "gerekce").expect("istek");
        assert!(istek.clone().touching("/etc/passwd").is_err());
        assert!(istek.clone().touching("../../gizli").is_err());
        assert!(istek.touching("crates/omni/x.rs").is_ok());
    }

    #[test]
    fn gerekcesiz_istek_reddedilir() {
        assert!(SelfModifyRequest::new(1, 1, "   ").is_err());
    }

    #[test]
    fn makine_kendine_self_modify_yazamaz() {
        let sonuc =
            CapabilityGrant::self_modify(1, "crates", ApprovalDecision::Allow, "system");
        assert!(matches!(sonuc, Err(SelfHostError::Oracle(_))));
    }

    #[test]
    fn calisma_alani_disi_dokunus_reddedilir() {
        let mut oturum = SelfModifySession::open(istek(), dal());
        let mut t = dokunus();
        t.outside_workspace = true;
        assert!(matches!(
            oturum.record_touch(t),
            Err(SelfHostError::UnsafePath { .. })
        ));
        assert!(oturum.touches().is_empty());
    }

    #[test]
    fn baska_ajanin_dokunusu_reddedilir() {
        let mut oturum = SelfModifySession::open(istek(), dal());
        let mut t = dokunus();
        t.agent_id = 77;
        assert!(matches!(
            oturum.record_touch(t),
            Err(SelfHostError::ForeignTouch { .. })
        ));
    }

    #[test]
    fn baska_dalda_uretilen_aday_reddedilir() {
        let mut oturum = SelfModifySession::open(istek(), dal());
        let yabanci = SelfModifyBranch::temporary("baska").expect("dal");
        let aday =
            CandidateBuild::new(&yabanci, "cas:aday", dogrulama(true)).expect("aday");
        assert!(matches!(
            oturum.attach_candidate(aday),
            Err(SelfHostError::BranchMismatch { .. })
        ));
    }

    #[test]
    fn kapilar_eksikken_onay_istenemez() {
        let oturum = SelfModifySession::open(istek(), dal());
        let sonuc = oturum.request_promotion();
        let Err(SelfHostError::GatesNotSatisfied { missing }) = sonuc else {
            unreachable!("kapilar eksik olmaliydi")
        };
        assert_eq!(missing, SelfHostGate::ALL.to_vec());
    }

    #[test]
    fn curutulen_yargic_terfiyi_kapatir() {
        let mut oturum = hazir_oturum();
        oturum.attach_verdict(hukum(false));
        assert_eq!(oturum.missing_gates(), vec![SelfHostGate::JudgeVerdict]);
        assert!(oturum.request_promotion().is_err());
    }

    #[test]
    fn kirmizi_build_terfiyi_kapatir() {
        let mut oturum = hazir_oturum();
        let aday = CandidateBuild::new(oturum.branch(), "cas:aday", dogrulama(false))
            .expect("aday");
        oturum.attach_candidate(aday).expect("aday");
        assert_eq!(oturum.missing_gates(), vec![SelfHostGate::CandidateBuild]);
        assert!(oturum.request_promotion().is_err());
    }

    #[test]
    fn reddedilen_yetki_terfiyi_kapatir() {
        let mut oturum = hazir_oturum();
        let ret = CapabilityGrant::self_modify(9, "crates", ApprovalDecision::Deny, "void0x14")
            .expect("yetki");
        oturum.attach_grant(ret).expect("yetki");
        assert_eq!(oturum.missing_gates(), vec![SelfHostGate::CapabilityGrant]);
    }

    #[test]
    fn makine_terfi_ettiremez() {
        let oturum = hazir_oturum();
        let istek = oturum.request_promotion().expect("onay istegi");
        assert!(istek.approve("omnitrix").is_err());
    }

    #[test]
    fn kullanici_onayi_ile_terfi_emri_uretilir() {
        let mut oturum = hazir_oturum();
        let onay = oturum
            .request_promotion()
            .expect("onay istegi")
            .approve("void0x14")
            .expect("onay");
        assert_eq!(onay.session_id(), oturum.id());

        let emir = oturum.promote(onay).expect("terfi");
        assert_eq!(emir.branch(), "xai-fast-worktree/faz10-selfhost");
        assert_eq!(emir.artifact_ref(), "cas:aday-binary");
        assert_eq!(emir.commit(), Some("abc123"));
        assert_eq!(emir.approver(), "void0x14");
        assert!(oturum.is_promoted());
    }

    #[test]
    fn ayni_oturum_iki_kez_terfi_edemez() {
        let mut oturum = hazir_oturum();
        let onay = oturum
            .request_promotion()
            .expect("istek")
            .approve("void0x14")
            .expect("onay");
        oturum.promote(onay).expect("terfi");
        assert!(matches!(
            oturum.request_promotion(),
            Err(SelfHostError::AlreadyPromoted { .. })
        ));
    }

    #[test]
    fn baska_oturumun_onayi_gecmez() {
        let a = hazir_oturum();
        // Ikinci oturum baska bir gerekceyle acilir; kimlikleri farklidir.
        let mut b = {
            let istek = SelfModifyRequest::new(9, 4, "baska bir is")
                .expect("istek")
                .touching("crates/omni/omni-core/src/selfhost.rs")
                .expect("yol");
            let mut oturum = SelfModifySession::open(istek, dal());
            oturum.record_touch(dokunus()).expect("dokunus");
            oturum.attach_verdict(hukum(true));
            oturum.attach_grant(yetki()).expect("yetki");
            let aday = CandidateBuild::new(oturum.branch(), "cas:aday-binary", dogrulama(true))
                .expect("aday")
                .with_commit("abc123");
            oturum.attach_candidate(aday).expect("aday");
            oturum
        };
        let onay = a
            .request_promotion()
            .expect("istek")
            .approve("void0x14")
            .expect("onay");
        // Farkli oturumlar farkli kimlik uretir; token capraz kullanilamaz.
        assert_ne!(a.id(), b.id());
        assert!(matches!(
            b.promote(onay),
            Err(SelfHostError::ApprovalMismatch { .. })
        ));
        assert!(!b.is_promoted());
    }

    #[test]
    fn onay_sonrasi_degisen_aday_terfi_edemez() {
        let mut oturum = hazir_oturum();
        let onay = oturum
            .request_promotion()
            .expect("istek")
            .approve("void0x14")
            .expect("onay");
        // Kullanici onayladiktan sonra aday degistirilirse token gecersizdir.
        let baska = CandidateBuild::new(oturum.branch(), "cas:baska-binary", dogrulama(true))
            .expect("aday");
        oturum.attach_candidate(baska).expect("aday");
        assert!(matches!(
            oturum.promote(onay),
            Err(SelfHostError::ApprovalMismatch { .. })
        ));
        assert!(!oturum.is_promoted());
    }

    #[test]
    fn red_token_uretmez() {
        let oturum = hazir_oturum();
        let red = oturum
            .request_promotion()
            .expect("istek")
            .reject("performans regresyonu");
        assert_eq!(red.session_id, oturum.id());
        assert!(!oturum.is_promoted());
    }

    #[test]
    fn birakilan_oturum_dali_atar() {
        let oturum = hazir_oturum();
        let dal_adi = oturum.branch().name().to_owned();
        let birakilan = oturum.abandon("kullanici vazgecti");
        assert_eq!(birakilan.branch, dal_adi);
        assert!(birakilan.reason.contains("vazgecti"));
    }
}
