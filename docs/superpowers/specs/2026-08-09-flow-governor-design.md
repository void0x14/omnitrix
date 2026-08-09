# Flow Governor — Deterministik Akış Boru Hattı Denetleyicisi (Tasarım)

Tarih: 2026-08-09 · Durum: Onaylandı (anahtar teslim, onay kapısı yok) · Otorite: `OTONOM ARACIM` belgesi + kullanıcı direktifleri

## 1. Problem

Mevcut sistemde çok adımlı görev akışı **AI güdümlüdür**: `goal_planner` /
`goal_orchestrator` zinciri, modelin kendi planını yapıp kendi inisiyatifiyle
uygulamasına izin verir. Kullanıcının talebi:

> "Çok adımlı akış algoritması boru hattı **asla AI'ın inisiyatifinde olmamalı**.
> Kararları yine AI verir, fakat **sonuçları her zaman durum makineleri ve
> deterministik, araç dışı ama aracın içine gömülü döngüler yönetir**.
> AI adım atlarsa sistem tespit eder, düzeltir; kullanıcı yalnızca akışın
> sorunsuz ilerlediğini görür. Basit görevler için aynı ağır akış zorunlu
> değildir — akış seçimini de AI değil sistem yapar."

Zorlayıcı kısıt: **Rust derleyicisi asla çalıştırılamaz** (test/check/build
yok). Doğrulama yalnızca statik kod incelemesiyle yapılır.

## 2. Tasarım Özeti

**Flow Governor**: oturum başına bir adet, `PlanModeTracker` deseniyle
(`Arc<parking_lot::Mutex<...>>` — bkz. `acp_session.rs:780`) `SessionActor`
tarafından sahiplenilen **saf deterministik durum makinesi**. AI'ın normal tur
döngüsünü (turn.rs) yönetmez; döngünün **dört karar noktasını** kilitler:

| Karar noktası | Mevcut kod | Governor müdahalesi |
|---|---|---|
| A. Prompt aktivasyonu | `handle_prompt` (turn.rs:239) | Görevi sınıflandır, akışı seç, direktifi enjekte et |
| B. Tool görünürlüğü | `prepare_tool_definitions_inner` (sampler_turn.rs:~229) | Kilitli aşamanın tool'larını LLM'den **gizle** |
| C. Tur sonu (erken bitirme) | `run_stop_gate` (stop_gate.rs:251) | Aşama kanıtı eksikse `KeepWorking{direktif}` |
| D. Goal devam | `run_goal_round_end` (goal.rs:1332) | Akış aktifken AI-güdümlü goal döngüsü yerine akış karar verir |

Ayrıca `flow_checkpoint` adında deterministik bir el sıkışma aracı: model her
aşamanın bitiminde çağırır; governor aşama sırasını ve kanıt dosyalarını
doğrular, geçerliyse aşamayı ilerletir, değilse reddedip düzeltici direktif
döndürür. Kilitli aşama tool'ları modelin tool listesinde **hiç yoktur**
(modele "üretemeyeceği" garanti edilir); model yine de kilitli bir tool adı
üretirse mevcut `NonExistingTool` yolu devreye girer + governor düzeltici
direktif ekler.

**Görünmez düzeltme ilkesi:** Tüm düzeltmeler sistem mesajı/direktif olarak
modelin turuna enjekte edilir; kullanıcı arayüzü yalnızca aşama geçişlerini
(`flow.phase_changed`) ve tamamlanmayı görür. İhlaller events kaydında
(`flow.violation`) tutulur — "7/24 takip" kaydı korunur, arayüz gürültüsü olmaz.

## 3. Akış Şablonları (config/flow/)

### 3.1 universal — `OTONOM ARACIM` belgesi satır 111-114'teki akış

```
analyze        (read/search/meta)  → problem_list        [AI: oku, parçala, anla]
research       (web/research/computer) → findings_archive [AI: surface/deep/ocean araştır]
digest         (read/search)       → digest_note         [AI: bulguları sindir]
stack_select   (read/web/search)   → stack_choice        [AI: stack önerir]
stack_verify   (read/web/search)   → stack_verified      [AI: internetle karşılaştır, çürütme döngüsü]
duration       (SİSTEM — tool yok) → duration_decision   [Sistem: MVP vs TAM, deterministik]
plan           (read/plan)         → plan_doc            [AI: plan çıkar]
decompose      (read/plan/write)   → building_blocks     [AI: yapı taşlarına böl]
parallel_query (SİSTEM — tool yok) → execution_graph     [Sistem: bağımlılık analizi]
execute        (TÜM ARAÇLAR + task) → work_done          [AI: taşları uygular, gerekirse multiajan]
verify         (read/bash-ro/task) → verified            [AI: doğrula; checkpoint + opsiyonel kullanıcı onayı]
notify         (SİSTEM — tool yok) → notified            [Sistem: bildirim (olay + opsiyonel webhook)]
```

- `duration` aşaması: `FlowDurationSystem` (deterministik): classifier skoru +
  kullanıcı modu + `config/flow/rules.toml` eşikleri → `mvp` | `full` kararı.
  Karar kanıt olarak kaydedilir; AI yalnızca uygular (ör. `mvp` ise execute
  aşamasında alt ajan sınırı düşer).
- `parallel_query`: decompose çıktısı (JSONL `building_blocks` — dosya yolları
  ile) üzerinde sistem pas geçer: ortak dosya yolu paylaşmayan bloklar =
  paralel. Çıktı `execution_graph` olarak kaydedilir.
- `notify`: mevcut bildirim kanalları üzerinden olay; yapılandırılmış webhook
  varsa POST (post-MVP). MVP'de olay kaydı + ACP bildirimi.

### 3.2 commit — kullanıcının verdiği örnek

```
status  (read + bash(kısıtlı: git status/diff/log/diff --stat, read-only)) → changes_seen
stage   (bash(kısıtlı: git add/rm --cached, tracked+modified))            → staged
commit  (bash(kısıtlı: git commit -m ...))                                 → committed
verify  (read + bash(git log/show --stat, read-only))                      → committed_verified
```

Bash kuralı: `--force`, `--hard`, `reset`, `rebase`, `push`, `cherry-pick`,
`merge`, `clean`, `reflog delete`, `gc`, `filter-branch` REDDEDİLİR
(`FlowGate::bash_command_allowed` — deterministik kelime matcher; derin koruma
için mevcut `CompiledPolicy` katmanı ikinci savunma olarak kalır).

### 3.3 direct — "very basic" görevler (ağır akış zorunlu değil)

Tek aşama: `do` (TÜM ARAÇLAR) → `done`. Yalnızca stop gate denetimi
(kontrolsüz erken bitirme yok). `commit`/`direct`/`universal` seçimi
sınıflandırıcıya aittir — AI'a değil.

## 4. Bileşenler (yeni dosyalar)

Tümü `crates/codegen/xai-grok-shell/src/session/flow/` altında (tek ürün tek
kod tabanı — xai-* düzenlenebilir; yine de değişiklikler mümkün olduğunca az
noktada ve yeni dosyalarda).

### 4.1 `flow/definition.rs` — akış şablonu
```rust
pub struct FlowDefinition {
    pub name: &'static str,          // "universal" | "commit" | "direct"
    pub stages: Vec<StageDefinition>,
}
pub struct StageDefinition {
    pub id: StageId,                 // enum (yeni) — universal/commit/direct aşamaları
    pub tool_groups: &'static [ToolGroup],  // bu aşamada açık araç grupları
    pub produces: &'static [ArtifactId],    // aşama bitince kaydedilen kanıt
    pub directive: &'static str,     // modele verilecek yönerge metni
}
pub enum ToolGroup { Read, Search, Write, Bash, Web, Research, Computer, Plan, Task, Meta, All }
pub enum ArtifactId { ProblemList, FindingsArchive, DigestNote, StackChoice, StackVerified,
    DurationDecision, PlanDoc, BuildingBlocks, ExecutionGraph, WorkDone, Verified, Notified,
    ChangesSeen, Staged, Committed, CommittedVerified, Done }
pub enum StageId { Analyze, Research, Digest, StackSelect, StackVerify, Duration, Plan,
    Decompose, ParallelQuery, Execute, Verify, Notify, Status, Stage_, Commit, CommitVerify, Do }
```
- Varsayılan şablonlar Rust içine gömülü (`default_flows() -> &[FlowDefinition]`).
- İsteğe bağlı override: `config/flow/flows.toml` (varsa yüklenir, yoksa gömülü
  varsayılan kullanılır — deterministik).

### 4.2 `flow/state.rs` — saf durum makinesi
```rust
pub enum FlowPhase { Idle, InStage(StageId), Completed, Aborted }
pub struct FlowStateMachine { flow: &'static FlowDefinition, phase: FlowPhase,
    artifact_log: Vec<(ArtifactId, bool)>, redirect_count: u32 }
impl FlowStateMachine {
    pub fn start(&mut self, flow: &'static FlowDefinition) -> StageId;      // ilk aşama
    pub fn current_stage(&self) -> Option<StageId>;
    pub fn record_artifact(&mut self, id: ArtifactId) -> Option<StageId>;   // stage advance
    pub fn try_advance(&mut self) -> Option<StageId>;                       // next stage
    pub fn is_complete(&self) -> bool;
    pub fn stage_tool_groups(&self, stage: StageId) -> &'static [ToolGroup];
    pub fn stage_directive(&self, stage: StageId) -> &'static str;
    pub fn bump_redirect(&mut self) -> bool;                                // limit kontrolü
}
```

### 4.3 `flow/classifier.rs` — deterministik sınıflandırıcı (AI YOK)
```rust
pub struct FlowClassifier;  // saf fonksiyonlar
pub fn classify(prompt_text: &str, user_mode: UserMode, rules: &FlowRules) -> ClassifiedFlow;
pub enum ClassifiedFlow { Universal, Commit, Direct }
```
Sinyaller: `commit|commit et|commit at|git commit` → Commit; uzunluk < eşik VE
araştırma sinyali yok (`araştır|research|nedir|nasıl|kıyasla|karşılaştır|web`)
VE yazma sinyali yok → Direct; aksi halde Universal. Eşikler `FlowRules`
(rules.toml, gömülü varsayılan).

### 4.4 `flow/gate.rs` — tool aşama kapısı + bash kuralları
```rust
pub struct FlowGate;  // saf
pub fn tool_group_of(tool_id: &str) -> Option<ToolGroup>;       // statik tablo (registry id'leri)
pub fn tool_allowed(tool: &str, stage: Option<StageId>, flow: &FlowDefinition) -> bool;
pub fn filter_definitions(defs: Vec<ToolDefinition>, stage: Option<StageId>, flow: &FlowDefinition) -> Vec<ToolDefinition>;
pub fn bash_command_allowed(command: &str, stage: Option<StageId>, flow: &FlowDefinition) -> BashVerdict;
pub enum BashVerdict { Allow, Deny(&'static str) }               // nedeni direktife girer
```
Tool→grup tablosu: registry'deki **gerçek tool id'leri** (bölüm 6'da envanter).

### 4.5 `flow/store.rs` — kanıt deposu (kalıcılık)
```rust
pub struct FlowStore { dir: PathBuf, log: Vec<FlowRecord> }
pub struct FlowRecord { seq: u64, artifact: &'static str, ok: bool, detail: String }
impl FlowStore {
    pub fn open(session_dir: &Path) -> Self;              // session_dir/flow_events.jsonl
    pub fn record(&mut self, artifact: ArtifactId, ok: bool, detail: String);
    pub fn has(&self, artifact: ArtifactId) -> bool;
    pub fn progress(&self) -> Vec<(String, bool)>;
}
```
Artifact'lar hem makine hem insan okur: JSONL satırları `{"seq":1,"artifact":"problem_list","ok":true,"detail":"..."}`.

### 4.6 `flow/duration.rs` — problem süre sistemi
```rust
pub struct FlowDurationSystem;  // saf
pub fn decide(task_len: usize, classified: &ClassifiedFlow, user_mode: UserMode, rules: &FlowRules) -> DurationDecision;
pub enum DurationDecision { Mvp, Full }
```
Eşikler: `mvp_max_len` (varsayılan 800 char), `full` → classified == Universal VE
(araştırma sinyali || user_mode == Autonomous).

### 4.7 `flow/governor.rs` — kompozisyon + seam API'si
```rust
pub struct FlowGovernor {
    machine: FlowStateMachine, store: FlowStore,
    handle: Arc<parking_lot::Mutex<CheckpointCell>>, events: FlowEvents,
    classified: Option<ClassifiedFlow>, stage_active: bool, user_mode: UserMode,
}
pub struct CheckpointCell {
    pub active: bool,
    pub stage: Option<StageId>,
    pub request: Option<CheckpointRequest>,
    pub result: Option<CheckpointResult>,
    pub redirects: u32,
}
pub struct CheckpointRequest { pub stage: String, pub summary: String, pub files: Vec<String> }
pub struct CheckpointResult { pub accepted: bool, pub stage: String, pub directive: String, pub detail: String }

impl FlowGovernor {
    pub fn new(session_dir: &Path) -> Self;
    // A. handle_prompt tarafından çağrılır
    pub fn activate(&mut self, prompt_text: &str, user_mode: UserMode) -> Option<String>; // (flow, stage) — direktif döner
    // B. sampler_turn tool filtresi
    pub fn tool_definitions_filter(&self, defs: Vec<ToolDefinition>) -> Vec<ToolDefinition>;
    // C. stop gate danışmanı
    pub fn stop_decision(&mut self) -> Option<String>; // Some(direktif) = KeepWorking
    // D. goal round danışmanı
    pub fn round_decision(&mut self) -> RoundVerdict; // Continue(String) | EndTurn
    // Tool başarı kancası (checkpoint cell okuma + kanıt)
    pub fn on_tool_success(&mut self, tool: &str);
    // checkpoint doğrulama (araç çağrısı içinden — saf, hızlı)
    pub fn validate_checkpoint(&mut self, req: &CheckpointRequest, cwd: &Path) -> CheckpointResult;
}
pub enum RoundVerdict { Continue(String), EndTurn }
```
- `CheckpointCell` — checkpoint aracı ile session arasındaki paylaşımlı köprü
  (`Arc<parking_lot::Mutex<...>>`), `agent_rebuild.rs` üzerinden araca enjekte.
- Stop gate cap: `MAX_REDIRECTS_PER_STAGE = 3` — aşılırsa governor `EndTurn`
  döner (deterministik kilit açma).

### 4.8 `flow/events.rs` — olay yayını
```rust
pub struct FlowEvents { session_dir: PathBuf }
impl FlowEvents {
    pub fn phase_changed(&self, stage: StageId);
    pub fn checkpoint_rejected(&self, req: &CheckpointRequest, reason: &str);
    pub fn violation(&self, tool: &str, reason: &str);
    pub fn completed(&self);
}
```
Hedefler: `events.jsonl` (`SessionEventRecorder` — persistence.rs, omnitrix'e
ait) üzerinden `flow.*` etiketli kayıtlar + `unified_log` + ACP
`SessionNotification` (TUI'da mevcut bildirim akışında görünür).

### 4.9 `flow/mod.rs` — re-export'lar

## 5. `flow_checkpoint` aracı (yeni dosya)

`crates/codegen/xai-grok-tools/src/flow_checkpoint.rs`:
- `pub struct FlowCheckpointTool { handle: Arc<parking_lot::Mutex<CheckpointCell>> }`
- `xai_tool_runtime::Tool` implementasyonu; input:
  `{ stage: String, summary: String, files: Vec<String> }`.
- `execute`: `cell.request = Some(req)`; `cwd` (ToolCallContext) üzerinden
  `FlowGovernor::validate_checkpoint` saf doğrulamayı ÇAĞIRMAZ — tool yalnızca
  köprüye yazar, **asıl doğrulama session tarafında** `on_tool_success`
  kancasında çalışır:
  1. tool çağrısı → cell'e yazar → sonuç "işleniyor, tekrar çağır"
     döner (`{accepted:false, pending:true}`),
  2. session `handle_bridge_tool_success` → `on_tool_success` → cell'deki
     isteği doğrular, sonucu cell'e yazar, kanıtı kaydeder,
  3. model `flow_checkpoint`'i tekrar çağırır → cell'deki sonuç döner
     (accept + sonraki aşama direktifi | reject + düzeltici direktif).
- Kayıt: `agent_rebuild.rs` içinde `register_computer_use_tool` (agent_rebuild.rs:453)
  deseniyle `ToolBridge::register_mcp_tools("flow", FlowCheckpointTool::new(handle), None)`.
- Handle'ın session'a ulaşması: `AgentRebuildSpec`'e alan (diff_cas deseni,
  agent_rebuild.rs:88-95) — spawn sırasında üretilir, spec üzerinden hem
  governor hem araç aynı `Arc`'ı paylaşır.

## 6. Tool ID envanteri (registry — types.rs:666)

Doğrulanacak gerçek id'ler (implementasyon ajanı `ToolDefinition.name`
değerlerini registry/kayıttan teyit eder; aşağısı aday liste):

| Grup | Aday tool'lar |
|---|---|
| Read | read_file, list_dir, grep, read_file_concise, search (SearchTool), memory tools |
| Search | search_tool, grep, list_dir |
| Write | search_replace, apply_patch, opencode_edit, opencode_write, hashline_edit |
| Bash | bash, opencode_bash |
| Web | web_search, web_fetch |
| Research | grok_research |
| Computer | grok_computer |
| Plan | enter_plan_mode, exit_plan_mode, ask_user_question, todo_write, update_goal |
| Task | task, task_output, wait_tasks, workflow, scheduler_* |
| Meta | ask_user_question, use_tool, skill, todo_write (her aşamada açık — TodoGate/ask hayati) |

Not: `meta` grubu her aşamada açıktır (ask_user_question, todo_write, update_goal)
— aşama kapısı bunları asla kapatmaz.

## 7. Entegrasyon seam'leri (mevcut dosyalara cerrahi dokunuş)

| # | Dosya | Değişiklik |
|---|---|---|
| S1 | `acp_session.rs` (~780) | `pub(crate) flow_governor: Arc<parking_lot::Mutex<FlowGovernor>>` alanı + spawn'da init (plan_mode yanı) |
| S2 | `turn.rs` handle_prompt (prompt çözümleme sonrası, round döngüsü öncesi ~847) | `activate()` çağrısı (sentetik olmayan promptlar için) + dönen direktifi `prompt_blocks`'a sistem bloğu olarak ekle |
| S3 | `sampler_turn.rs` prepare_tool_definitions_inner (~229) | dönüş öncesi: `flow.tool_definitions_filter(defs)` |
| S4 | `stop_gate.rs` run_stop_gate (251, hook erken dönüşünden ÖNCE) | akış aktifse `stop_decision()` → Some = `KeepWorking{feedback}` |
| S5 | `goal.rs` run_goal_round_end (1332, en üst) | akış aktifse goal döngüsünü atla → `round_decision()` |
| S6 | `tool_calls.rs` handle_bridge_tool_success (~2142) | başarı sonrası `flow_governor.lock().on_tool_success(name)` |
| S7 | `agent_rebuild.rs` (~453) | checkpoint aracını kaydet (handle AgentRebuildSpec üzerinden) |

Dokunulmayan: `xai-agent-lifecycle`, `xai-file-utils` Event enum'u (kanıt olayları
`SessionEventRecorder`/unified_log üzerinden, enum'a dokunmadan).

## 8. Görünmezlik ve takip

- Kullanıcı: yalnızca aşama geçişleri + tamamlanma (ACP bildirimi).
- Kayıt: `flow_events.jsonl` (tüm ihlal/direktif/red — 7/24 denetim),
  `events.jsonl`'a `flow.*` etiketli satırlar.
- Model: direktifler sistem mesajı olarak; redler tool sonucu olarak.

## 9. Kapsam dışı (post-MVP)

- Judge (omni-router'dan taşınan yargıç) verify aşamasına bağlanması.
- `AutonomousLoop`'un (autonomous.rs — ölü kod) EXECUTE aşaması executor'ı
  olarak diriltilmesi.
- TUI'da ayrı "flow paneli" görünümü (pager tarafı).
- notify webhook/telgraf/telefon kanalları.
- `config/flow/*.toml` canlı reload.

## 10. Doğrulama (derleyici YASAK)

- Statik tutarlılık: yeni modül içi imza/çağrı eşleşmesi (reviewer ajanı).
- Seam tutarlılığı: her seam'de mevcut imza ile çağrı eşleşmesi (grep).
- Sembol taraması: `grep -rn "flow_governor\|FlowGovernor\|flow_checkpoint"` —
  referans edilen her sembol tanımlı olmalı.
- `cargo`/`rustc`/`rust-analyzer` KESİNLİKLE çalıştırılmaz.
