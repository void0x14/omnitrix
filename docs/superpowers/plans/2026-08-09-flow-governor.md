# Flow Governor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Oturuma gömülü deterministik akış durum makinesi (Flow Governor) ile AI'ın çok adımlı görev akışını asla inisiyatifiyle yönetememesini sağlamak: aşama kapıları, checkpoint el sıkışması, ihlal tespiti + görünmez düzeltme.

**Architecture:** `PlanModeTracker` deseni — `SessionActor` içinde `Arc<parking_lot::Mutex<FlowGovernor>>`; saf durum makinesi (`flow/state.rs`) + deterministik sınıflandırıcı (`flow/classifier.rs`) + aşama kapısı (`flow/gate.rs`) + kanıt deposu (`flow/store.rs`). Dört karar noktasına cerrahi kanca: handle_prompt (aktivasyon), prepare_tool_definitions_inner (tool gizleme), run_stop_gate (erken bitirme), run_goal_round_end (goal devam). `flow_checkpoint` aracı (xai-grok-tools) = deterministik aşama bitirme el sıkışması.

**Tech Stack:** Rust 2024, tokio (single-threaded actor), parking_lot, serde, xai_grok_tools (Tool trait), xai_grok_shell session modülü, xai_grok_telemetry unified_log.

## Global Constraints

- **KRITIK YASAK:** `cargo`, `rustc`, `rust-analyzer`, `cargo test/check/build/clippy/fmt` KESİNLİKLE ÇALIŞTIRILMAZ. Hiçbir rust süreci başlatılmaz. Doğrulama yalnızca statik inceleme (grep, dosya okuma, python-ile denge kontrolü) ile yapılır.
- Tek ürün tek kod tabanı: `crates/codegen/*` düzenlenebilir, ama değişiklikler yalnızca planın seam tablosundaki noktalarda ve yeni dosyalarda olur.
- `meta` tool grubu her aşamada açıktır (ask_user_question, todo_write, update_goal, use_tool, skill asla kilitlenmez).
- Yeni modül saf ve test edilebilir olmalı: `SessionActor`'a doğrudan referans YOK (`governor.rs` dahil), state `CheckpointCell` üzerinden paylaşılır.
- I6 (production'da panic yok) korunur: yeni kodda `unwrap`/`expect`/`panic!` YOK.
- Yorumlar Türkçe (kod tabanı geleneği).
- Komutlar: görevlerin her statik doğrulama adımı `bash` (rust dışı) ile çalışır.

---

### Task 1: Saf akış tanımı + durum makinesi

**Files:**
- Create: `crates/codegen/xai-grok-shell/src/session/flow/mod.rs`
- Create: `crates/codegen/xai-grok-shell/src/session/flow/definition.rs`
- Create: `crates/codegen/xai-grok-shell/src/session/flow/state.rs`

**Interfaces:**
- Consumes: yalnızca std.
- Produces (sonraki görevlerin kullandığı API — değiştirmeyin):
  - `definition.rs`: `ToolGroup` enum, `ArtifactId` enum, `StageId` enum, `StageDefinition`, `FlowDefinition`, `default_flows() -> &'static [FlowDefinition]`, `find_flow(&str) -> Option<&'static FlowDefinition>`.
  - `state.rs`: `FlowPhase` enum, `FlowStateMachine` (bkz. spec 4.2 — metot imzaları birebir).

- [ ] **Step 1: `flow/mod.rs` oluştur**

```rust
//! Deterministik akış boru hattı denetleyicisi (Flow Governor).
//!
//! Görev akışı ASLA AI insiyatifinde değildir: akış seçimi, aşama sırası,
//! kanıt doğrulaması ve ihlal düzeltmesi tamamen bu modüldeki saf durum
//! makinelerine aittir. AI yalnızca mevcut aşamanın araçlarıyla çalışır ve
//! aşamayı `flow_checkpoint` ile kapatır.

pub mod classifier;
pub mod definition;
pub mod duration;
pub mod events;
pub mod gate;
pub mod governor;
pub mod state;
pub mod store;
```

Not: modüller sonraki görevlerde oluşturulur; şimdilik yalnızca `definition`
ve `state` var olduğu için geçici olarak diğer satırları yorum satırı YAPMA —
görev 3 sonunda hepsi oluşacak; görev 1'de sadece `definition` + `state` satırlarını bırak, diğerlerini aşağıdaki gibi yaz ama dosyalar oluşana dek yorumda tut:

```rust
// pub mod classifier;  // Task 2
// pub mod duration;    // Task 2
// pub mod events;      // Task 3
// pub mod gate;        // Task 2
// pub mod governor;    // Task 3
// pub mod store;       // Task 3
```

- [ ] **Step 2: `definition.rs` yaz**

```rust
//! Akış şablonları: aşamalar, araç grupları, üretilen kanıtlar, yönergeler.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolGroup {
    Read, Search, Write, Bash, Web, Research, Computer, Plan, Task, Meta, All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ArtifactId {
    ProblemList, FindingsArchive, DigestNote, StackChoice, StackVerified,
    DurationDecision, PlanDoc, BuildingBlocks, ExecutionGraph, WorkDone, Verified, Notified,
    ChangesSeen, Staged, Committed, CommittedVerified, Done,
}

impl ArtifactId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProblemList => "problem_list",
            Self::FindingsArchive => "findings_archive",
            Self::DigestNote => "digest_note",
            Self::StackChoice => "stack_choice",
            Self::StackVerified => "stack_verified",
            Self::DurationDecision => "duration_decision",
            Self::PlanDoc => "plan_doc",
            Self::BuildingBlocks => "building_blocks",
            Self::ExecutionGraph => "execution_graph",
            Self::WorkDone => "work_done",
            Self::Verified => "verified",
            Self::Notified => "notified",
            Self::ChangesSeen => "changes_seen",
            Self::Staged => "staged",
            Self::Committed => "committed",
            Self::CommittedVerified => "committed_verified",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum StageId {
    Analyze, Research, Digest, StackSelect, StackVerify, Duration, Plan,
    Decompose, ParallelQuery, Execute, Verify, Notify,
    Status, Stage, Commit, CommitVerify, Do,
}

impl StageId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Analyze => "analyze",
            Self::Research => "research",
            Self::Digest => "digest",
            Self::StackSelect => "stack_select",
            Self::StackVerify => "stack_verify",
            Self::Duration => "duration",
            Self::Plan => "plan",
            Self::Decompose => "decompose",
            Self::ParallelQuery => "parallel_query",
            Self::Execute => "execute",
            Self::Verify => "verify",
            Self::Notify => "notify",
            Self::Status => "status",
            Self::Stage => "stage",
            Self::Commit => "commit",
            Self::CommitVerify => "commit_verify",
            Self::Do => "do",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StageDefinition {
    pub id: StageId,
    pub tool_groups: &'static [ToolGroup],
    pub produces: &'static [ArtifactId],
    pub directive: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct FlowDefinition {
    pub name: &'static str,
    pub stages: &'static [StageDefinition],
}

pub fn find_flow(name: &str) -> Option<&'static FlowDefinition> {
    default_flows().iter().find(|f| f.name == name)
}

/// Gömülü varsayılan akışlar. `config/flow/flows.toml` override'ı (Task 3)
/// bu fonksiyonun çıktısının üzerine biner.
pub fn default_flows() -> &'static [FlowDefinition] {
    const UNIVERSAL: &[StageDefinition] = &[
        StageDefinition {
            id: StageId::Analyze,
            tool_groups: &[ToolGroup::Read, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::ProblemList],
            directive: "ADIM 1/12 — ANALİZ: Problemi kelime kelime oku, küçük parçalara ayır ve ne anlattığını anla. Yalnızca okuma/arama araçlarını kullanabilirsin; henüz hiçbir şeye yazamaz, web'e giremez, kod değiştiremezsin. Anladıklarını zihinsel bir listeye dök ve işin bitince flow_checkpoint ile 'analyze' aşamasını kapat (dosyalar alanına boş liste ver; summary'ye parça listesini yaz).",
        },
        StageDefinition {
            id: StageId::Research,
            tool_groups: &[ToolGroup::Web, ToolGroup::Research, ToolGroup::Computer, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::FindingsArchive],
            directive: "ADIM 2/12 — ARAŞTIRMA: Listeyi kullanarak problemi günümüz gerçeğiyle derin araştır (web/research araçları; yüzey → derin → okyanus modlarını kullan). Bulguları depola (bir bulgular dosyasına yaz). İşin bitince flow_checkpoint ile 'research' aşamasını kapat — dosyalar alanına yazdığın bulgular dosyasının yolunu ver (çalışma dizinine göreli).",
        },
        StageDefinition {
            id: StageId::Digest,
            tool_groups: &[ToolGroup::Read, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::DigestNote],
            directive: "ADIM 3/12 — SİNDİRİM: Kaydettiğin araştırma sonuçlarını oku; çok büyükse parçalara bölüp tane tane işle. Özünü çıkar. Bitince flow_checkpoint ile 'digest' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::StackSelect,
            tool_groups: &[ToolGroup::Read, ToolGroup::Web, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::StackChoice],
            directive: "ADIM 4/12 — STACK SEÇİMİ: Çözüm için uygun yığınları (stack) sen belirle ve gerekçesiyle kaydet. Bitince flow_checkpoint ile 'stack_select' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::StackVerify,
            tool_groups: &[ToolGroup::Read, ToolGroup::Web, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::StackVerified],
            directive: "ADIM 5/12 — STACK DOĞRULAMA: Sanki hiç seçim yapmamış gibi internetin neyi önerdiğini araştır, kendi önerinle karşılaştır; çürütme yoluyla en doğru stacke ulaşana dek tekrarla. Nihai stacke karar verince flow_checkpoint ile 'stack_verify' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::Duration,
            tool_groups: &[ToolGroup::Meta],
            produces: &[ArtifactId::DurationDecision],
            directive: "ADIM 6/12 — SÜRE KARARI: Bu aşama sistemindir; araç kullanmana gerek yok. flow_checkpoint ile 'duration' aşamasını kapat (summary'ye 'sistem kararı bekleniyor' yaz).",
        },
        StageDefinition {
            id: StageId::Plan,
            tool_groups: &[ToolGroup::Read, ToolGroup::Plan, ToolGroup::Meta],
            produces: &[ArtifactId::PlanDoc],
            directive: "ADIM 7/12 — PLAN: Önceki aşamaların çıktılarına dayanarak plan çıkar; plan dosyasına yaz. Bitince flow_checkpoint ile 'plan' aşamasını kapat (plan dosyasının yolunu ver).",
        },
        StageDefinition {
            id: StageId::Decompose,
            tool_groups: &[ToolGroup::Read, ToolGroup::Plan, ToolGroup::Write, ToolGroup::Meta],
            produces: &[ArtifactId::BuildingBlocks],
            directive: "ADIM 8/12 — YAPI TAŞLARI: Planı küçük yapı taşlarına böl; her taş için dosya yollarıyla birlikte building_blocks dosyasına yaz. Bitince flow_checkpoint ile 'decompose' aşamasını kapat (dosya yolunu ver).",
        },
        StageDefinition {
            id: StageId::ParallelQuery,
            tool_groups: &[ToolGroup::Meta],
            produces: &[ArtifactId::ExecutionGraph],
            directive: "ADIM 9/12 — PARALELİZM: Sistem yapı taşlarının hangilerinin paralel, hangilerinin sıralı olduğunu belirler; araç gerekmez. flow_checkpoint ile 'parallel_query' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::Execute,
            tool_groups: &[ToolGroup::All],
            produces: &[ArtifactId::WorkDone],
            directive: "ADIM 10/12 — UYGULAMA: Yapı taşlarını uygula. Paralel taşlar için task araçlarıyla alt ajan görevlendirebilirsin (süre kararı 'mvp' ise alt ajan sayısını 2 ile sınırla). Tüm taşlar bitince flow_checkpoint ile 'execute' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::Verify,
            tool_groups: &[ToolGroup::Read, ToolGroup::Bash, ToolGroup::Task, ToolGroup::Meta],
            produces: &[ArtifactId::Verified],
            directive: "ADIM 11/12 — DOĞRULAMA: Sonucu doğrula (test/çalıştır/incele; yazma yok). Bitince flow_checkpoint ile 'verify' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::Notify,
            tool_groups: &[ToolGroup::Meta],
            produces: &[ArtifactId::Notified],
            directive: "ADIM 12/12 — BİLDİRİM: Sistem kullanıcıya bildirim gönderir. flow_checkpoint ile 'notify' aşamasını kapatarak görevi bitir.",
        },
    ];
    const COMMIT: &[StageDefinition] = &[
        StageDefinition {
            id: StageId::Status,
            tool_groups: &[ToolGroup::Read, ToolGroup::Bash, ToolGroup::Meta],
            produces: &[ArtifactId::ChangesSeen],
            directive: "COMMIT ADIMI 1/3 — DURUM: git status/diff/log ile değişiklikleri incele (salt okunur; bu aşamada hiçbir yazma komutu yasaktır). Değişiklikleri gördüğünü flow_checkpoint ile onayla.",
        },
        StageDefinition {
            id: StageId::Stage,
            tool_groups: &[ToolGroup::Bash, ToolGroup::Meta],
            produces: &[ArtifactId::Staged],
            directive: "COMMIT ADIMI 2/3 — STAGE: Yalnızca tracked ve modified dosyaları stage et (git add <yol>). --force/--hard/reset/push/rebase KESİNLİKLE YASAK. Bitince flow_checkpoint ile onayla.",
        },
        StageDefinition {
            id: StageId::Commit,
            tool_groups: &[ToolGroup::Bash, ToolGroup::Meta],
            produces: &[ArtifactId::Committed],
            directive: "COMMIT ADIMI 3/3 — COMMIT: git commit -m '<mesaj>' ile commit at (geçmişe saygı: force/hard yok). Bitince flow_checkpoint ile onayla.",
        },
        StageDefinition {
            id: StageId::CommitVerify,
            tool_groups: &[ToolGroup::Read, ToolGroup::Bash, ToolGroup::Meta],
            produces: &[ArtifactId::CommittedVerified],
            directive: "DOĞRULAMA: git log/show ile commit'i doğrula (salt okunur). flow_checkpoint ile kapat.",
        },
    ];
    const DIRECT: &[StageDefinition] = &[
        StageDefinition {
            id: StageId::Do,
            tool_groups: &[ToolGroup::All],
            produces: &[ArtifactId::Done],
            directive: "GÖREV: İstediğini yap. Yalnızca gerçekten bittiğinde flow_checkpoint ile 'do' aşamasını kapat.",
        },
    ];
    const FLOWS: &[FlowDefinition] = &[
        FlowDefinition { name: "universal", stages: UNIVERSAL },
        FlowDefinition { name: "commit", stages: COMMIT },
        FlowDefinition { name: "direct", stages: DIRECT },
    ];
    FLOWS
}
```

- [ ] **Step 3: `state.rs` yaz**

```rust
//! Saf akış durum makinesi. Oturum içi hiçbir duruma bağlanmaz; yalnızca
//! kendi içinde geçişleri yönetir (PlanModeTracker deseni).

use super::definition::{ArtifactId, FlowDefinition, StageDefinition, StageId, ToolGroup};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowPhase {
    Idle,
    InStage(StageId),
    Completed,
    Aborted,
}

pub struct FlowStateMachine {
    flow: &'static FlowDefinition,
    phase: FlowPhase,
    stage_index: usize,
    artifact_log: Vec<(ArtifactId, bool)>,
    redirect_count: u32,
}

/// Aşama başına izin verilen maksimum düzeltme (KeepWorking) sayısı.
/// Aşılırsa governor deterministik olarak turu bitirir (kilit açma).
pub const MAX_REDIRECTS_PER_STAGE: u32 = 3;

impl FlowStateMachine {
    pub fn idle() -> Self {
        Self {
            flow: Self::placeholder_flow(),
            phase: FlowPhase::Idle,
            stage_index: 0,
            artifact_log: Vec::new(),
            redirect_count: 0,
        }
    }

    fn placeholder_flow() -> &'static FlowDefinition {
        // Idle iken erişim yok; güvenli bir varsayılan (direct akışı).
        super::definition::find_flow("direct").expect("direct akışı tanımlı olmalı")
    }

    pub fn start(&mut self, flow: &'static FlowDefinition) -> StageId {
        self.flow = flow;
        self.phase = FlowPhase::InStage(flow.stages[0].id);
        self.stage_index = 0;
        self.artifact_log.clear();
        self.redirect_count = 0;
        flow.stages[0].id
    }

    pub fn flow(&self) -> &'static FlowDefinition {
        self.flow
    }

    pub fn phase(&self) -> FlowPhase {
        self.phase
    }

    pub fn current_stage(&self) -> Option<StageId> {
        match self.phase {
            FlowPhase::InStage(id) => Some(id),
            _ => None,
        }
    }

    pub fn current_stage_def(&self) -> Option<&'static StageDefinition> {
        self.current_stage().and_then(|id| self.stage_def(id))
    }

    pub fn stage_def(&self, id: StageId) -> Option<&'static StageDefinition> {
        self.flow.stages.iter().find(|s| s.id == id)
    }

    pub fn stage_tool_groups(&self, stage: StageId) -> &'static [ToolGroup] {
        self.stage_def(stage).map(|s| s.tool_groups).unwrap_or(&[])
    }

    pub fn stage_directive(&self, stage: StageId) -> &'static str {
        self.stage_def(stage).map(|s| s.directive).unwrap_or("")
    }

    /// Kanıt kaydı; aşamanın tüm `produces` kanıtları kayıtlıysa ilerler.
    /// Dönen değer: yeni aşama (ilerlendiyse).
    pub fn record_artifact(&mut self, id: ArtifactId, ok: bool) -> Option<StageId> {
        self.artifact_log.push((id, ok));
        if let Some(stage) = self.current_stage()
            && let Some(def) = self.stage_def(stage)
            && def.produces.iter().all(|p| self.artifact_log.iter().any(|(a, k)| a == p && *k))
        {
            return self.try_advance();
        }
        None
    }

    /// Bir sonraki aşamaya geçer; son aşamaysa Completed yapar.
    pub fn try_advance(&mut self) -> Option<StageId> {
        self.redirect_count = 0;
        let next = self.stage_index + 1;
        if next < self.flow.stages.len() {
            self.stage_index = next;
            let id = self.flow.stages[next].id;
            self.phase = FlowPhase::InStage(id);
            Some(id)
        } else {
            self.phase = FlowPhase::Completed;
            None
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.phase, FlowPhase::Completed)
    }

    /// Aşama kilitli mi? `meta` ve `All` her zaman açıktır.
    pub fn tool_unlocked(&self, group: ToolGroup) -> bool {
        if matches!(group, ToolGroup::Meta | ToolGroup::All) {
            return true;
        }
        match self.current_stage() {
            None => false,
            Some(stage) => self.stage_tool_groups(stage).contains(&group)
                || self.stage_tool_groups(stage).contains(&ToolGroup::All),
        }
    }

    /// Düzeltme sayacı; limit aşıldıysa false (governor turu bitirir).
    pub fn bump_redirect(&mut self) -> bool {
        self.redirect_count += 1;
        self.redirect_count <= MAX_REDIRECTS_PER_STAGE
    }

    pub fn redirect_count(&self) -> u32 {
        self.redirect_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::flow::definition::default_flows;

    #[test]
    fn universal_flow_advances_in_order() {
        let flows = default_flows();
        let universal = flows.iter().find(|f| f.name == "universal").unwrap();
        let mut sm = FlowStateMachine::idle();
        let first = sm.start(universal);
        assert_eq!(first, StageId::Analyze);
        // her aşama produces kanıtını kaydet
        let mut stage = sm.current_stage();
        while let Some(s) = stage {
            let def = sm.stage_def(s).unwrap();
            for p in def.produces {
                sm.record_artifact(*p, true);
            }
            stage = sm.current_stage();
        }
        assert!(sm.is_complete());
    }

    #[test]
    fn meta_group_always_unlocked() {
        let flows = default_flows();
        let universal = flows.iter().find(|f| f.name == "universal").unwrap();
        let mut sm = FlowStateMachine::idle();
        sm.start(universal);
        assert!(sm.tool_unlocked(ToolGroup::Meta));
        assert!(!sm.tool_unlocked(ToolGroup::Web)); // research aşaması kilitli
    }
}
```

- [ ] **Step 4: Statik doğrulama (derleyici YASAK — grep tabanlı)**

```bash
cd /home/void0x14/Documents/omnitrix
python3 - <<'EOF'
import re
for f in ["crates/codegen/xai-grok-shell/src/session/flow/definition.rs",
          "crates/codegen/xai-grok-shell/src/session/flow/state.rs"]:
    src = open(f).read()
    depth = 0
    for ch in src:
        if ch == '{': depth += 1
        elif ch == '}': depth -= 1
        if depth < 0:
            print(f"FAIL: {f} negative brace depth"); break
    else:
        print(f"{f}: braces ok (depth={depth})")
    # enum/struct/method kapanış sayıları
    for pat in ["enum ", "struct ", "fn ", "impl "]:
        print(f"  {pat.strip()}: {len(re.findall(pat, src))}")
EOF
```
Expected: her dosya için `braces ok`, `depth=0`.

- [ ] **Step 5: Commit**

```bash
git add crates/codegen/xai-grok-shell/src/session/flow/
git commit -m "feat(flow): saf akış tanımı + durum makinesi (definition/state)"
```

---

### Task 2: Sınıflandırıcı + süre sistemi + aşama kapısı

**Files:**
- Create: `crates/codegen/xai-grok-shell/src/session/flow/classifier.rs`
- Create: `crates/codegen/xai-grok-shell/src/session/flow/duration.rs`
- Create: `crates/codegen/xai-grok-shell/src/session/flow/gate.rs`

**Interfaces:**
- Consumes: Task 1 — `definition::{ArtifactId, FlowDefinition, StageDefinition, StageId, ToolGroup}`, `find_flow`; `state::{FlowPhase, FlowStateMachine}`.
- Produces (değiştirmeyin):
  - `classifier.rs`: `UserMode` enum (`UserOriented`, `Autonomous`), `FlowRules` struct (+ `default_rules()`), `ClassifiedFlow` enum (`Universal`, `Commit`, `Direct`), `FlowClassifier::classify(&str, UserMode, &FlowRules) -> ClassifiedFlow`, `ClassifiedFlow::flow() -> &'static FlowDefinition`.
  - `duration.rs`: `DurationDecision` enum (`Mvp`, `Full`), `FlowDurationSystem::decide(usize, &ClassifiedFlow, UserMode, &FlowRules) -> DurationDecision`.
  - `gate.rs`: `BashVerdict` enum (`Allow`, `Deny(&'static str)`), `FlowGate` (statik) — `tool_group_of(&str) -> Option<ToolGroup>`, `tool_allowed(&str, &FlowStateMachine) -> bool`, `bash_command_allowed(&str, &FlowStateMachine) -> BashVerdict`.
- Tool id'leri (registry'den doğrulanmış adaylar — `ToolDefinition.name` ile eşleşecek): bash, opencode_bash, read_file, read_file_concise, list_dir, grep, search (SearchTool), search_replace, apply_patch, opencode_edit, opencode_write, hashline_edit, web_search, web_fetch, grok_research, grok_computer, enter_plan_mode, exit_plan_mode, ask_user_question, todo_write, update_goal, task, task_output, wait_tasks, workflow, lsp, use_tool, skill.

- [ ] **Step 1: `classifier.rs` yaz**

```rust
//! Deterministik görev sınıflandırıcı. AI YOK: akış seçimi burada, saf
//! kelime/eşik kurallarıyla yapılır. `config/flow/rules.toml` override
//! edilebilir (Task 3), varsayılan gömülüdür.

use super::definition::FlowDefinition;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserMode {
    UserOriented,
    Autonomous,
}

#[derive(Debug, Clone)]
pub struct FlowRules {
    /// commit akışını tetikleyen kelimeler (küçük harfe çevrilmiş prompt'ta aranır)
    pub commit_keywords: Vec<&'static str>,
    /// araştırma sinyali kelimeleri
    pub research_keywords: Vec<&'static str>,
    /// yazma sinyali kelimeleri (dosya/kod değişikliği isteyen görev)
    pub write_keywords: Vec<&'static str>,
    /// direct akış eşiği: bu değerden kısa ve araştırma/yazma sinyali yoksa direct
    pub direct_max_len: usize,
    /// commit akışı için minimal uzunluk üst sınırı (uzun metin = universal)
    pub commit_max_len: usize,
    /// mvp kararı için task uzunluk üst sınırı
    pub mvp_max_len: usize,
}

impl Default for FlowRules {
    fn default() -> Self {
        Self {
            commit_keywords: vec!["commit", "commit et", "commit at", "git commit", "değişiklikleri kaydet"],
            research_keywords: vec!["araştır", "research", "nedir", "nasıl çalışır", "kıyasla", "karşılaştır",
                "web", "kaynak", "güncel", "2026", "ne yapmalı", "en iyi"],
            write_keywords: vec!["yaz", "ekle", "oluştur", "düzelt", "fix", "implement", "kodla",
                "dosya", "fonksiyon", "modül", "hata", "bug", "refactor", "feature"],
            direct_max_len: 120,
            commit_max_len: 400,
            mvp_max_len: 800,
        }
    }
}

pub fn default_rules() -> FlowRules {
    FlowRules::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifiedFlow {
    Universal,
    Commit,
    Direct,
}

impl ClassifiedFlow {
    pub fn flow(&self) -> &'static FlowDefinition {
        let name = match self {
            Self::Universal => "universal",
            Self::Commit => "commit",
            Self::Direct => "direct",
        };
        super::definition::find_flow(name).expect("varsayılan akış tanımlı olmalı")
    }
}

pub struct FlowClassifier;

impl FlowClassifier {
    pub fn classify(prompt: &str, _mode: UserMode, rules: &FlowRules) -> ClassifiedFlow {
        let lower = prompt.to_lowercase();
        let has = |words: &[&'static str]| words.iter().any(|w| lower.contains(w));

        // 1) commit sinyali
        if has(&rules.commit_keywords) && lower.len() <= rules.commit_max_len {
            return ClassifiedFlow::Commit;
        }
        // 2) direct: kısa ve araştırma/yazma sinyali yok
        let research = has(&rules.research_keywords);
        let write = has(&rules.write_keywords);
        if lower.len() <= rules.direct_max_len && !research && !write {
            return ClassifiedFlow::Direct;
        }
        // 3) aksi halde universal
        ClassifiedFlow::Universal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_detected() {
        let r = default_rules();
        let f = FlowClassifier::classify("commit at bakalım değişiklikleri", UserMode::UserOriented, &r);
        assert_eq!(f, ClassifiedFlow::Commit);
    }

    #[test]
    fn short_trivial_goes_direct() {
        let r = default_rules();
        let f = FlowClassifier::classify("bu dosyayı sil", UserMode::UserOriented, &r);
        assert_eq!(f, ClassifiedFlow::Direct);
    }

    #[test]
    fn research_task_goes_universal() {
        let r = default_rules();
        let f = FlowClassifier::classify(
            "2026 yılında en iyi rust web frameworkü nedir araştır ve karşılaştır",
            UserMode::UserOriented, &r);
        assert_eq!(f, ClassifiedFlow::Universal);
    }
}
```

- [ ] **Step 2: `duration.rs` yaz**

```rust
//! Problem süre sistemi: MVP mi TAM mı kararını AI DEĞİL, bu saf sistem verir.

use super::classifier::{ClassifiedFlow, FlowRules, UserMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationDecision {
    Mvp,
    Full,
}

pub struct FlowDurationSystem;

impl FlowDurationSystem {
    pub fn decide(task_len: usize, classified: &ClassifiedFlow, mode: UserMode, rules: &FlowRules) -> DurationDecision {
        match classified {
            ClassifiedFlow::Commit | ClassifiedFlow::Direct => DurationDecision::Mvp,
            ClassifiedFlow::Universal => {
                if matches!(mode, UserMode::Autonomous) {
                    return DurationDecision::Full;
                }
                if task_len <= rules.mvp_max_len {
                    DurationDecision::Mvp
                } else {
                    DurationDecision::Full
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::flow::classifier::default_rules;

    #[test]
    fn short_task_is_mvp() {
        let r = default_rules();
        let d = FlowDurationSystem::decide(50, &ClassifiedFlow::Universal, UserMode::UserOriented, &r);
        assert_eq!(d, DurationDecision::Mvp);
    }

    #[test]
    fn autonomous_mode_is_full() {
        let r = default_rules();
        let d = FlowDurationSystem::decide(50, &ClassifiedFlow::Universal, UserMode::Autonomous, &r);
        assert_eq!(d, DurationDecision::Full);
    }
}
```

- [ ] **Step 3: `gate.rs` yaz**

```rust
//! Aşama kapısı: tool id → grup eşlemesi ve bash kural denetimi.
//! Eşleme statiktir (registry gerçekleri); `config/flow/rules.toml` override'ı
//! Task 3'te eklenebilir.

use super::definition::ToolGroup;
use super::state::FlowStateMachine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashVerdict {
    Allow,
    Deny(&'static str),
}

/// Tool id (ToolDefinition.name) → grup eşleme tablosu.
/// Registry: crates/codegen/xai-grok-tools/src/registry/types.rs:666+.
pub fn tool_group_of(tool_id: &str) -> Option<ToolGroup> {
    let g = match tool_id {
        "read_file" | "read_file_concise" | "list_dir" | "grep" | "hashline_read"
        | "codex_read_file" | "opencode_read" | "search" => ToolGroup::Read,
        "search_tool" | "grep" | "list_dir" => ToolGroup::Search,
        "search_replace" | "apply_patch" | "opencode_edit" | "opencode_write"
        | "hashline_edit" | "image_gen" | "image_edit" => ToolGroup::Write,
        "bash" | "opencode_bash" => ToolGroup::Bash,
        "web_search" | "web_fetch" => ToolGroup::Web,
        "grok_research" => ToolGroup::Research,
        "grok_computer" => ToolGroup::Computer,
        "enter_plan_mode" | "exit_plan_mode" => ToolGroup::Plan,
        "task" | "task_output" | "wait_tasks" | "workflow" | "scheduler_create"
        | "scheduler_delete" | "scheduler_list" => ToolGroup::Task,
        // meta: her aşamada açık
        "ask_user_question" | "todo_write" | "update_goal" | "use_tool" | "skill"
        | "memory_search" | "memory_get" => ToolGroup::Meta,
        _ => return None, // bilinmeyen id: kapı yok sayar (mevcut mekanizmalar korur)
    };
    Some(g)
}

pub struct FlowGate;

impl FlowGate {
    pub fn tool_allowed(tool_id: &str, sm: &FlowStateMachine) -> bool {
        match tool_group_of(tool_id) {
            None => true,
            Some(group) => sm.tool_unlocked(group),
        }
    }

    /// Bash komut kelime denetimi — aşamaya göre. Kural matcher'ı deterministiktir;
    /// derin koruma için mevcut CompiledPolicy ikinci savunma olarak kalır.
    pub fn bash_command_allowed(command: &str, sm: &FlowStateMachine) -> BashVerdict {
        let stage = match sm.current_stage() {
            None => return BashVerdict::Allow,
            Some(s) => s,
        };
        use super::definition::StageId;
        // universal akış: yalnızca execute/verify aşamalarında bash açık (grup zaten
        // kapıyor) — burada yalnızca commit akışının komut kuralları.
        if sm.flow().name != "commit" {
            return BashVerdict::Allow;
        }
        let lower = command.to_lowercase();
        let deny: &[(&str, &str)] = &[
            ("--force", "force komutları yasak: geçmişe saygı (I-kural)"),
            ("--hard", "hard reset yasak"),
            ("push", "push yasak (commit akışı yalnızca yerel)"),
            ("rebase", "rebase yasak"),
            ("cherry-pick", "cherry-pick yasak"),
            ("reset", "reset yasak"),
            ("merge", "merge yasak"),
            ("clean", "clean yasak"),
            ("reflog delete", "reflog delete yasak"),
            ("filter-branch", "filter-branch yasak"),
            ("gc", "gc yasak"),
        ];
        for (pat, reason) in deny {
            if lower.contains(pat) {
                return BashVerdict::Deny(reason);
            }
        }
        // Aşama kuralı: status/verify aşamasında yalnızca salt-okunur git komutları
        match stage {
            StageId::Status | StageId::CommitVerify => {
                let readonly = ["git status", "git diff", "git log", "git show", "git stash list",
                    "git branch", "git remote", "git config", "git rev-parse", "git ls-files"];
                if lower.starts_with("git") && !readonly.iter().any(|c| lower.starts_with(c)) {
                    return BashVerdict::Deny("bu aşamada yalnızca salt-okunur git komutlarına izin var");
                }
            }
            StageId::Stage => {
                let allowed = ["git add", "git rm", "git status", "git diff", "git ls-files"];
                if lower.starts_with("git") && !allowed.iter().any(|c| lower.starts_with(c)) {
                    return BashVerdict::Deny("stage aşamasında yalnızca git add/rm/status/diff/ls-files");
                }
            }
            StageId::Commit => {
                if lower.starts_with("git") && !lower.starts_with("git commit") {
                    return BashVerdict::Deny("commit aşamasında yalnızca git commit");
                }
            }
            _ => {}
        }
        BashVerdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::flow::definition::{default_flows, StageId, ToolGroup};
    use crate::session::flow::state::FlowStateMachine;

    fn commit_sm() -> FlowStateMachine {
        let flows = default_flows();
        let commit = flows.iter().find(|f| f.name == "commit").unwrap();
        let mut sm = FlowStateMachine::idle();
        sm.start(commit);
        sm
    }

    #[test]
    fn web_locked_outside_research() {
        let flows = default_flows();
        let universal = flows.iter().find(|f| f.name == "universal").unwrap();
        let mut sm = FlowStateMachine::idle();
        sm.start(universal);
        assert!(!sm.tool_unlocked(ToolGroup::Web));
        assert!(!FlowGate::tool_allowed("web_search", &sm));
        assert!(FlowGate::tool_allowed("read_file", &sm));
        assert!(FlowGate::tool_allowed("ask_user_question", &sm)); // meta
    }

    #[test]
    fn force_flag_denied() {
        let sm = commit_sm();
        assert_eq!(
            FlowGate::bash_command_allowed("git reset --hard HEAD~1", &sm),
            BashVerdict::Deny("hard reset yasak")
        );
    }

    #[test]
    fn status_stage_readonly() {
        let sm = commit_sm();
        assert_eq!(
            FlowGate::bash_command_allowed("git commit -m x", &sm),
            BashVerdict::Deny("bu aşamada yalnızca salt-okunur git komutlarına izin var")
        );
        assert_eq!(FlowGate::bash_command_allowed("git status", &sm), BashVerdict::Allow);
    }
}
```

- [ ] **Step 4: `mod.rs`'de ilgili satırların yorumunu kaldır**

`flow/mod.rs` içinde `// pub mod classifier; // Task 2`, `// pub mod duration;`, `// pub mod gate;` satırlarını aktifleştir.

- [ ] **Step 5: Statik doğrulama**

```bash
cd /home/void0x14/Documents/omnitrix
python3 - <<'EOF'
import re, glob
for f in sorted(glob.glob("crates/codegen/xai-grok-shell/src/session/flow/*.rs")):
    src = open(f).read()
    depth = 0
    for ch in src:
        depth += (ch == '{') - (ch == '}')
        if depth < 0: break
    print(f, "braces", "ok" if depth == 0 else f"FAIL depth={depth}")
EOF
grep -rn "flow::" crates/codegen/xai-grok-shell/src/session/flow/*.rs | grep "use " | head -20
```
Expected: tüm dosyalar `braces ok`; `use super::...` yolları doğru.

- [ ] **Step 6: Commit**

```bash
git add crates/codegen/xai-grok-shell/src/session/flow/
git commit -m "feat(flow): deterministik sınıflandırıcı + süre sistemi + aşama kapısı"
```

---

### Task 3: Kanıt deposu + olaylar + Governor (kompozisyon)

**Files:**
- Create: `crates/codegen/xai-grok-shell/src/session/flow/store.rs`
- Create: `crates/codegen/xai-grok-shell/src/session/flow/events.rs`
- Create: `crates/codegen/xai-grok-shell/src/session/flow/governor.rs`
- Modify: `crates/codegen/xai-grok-shell/src/session/flow/mod.rs` (yorum satırlarını aç)

**Interfaces:**
- Consumes: Task 1 + Task 2 API'leri (birebir imzalar).
- Produces (değiştirmeyin):
  - `store.rs`: `FlowRecord { seq: u64, artifact: String, ok: bool, detail: String }`, `FlowStore { open(&Path) -> Self, record(&mut self, ArtifactId, bool, String), has(&self, ArtifactId) -> bool, progress(&self) -> Vec<FlowRecord> }` — JSONL: `session_dir/flow_events.jsonl`.
  - `events.rs`: `FlowEvents { new(&Path) -> Self, phase_changed(&self, StageId), checkpoint_rejected(&self, &str, &str), violation(&self, &str, &str), completed(&self) }` — unified_log (`xai_grok_telemetry::unified_log::info`) + `flow_events.jsonl` ortak dosyası (FlowStore ile aynı dosya, tek yazar: governor).
  - `governor.rs`: `CheckpointCell`, `CheckpointRequest { stage: String, summary: String, files: Vec<String> }`, `CheckpointResult { accepted: bool, pending: bool, stage: String, directive: String, detail: String }`, `RoundVerdict` enum (`Continue(String)`, `EndTurn`), `FlowGovernor` (spec 4.7 metotları birebir).
- `ToolDefinition` tipi: `sampler_turn.rs`'deki mevcut import'tan (ajan doğrular: `xai_grok_tools::...::ToolDefinition` ya da tool_protocol). `tool_definitions_filter(&self, Vec<ToolDefinition>) -> Vec<ToolDefinition>` imzasında o tip kullanılır.
- `UserMode` governor'a `activate` ile verilir.

- [ ] **Step 1: `store.rs` yaz**

```rust
//! Kanıt deposu: aşama kanıtlarının kalıcı JSONL kaydı (makine + insan okur).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::definition::ArtifactId;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowRecord {
    pub seq: u64,
    pub artifact: String,
    pub ok: bool,
    pub detail: String,
}

pub struct FlowStore {
    path: PathBuf,
    records: Vec<FlowRecord>,
}

const FLOW_EVENTS_FILE: &str = "flow_events.jsonl";

impl FlowStore {
    pub fn open(session_dir: &Path) -> Self {
        let path = session_dir.join(FLOW_EVENTS_FILE);
        let records = std::fs::read_to_string(&path)
            .map(|s| s.lines().filter_map(|l| serde_json::from_str::<FlowRecord>(l).ok()).collect())
            .unwrap_or_default();
        Self { path, records }
    }

    pub fn record(&mut self, artifact: ArtifactId, ok: bool, detail: String) {
        let seq = self.records.len() as u64 + 1;
        let rec = FlowRecord { seq, artifact: artifact.as_str().to_string(), ok, detail };
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{}", serde_json::to_string(&rec).unwrap_or_default());
        }
        self.records.push(rec);
    }

    pub fn has(&self, artifact: ArtifactId) -> bool {
        self.records.iter().any(|r| r.artifact == artifact.as_str() && r.ok)
    }

    pub fn progress(&self) -> Vec<FlowRecord> {
        self.records.clone()
    }
}
```

- [ ] **Step 2: `events.rs` yaz**

```rust
//! Akış olay yayını: unified_log + flow_events.jsonl (FlowStore ile aynı dosya).
//! Arayüz gürültüsü üretmez; denetim kaydı üretir.

use std::path::Path;

use super::definition::StageId;

pub struct FlowEvents {
    session_dir: std::path::PathBuf,
}

impl FlowEvents {
    pub fn new(session_dir: &Path) -> Self {
        Self { session_dir: session_dir.to_path_buf() }
    }

    fn emit(&self, tag: &str, payload: serde_json::Value) {
        xai_grok_telemetry::unified_log::info(
            tag,
            None,
            Some(serde_json::json!({ "flow": true, "detail": payload })),
        );
        let path = self.session_dir.join("flow_events.jsonl");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let _ = writeln!(f, "{}", serde_json::json!({
                "event": tag, "detail": payload
            }));
        }
    }

    pub fn phase_changed(&self, stage: StageId) {
        self.emit("flow.phase_changed", serde_json::json!({ "stage": stage.as_str() }));
    }

    pub fn checkpoint_rejected(&self, stage: &str, reason: &str) {
        self.emit("flow.checkpoint_rejected", serde_json::json!({ "stage": stage, "reason": reason }));
    }

    pub fn violation(&self, tool: &str, reason: &str) {
        self.emit("flow.violation", serde_json::json!({ "tool": tool, "reason": reason }));
    }

    pub fn completed(&self) {
        self.emit("flow.completed", serde_json::json!({}));
    }
}
```

- [ ] **Step 3: `governor.rs` yaz**

```rust
//! Flow Governor — akış kompozisyonu ve seam API'si.
//!
//! SessionActor bu yapıyı `Arc<parking_lot::Mutex<FlowGovernor>>` olarak tutar
//! (PlanModeTracker deseni) ve dört karar noktasında çağırır:
//! A) handle_prompt → activate, B) tool filtresi → tool_definitions_filter,
//! C) stop gate → stop_decision, D) goal round → round_decision.
//! Checkpoint aracıyla köprü: paylaşımlı `CheckpointCell` (Arc<Mutex>).

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use super::classifier::{ClassifiedFlow, FlowClassifier, FlowRules, UserMode};
use super::definition::{ArtifactId, FlowDefinition, StageId};
use super::duration::{DurationDecision, FlowDurationSystem};
use super::events::FlowEvents;
use super::gate::FlowGate;
use super::state::{FlowPhase, FlowStateMachine};
use super::store::FlowStore;

/// Checkpoint aracı ile session arasındaki paylaşımlı köprü.
#[derive(Debug, Default)]
pub struct CheckpointCell {
    pub active: bool,
    pub stage: Option<StageId>,
    pub request: Option<CheckpointRequest>,
    pub result: Option<CheckpointResult>,
    pub redirects: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckpointRequest {
    pub stage: String,
    pub summary: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckpointResult {
    pub accepted: bool,
    pub pending: bool,
    pub stage: String,
    pub directive: String,
    pub detail: String,
}

impl CheckpointResult {
    fn pending() -> Self {
        Self {
            accepted: false,
            pending: true,
            stage: String::new(),
            directive: "checkpoint işleniyor; tekrar çağır".to_string(),
            detail: String::new(),
        }
    }

    fn done(accepted: bool, stage: &StageId, directive: &str, detail: String) -> Self {
        Self {
            accepted,
            pending: false,
            stage: stage.as_str().to_string(),
            directive: directive.to_string(),
            detail,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundVerdict {
    Continue(String),
    EndTurn,
}

pub struct FlowGovernor {
    machine: FlowStateMachine,
    store: FlowStore,
    events: FlowEvents,
    cell: std::sync::Arc<Mutex<CheckpointCell>>,
    classified: Option<ClassifiedFlow>,
    user_mode: UserMode,
    rules: FlowRules,
}

impl FlowGovernor {
    pub fn new(session_dir: &Path) -> Self {
        let cell = std::sync::Arc::new(Mutex::new(CheckpointCell::default()));
        Self {
            machine: FlowStateMachine::idle(),
            store: FlowStore::open(session_dir),
            events: FlowEvents::new(session_dir),
            cell,
            classified: None,
            user_mode: UserMode::UserOriented,
            rules: FlowRules::default(),
        }
    }

    /// Paylaşımlı köprü (checkpoint aracına enjekte edilir).
    pub fn cell(&self) -> std::sync::Arc<Mutex<CheckpointCell>> {
        self.cell.clone()
    }

    pub fn is_active(&self) -> bool {
        self.classified.is_some() && !self.machine.is_complete()
    }

    pub fn flow(&self) -> Option<&'static FlowDefinition> {
        self.classified.map(|c| c.flow())
    }

    // ── A) handle_prompt aktivasyonu ──────────────────────────────────────
    /// Görevi sınıflandırır, akışı başlatır, ilk aşama direktifini döndürür.
    /// Direct akışa düşen görevlerde dahi stop gate denetimi çalışır.
    pub fn activate(&mut self, prompt_text: &str, user_mode: UserMode) -> Option<String> {
        if self.is_active() {
            return None; // mevcut akış korunur
        }
        let classified = FlowClassifier::classify(prompt_text, user_mode, &self.rules);
        self.classified = Some(classified);
        self.user_mode = user_mode;
        let flow = classified.flow();
        let first = self.machine.start(flow);
        self.store.record(
            ArtifactId::ProblemList,
            true,
            format!("akış seçimi (sistem): {}", flow.name),
        );
        if classified != ClassifiedFlow::Direct {
            self.store.record(
                ArtifactId::DurationDecision,
                true,
                format!(
                    "süre kararı (sistem): {:?}",
                    FlowDurationSystem::decide(
                        prompt_text.len(),
                        &classified,
                        user_mode,
                        &self.rules
                    )
                ),
            );
        }
        self.cell.lock().active = true;
        self.cell.lock().stage = Some(first);
        self.events.phase_changed(first);
        Some(self.machine.stage_directive(first).to_string())
    }

    // ── B) tool görünürlük filtresi ───────────────────────────────────────
    pub fn tool_definitions_filter<T: Clone>(&self, defs: Vec<T>, id_of: impl Fn(&T) -> &str) -> Vec<T> {
        if !self.is_active() {
            return defs;
        }
        defs.into_iter()
            .filter(|d| FlowGate::tool_allowed(id_of(d), &self.machine))
            .collect()
    }

    // ── C) stop gate danışmanı ────────────────────────────────────────────
    /// Some(direktif) = KeepWorking (aşama kanıtı eksik), None = dur.
    pub fn stop_decision(&mut self) -> Option<String> {
        if !self.is_active() {
            return None;
        }
        if self.machine.bump_redirect() {
            let stage = self.machine.current_stage().unwrap_or(StageId::Do);
            let directive = self.machine.stage_directive(stage);
            let detail = format!("aşama {} tamamlanmadı; stop reddedildi", stage.as_str());
            self.store.record(ArtifactId::DurationDecision, false, detail.clone());
            self.events.violation("stop", &detail);
            Some(directive.to_string())
        } else {
            None // redirect limiti doldu → deterministik kilit açma
        }
    }

    // ── D) goal round danışmanı ───────────────────────────────────────────
    pub fn round_decision(&mut self) -> RoundVerdict {
        if !self.is_active() {
            return RoundVerdict::EndTurn;
        }
        match self.machine.current_stage() {
            None => RoundVerdict::EndTurn,
            Some(stage) => RoundVerdict::Continue(self.machine.stage_directive(stage).to_string()),
        }
    }

    // ── Tool başarı kancası (S6: handle_bridge_tool_success) ──────────────
    pub fn on_tool_success(&mut self, tool: &str) {
        if !self.is_active() {
            return;
        }
        // checkpoint isteği varsa doğrula
        let mut cell = self.cell.lock();
        if let Some(req) = cell.request.take() {
            let stage = cell.stage.unwrap_or(StageId::Do);
            let result = self.validate_checkpoint(&req, stage, &cell);
            cell.result = Some(result);
        }
        // ihlal kaydı: bu tool bu aşamada yasaklanmış bir gruba mı ait?
        if let Some(group) = super::gate::tool_group_of(tool) {
            if !self.machine.tool_unlocked(group) {
                drop(cell);
                self.events.violation(tool, "kilitli grup çağrısı (filtre aşıldı)");
            }
        }
    }

    /// Checkpoint doğrulaması — saf, hızlı; tool isteği + mevcut aşama + cwd.
    pub fn validate_checkpoint(
        &mut self,
        req: &CheckpointRequest,
        current: StageId,
        cell: &CheckpointCell,
    ) -> CheckpointResult {
        // aşama sırası kontrolü
        if req.stage != current.as_str() {
            self.events.checkpoint_rejected(&req.stage, "yanlış aşama");
            return CheckpointResult::done(
                false,
                &current,
                self.machine.stage_directive(current),
                format!("Bu aşama değil: sen {} aşamasındasın. {} aşamasına geçemezsin.", current.as_str(), req.stage),
            );
        }
        // kanıt dosyası kontrolü (isteğe bağlı files alanı)
        for f in &req.files {
            let p = Path::new(f);
            if !p.is_file() || p.metadata().map(|m| m.len() == 0).unwrap_or(true) {
                self.events.checkpoint_rejected(&req.stage, "kanıt dosyası eksik/boş");
                return CheckpointResult::done(
                    false,
                    &current,
                    self.machine.stage_directive(current),
                    format!("Kanıt dosyası eksik veya boş: {f}. Önce aşama çıktısını üret, sonra kapat.", ),
                );
            }
        }
        // kabul: kanıtları kaydet, aşamayı ilerlet
        if let Some(def) = self.machine.stage_def(current) {
            for artifact in def.produces {
                self.store.record(*artifact, true, req.summary.clone());
                self.machine.record_artifact(*artifact, true);
            }
        }
        let next = self.machine.current_stage();
        cell.stage = next;
        match next {
            None => {
                self.cell.lock().active = false;
                self.events.completed();
                CheckpointResult::done(true, &current, "", "Görev tamamlandı.".to_string())
            }
            Some(ns) => {
                self.events.phase_changed(ns);
                CheckpointResult::done(
                    true,
                    &current,
                    self.machine.stage_directive(ns),
                    format!("Aşama tamam: {} → {}", current.as_str(), ns.as_str()),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn temp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("flow_gov_test_{}", std::process::id()));
        std::fs::create_dir_all(&d).ok();
        d
    }

    #[test]
    fn activate_returns_directive_and_classifies() {
        let mut g = FlowGovernor::new(&temp_dir());
        let d = g.activate("2026'da en iyi rust frameworkü nedir araştır", UserMode::UserOriented);
        assert!(d.is_some());
        assert_eq!(g.flow().map(|f| f.name), Some("universal"));
        assert!(g.is_active());
    }

    #[test]
    fn checkpoint_wrong_stage_rejected() {
        let mut g = FlowGovernor::new(&temp_dir());
        g.activate("2026'da en iyi rust frameworkü nedir araştır", UserMode::UserOriented);
        let cell = g.cell();
        let req = CheckpointRequest { stage: "research".to_string(), summary: "x".to_string(), files: vec![] };
        let res = g.validate_checkpoint(&req, StageId::Analyze, &cell.lock());
        assert!(!res.accepted);
        assert!(!res.directive.is_empty());
    }

    #[test]
    fn stop_decision_blocks_until_done() {
        let mut g = FlowGovernor::new(&temp_dir());
        g.activate("selam", UserMode::UserOriented); // direct akış
        // direct akış: stop denetimi tamamlanana kadar engeller
        assert!(g.stop_decision().is_some());
    }
}
```

- [ ] **Step 4: `mod.rs` yorum satırlarını aç** (classifier/duration/events/gate/governor/store)

- [ ] **Step 5: Statik doğrulama**

```bash
cd /home/void0x14/Documents/omnitrix
python3 - <<'EOF'
import glob
for f in sorted(glob.glob("crates/codegen/xai-grok-shell/src/session/flow/*.rs")):
    src = open(f).read()
    depth = 0
    for ch in src:
        depth += (ch == '{') - (ch == '}')
    print(f, "braces", "ok" if depth == 0 else f"FAIL depth={depth}")
EOF
# unified_log API teyidi
grep -rn "pub fn info" crates/codegen/xai-grok-telemetry/src/unified_log.rs | head -3
grep -rn "parking_lot" crates/codegen/xai-grok-shell/Cargo.toml
```
Expected: `braces ok` tümü; unified_log `info` imzası görünür; `parking_lot` xai-grok-shell Cargo.toml'da bağımlılık olarak mevcut (değilse Task 5'e not düş — ajan ekler).

- [ ] **Step 6: Commit**

```bash
git add crates/codegen/xai-grok-shell/src/session/flow/
git commit -m "feat(flow): kanıt deposu + olay yayını + FlowGovernor kompozisyonu"
```

---

### Task 4: `flow_checkpoint` aracı + session field + kayıt (S1, S7)

**Files:**
- Create: `crates/codegen/xai-grok-tools/src/flow_checkpoint.rs`
- Modify: `crates/codegen/xai-grok-tools/src/lib.rs` (mod ekle — mevcut mod listesine bakıp aynı desende)
- Modify: `crates/codegen/xai-grok-shell/src/session/acp_session.rs` (~780 satır civarı, plan_mode alanının yanı)
- Modify: `crates/codegen/xai-grok-shell/src/session/agent_rebuild.rs` (~453, register_computer_use_tool deseni)

**Interfaces:**
- Consumes: Task 3 — `FlowGovernor::cell() -> Arc<Mutex<CheckpointCell>>`, `CheckpointCell { active, stage, request, result, redirects }`, `CheckpointRequest/CheckpointResult` (serde).
- Produces: `xai_grok_tools::flow_checkpoint::FlowCheckpointTool` (Tool trait impl).

- [ ] **Step 1: `flow_checkpoint.rs` yaz (xai-grok-tools)**

Önce mevcut bir araç örneğini incele (bilgi için, değiştirme):

```bash
cd /home/void0x14/Documents/omnitrix
grep -rn "impl xai_tool_runtime::Tool for" crates/codegen/xai-grok-tools/src/implementations/grok_build/*/mod.rs | head -3
sed -n '1,60p' crates/codegen/xai-grok-tools/src/implementations/grok_build/workflow.rs
```

Ardından dosyayı yaz (imza desenini yukarıdaki çıktıya göre eşle):

```rust
//! `flow_checkpoint` — deterministik aşama bitirme el sıkışması.
//!
//! Ajan yalnızca mevcut aşamayı kapatabilir; aşama sırası ve kanıt denetimi
//! FlowGovernor'a aittir (session tarafı). Bu araç yalnızca paylaşımlı
//! `CheckpointCell` köprüsüne isteği yazar; sonuç sonraki çağrıda döner.

use parking_lot::Mutex;
use std::sync::Arc;

use xai_tool_runtime::Tool;
use xai_tool_runtime::ToolCallContext;
use xai_tool_runtime::ToolError;
use xai_tool_runtime::ToolOutput;

use crate::session_flow_types::{CheckpointCell, CheckpointRequest};

/// xai-grok-shell `session::flow::governor` tipleriyle paylaşım: köprü tipi
/// burada yeniden tanımlanmaz; `register_mcp_tools` çağrısı `FlowCheckpointTool::new(handle)`
/// ile yapılır ve handle tipi aynı crate'te tanımlıysa doğrudan kullanılır.
/// (Gerçek yerleşim: aşağıdaki struct xai-grok-tools içinde tanımlanır; session
/// tarafı `xai_grok_tools::flow_checkpoint::FlowCheckpointCellBridge` kullanır.)
```

DUR — gerçek bağımlılık yönü kritik: `xai-grok-tools` → `xai-grok-shell` BAĞIMLILIĞI YASAK (shell, tools'a bağımlı). Çözüm: köprü tiplerini **xai-grok-tools içinde** tanımla, shell onları kullansın:

```rust
// flow_checkpoint.rs — xai-grok-tools
use parking_lot::Mutex;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

/// Paylaşımlı köprü: xai-grok-shell'in FlowGovernor'u ile bu araç aynı
/// `Arc<Mutex<FlowCheckpointCell>>`'i paylaşır (AgentRebuildSpec üzerinden).
#[derive(Debug, Default)]
pub struct FlowCheckpointCell {
    pub active: bool,
    pub stage: Option<String>,
    pub request: Option<FlowCheckpointRequest>,
    pub result: Option<FlowCheckpointResult>,
    pub redirects: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowCheckpointRequest {
    pub stage: String,
    pub summary: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowCheckpointResult {
    pub accepted: bool,
    pub pending: bool,
    pub stage: String,
    pub directive: String,
    pub detail: String,
}

pub struct FlowCheckpointTool {
    cell: Arc<Mutex<FlowCheckpointCell>>,
}

impl FlowCheckpointTool {
    pub fn new(cell: Arc<Mutex<FlowCheckpointCell>>) -> Self {
        Self { cell }
    }
}
```

Not: `xai_tool_runtime::Tool` trait'inin gerçek imzasını Task 4'ün ajanı şu komutla çıkarır ve implementasyonu ona göre tamamlar:

```bash
grep -rn "pub trait Tool" crates/codegen/xai-tool-runtime/src/ | head -2
grep -rn "fn execute" crates/codegen/xai-tool-runtime/src/*.rs | head -5
```

Tool impl davranışı:
- `execute(input, ctx)`: input `{stage, summary, files}` (serde_json). `cell.active` değilse "akış yok" sonucu (kabul=false, pending=false). Aksi halde: `cell.request = Some(req)`, `cell.result` varsa döndür, yoksa `pending=true` sonucu ("işleniyor, tekrar çağır").

- [ ] **Step 2: `lib.rs` mod kaydı**

`crates/codegen/xai-grok-tools/src/lib.rs` içinde mevcut `pub mod computer_tool;` gibi satırların yanına `pub mod flow_checkpoint;` ekle (mevcut deseni kopyala).

- [ ] **Step 3: `acp_session.rs` — governor alanı**

`~780` satırındaki `pub(crate) plan_mode: Arc<parking_lot::Mutex<...PlanModeTracker>>,` alanının yanına:

```rust
/// Flow Governor: deterministik görev akışı denetleyicisi (PlanModeTracker deseni).
/// Akış seçimi, aşama kapıları, kanıt ve ihlal düzeltmesi AI'dan bağımsızdır.
pub(crate) flow_governor: Arc<parking_lot::Mutex<crate::session::flow::governor::FlowGovernor>>,
```

Init: `SessionActor`'ın inşa edildiği yeri bul (plan_mode'un init edildiği satır):

```bash
grep -n "plan_mode:" crates/codegen/xai-grok-shell/src/session/acp_session.rs crates/codegen/xai-grok-shell/src/session/acp_session_impl/spawn.rs | head -5
```

Orada `session_dir`/persistence yolunun nasıl elde edildiğini gör ve aynı şekilde:

```rust
flow_governor: Arc::new(parking_lot::Mutex::new(
    crate::session::flow::governor::FlowGovernor::new(&session_dir),
)),
```

- [ ] **Step 4: `agent_rebuild.rs` — checkpoint aracını kaydet**

`register_computer_use_tool` desenine (agent_rebuild.rs:453) paralel:

```rust
/// Yerleşik `flow_checkpoint` aracını agent toolset'ine kaydeder (computer aracı deseni).
/// Paylaşımlı köprü: aynı `Arc<Mutex<FlowCheckpointCell>>` hem governor hem araç tarafında.
async fn register_flow_checkpoint_tool(agent: &Agent, cell: std::sync::Arc<parking_lot::Mutex<xai_grok_tools::flow_checkpoint::FlowCheckpointCell>>) -> Result<(), String> {
    agent
        .tool_bridge()
        .register_mcp_tools(
            "flow".to_owned(),
            xai_grok_tools::flow_checkpoint::FlowCheckpointTool::new(cell),
            None,
        )
        .await
        .map_err(|err| format!("flow_checkpoint aracı kaydedilemedi: {err}"))
}
```

Çağrı yeri: `build_agent_inner` içinde `register_computer_use_tool`'un çağrıldığı yeri bul (`grep -n "register_computer_use_tool" agent_rebuild.rs`) ve hemen yanına köprü ile birlikte ekle. Köprü nerden gelir? `build_agent_inner`'ın imzasına `flow_cell: Arc<parking_lot::Mutex<FlowCheckpointCell>>` parametresi EKLE ve çağıranlarını (spawn_session_actor vb.) governor'ın `cell()` çıktısıyla besle. Çağıran zinciri:

```bash
grep -rn "build_agent_inner\|build_agent(\|build_agent_with_initial_overrides" crates/codegen/xai-grok-shell/src/session/agent_rebuild.rs | head -10
```

Her çağrıcıyı güncelle; governor zaten SessionActor inşasında yaratıldığı için `agent.flow_governor` hâlâ mevcut değil — sıralama: governor önce inşa edilir (acp_session.rs init), sonra `spawn_session_actor` onun `cell()`'ini spec'e koyar, spec'ten build_agent_inner'a geçer. Ajan zinciri buna göre çözer; `AgentRebuildSpec`'e alan eklemek yerine build_agent_inner'a parametre eklemek daha az yüzeydir — ajan iki seçenekten mevcut koda en az dokunanı seçer ve `grep` ile tüm çağrı noktalarını günceller.

- [ ] **Step 5: Statik doğrulama**

```bash
cd /home/void0x14/Documents/omnitrix
grep -rn "flow_governor" crates/codegen/xai-grok-shell/src/ | head
grep -rn "FlowCheckpointTool\|flow_checkpoint" crates/codegen/ | grep -v target | head
grep -rn "register_mcp_tools" crates/codegen/xai-grok-shell/src/session/agent_rebuild.rs
python3 -c "import glob
for f in glob.glob('crates/codegen/xai-grok-tools/src/flow_checkpoint.rs')+glob.glob('crates/codegen/xai-grok-shell/src/session/flow/*.rs'):
    s=open(f).read(); d=0
    for ch in s: d+=(ch=='{')-(ch=='}')
    print(f, d)
"
```
Expected: `flow_governor` referansları (alan + init); `flow_checkpoint` dosyası ve lib.rs kaydı; her dosya brace denge `0`.

- [ ] **Step 6: Commit**

```bash
git add crates/codegen/xai-grok-tools/src/flow_checkpoint.rs crates/codegen/xai-grok-tools/src/lib.rs crates/codegen/xai-grok-shell/src/session/acp_session.rs crates/codegen/xai-grok-shell/src/session/agent_rebuild.rs
git commit -m "feat(flow): flow_checkpoint aracı + SessionActor governor alanı + kayıt (S1/S7)"
```

---

### Task 5: Dört karar noktasına kanca (S2, S3, S4, S5, S6)

**Files:**
- Modify: `crates/codegen/xai-grok-shell/src/session/acp_session_impl/turn.rs` (~847, round döngüsü öncesi; ve `use` bloğu)
- Modify: `crates/codegen/xai-grok-shell/src/session/acp_session_impl/sampler_turn.rs` (~229, prepare_tool_definitions_inner dönüşü)
- Modify: `crates/codegen/xai-grok-shell/src/session/acp_session_impl/stop_gate.rs` (run_stop_gate başı)
- Modify: `crates/codegen/xai-grok-shell/src/session/acp_session_impl/goal.rs` (run_goal_round_end başı)
- Modify: `crates/codegen/xai-grok-shell/src/session/acp_session_impl/tool_calls.rs` (handle_bridge_tool_success başarı noktası)

**Interfaces:**
- Consumes: Task 3 governor API: `activate(&mut self, &str, UserMode) -> Option<String>`, `tool_definitions_filter`, `stop_decision() -> Option<String>`, `round_decision() -> RoundVerdict`, `on_tool_success(&mut self, &str)`.
- `UserMode` import: `crate::session::flow::classifier::UserMode`.

- [ ] **Step 1: S2 — `turn.rs` handle_prompt aktivasyonu**

Round döngüsünün (`turn.rs:847` `let result = {`) HEMEN öncesinde, `original_prompt_text` değişkeni tanımlandıktan SONRA (yaklaşık `turn.rs:845` civarı — ajan `original_prompt_text`'in son kullanımından sonraki noktayı seçer) ekle:

```rust
        // Flow Governor (S2): sentetik olmayan kullanıcı promptlarında akışı
        // başlat; dönen direktifi ilk turda sistem mesajı olarak enjekte et.
        if !origin.is_synthetic() {
            let directive = self
                .flow_governor
                .lock()
                .activate(&original_prompt_text, crate::session::flow::classifier::UserMode::UserOriented);
            if let Some(directive) = directive {
                prompt_blocks.push(acp::ContentBlock::Text(acp::TextBlock {
                    text: directive,
                    ..Default::default()
                }));
            }
        }
```

Not: `prompt_blocks` bu noktada `mut` değilse `let mut prompt_blocks = ...` — ajan değişkeni `mut` yapar. `acp::TextBlock`'un gerçek alan adlarını doğrula:

```bash
grep -rn "pub struct TextBlock" crates/codegen/xai-acp-lib/src/ 2>/dev/null || grep -rn "TextBlock" crates/codegen/xai-acp-lib/src/types.rs | head -3
```

- [ ] **Step 2: S3 — `sampler_turn.rs` tool filtresi**

`prepare_tool_definitions_inner`'ın dönüşünü değiştir:

```rust
    pub(super) async fn prepare_tool_definitions_inner(&self) -> Vec<ToolDefinition> {
        let bridge = self.agent.borrow().tool_bridge().clone();
        let defs = bridge.tool_definitions_builtins_only().await;
        let plan_active = self.plan_mode.lock().is_active();
        let defs = filter_cursor_tools_by_plan_mode(defs, plan_active);
        // Flow Governor (S3): kilitli aşama araçlarını modelin tool listesinden
        // gizle — model yasak aracı "üretemez".
        self.flow_governor.lock().tool_definitions_filter(
            defs,
            |d: &ToolDefinition| d.name.as_str(),
        )
    }
```

`ToolDefinition.name` alanının gerçek tipini doğrula:

```bash
grep -rn "pub struct ToolDefinition" crates/common/xai-tool-protocol/src/ crates/codegen/xai-tool-runtime/src/ 2>/dev/null | head -2
grep -rn "pub name" crates/common/xai-tool-protocol/src/*.rs | head -3
```

(`name` `String` ise `d.name.as_str()`, `Cow<str>` ise `d.name.as_ref()` kullan.)

- [ ] **Step 3: S4 — `stop_gate.rs` erken bitirme kapısı**

`run_stop_gate` gövdesinin EN BAŞINA (hook erken dönüşünden önce):

```rust
        // Flow Governor (S4): akış aşaması tamamlanmadıysa stop reddedilir;
        // model aşama direktifiyle devam etmeye zorlanır.
        if let Some(directive) = self.flow_governor.lock().stop_decision() {
            return StopGateDecision::KeepWorking { feedback: directive };
        }
```

`StopGateDecision::KeepWorking` alan adını doğrula:

```bash
grep -rn "enum StopGateDecision" -A 10 crates/codegen/xai-grok-shell/src/session/acp_session_impl/types.rs
```

- [ ] **Step 4: S5 — `goal.rs` goal round baypası**

`run_goal_round_end` gövdesinin EN BAŞINA:

```rust
        // Flow Governor (S5): akış aktifken AI-güdümlü goal döngüsü devre dışı;
        // devam kararını akış durum makinesi verir.
        {
            use crate::session::flow::governor::RoundVerdict;
            let verdict = self.flow_governor.lock().round_decision();
            match verdict {
                RoundVerdict::EndTurn => return GoalRoundDecision::EndTurn,
                RoundVerdict::Continue(directive) => {
                    return GoalRoundDecision::Continue(directive);
                }
            }
        }
```

`GoalRoundDecision::Continue` alan tipini doğrula:

```bash
grep -rn "enum GoalRoundDecision" -A 8 crates/codegen/xai-grok-shell/src/session/acp_session_impl/types.rs
```

- [ ] **Step 5: S6 — `tool_calls.rs` başarı kancası**

`handle_bridge_tool_success` içinde tool adının bilindiği noktada (ajan `effective_tool_name` / `tool_name` parametresini bulur) ekle:

```rust
        // Flow Governor (S6): aşama kanıtı + checkpoint işleme.
        self.flow_governor.lock().on_tool_success(&tool_name);
```

- [ ] **Step 6: Statik doğrulama**

```bash
cd /home/void0x14/Documents/omnitrix
grep -n "flow_governor" crates/codegen/xai-grok-shell/src/session/acp_session_impl/turn.rs crates/codegen/xai-grok-shell/src/session/acp_session_impl/sampler_turn.rs crates/codegen/xai-grok-shell/src/session/acp_session_impl/stop_gate.rs crates/codegen/xai-grok-shell/src/session/acp_session_impl/goal.rs crates/codegen/xai-grok-shell/src/session/acp_session_impl/tool_calls.rs
python3 - <<'EOF'
import glob
for f in ["turn.rs","sampler_turn.rs","stop_gate.rs","goal.rs","tool_calls.rs"]:
    p=f"crates/codegen/xai-grok-shell/src/session/acp_session_impl/{f}"
    s=open(p).read(); d=0
    for ch in s: d+=(ch=='{')-(ch=='}')
    print(f, "braces", "ok" if d==0 else f"FAIL {d}")
EOF
```
Expected: beş dosyada da `flow_governor` kancaları; brace dengesi `0`.

- [ ] **Step 7: Commit**

```bash
git add crates/codegen/xai-grok-shell/src/session/acp_session_impl/
git commit -m "feat(flow): dört karar noktasına governor kancaları (S2-S6)"
```

---

### Task 6: Config varsayılanları + genel statik doğrulama + doküman

**Files:**
- Create: `config/flow/README.md` (açıklama: akışlar, kısıtlar, ihlal görünmezliği)
- Create: `config/flow/flows.toml.example` (universal/commit/direct şablonunun TOML yansıması — bilgilendirici; çalışma zamanı yükleme post-MVP)

**Interfaces:** yok (dokümantasyon).

- [ ] **Step 1: README yaz** — akış listesi, kısıtlar, doğrulama notu (cargo yasak), post-MVP listesi.
- [ ] **Step 2: flows.toml.example yaz** — spec 3.1/3.2'deki aşamaların TOML karşılığı.
- [ ] **Step 3: Kapsamlı statik doğrulama**

```bash
cd /home/void0x14/Documents/omnitrix
echo "=== 1) tüm yeni dosyalar ==="
ls -la crates/codegen/xai-grok-shell/src/session/flow/ crates/codegen/xai-grok-tools/src/flow_checkpoint.rs
echo "=== 2) sembol tutarlılığı: her referans tanımlı mı ==="
for sym in FlowGovernor FlowStateMachine FlowClassifier FlowDurationSystem FlowGate FlowStore FlowEvents CheckpointCell RoundVerdict MAX_REDIRECTS_PER_STAGE; do
  defs=$(grep -rn "pub \(struct\|enum\|const\|fn\) $sym\|pub fn $sym" crates/codegen/xai-grok-shell/src/session/flow/ | wc -l)
  refs=$(grep -rln "$sym" crates/codegen/ --include="*.rs" | grep -v target | wc -l)
  echo "$sym: defs=$defs files=$refs"
done
echo "=== 3) mod kaydı ==="
grep -n "pub mod flow\|pub mod flow_checkpoint" crates/codegen/xai-grok-shell/src/session/mod.rs crates/codegen/xai-grok-tools/src/lib.rs
```
Not: `session/mod.rs`'de `flow` modülü kayıtlı değilse ekle (mevcut `#[path]` desenine bak; plan_mode gibi doğrudan `pub mod`).

Expected: her sembolün en az 1 tanımı + kullanan dosyalar; mod kayıtları mevcut.

- [ ] **Step 4: Commit**

```bash
git add config/flow/
git commit -m "docs(flow): akış şablonları + config örneği + doğrulama notları"
```

---

### Task 7: Bağımsız inceleme (reviewer)

**Files:** tüm diff.

**Interfaces:** plan + spec.

- [ ] **Step 1: Review ajanı görevlendir** — `git diff` (çalışma ağacı HEAD'e karşı) ve `git log --oneline -8` üzerinden:
  1. Her seam'de çağrılan imzaların tanımlarla eşleştiğini doğrula (grep).
  2. `xai-grok-tools` ↔ `xai-grok-shell` arasında yanlış bağımlılık yönü yok mu (shell→tools tek yön).
  3. I6: yeni kodda `unwrap`/`expect`/`panic!` yalnızca `#[cfg(test)]` bloklarında ve `find_flow` expect'inde (aşama: çalışma zamanında asla None dönmez) — production yolunda sıfır.
  4. Brace/parantez dengesi (python betiği).
  5. Spesifikasyon kapsamı: spec 4.1-4.8, 5, 7 tablosundaki her maddenin karşılığı var mı.
  6. Rapor: bulgular listesi (kabul/red) + düzeltme önerileri.

- [ ] **Step 2: Bulguları uygula** (reviewer bulgularına göre küçük düzeltmeler; yeni tasarım değişikliği gerekirse not düş, uygulama devam eder).
- [ ] **Step 3: Final commit**

```bash
git add -A
git commit -m "feat(flow): deterministik akış boru hattı denetleyicisi — tamamlama + inceleme düzeltmeleri"
```
