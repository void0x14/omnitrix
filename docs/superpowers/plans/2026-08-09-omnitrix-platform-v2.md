# Omnitrix Platform v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended). Steps use checkbox (`- [ ]`) syntax for tracking.  
> **Controller MUST:** her task öncesi `sequential-thinking` MCP; araştırma task'larında Context7 + web MCP + paper-search; taze subagent per task; progress ledger `.superpowers/sdd/platform-v2-progress.md`.

**Goal:** Sekiz sütunlu platform v2: Auto-Connect, Routing Modes Catalog, Credential Feeder, Loop Engineering, Truth layer, Persona Menagerie, Computer Use, Native Tools — mevcut Omnitrix seam'leri üzerine, deterministik Flow Governor merkezli.

**Architecture:** Tüm yeni mantık `crates/codegen/xai-grok-{shell,pager,tools,sampler}` + `xai-omni-keychain` + `config/*` içinde. Hayali `omni-*` crate açma. AI aşama seçmez; Governor + RouterEngine + ToolBroker karar verir.

**Tech Stack:** Rust 2024 workspace, ratatui TUI, clap CLI, tokio, rusqlite/sqlx (feeder), reqwest, models.dev, mevcut keychain AES-GCM, Flow Governor, computer_tool trait.

**Spec:** `docs/superpowers/specs/2026-08-09-omnitrix-platform-v2-design.md`

## Global Constraints

- **I2:** `crates/common/` vendored diff üretme; sadece tüket.
- **I5:** model adı/fiyat literal yok — rol/katalog.
- **I6:** production `unwrap`/`expect`/`panic!` yok.
- **I7:** yan etkili yazımlar niyet/WAL veya keychain atomic save.
- **I8:** olgusal iddia kanıt ref.
- Yorumlar Türkçe (kod tabanı geleneği).
- Manuel `/connect` akışı bozulmaz.
- verifier.db sütunu şimdilik **`usable_credentials`** — kodda `// NOT: sütun geçici; özel ayar sonra` yorumu zorunlu.
- **Subagent-driven:** bir task = bir implementer; controller tüm planı tek context'te kodlamaz.
- **Sequential-thinking:** her task başında en az 3 thought.
- **Araştırma:** P1 routing literatürü, P6 CU, P7 tool design için web + Context7 + paper-search.
- Derleme: mümkünse `cargo test -p <crate> --lib` ilgili paket; OOM olursa test yaz + static doğrulama, ledger'a not düş.
- Commit: her task sonunda conventional commit; secrets commit etme.

## Dosya haritası (hedef)

```
crates/codegen/xai-omni-keychain/src/detect.rs          # prefix+skor
crates/codegen/xai-grok-shell/src/util/provider_probe.rs
crates/codegen/xai-grok-shell/src/util/routing_catalog.rs
crates/codegen/xai-grok-shell/src/util/credential_feeder.rs
crates/codegen/xai-grok-sampler/src/router_engine.rs     # genişletme veya yeni
crates/codegen/xai-grok-pager/src/views/provider_picker/ # Auto steps
crates/codegen/xai-grok-pager/src/views/routing_picker.rs
crates/codegen/xai-grok-pager/src/slash/commands/routing.rs
crates/codegen/xai-grok-pager/src/slash/commands/loop_cmd.rs
crates/codegen/xai-grok-pager/src/app/cli.rs             # flags
crates/codegen/xai-grok-tools/src/implementations/.../bash/  # deny taxonomy
crates/codegen/xai-grok-tools/src/codebase_learn.rs
crates/codegen/xai-grok-tools/src/computer_tool.rs       # Codex backend
crates/codegen/xai-grok-shell/src/session/flow/          # loop defs bağ
config/routing_modes.toml                               # katalog
config/flow/*.toml
config/personas/<slug>.toml                             # 60+
prompts/<slug>.md
docs/provider-connect.md
docs/routing-modes.md
docs/loops.md
docs/superpowers/checklists/2026-08-09-persona-menagerie.md
.superpowers/sdd/platform-v2-progress.md
```

---

## FAZ P0 — Auto-Connect

### Task P0.1: Prefix detect tablosu (keychain)

**Files:**
- Create: `crates/codegen/xai-omni-keychain/src/detect.rs`
- Create: `crates/codegen/xai-omni-keychain/src/detect_tests.rs`
- Modify: `crates/codegen/xai-omni-keychain/src/lib.rs`

**Interfaces:**
- Produces:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectCandidate {
    pub provider_id: String,
    pub confidence: u8, // 0-100
    pub reason: String, // "prefix:sk-ant-"
    pub suggested_regions: Vec<String>,
}
pub fn detect_providers_from_key(api_key: &str) -> Vec<DetectCandidate>;
```

- [ ] **Step 1:** sequential-thinking ile prefix çakışma stratejisini netleştir (sk- çok aday, confidence düşük).
- [ ] **Step 2:** `detect_tests.rs` — sk-ant-, AIza, gsk_, xai-, sk-proj-, bilinmeyen.
- [ ] **Step 3:** `detect.rs` implementasyonu (saf, ağ yok). En az 25 provider kuralı.
- [ ] **Step 4:** `lib.rs` `pub mod detect; pub use detect::*`.
- [ ] **Step 5:** `cargo test -p xai-omni-keychain detect` (veya static).
- [ ] **Step 6:** commit `feat(keychain): auto-detect providers from api key prefix`

### Task P0.2: Live provider probe

**Files:**
- Create: `crates/codegen/xai-grok-shell/src/util/provider_probe.rs`
- Create: `crates/codegen/xai-grok-shell/src/util/provider_probe_tests.rs`
- Modify: `crates/codegen/xai-grok-shell/src/util/mod.rs`

**Interfaces:**
```rust
pub struct ProbeRequest {
    pub provider_id: String,
    pub api_key: Zeroizing<String>,
    pub base_urls: Vec<String>, // region endpoints
    pub timeout: Duration, // default 2500ms
}
pub struct ProbeResult {
    pub provider_id: String,
    pub base_url: String,
    pub region: Option<String>,
    pub ok: bool,
    pub http_status: Option<u16>,
    pub latency_ms: u64,
    pub auth_seems_valid: bool, // 401 vs 404 vs 200
}
pub async fn probe_candidates(reqs: Vec<ProbeRequest>) -> Vec<ProbeResult>;
pub fn pick_winner(results: &[ProbeResult], offline: &[DetectCandidate]) -> Option<ProbeResult>;
```

- [ ] **Step 1:** models.dev base_url haritasını oku (`util/models_dev.rs`).
- [ ] **Step 2:** mockable HTTP trait ile unit test (401 = doğru host yanlış key vs connection fail).
- [ ] **Step 3:** implement + wire mod.
- [ ] **Step 4:** commit `feat(shell): live provider/region probe for auto-connect`

### Task P0.3: Auto-Connect orkestrasyonu

**Files:**
- Create: `crates/codegen/xai-grok-shell/src/util/auto_connect.rs`
- Modify: shell util mod

```rust
pub struct AutoConnectOutcome {
    pub provider_id: String,
    pub base_url: String,
    pub region: Option<String>,
    pub models: Vec<ModelInfo>,
    pub candidates_considered: usize,
}
pub async fn auto_connect_from_key(api_key: &str, catalog: &CatalogCache) -> Result<AutoConnectOutcome, AutoConnectError>;
```

- [ ] detect → probe → models.dev models → Outcome
- [ ] Ambiguous error variant
- [ ] tests with mocked probe
- [ ] commit `feat(shell): auto_connect_from_key orchestration`

### Task P0.4: TUI Auto sekmesi

**Files:**
- Modify: `crates/codegen/xai-grok-pager/src/views/provider_picker/mod.rs`
- Create: `.../provider_picker/auto.rs`
- Modify: apply/key_input as needed

- [ ] `ConnectStep` genişlet: `ModeSelect { Auto, Manual }`, `AutoKey`, `AutoDetecting`, `AutoAmbiguous`
- [ ] ModeSelect varsayılan ilk ekran; Manual mevcut Provider adımına gider
- [ ] AutoKey → maskeli paste → async detect task → Model adımına atla
- [ ] Error path mevcut Error step
- [ ] Manuel akış regression: Provider→… sırası aynı
- [ ] commit `feat(tui): auto-connect wizard path`

### Task P0.5: CLI `--auto`

**Files:**
- Modify: `crates/codegen/xai-grok-pager/src/app/cli.rs`
- Modify: headless connect dispatch

- [ ] `grok connect --auto --api-key … [--model …] [--no-session]`
- [ ] provider flag yoksa auto; hem provider hem auto → hata
- [ ] docs/provider-connect.md Auto bölümü
- [ ] commit `feat(cli): grok connect --auto`

### Task P0.6: P0 review gate

- [ ] Controller: review-package P0 range; checklist spec §3.1 + başarı kriteri 1
- [ ] ledger: `P0 complete`

---

## FAZ P1 — Routing Modes Catalog

### Task P1.1: Literatür + rakip tarama (araştırma-only)

- [ ] sequential-thinking
- [ ] Web: 9router, OpenRouter routing, LiteLLM router strategies, AWS bedrock routing
- [ ] paper-search: load balancing LLM inference, fallback cascades
- [ ] Context7: ilgili lib yoksa atla
- [ ] Çıktı dosyası: `.superpowers/sdd/routing-research.md` (≥40 mod id + 1 cümle blurb taslağı)
- [ ] commit docs only `docs: routing modes research notes`

### Task P1.2: `RoutingModeDef` + katalog

**Files:**
- Create: `crates/codegen/xai-grok-shell/src/util/routing_catalog.rs`
- Create: `config/routing_modes.toml` (≥40 mod)
- Tests: parse + unknown id err

```rust
pub enum ModeFamily { Balance, Failover, RoleSplit, Hybrid, Specialty, Cost, Privacy }
pub struct RoutingModeDef { /* spec 3.2 */ }
pub fn load_routing_modes(path: &Path) -> Result<Vec<RoutingModeDef>, …>;
pub fn find_mode(id: &str) -> Option<&'static RoutingModeDef>; // or owned catalog
```

- [ ] TOML şema + built-in fallback if file missing
- [ ] commit `feat(routing): mode catalog with 40+ definitions`

### Task P1.3: RouterEngine

**Files:**
- Create or extend: `crates/codegen/xai-grok-sampler/src/router_engine.rs`
- Integrate with existing `FallbackRouter` in `retry.rs`

```rust
pub struct RouteContext {
    pub prompt_hash: u64,
    pub role: Option<String>, // judge|executor|planner
    pub alive: Vec<Endpoint>,
    pub budgets: BudgetSnapshot,
}
pub struct RouterEngine { pub mode_id: String, /* … */ }
impl RouterEngine {
    pub fn pick(&self, ctx: &RouteContext) -> Result<Endpoint, RouteError>;
    pub fn on_result(&mut self, ep: &Endpoint, res: &AttemptResult);
}
```

- [ ] Implement selectors: rr, wrr, fallback-strict, jep-classic, cheapest-alive, balance-then-fallback, hedge (basit), sticky-session
- [ ] Diğer modlar: composition of primitives (document mapping in TOML `selector = "fallback"`)
- [ ] unit tests per selector
- [ ] commit `feat(sampler): RouterEngine multi-mode selection`

### Task P1.4: Config bridge

- [ ] `config/routing.toml` `strategy` alanı ya id kabul eder ya legacy alias (`fallback`→`fallback-strict`)
- [ ] models.toml/jep roller I5
- [ ] commit `feat(config): routing mode id + legacy aliases`

### Task P1.5: TUI + CLI

- [ ] `/routing` slash + palette
- [ ] `routing_picker.rs` fuzzy + family filter + blurb pane
- [ ] CLI: `grok routing list|set <id>|show|explain <id>`
- [ ] `docs/routing-modes.md`
- [ ] commit `feat(ux): routing mode picker and cli`

### Task P1.6: P1 review gate → ledger

---

## FAZ P2 — Credential Feeder

### Task P2.1: SQLite reader

**Files:** `credential_feeder.rs`

```rust
/// NOT: sütun geçici; özel ayar sonra. Kaynak: usable_credentials
pub struct FeederConfig {
    pub db_path: PathBuf, // default user path
    pub column: String,   // default "usable_credentials"
    pub poll_secs: u64,   // 900
}
pub fn read_usable_credentials(cfg: &FeederConfig) -> Result<Vec<RawCredential>, FeederError>;
```

- [ ] read-only SQLite open
- [ ] column missing → clear error
- [ ] tests with temp db
- [ ] commit `feat(feeder): read usable_credentials from sqlite`

### Task P2.2: Live/dead classification + keychain sink

- [ ] her raw → auto_connect detect+probe
- [ ] live → category `feeder-live`
- [ ] dead → `feeder-dead-hold`
- [ ] never log full key
- [ ] commit `feat(feeder): probe and sink to keychain categories`

### Task P2.3: Triggers

- [ ] RouterEngine chain exhausted → `feed_now()`
- [ ] background task poll
- [ ] CLI `grok keys feed --now`
- [ ] commit `feat(feeder): reactive + passive + cli triggers`

### Task P2.4: P2 review gate

---

## FAZ P3 — Loop Engineering

### Task P3.1: Flow definition dosyaları

- [ ] `config/flow/user_focused.toml` + `full_autonomous.toml`
- [ ] loader in `session/flow/config.rs` (mevcut varsa genişlet)
- [ ] validate: stages must match governor StageId set
- [ ] commit `feat(flow): user_focused and full_autonomous definitions`

### Task P3.2: Loop CLI/TUI

- [ ] `grok loop list|new|validate|run`
- [ ] `new`: interaktif form **veya** `--from-file` / `--problem` — şablon TOML üretir (AI yok)
- [ ] TUI `/loop`
- [ ] `docs/loops.md`
- [ ] commit `feat(ux): loop engineering cli/tui`

### Task P3.3: AutonomousLoop ← Governor Execute

- [ ] `autonomous.rs` run'ı Execute stage backend olarak çağır
- [ ] TerminationOracle kapıları Verify stage ile hizala
- [ ] AI stage skip denemesi gate tarafından engellenir (test)
- [ ] commit `feat(flow): wire AutonomousLoop into governor execute`

### Task P3.4: Research modes + findings store

- [ ] FindingsArchive jsonl path under session dir
- [ ] surface/deep/ocean already in research_tool — governor Research stage zorunlu artifact
- [ ] commit `feat(flow): findings archive artifact persistence`

### Task P3.5: Notify stage

- [ ] telegram/sms channel trait hook (mevcut notify eritme ile birleştir)
- [ ] dry-run default in tests
- [ ] commit `feat(flow): notify stage channel hook`

### Task P3.6: P3 review gate

---

## FAZ P4 — Truth layer

### Task P4.1: Bash deny taxonomy

**Files:** bash tool implementation under `xai-grok-tools`

- [ ] deny: cat, head, tail, more, less, grep, egrep, fgrep, rg, sed, awk, perl -pi, python -c, ruby -e, tee, heredoc EOF write patterns
- [ ] error message: native tool öner (`hashline_read`, `hashline_grep`, `write`, `hashline_edit`)
- [ ] EmergencyTools config escape + audit log
- [ ] redteam tests `tool_allowlist` style
- [ ] commit `feat(tools): bash deny taxonomy against lazy shell`

### Task P4.2: Grounding defaults

- [ ] default grounding required already — enforce in router + sampler path
- [ ] claim without evidence → revise loop (mevcut grounding genişlet)
- [ ] commit `feat(grounding): harden required evidence path`

### Task P4.3: P4 review gate

---

## FAZ P5 — Persona Menagerie

### Task P5.1: Checklist dosyası

- [ ] Create `docs/superpowers/checklists/2026-08-09-persona-menagerie.md` with all slugs + boxes
- [ ] commit `docs: persona menagerie checklist`

### Task P5.2: Generate stubs

- [ ] script or manual: her slug için toml + prompts/*.md stub
- [ ] role mapping heuristic (juryrigg→executor, baykus→judge, firtina-beyin→planner, xlr8→executor, …)
- [ ] tools allowlist minimal set per role
- [ ] loader smoke: directory scan count ≥ 60
- [ ] commit `feat(personas): add menagerie stubs (ben10+custom)`

### Task P5.3: subagent_type resolution

- [ ] map persona name → spawn type in task backend
- [ ] commit `feat(agent): resolve menagerie personas for subagents`

### Task P5.4: P5 review gate

---

## FAZ P6 — Computer Use

### Task P6.1: Araştırma

- [ ] web: OpenAI Codex computer-use, linux community ports
- [ ] paper-search optional
- [ ] çıktı: `.superpowers/sdd/computer-use-research.md`
- [ ] commit docs

### Task P6.2: Backend impl

- [ ] `CodexCuBackend` or community adapter implementing `ComputerBackend`
- [ ] feature flag / config enable
- [ ] permission gate
- [ ] tests with mock backend (mevcut pattern)
- [ ] commit `feat(computer): codex-class backend adapter`

### Task P6.3: Session recording hooks

- [ ] screenshot + action → session events
- [ ] commit `feat(computer): record actions to session log`

### Task P6.4: P6 review gate

---

## FAZ P7 — Native tools

### Task P7.1: codebase_learn tool

- [ ] wrap `xai-codebase-graph` or external index
- [ ] API: `query` / `overview` / `symbol` / `deps`
- [ ] register in grok-build registry
- [ ] commit `feat(tools): codebase_learn native tool`

### Task P7.2: apply_patch hardening

- [ ] reject raw EOF writes in write tool if content looks like shell heredoc abuse (optional)
- [ ] prefer unified diff path
- [ ] commit `feat(tools): harden patch/write paths`

### Task P7.3: Persona allowlists update (executor + menagerie coding types)

- [ ] commit `feat(personas): grant codebase_learn to coding agents`

### Task P7.4: P7 review gate

---

## FAZ P8 — E2E dikey dilim

### Task P8.1: Demo script + integration test (ignored by default if needs network)

- [ ] `tests/` or `bin/` demo: auto-connect mock → routing fallback → short user_focused loop dry-run
- [ ] docs/ARCHITECTURE.md kısa v2 paragrafı
- [ ] README bölüm linkleri
- [ ] commit `test: platform v2 vertical slice dry-run`

### Task P8.2: Final whole-branch review

- [ ] merge-base..HEAD review-package
- [ ] spec başarı kriterleri 1–8 tek tek kanıtla
- [ ] ledger `PLATFORM_V2 DONE`
- [ ] finishing-a-development-branch skill

---

## Task bağımlılık grafiği

```
P0.1 → P0.2 → P0.3 → P0.4 → P0.5 → P0.6
P1.1 → P1.2 → P1.3 → P1.4 → P1.5 → P1.6
P0.3 → P2.1 → P2.2 → P2.3 → P2.4
P3.* parallel after P0.6 (flow already exists); P3.3 after P1.3 preferred
P4 after P0
P5 independent after schema read
P6 research independent; impl after computer_tool
P7 after P4 bash deny
P8 after P0–P7 gates
```

Controller sırası (önerilen): **P0 → P1 → P2 → P3 → P4 → P5 → P6 → P7 → P8**

---

## Progress ledger formatı

`.superpowers/sdd/platform-v2-progress.md`:
```
# Platform v2 Progress
- Task P0.1: complete (commits abc..def, review clean)
...
```

---

## Self-review (plan yazarı)

| Spec § | Task |
|---|---|
| 3.1 Auto-Connect | P0.* |
| 3.2 Routing | P1.* |
| 3.3 Feeder | P2.* |
| 3.4 Loops | P3.* |
| 3.5 Truth | P4.* |
| 3.6 Personas | P5.* |
| 3.7 CU | P6.* |
| 3.8 Native tools | P7.* |
| Lifecycle + notify + record | P3.4–5, P8 |
| Başarı kriterleri | P8.2 |

Placeholder yok. Hayali omni crate yok. Manuel connect korunuyor.
