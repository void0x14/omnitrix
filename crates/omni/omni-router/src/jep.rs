//! Faz 4 — JEP (Judge-Executor-Planner) dongusu (MASTER-PLAN 10.1 / 10.2 / 10.3).
//!
//! Akis tek yonlu degil, **kapali dongudur**:
//!
//! ```text
//!   Planner ── gorevi alt gorevlere boler ──▶ Plan
//!      ▲                                        │
//!      │                                        ▼
//!   curutmeler                              Executor ── tool cagrilari ──▶ ExecutionOutput
//!      │                                                                        │
//!      └────────────── Judge (YANLISLAMACI) ◀───────────────────────────────────┘
//! ```
//!
//! Yargicin sorusu "dogru mu?" degil "**curutebilir miyim?**" (10.3 madde 3).
//! Tek basarili curutme turu dusurur; dusen turun curutmeleri bir sonraki
//! planlama turuna **geri beslenir**.
//!
//! Uc kural bu dosyanin seklini belirliyor:
//!
//! 1. **Roller config'te secilir** (`config/routing.toml` -> [`JepRoles`]).
//!    Bu dosyada literal model adi/fiyati **yoktur** (I5); her faz kendi
//!    modelini `crate::catalog::{ModelCatalog, Role}` uzerinden calisma
//!    zamaninda cozer (AS7, 10.2).
//! 2. **Ureten != dogrulayan** (10.3 madde 2): yargic fazi yurutme fazindan
//!    **farkli** model/anahtar kimligine baglanir. Ayrim kurulamazsa yargic
//!    turu curutur — sessizce gecmez.
//! 3. **Yargic kaniti kendi yeniden calistirabilir**: [`EvidenceReplayer`]
//!    motora baglanir ve yargica gecirilir; yeniden calistirmada cikti degisirse
//!    iddia duser (`Refutation::ReplayMismatch`).
//!
//! I6: bu modulun uretim yolunda `unwrap`/`expect`/`panic!`/`todo!` yoktur;
//! her faz hatasi [`JepError`] olarak yukari tasinir.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::catalog::{ModelCatalog, ModelRef, Role};
use crate::executor::{ExecutionOutput, Executor, ToolExecutor};
use crate::judge::{EvidenceReplayer, Judge, JudgeVerdict};
use crate::planner::{Plan, Planner};
use crate::strategies::{
    GroundingMode, JepAssignment, JepRoles, ProviderModel, RouterError, RoutingConfig,
    RoutingPolicy, RoutingStrategy, parse_role,
};
use crate::trust::TrustStore;

/// Curutme geri beslemesinde bir tura tasinacak en fazla madde. Prompt'un
/// sinirsiz buyumesini engeller.
const MAX_FEEDBACK_LINES: usize = 8;

/// `run_cycle` varsayilan tur ustu.
pub const DEFAULT_MAX_ITERATIONS: u32 = 3;

/// Ozne kimligi verilmediginde kullanilan varsayilan (`trust_scores.subject`).
pub const DEFAULT_SUBJECT_ID: &str = "jep";

// ---------------------------------------------------------------------------
// Fazlar ve rol baglamalari
// ---------------------------------------------------------------------------

/// JEP dongusunun uc fazi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JepPhase {
    /// Gorevi alt gorevlere bolme.
    Plan,
    /// Alt gorevleri tool cagrilarina cevirip uygulama.
    Execute,
    /// Ciktinin yanlislamaci denetimi.
    Judge,
}

impl JepPhase {
    /// Tum fazlar, dongudeki sirasiyla.
    pub const ALL: [JepPhase; 3] = [JepPhase::Plan, JepPhase::Execute, JepPhase::Judge];

    /// Fazin katalogda karsilik geldigi rol (10.2).
    pub fn role(self) -> Role {
        match self {
            JepPhase::Plan => Role::Planner,
            JepPhase::Execute => Role::Executor,
            JepPhase::Judge => Role::Judge,
        }
    }

    /// Log/kayit icin kanonik metin.
    pub fn as_str(self) -> &'static str {
        match self {
            JepPhase::Plan => "plan",
            JepPhase::Execute => "execute",
            JepPhase::Judge => "judge",
        }
    }
}

impl fmt::Display for JepPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Tek bir fazin somut baglamasi: hangi rol, hangi model, hangi anahtar.
///
/// `model` alani **koda gomulu degildir**; katalogdan ya da router'in
/// [`JepAssignment`] ciktisindan gelir (I5/AS7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseBinding {
    /// Baglanan faz.
    pub phase: JepPhase,
    /// Config'te secilen rol.
    pub role: Role,
    /// Rolun cozuldugu model kimligi.
    pub model: String,
    /// Cagrinin gidecegi saglayici (`providers.name`); bilinmiyorsa `None`.
    pub provider: Option<String>,
    /// Degerin hangi katmandan geldigi (denetlenebilirlik, I8).
    pub source: String,
}

impl PhaseBinding {
    fn from_ref(phase: JepPhase, model_ref: &ModelRef) -> Self {
        Self {
            phase,
            role: model_ref.role,
            model: model_ref.model.clone(),
            provider: model_ref.provider.clone(),
            source: model_ref.source.as_str().to_string(),
        }
    }

    /// Ayrim karsilastirmasinin uzerinde yapildigi kimlik.
    ///
    /// Saglayici biliniyorsa `provider::model`, bilinmiyorsa yalnizca `model`.
    /// Ayni modelin iki **farkli anahtari** de gecerli bir ayrimdir: 10.3'un
    /// istedigi sey ureten ile dogrulayanin ayni cagri baglamini paylasmamasi.
    pub fn identity(&self) -> String {
        match &self.provider {
            Some(provider) => format!("{provider}::{}", self.model),
            None => self.model.clone(),
        }
    }

    /// Router'in secmis oldugu somut saglayici+model ciftini uygular.
    fn apply_host(&mut self, host: &ProviderModel) {
        self.provider = Some(host.provider.clone());
        if !host.model.trim().is_empty() {
            self.model = host.model.clone();
        }
        self.source = "router_assignment".to_string();
    }
}

impl fmt::Display for PhaseBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})={}", self.phase, self.role, self.identity())
    }
}

/// Uc fazin tam baglamasi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JepBinding {
    /// Plan fazi.
    pub planner: PhaseBinding,
    /// Yurutme fazi.
    pub executor: PhaseBinding,
    /// Yargi fazi — yurutmeden **farkli** olmalidir (10.3 madde 2).
    pub judge: PhaseBinding,
}

impl JepBinding {
    /// Config'teki rol adlarini katalogla somut modellere cevirir.
    ///
    /// # Errors
    /// Rol adi taninmazsa ya da katalogda karsiligi yoksa
    /// [`RouterError::UnresolvedRole`] doner.
    pub fn resolve(catalog: &ModelCatalog, roles: &JepRoles) -> Result<Self, RouterError> {
        Ok(Self {
            planner: resolve_phase(catalog, JepPhase::Plan, &roles.planner)?,
            executor: resolve_phase(catalog, JepPhase::Execute, &roles.executor)?,
            judge: resolve_phase(catalog, JepPhase::Judge, &roles.judge)?,
        })
    }

    /// Router'in canli anahtarlara dagittigi atamayi uygular (10.1 -> 10.3).
    pub fn with_assignment(mut self, assignment: &JepAssignment) -> Self {
        self.planner.apply_host(&assignment.planner);
        self.executor.apply_host(&assignment.executor);
        self.judge.apply_host(&assignment.judge);
        self
    }

    /// Fazin baglamasi.
    pub fn phase(&self, phase: JepPhase) -> &PhaseBinding {
        match phase {
            JepPhase::Plan => &self.planner,
            JepPhase::Execute => &self.executor,
            JepPhase::Judge => &self.judge,
        }
    }

    /// Ureten ile dogrulayan gercekten ayristi mi (10.3 madde 2).
    pub fn separation(&self) -> SeparationStatus {
        let producer = self.executor.identity();
        let verifier = self.judge.identity();
        if producer != verifier {
            return SeparationStatus::Separated {
                producer,
                verifier,
            };
        }
        if self.executor.model == self.judge.model {
            SeparationStatus::SharedModel {
                model: self.judge.model.clone(),
            }
        } else {
            SeparationStatus::SharedHost { identity: verifier }
        }
    }

    /// Kisayol: ayrim kuruldu mu.
    pub fn separated(&self) -> bool {
        matches!(self.separation(), SeparationStatus::Separated { .. })
    }
}

fn resolve_phase(
    catalog: &ModelCatalog,
    phase: JepPhase,
    role_name: &str,
) -> Result<PhaseBinding, RouterError> {
    // Rol adi serbest metindir (config); once kanonik role cevrilir.
    let role = parse_role(role_name)
        .ok_or_else(|| RouterError::UnresolvedRole(format!("{phase}: {role_name}")))?;
    let model_ref = catalog.resolve(role)?;
    Ok(PhaseBinding::from_ref(phase, &model_ref))
}

/// Dogrulama ayriminin durumu (10.3 madde 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum SeparationStatus {
    /// Ureten ve dogrulayan farkli kimlikte.
    Separated {
        /// Yurutme fazinin kimligi.
        producer: String,
        /// Yargi fazinin kimligi.
        verifier: String,
    },
    /// Ayni model — 10.3 madde 2 ihlali.
    SharedModel {
        /// Iki fazin paylastigi model.
        model: String,
    },
    /// Model farkli olsa da ayni cagri baglami.
    SharedHost {
        /// Paylasilan kimlik.
        identity: String,
    },
    /// Katalog baglamasi yok; kimlik bilinmiyor.
    Unknown,
}

impl SeparationStatus {
    /// Ayrim kuruldu mu.
    pub fn is_separated(&self) -> bool {
        matches!(self, SeparationStatus::Separated { .. })
    }
}

impl fmt::Display for SeparationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeparationStatus::Separated { producer, verifier } => {
                write!(f, "separated (producer={producer}, verifier={verifier})")
            }
            SeparationStatus::SharedModel { model } => {
                write!(f, "not separated: producer and judge share model '{model}'")
            }
            SeparationStatus::SharedHost { identity } => {
                write!(f, "not separated: producer and judge share host '{identity}'")
            }
            SeparationStatus::Unknown => f.write_str("separation unknown: no catalog binding"),
        }
    }
}

// ---------------------------------------------------------------------------
// Hatalar
// ---------------------------------------------------------------------------

/// JEP dongusu hatalari.
#[derive(Debug, Error)]
pub enum JepError {
    /// Planlama fazi dustu.
    #[error("jep plan phase failed: {0}")]
    Plan(String),
    /// Yurutme fazi dustu.
    #[error("jep execute phase failed: {0}")]
    Execute(String),
    /// Yargi fazi dustu.
    #[error("jep judge phase failed: {0}")]
    Judge(String),
    /// `max_iterations = 0`: hic tur kosulmadi.
    #[error("jep produced no iteration (max_iterations = {0})")]
    NoIteration(u32),
    /// Rol cozumu / config hatasi.
    #[error(transparent)]
    Router(#[from] RouterError),
}

// ---------------------------------------------------------------------------
// Tur ozeti ve sonuc
// ---------------------------------------------------------------------------

/// Tek bir turun hafif ozeti — plan/cikti kopyalanmadan denetim izi (I8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JepIteration {
    /// Kacinci tur (1'den baslar).
    pub iteration: u32,
    /// Plan kimligi.
    pub plan_id: String,
    /// Plandaki alt gorev sayisi.
    pub sub_tasks: usize,
    /// Tamamlanan alt gorev sayisi.
    pub completed: usize,
    /// Dusen alt gorev sayisi.
    pub failed: usize,
    /// Yurutmede yapilan tool cagrisi sayisi.
    pub tool_calls: usize,
    /// Turun yargic karari.
    pub passed: bool,
    /// Yargicin skoru.
    pub score: f64,
    /// Curutulen iddia kimlikleri.
    pub refuted_claims: Vec<String>,
    /// Bir sonraki plana beslenen curutme gerekceleri.
    pub feedback: Vec<String>,
    /// Tur sonunda oznenin guven puani (guven deposu varsa).
    pub trust_score: Option<f64>,
    /// Ozne bu tur sonunda karantinaya alindi mi.
    pub quarantined: bool,
}

/// Dongunun tam sonucu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JepResult {
    /// Son turun plani.
    pub plan: Plan,
    /// Son turun yurutme ciktisi.
    pub execution: ExecutionOutput,
    /// Son turun yargic karari.
    pub verdict: JudgeVerdict,
    /// Kosulan tur sayisi.
    pub iterations: u32,
    /// Tur tur denetim izi.
    pub history: Vec<JepIteration>,
    /// Fazlarin model baglamasi (katalogtan; I5).
    pub binding: Option<JepBinding>,
    /// Dogrulama ayriminin durumu.
    pub separation: SeparationStatus,
}

impl JepResult {
    /// Dongu gecti mi.
    pub fn passed(&self) -> bool {
        self.verdict.passed
    }

    /// Ozne karantinaya alindiysa `true` (son tur ozetinden).
    pub fn quarantined(&self) -> bool {
        self.history.last().is_some_and(|it| it.quarantined)
    }
}

// ---------------------------------------------------------------------------
// Yargic tarifi — motorun builder'lari yargici bu tariften yeniden kurar.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct JudgeSpec {
    threshold: Option<f64>,
    trust: Option<TrustStore>,
    subject_id: String,
    judge_model: Option<String>,
    producer_model: Option<String>,
    require_separation: bool,
    replayer: Option<Arc<dyn EvidenceReplayer>>,
}

impl JudgeSpec {
    fn new(subject_id: String) -> Self {
        Self {
            threshold: None,
            trust: None,
            subject_id,
            judge_model: None,
            producer_model: None,
            require_separation: false,
            replayer: None,
        }
    }

    fn build(&self) -> Judge {
        let mut judge = match self.threshold {
            Some(threshold) => Judge::with_threshold(threshold, self.trust.clone()),
            None => Judge::new(self.trust.clone()),
        };
        judge = judge.with_subject_id(self.subject_id.clone());
        if let Some(model) = &self.judge_model {
            judge = judge.with_judge_model(model.clone());
        }
        if let Some(model) = &self.producer_model {
            judge = judge.with_producer_model(model.clone());
        }
        judge = judge.require_separation(self.require_separation);
        if let Some(replayer) = &self.replayer {
            judge = judge.with_replayer(Arc::clone(replayer));
        }
        judge
    }
}

// ---------------------------------------------------------------------------
// Motor
// ---------------------------------------------------------------------------

/// JEP dongusu motoru (10.1).
pub struct JepEngine {
    planner: Planner,
    executor: Executor,
    judge: Judge,
    spec: JudgeSpec,
    binding: Option<JepBinding>,
    policy: RoutingPolicy,
    tool_executor: Option<Arc<dyn ToolExecutor>>,
    max_iterations: u32,
}

impl JepEngine {
    /// Katalogsuz motor: roller cozulmez, ayrim denetimi kapalidir.
    ///
    /// Tam JEP davranisi icin [`JepEngine::with_catalog`] ya da
    /// [`JepEngine::from_routing_config`] kullanin.
    pub fn new(policy: RoutingPolicy) -> Self {
        let spec = JudgeSpec::new(DEFAULT_SUBJECT_ID.to_string());
        Self {
            planner: Planner::new(),
            executor: Executor::new(policy.clone()),
            judge: spec.build(),
            spec,
            binding: None,
            policy,
            tool_executor: None,
            max_iterations: DEFAULT_MAX_ITERATIONS,
        }
    }

    /// Rolleri katalogdan cozerek motor kurar (AS7/I5).
    ///
    /// Yargic modeli `Role::Judge`, yurutme modeli `Role::Executor` uzerinden
    /// gelir; ikisi ayni kimlige duserse **uyari** verilir ve — kanit kipi
    /// `off` degilse — yargic ayrim sarti acik kalir, yani dongu curutulur.
    ///
    /// # Errors
    /// Rol cozulemezse [`RouterError::UnresolvedRole`].
    pub fn with_catalog(
        policy: RoutingPolicy,
        catalog: &ModelCatalog,
        roles: &JepRoles,
    ) -> Result<Self, RouterError> {
        let binding = JepBinding::resolve(catalog, roles)?;
        let mut engine = Self::new(policy);
        let require_separation = engine.policy.grounding != GroundingMode::Off;
        engine.spec.require_separation = require_separation;
        engine.apply_binding(binding);
        Ok(engine)
    }

    /// `config/routing.toml` + katalogdan motor kurar.
    ///
    /// # Errors
    /// Zincir bossa [`RouterError::EmptyChain`], rol cozulemezse
    /// [`RouterError::UnresolvedRole`].
    pub fn from_routing_config(
        config: &RoutingConfig,
        catalog: &ModelCatalog,
    ) -> Result<Self, RouterError> {
        if config.strategy != RoutingStrategy::JudgeExecutorPlanner {
            warn!(
                "jep motoru '{}' stratejisiyle kuruldu; roller yine de [jep] blogundan okunur",
                config.strategy.as_db_str()
            );
        }
        let policy = config.to_policy(catalog)?;
        Self::with_catalog(policy, catalog, &config.jep)
    }

    fn apply_binding(&mut self, binding: JepBinding) {
        let separation = binding.separation();
        match &separation {
            SeparationStatus::Separated { producer, verifier } => {
                info!("jep separation ok: producer={producer}, verifier={verifier}");
            }
            other => warn!("jep: {other}"),
        }
        self.spec.producer_model = Some(binding.executor.identity());
        self.spec.judge_model = Some(binding.judge.identity());
        self.binding = Some(binding);
        self.rebuild_judge();
    }

    /// Yargici tarifinden yeniden kurar. Builder zincirleri kosarken cagrilir;
    /// ihlal sayaclari bu noktada henuz bostur.
    fn rebuild_judge(&mut self) {
        self.judge = self.spec.build();
    }

    /// Tur ustunu ayarla.
    pub fn with_max_iterations(mut self, max_iterations: u32) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    /// Ceza/karantina kayitlarinin baglandigi ozne (`agent_id`).
    pub fn with_subject_id(mut self, subject_id: impl Into<String>) -> Self {
        self.spec.subject_id = subject_id.into();
        self.rebuild_judge();
        self
    }

    /// Guven deposunu bagla (`trust_scores`).
    pub fn with_trust_store(mut self, trust: TrustStore) -> Self {
        self.spec.trust = Some(trust);
        self.rebuild_judge();
        self
    }

    /// Yargic gecme esigini ayarla.
    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.spec.threshold = Some(threshold);
        self.rebuild_judge();
        self
    }

    /// Ayrim kanitlanamazsa dongu dussun mu (10.3 madde 2).
    pub fn require_separation(mut self, required: bool) -> Self {
        self.spec.require_separation = required;
        self.rebuild_judge();
        self
    }

    /// Yurutme fazinin tool koprusunu bagla.
    pub fn with_tool_executor(mut self, tool_executor: Arc<dyn ToolExecutor>) -> Self {
        self.executor = Executor::new(self.policy.clone()).with_tool_executor(Arc::clone(&tool_executor));
        self.tool_executor = Some(tool_executor);
        self
    }

    /// Yargicin tool ciktisini **kendi** yeniden calistirmasini sagla
    /// (10.3 madde 2).
    pub fn with_replayer(mut self, replayer: Arc<dyn EvidenceReplayer>) -> Self {
        self.spec.replayer = Some(replayer);
        self.rebuild_judge();
        self
    }

    /// Router'in canli anahtarlara yaptigi atamayi uygula (10.1).
    ///
    /// Ayni model iki farkli anahtarda kosuluyorsa ayrim bu adimda kurulur.
    pub fn with_assignment(mut self, assignment: &JepAssignment) -> Self {
        let binding = match self.binding.take() {
            Some(binding) => binding.with_assignment(assignment),
            None => JepBinding {
                planner: host_only(JepPhase::Plan, &assignment.planner),
                executor: host_only(JepPhase::Execute, &assignment.executor),
                judge: host_only(JepPhase::Judge, &assignment.judge),
            },
        };
        self.apply_binding(binding);
        self
    }

    /// Faz baglamalari (katalogdan cozulmusse).
    pub fn binding(&self) -> Option<&JepBinding> {
        self.binding.as_ref()
    }

    /// Dogrulama ayriminin durumu.
    pub fn separation(&self) -> SeparationStatus {
        match &self.binding {
            Some(binding) => binding.separation(),
            None => SeparationStatus::Unknown,
        }
    }

    /// Yargica salt-okunur erisim (ihlal sayaci, guven puani, esik).
    pub fn judge(&self) -> &Judge {
        &self.judge
    }

    /// Motorun calistigi yonlendirme politikasi.
    pub fn policy(&self) -> &RoutingPolicy {
        &self.policy
    }

    /// Tur ustu.
    pub fn max_iterations(&self) -> u32 {
        self.max_iterations
    }

    // -----------------------------------------------------------------------
    // Dongu
    // -----------------------------------------------------------------------

    /// Kapali dongu: planla -> uygula -> curutmeye calis; curutuldukce yeniden
    /// planla. Gecen ilk turda doner; hicbir tur gecmezse **son** turun sonucu
    /// (curutmeleriyle birlikte) donulur.
    ///
    /// # Errors
    /// Faz hatalarinda [`JepError`]; `max_iterations = 0` ise
    /// [`JepError::NoIteration`].
    pub async fn run_cycle(&self, task: &str) -> Result<JepResult, JepError> {
        if self.max_iterations == 0 {
            return Err(JepError::NoIteration(self.max_iterations));
        }

        let subject = self.spec.subject_id.clone();
        let mut history: Vec<JepIteration> = Vec::new();
        let mut feedback: Vec<String> = Vec::new();
        let mut last: Option<(Plan, ExecutionOutput, JudgeVerdict)> = None;

        for iteration in 1..=self.max_iterations {
            info!(
                "jep iteration {}/{} for subject '{}'",
                iteration, self.max_iterations, subject
            );

            // (1) Planner gorevi parcalar. Onceki turun curutmeleri prompt'a
            // geri beslenir; ayni plani tekrar uretmenin anlami yok.
            let prompt = refine_task(task, &feedback);
            let plan = self.plan_phase(&prompt).await?;

            // (2) Executor plani uygular.
            let execution = self.execute_phase(&plan).await?;

            // (3) Judge curutmeye calisir (kanit kapisi + ayrim + yeniden
            // calistirma + ihlal maliyeti hepsi `Judge::evaluate` icinde).
            let verdict = self.judge_phase(&execution).await?;

            let quarantined = self.judge.is_quarantined(&subject);
            history.push(summarize(
                iteration,
                &plan,
                &execution,
                &verdict,
                self.judge.trust_score(&subject),
                quarantined,
            ));

            let passed = verdict.passed;
            feedback = verdict.feedback.clone();
            last = Some((plan, execution, verdict));

            if passed {
                info!("jep passed on iteration {iteration}");
                break;
            }

            debug!(
                "jep iteration {iteration} refuted: {}",
                feedback.join("; ")
            );

            // Ihlal maliyeti: karantinaya alinan ozneyle donmeye devam etmek
            // sadece cezayi buyutur (10.3 madde 4).
            if quarantined {
                warn!("jep: subject '{subject}' quarantined, aborting the loop");
                break;
            }
        }

        let iterations = history.len() as u32;
        let Some((plan, execution, verdict)) = last else {
            return Err(JepError::NoIteration(self.max_iterations));
        };

        Ok(JepResult {
            plan,
            execution,
            verdict,
            iterations,
            history,
            binding: self.binding.clone(),
            separation: self.separation(),
        })
    }

    /// Plan fazi: gorevi alt gorevlere boler.
    ///
    /// # Errors
    /// Planlayici dusserse [`JepError::Plan`].
    pub async fn plan_phase(&self, task: &str) -> Result<Plan, JepError> {
        debug!("jep plan phase ({})", self.phase_label(JepPhase::Plan));
        self.planner
            .generate_plan(task)
            .await
            .map_err(|e| JepError::Plan(e.to_string()))
    }

    /// Yurutme fazi: plandaki alt gorevleri bagimlilik sirasinda uygular.
    ///
    /// # Errors
    /// Yurutucu dusserse [`JepError::Execute`].
    pub async fn execute_phase(&self, plan: &Plan) -> Result<ExecutionOutput, JepError> {
        debug!(
            "jep execute phase ({}) with {} sub-tasks",
            self.phase_label(JepPhase::Execute),
            plan.sub_tasks.len()
        );
        self.executor
            .execute_plan(plan)
            .await
            .map_err(|e| JepError::Execute(e.to_string()))
    }

    /// Yargi fazi: ciktiyi **curutmeye** calisir.
    ///
    /// # Errors
    /// Yargic dusserse [`JepError::Judge`].
    pub async fn judge_phase(&self, output: &ExecutionOutput) -> Result<JudgeVerdict, JepError> {
        debug!(
            "jep judge phase ({}) over {} tool calls",
            self.phase_label(JepPhase::Judge),
            output.tool_calls.len()
        );
        self.judge
            .evaluate(output)
            .await
            .map_err(|e| JepError::Judge(e.to_string()))
    }

    fn phase_label(&self, phase: JepPhase) -> String {
        match &self.binding {
            Some(binding) => binding.phase(phase).identity(),
            None => format!("{phase}:unbound"),
        }
    }
}

/// Katalog baglamasi yokken router atamasindan uretilen faz baglamasi.
fn host_only(phase: JepPhase, host: &ProviderModel) -> PhaseBinding {
    PhaseBinding {
        phase,
        role: phase.role(),
        model: host.model.clone(),
        provider: Some(host.provider.clone()),
        source: "router_assignment".to_string(),
    }
}

/// Curutmeleri bir sonraki plan promptuna geri besler.
///
/// Bos geri besleme gorevi oldugu gibi birakir; boylece ilk tur saf gorevle
/// kosar ve dongu deterministik kalir.
pub fn refine_task(task: &str, feedback: &[String]) -> String {
    if feedback.is_empty() {
        return task.to_string();
    }
    let mut out = String::with_capacity(task.len() + 128);
    out.push_str(task);
    out.push_str("\n\n[judge refutations to address]");
    for line in feedback.iter().take(MAX_FEEDBACK_LINES) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push_str("\n- ");
        out.push_str(trimmed);
    }
    if feedback.len() > MAX_FEEDBACK_LINES {
        out.push_str(&format!(
            "\n- (+{} more refutations omitted)",
            feedback.len() - MAX_FEEDBACK_LINES
        ));
    }
    out
}

fn summarize(
    iteration: u32,
    plan: &Plan,
    execution: &ExecutionOutput,
    verdict: &JudgeVerdict,
    trust_score: Option<f64>,
    quarantined: bool,
) -> JepIteration {
    let refuted_claims = verdict
        .grounding_report
        .as_ref()
        .map(|report| {
            report
                .claims
                .iter()
                .filter(|claim| !claim.grounded)
                .map(|claim| claim.claim_id.clone())
                .collect()
        })
        .unwrap_or_default();

    JepIteration {
        iteration,
        plan_id: plan.id.clone(),
        sub_tasks: plan.sub_tasks.len(),
        completed: execution.completed_tasks.len(),
        failed: execution.failed_tasks.len(),
        tool_calls: execution.tool_calls.len(),
        passed: verdict.passed,
        score: verdict.score,
        refuted_claims,
        feedback: verdict.feedback.clone(),
        trust_score,
        quarantined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use crate::executor::{ToolCallRecord, ToolCallStatus};
    use crate::strategies::{GroundingMode, RoutingBudget, Usage};

    /// Test model kimlikleri sentetiktir; gercek model adi/fiyati tasimaz (I5).
    const SYNTH_PLANNER: &str = "test-planner-model";
    const SYNTH_EXECUTOR: &str = "test-executor-model";
    const SYNTH_JUDGE: &str = "test-judge-model";

    fn policy(grounding: GroundingMode) -> RoutingPolicy {
        RoutingPolicy {
            strategy: RoutingStrategy::JudgeExecutorPlanner,
            fallback_chain: vec![],
            budget: None,
            grounding,
        }
    }

    /// Roller **config'te** secilir: bu dosya rol->model haritasini diske yazar,
    /// katalog onu okur. Model adi hicbir zaman koda gomulu degildir.
    fn write_models_toml(dir: &tempfile::TempDir) -> PathBuf {
        let path = dir.path().join("models.toml");
        let body = format!(
            "[roles]\nplanner = \"{SYNTH_PLANNER}\"\nexecutor = \"{SYNTH_EXECUTOR}\"\n\
             judge = \"{SYNTH_JUDGE}\"\nsummary = \"{SYNTH_PLANNER}\"\n\
             web_search = \"{SYNTH_PLANNER}\"\n"
        );
        let mut file = std::fs::File::create(&path).expect("models.toml olusturulmali");
        file.write_all(body.as_bytes()).expect("models.toml yazilmali");
        path
    }

    fn split_catalog(dir: &tempfile::TempDir) -> ModelCatalog {
        let path = write_models_toml(dir);
        ModelCatalog::load(Some(&path)).expect("katalog yuklenmeli")
    }

    struct StubTools {
        calls: AtomicUsize,
        fail_all: bool,
    }

    impl StubTools {
        fn ok() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_all: false,
            }
        }

        fn broken() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_all: true,
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ToolExecutor for StubTools {
        async fn execute(&self, tool_name: &str, arguments: serde_json::Value) -> ToolCallRecord {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_all {
                return ToolCallRecord {
                    tool_name: tool_name.to_string(),
                    arguments,
                    result: serde_json::json!({"error": "stub failure"}),
                    status: ToolCallStatus::Error("stub failure".into()),
                };
            }
            ToolCallRecord {
                tool_name: tool_name.to_string(),
                arguments,
                result: serde_json::json!({"status": "ok"}),
                status: ToolCallStatus::Success,
            }
        }

        async fn check_available(&self, _tool_name: &str) -> bool {
            true
        }

        async fn estimate_cost(&self, _tool_name: &str) -> Usage {
            Usage {
                tokens_used: 1,
                cost_incurred: 0.0,
                latency_ms: 1,
            }
        }
    }

    /// Yargic tool'u yeniden calistirir ve **ayni** ciktiyi alir.
    struct StableReplayer;

    #[async_trait]
    impl EvidenceReplayer for StableReplayer {
        async fn replay(&self, _tool_call_id: &str) -> Option<String> {
            Some("{\"status\":\"ok\"}".to_string())
        }
    }

    /// Yargic tool'u yeniden calistirir ve **farkli** cikti alir -> curutme.
    struct DriftingReplayer;

    #[async_trait]
    impl EvidenceReplayer for DriftingReplayer {
        async fn replay(&self, _tool_call_id: &str) -> Option<String> {
            Some("{\"status\":\"drifted\"}".to_string())
        }
    }

    fn separated_engine(catalog: &ModelCatalog) -> JepEngine {
        JepEngine::with_catalog(
            policy(GroundingMode::Required),
            catalog,
            &JepRoles::default(),
        )
        .expect("roller cozulmeli")
    }

    #[test]
    fn roles_come_from_the_catalog_not_from_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = split_catalog(&dir);
        let binding =
            JepBinding::resolve(&catalog, &JepRoles::default()).expect("roller cozulmeli");

        assert_eq!(binding.planner.role, Role::Planner);
        assert_eq!(binding.executor.role, Role::Executor);
        assert_eq!(binding.judge.role, Role::Judge);
        assert_eq!(binding.planner.model, SYNTH_PLANNER);
        assert_eq!(binding.executor.model, SYNTH_EXECUTOR);
        assert_eq!(binding.judge.model, SYNTH_JUDGE);
        assert_eq!(binding.phase(JepPhase::Judge).model, SYNTH_JUDGE);
    }

    #[test]
    fn unknown_role_name_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = split_catalog(&dir);
        let roles = JepRoles {
            planner: "planner".into(),
            executor: "executor".into(),
            judge: "yargic".into(),
        };
        let err = JepBinding::resolve(&catalog, &roles).expect_err("bilinmeyen rol reddedilmeli");
        assert!(matches!(err, RouterError::UnresolvedRole(_)));
    }

    #[test]
    fn distinct_judge_model_establishes_separation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = split_catalog(&dir);
        let engine = separated_engine(&catalog);
        assert!(engine.separation().is_separated(), "{}", engine.separation());
    }

    /// Gomulu varsayilanlarda her rol ayni modele duser; bu **ayrim degildir**
    /// (10.3 madde 2) ve motor bunu sessizce gecirmemelidir.
    #[test]
    fn shared_model_is_not_separation() {
        let catalog = ModelCatalog::from_embedded().expect("gomulu katalog");
        let engine = JepEngine::with_catalog(
            policy(GroundingMode::Required),
            &catalog,
            &JepRoles::default(),
        )
        .expect("roller cozulmeli");
        assert!(matches!(
            engine.separation(),
            SeparationStatus::SharedModel { .. }
        ));
    }

    /// Ayni model iki **farkli anahtarda** kosuyorsa ayrim kurulur.
    #[test]
    fn router_assignment_separates_by_host() {
        let catalog = ModelCatalog::from_embedded().expect("gomulu katalog");
        let shared = catalog
            .resolve(Role::Executor)
            .expect("executor cozulmeli")
            .model;
        let assignment = JepAssignment {
            planner: ProviderModel {
                provider: "host-a".into(),
                model: shared.clone(),
                weight: None,
            },
            executor: ProviderModel {
                provider: "host-a".into(),
                model: shared.clone(),
                weight: None,
            },
            judge: ProviderModel {
                provider: "host-b".into(),
                model: shared,
                weight: None,
            },
        };

        let engine = JepEngine::with_catalog(
            policy(GroundingMode::Required),
            &catalog,
            &JepRoles::default(),
        )
        .expect("roller cozulmeli")
        .with_assignment(&assignment);

        assert!(engine.separation().is_separated(), "{}", engine.separation());
        let binding = engine.binding().expect("baglama olmali");
        assert_eq!(binding.judge.provider.as_deref(), Some("host-b"));
    }

    #[tokio::test]
    async fn cycle_passes_when_evidence_survives_refutation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = split_catalog(&dir);
        let tools = Arc::new(StubTools::ok());
        let engine = separated_engine(&catalog)
            .with_subject_id("agent-pass")
            .with_tool_executor(tools.clone())
            .with_replayer(Arc::new(StableReplayer));

        let result = match engine.run_cycle("write the migration note").await {
            Ok(r) => r,
            Err(e) => panic!("cycle must not error: {e}"),
        };

        assert!(result.passed(), "history: {:?}", result.history);
        assert_eq!(result.iterations, 1, "gecen tur tekrar edilmez");
        assert!(result.separation.is_separated());
        assert!(tools.call_count() > 0, "executor tool cagirmali");
        assert!(result.history[0].refuted_claims.is_empty());
    }

    /// Yargic tool'u kendi yeniden calistirir; cikti kayarsa iddia duser.
    #[tokio::test]
    async fn drifting_replay_refutes_every_iteration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = split_catalog(&dir);
        let engine = separated_engine(&catalog)
            .with_subject_id("agent-drift")
            .with_tool_executor(Arc::new(StubTools::ok()))
            .with_replayer(Arc::new(DriftingReplayer))
            .with_max_iterations(2);

        let result = match engine.run_cycle("verify the deployment").await {
            Ok(r) => r,
            Err(e) => panic!("cycle must not error: {e}"),
        };

        assert!(!result.passed());
        assert!(
            result
                .history
                .iter()
                .flat_map(|it| it.feedback.iter())
                .any(|line| line.contains("replay")),
            "history: {:?}",
            result.history
        );
    }

    /// Ureten == dogrulayan iken hicbir tur gecemez (10.3 madde 2).
    #[tokio::test]
    async fn shared_model_cannot_pass_the_cycle() {
        let catalog = ModelCatalog::from_embedded().expect("gomulu katalog");
        let engine = JepEngine::with_catalog(
            policy(GroundingMode::Required),
            &catalog,
            &JepRoles::default(),
        )
        .expect("roller cozulmeli")
        .with_subject_id("agent-shared")
        .with_tool_executor(Arc::new(StubTools::ok()))
        .with_max_iterations(1);

        let result = match engine.run_cycle("ship it").await {
            Ok(r) => r,
            Err(e) => panic!("cycle must not error: {e}"),
        };
        assert!(!result.passed());
        assert!(
            result.verdict.feedback.iter().any(|f| f.contains("separation")),
            "feedback: {:?}",
            result.verdict.feedback
        );
    }

    /// Tekrarlanan kanitsiz iddia karantinaya gotururur ve dongu orada durur.
    #[tokio::test]
    async fn quarantine_aborts_the_loop_early() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = split_catalog(&dir);
        let engine = separated_engine(&catalog)
            .with_subject_id("agent-liar")
            .with_trust_store(TrustStore::new())
            .with_tool_executor(Arc::new(StubTools::broken()))
            .with_max_iterations(6);

        let result = match engine.run_cycle("claim success without evidence").await {
            Ok(r) => r,
            Err(e) => panic!("cycle must not error: {e}"),
        };

        assert!(!result.passed());
        assert!(result.quarantined(), "history: {:?}", result.history);
        assert!(
            result.iterations < 6,
            "karantina dongunun devamini engellemeli: {}",
            result.iterations
        );
        assert!(engine.judge().is_quarantined("agent-liar"));
    }

    #[tokio::test]
    async fn zero_iterations_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = split_catalog(&dir);
        let engine = separated_engine(&catalog).with_max_iterations(0);
        let err = engine
            .run_cycle("nothing")
            .await
            .expect_err("0 tur hata olmali");
        assert!(matches!(err, JepError::NoIteration(0)));
    }

    #[test]
    fn refutations_are_fed_back_into_the_next_plan() {
        let plain = refine_task("do the thing", &[]);
        assert_eq!(plain, "do the thing");

        let refined = refine_task(
            "do the thing",
            &["claim 'c0' refuted".to_string(), "  ".to_string()],
        );
        assert!(refined.starts_with("do the thing"));
        assert!(refined.contains("claim 'c0' refuted"));
        assert!(!refined.contains("\n- \n"), "bos satir eklenmemeli");

        let many: Vec<String> = (0..MAX_FEEDBACK_LINES + 3)
            .map(|i| format!("refutation {i}"))
            .collect();
        let capped = refine_task("t", &many);
        assert!(capped.contains("more refutations omitted"));
    }

    #[test]
    fn budgeted_policy_flows_into_the_executor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = split_catalog(&dir);
        let mut budgeted = policy(GroundingMode::Required);
        budgeted.budget = Some(RoutingBudget {
            max_tokens: Some(1),
            max_cost: None,
            max_latency_ms: None,
        });

        let engine = JepEngine::with_catalog(budgeted, &catalog, &JepRoles::default())
            .expect("roller cozulmeli")
            .with_tool_executor(Arc::new(StubTools::ok()));
        assert!(engine.policy().budget.is_some());
    }

    #[test]
    fn routing_config_drives_roles_and_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = split_catalog(&dir);
        let config = RoutingConfig::parse(
            r#"
strategy  = "jep"
grounding = "required"

[[provider]]
provider = "primary"
role     = "executor"

[[provider]]
provider = "secondary"
role     = "judge"

[jep]
planner  = "planner"
executor = "executor"
judge    = "judge"
"#,
        )
        .expect("routing.toml cozulmeli");

        let engine = JepEngine::from_routing_config(&config, &catalog).expect("motor kurulmali");
        let binding = engine.binding().expect("baglama olmali");
        assert_eq!(binding.executor.model, SYNTH_EXECUTOR);
        assert_eq!(binding.judge.model, SYNTH_JUDGE);
        assert!(engine.separation().is_separated());
        assert_eq!(engine.policy().fallback_chain.len(), 2);
    }

    #[test]
    fn phase_roles_are_stable() {
        assert_eq!(JepPhase::Plan.role(), Role::Planner);
        assert_eq!(JepPhase::Execute.role(), Role::Executor);
        assert_eq!(JepPhase::Judge.role(), Role::Judge);
        assert_eq!(JepPhase::ALL.len(), 3);
    }

    #[test]
    fn identity_prefers_provider_scoped_form() {
        let bare = PhaseBinding {
            phase: JepPhase::Judge,
            role: Role::Judge,
            model: SYNTH_JUDGE.into(),
            provider: None,
            source: "file_role".into(),
        };
        assert_eq!(bare.identity(), SYNTH_JUDGE);

        let hosted = PhaseBinding {
            provider: Some("host-b".into()),
            ..bare
        };
        assert_eq!(hosted.identity(), format!("host-b::{SYNTH_JUDGE}"));
    }
}
