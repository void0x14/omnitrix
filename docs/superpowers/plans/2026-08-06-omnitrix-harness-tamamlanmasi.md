# Omnitrix Harness Tamamlanması — Implementasyon Planı

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** MASTER-PLAN Faz 0-10 kapılarının TAMAMINI kapatmak; omnitrix artık xai-grok-pager TUI'sinin üzerine kurulu tam otonom harness: TUI'den gerçek iş yapılır, çekirdek (scheduler/router/storage/research/notify/record/backup/webui) CANLI çalışır, `omnitrix run` gibi dolambaçlı CLI yok.

**Architecture:** Üç katman tek süreçte birleşir: (1) xai-grok-pager TUI = yüz; (2) xai-grok-shell MvpAgent = ajan runtime (ACP üzerinden pager'a bağlı); (3) omni-* çekirdek = harness. Entegrasyon noktaları keşifte doğrulandı: `app::run` girişi (omnitrix warm-up), `builtin_commands()` (slash komutları), `event_loop.rs tokio::select!` (omni olay kanalı), `agent_rebuild.rs AgentBuilder` (tool/FS/provider bağlama), `minimal/hook.rs` deseni (fn-pointer IoC).

**Tech Stack:** Rust 2024, tokio, ratatui/crossterm (pager), axum (omni-control API + webui), rusqlite/redb/zstd (omni-storage), xai-grok-sampler (LLM), xai-grok-tools, xai-grok-models (katalog).

## Global Constraints

(ALPHA-PLAN Bölüm 10, tartışmasız)
- K1-K15 kararları veri olarak alınır, yeniden tartışılmaz.
- Her fazın sonunda çalıştırılabilir kabul kapısı: komut + metrik + eşik. Kapı geçilmeden faz "tamamlandı" sayılmaz.
- `xai-*` entegrasyonu imza düzeyinde: hangi trait, hangi metot, hangi tip. "Genişletilir" yasak.
- Sandbox/seccomp/bwrap YOK (K5/I4). FS koruması tool broker allowlist + FS-shim'dir.
- Ortak çekirdek durum iki UI'dan ÖNCE (K7/K8): `omni-proto` tek kaynak.
- Model isimleri/fiyatlar gömülmez (AS7/I5): `xai-grok-models` kataloğu + config override.
- Üretim yolunda panic/unwrap/expect yok (I6).
- Tek yazar çekirdek (I3): CoreState tek `Arc<Mutex>`; ikinci durum kopyası yok.
- Crash-only kapanış (8.1): SIGINT → anında exit, flush yok, veri WAL/journal'da.
- Cold-start lazy-init (8.2): ilk TUI frame'i < 100ms, arka planda ısınma.
- Anahtar besleme kapsam sınırı (6.3): sızmış-anahtar üçüncü-taraf hattı YOK.
- `tests/*.rs` kapı testleri: crash_recovery, diff_visibility, grounding_redteam, interrupt_granularity, multiagent_fanout, provider_fallback, tool_allowlist_redteam, ui_parity.

---

# FAZ 0 — Yeniden adlandırma + yeşil workspace (DURUM: TAMAMLANMIŞ GÖRÜNÜYOR, DOĞRULA)

**Kapı:** `omnitrix --version` çalışır · `cargo check --all-targets --workspace` yeşil · `cargo clippy --workspace -- -D warnings` temiz · CI diff kuralı (I2).

### Task 0.1: Kapı doğrulama
- [ ] Koş: `cargo check --all-targets --workspace 2>&1 | tail -3` — hata sayısı
- [ ] Koş: `cargo clippy --workspace -- -D warnings 2>&1 | tail -5`
- [ ] Koş: `cargo run -p omnitrix -- --version`
- [ ] Rapor: 3 komutun çıktısı. Kırmızı varsa minimal düzelt (sadece hata, refactor yok).

---

# FAZ 1 — Dikey dilim: tek görev, tek ajan, uçtan uca, TUI İÇİNDE

**Kapsam (MASTER-PLAN):** anahtar gir→detect+doğrula (VAR: `omnitrix key add`) · TUI < 100ms lazy · görev yaz · tek ajan gerçek LLM'e bağlanır · gerçek tool'lar · her adım event-log + her dosya dokunuşu diff akışı · Ctrl+C < 50ms · restart'ta yarım işlem yok · RSS raporlanır.

**EKSİK (keşif kanıtı):** TUI açıldığında omnitrix çekirdeği HİÇ başlamıyor — `bootstrap::warm_up` (bootstrap.rs:417) çağrılmıyor ("never used" uyarısı); `run_tui` yalnızca `xai_grok_pager::app::run` çağırıyor; omni storage/provider/scheduler TUI içinde yok.

### Task 1.1: TUI girişine omnitrix core bağla
**Files:**
- Modify: `crates/omni/omnitrix/src/main.rs` (run_tui, ~180-205)
- Modify: `crates/omni/omnitrix/src/bootstrap.rs` (warm_up çağrı zinciri — zaten var, sadece bağlanacak)

**Interfaces:**
- Consumes: `bootstrap::warm_up(&watch::Sender<WarmupPhase>) -> Result<OmnitrixContext>` (bootstrap.rs:417), `bootstrap::park_context(OmnitrixContext)` (bootstrap.rs:476), `OmnitrixContext` (bootstrap.rs:105)
- Produces: `run_tui` omnitrix context'i başlatır; `OmnitrixContext` handle'ı pager'a (bir kanal/Arc üzerinden) taşınır.

- [ ] **Step 1:** `run_tui` içine: tokio runtime kurulduktan sonra `let (phase_tx, phase_rx) = watch::channel(WarmupPhase::Cold);` + `tokio::spawn(bootstrap::warm_up(&phase_tx))` + spawn edilen task tamamlanınca `bootstrap::park_context(ctx)`. İlk frame'i beklemeden spawn et (lazy-init, 8.2).
- [ ] **Step 2:** `OMNITRIX_TRACE_STARTUP=1` iken warm-up phase geçişlerini `trace_startup` ile stderr'e bas.
- [ ] **Step 3:** Test: `cargo run -p omnitrix` TUI açılır; `OMNITRIX_TRACE_STARTUP=1 cargo run -p omnitrix` stderr'de phase satırları görünür; Ctrl+C anında çıkar.
- [ ] **Step 4:** Commit.

### Task 1.2: Omnitrix /omni slash komutları (TUI'de görünür çekirdek)
**Files:**
- Create: `crates/codegen/xai-grok-pager/src/slash/commands/omni_status.rs`
- Create: `crates/codegen/xai-grok-pager/src/slash/commands/omni_tasks.rs`
- Modify: `crates/codegen/xai-grok-pager/src/slash/commands/mod.rs` (mod + builtin_commands() kayıt)
- Modify: `crates/codegen/xai-grok-pager/src/slash/commands/mod.rs:78-153` (liste)

**Interfaces:**
- Consumes: `SlashCommand` trait (slash/command.rs:146: `name/aliases/description/usage/takes_args/suggest_args/visible/run(&self, ctx: &mut CommandExecCtx, args) -> CommandResult`), `CommandExecCtx` (slash/command.rs), `CommandResult` (:41)
- Produces: `/omni` (durum özeti: provider sayısı, scheduler aktif ajan, storage boyutu, health), `/omni-tasks` (tasks tablosu listesi — `omni-storage` üzerinden SQLite okuma). Pager'da omni'ye erişim: omnitrix bin, pager'a `Arc<OmnitrixContext>` veremiyorsa komutlar omni-scheduler'ı DOĞRUDAN değil, ortak bir static/seam üzerinden okur — en temizi: `xai-grok-pager`'a küçük bir `omni_bridge` modülü ekle (OnceLock<Arc<dyn OmniSnapshotProvider>>), omnitrix bin bunu kurar.

- [ ] **Step 1:** `xai-grok-pager` içine `src/omni_bridge.rs`: `pub struct OmniSnapshot { providers: usize, active_agents: usize, storage_bytes: u64, healthy: bool }` + `pub trait OmniSnapshotProvider: Send + Sync { fn snapshot(&self) -> OmniSnapshot; }` + `OnceLock<Arc<dyn OmniSnapshotProvider>>` + `pub fn install(p: Arc<dyn OmniSnapshotProvider>)` + `pub fn snapshot() -> Option<OmniSnapshot>`. `lib.rs`'ye `pub mod omni_bridge;` ekle.
- [ ] **Step 2:** omnitrix bin'de `OmniSnapshotProvider` impl: `OmnitrixContext`'ten scheduler (`active_agents`), health_probe (`healthy`), storage (`storage_bytes` — cas dizini boyutu), provider layer. `run_tui`'de warm_up tamamlanınca `xai_grok_pager::omni_bridge::install(arc)` çağır.
- [ ] **Step 3:** `omni_status.rs`: `/omni` → bridge snapshot al, ekranda 4-6 satır özet (render: `ctx.response(...)` veya CommandResult::Message ile scrollback'e yaz). Test: TUI'de `/omni` yaz → özet görünür. Bridge kurulmamışsa "omnitrix core başlatılmadı" mesajı.
- [ ] **Step 4:** `omni_tasks.rs`: `/omni-tasks` → omni-storage SQLite tasks tablosunu okur (omnitrix bin'in `record_task` kullandığı aynı DB: `dirs::data_dir()/omnitrix/omnitrix.sqlite`), listeyi Message olarak basar. DB yoksa "görev yok".
- [ ] **Step 5:** `commands/mod.rs`'e kayıt. Build + test. Commit.

### Task 1.3: TUI olaylarını omni-storage'a akıt (event-log + diff akışı)
**Files:**
- Modify: `crates/codegen/xai-grok-pager/src/app/event_loop.rs` (select! kolları ~2017-2210, acp handler ~2057)
- Modify: `crates/codegen/xai-grok-pager/src/app/acp_handler/mod.rs` veya `turn_completion.rs` (tur bitti olayı)
- Modify: `crates/omni/omnitrix/src/bootstrap.rs` (EventWriter'ı bridge'e bağla)

**Interfaces:**
- Consumes: `omni_storage::EventWriter` (record_event/record_message/record_tool_call/record_file_touch), pager `AcpMessage` stream (event_loop.rs:2057)
- Produces: her ACP `SessionUpdate`/`MessageDelta`/`ToolCall` → `EventWriter::record_*`; TUI'de her kullanıcı prompt'u `MessageRecord`. FileTouch'lar zaten shell tarafında FS-shim ile (Faz 1.6) yakalanacak; bu task pager tarafındaki mesaj/tool-call kaydını ekler.

- [ ] **Step 1:** omni_bridge'e ikinci seam: `OnceLock<Arc<dyn OmniEventSink>>` + `trait OmniEventSink { fn on_acp_message(&self, json: &str); fn on_tool_call(&self, name: &str, args: &str); fn on_prompt(&self, text: &str); }`.
- [ ] **Step 2:** omnitrix bin'de impl: `EventWriter` actor'ünü kullanarak sqlite'a yaz (writer_actor zaten omnitrix bin'de mevcut — bootstrap::init_storage).
- [ ] **Step 3:** event_loop.rs:2057 acp_rx.recv() yanında `omni_bridge::event_sink().map(|s| s.on_acp_message(&serde_json::to_string(msg)))`. dispatch/prompt.rs:428 `dispatch_send_prompt_inner` içinde `s.on_prompt(text)`.
- [ ] **Step 4:** Test: TUI'de 2-3 mesaj gönder, `sqlite3 ~/.local/share/omnitrix/omnitrix.sqlite "select count(*) from messages"` artış gör. Commit.

### Task 1.4: Ctrl+C < 50ms + crash-only doğrulama (kapı 8.1)
- [ ] **Step 1:** pager'ın kendi exit yolu + omnitrix `instant_exit` (bootstrap.rs:385). TUI'de Ctrl+C → `instant_exit(0)` çağrılıyor mu doğrula; değilse pager'ın signal_handler.rs:1 ile omnitrix kapanışını bağla (aynı süreç, tek exit yolu).
- [ ] **Step 2:** Kapı ölçümü: `omni-bench` (mevcut bin) veya elle `time` ile Ctrl+C ölçümü; `tests/crash_recovery.rs` koş. Rapor: exit süresi + test sonucu. Commit.

### Task 1.5: Kapı — tests/crash_recovery.rs + diff_visibility.rs yeşil
- [ ] Koş: `cargo test -p omni-tests --test crash_recovery` → PASS/FAIL raporla. FAIL ise düzelt (testin ne iddia ettiğine sadık, testi yumuşatma).
- [ ] Koş: `cargo test -p omni-tests --test diff_visibility` → PASS/FAIL raporla. FAIL ise FS-shim tarafını düzelt.
- [ ] Rapor: iki test çıktısı + düzeltilen satırlar. Commit.

### Task 1.6: AgentBuilder'a omnitrix FS-shim + tool seti (diff görünürlüğü CANLI)
**Files:**
- Modify: `crates/codegen/xai-grok-shell/src/session/agent_rebuild.rs:238` (AgentBuilder zinciri)
- Modify: `crates/codegen/xai-grok-shell/src/session/acp_session_impl/model_switch.rs:165` (model değişiminde rebuild — aynı zincir)

**Interfaces:**
- Consumes: `AgentBuilder::with_fs(Arc<dyn AsyncFileSystem>)` (builder.rs:413), `AgentBuilder::with_tools(Vec<String>)` (:321), `omni_tools::fs_shim::DiffShimFs` + `faz1_toolset_with` (omni-tools registry.rs)
- Produces: shell tarafındaki her Agent build'inde `DiffShimFs` (CAS'a yazan) + omnitrix tool seti. Shell build'i omni'ye bağımlı hale gelir → **bağımlılık yönü**: omnitrix çekirdeği xai-*'a bağlıydı; burada shell'in omni-tools'a bağlanması gerekir. Çözüm: xai-grok-shell Cargo.toml'a `omni-tools` workspace dep ekle (repo artık tek ürün, upstream yok — kullanıcı onayı).

- [ ] **Step 1:** xai-grok-shell Cargo.toml → `omni-tools = { workspace = true }` (ve gerekirse `omni-storage`). Workspace dep tanımı zaten Cargo.toml'da var.
- [ ] **Step 2:** agent_rebuild.rs'de builder zincirine `with_fs(Arc::new(DiffShimFs::new(...)))` + tool listesine omni tool adları ekle. DiffShimFs kurulumu: `touch_channel` + CAS path (omnitrix data_dir).
- [ ] **Step 3:** Build: `cargo check -p xai-grok-shell`. Test: `tests/diff_visibility.rs` yeşil (dizin-dışı dokunuş diff akışında görünür).
- [ ] **Step 4:** Commit.

---

# FAZ 2 — Ortak durum + iki yüz (K7/K8)

**Kapsam:** omni-proto ortak model (VAR) · omni-control SSE/WS + auth (VAR) · omni-webui aynı akıştan türer (VAR ama servis edilmiyor).
**EKSİK (kanıt):** `bootstrap::init_api` yalnızca `warm_up` içinde çağrılıyor; `warm_up` hiç çağrılmıyor → API 127.0.0.1:9876 HİÇ açılmıyor; webui router'ı hiçbir sunucuya monte edilmiyor (omni-webui "use edilmiyor").

### Task 2.1: Kontrol düzlemi API'sini canlı aç (warm_up → api.serve)
- [ ] **Step 1:** warm_up çağrıldıktan sonra (Task 1.1) API zaten spawn ediliyor (bootstrap.rs:455-467) — doğrula: `curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:9876/v1/snapshot` → 401 (auth zorunlu, K9) beklenir. 200/bağlantı reddi sorunlarını raporla.
- [ ] **Step 2:** `OMNITRIX_API_TOKEN_HASH` env ile token kur, `curl` ile snapshot endpoint'ine token'lı istek → 200 + JSON. Rapor. Commit (gerekirse).

### Task 2.2: omni-webui'yi API'ye monte et
**Files:**
- Modify: `crates/omni/omnitrix/src/api.rs` (router montajı — `omni_control::api::router` yanına `omni_webui::router`)

**Interfaces:**
- Consumes: `omni_webui::router::<S: ControlPlane>()` (webui router fonksiyonu), `omni_control::api::router` (control router)
- Produces: `http://127.0.0.1:9876/` → webui ana sayfa; `/ui/stream` SSE; `/ui/ws`; `/ui/command`.

- [ ] **Step 1:** api.rs'de axum Router'a webui router'ını `nest` et veya birleştir (aynı state). 
- [ ] **Step 2:** Test: tarayıcıdan `http://127.0.0.1:9876/` açılır, WebUI görünür; `tests/ui_parity.rs` koş.
- [ ] **Step 3:** Commit.

### Task 2.3: Kapı — tests/ui_parity.rs yeşil
- [ ] Koş: `cargo test -p omni-tests --test ui_parity` → PASS/FAIL. FAIL düzelt (TUI komutu → WebUI akışında aynı StateEvent; snapshot bit-eş). Commit.

---

# FAZ 3 — Çok-ajan scheduler + kaynak valisi (K1, K2, 3.1)

**Kapsam:** omni-scheduler `impl SubagentBackend` (VAR — subagent_backend.rs 1689 satır) · tier makinesi · kaynak valisi · rekürsiyon/fan-out tavanları (AS3) · bütçe kalıtımı (AS4).
**EKSİK:** scheduler yalnızca `omnitrix run` altında dolaylı; TUI'de multiagent yok. TUI'nin kendi subagent/leader mekanizması (pager leader_cluster, subagent.rs) VAR ama omnitrix scheduler'dan bağımsız.

### Task 3.1: omni-scheduler'ı TUI'ye bağla (/omni-dashboard)
**Files:**
- Create: `crates/codegen/xai-grok-pager/src/slash/commands/omni_dashboard.rs`
- Modify: `commands/mod.rs` kayıt; `omni_bridge` snapshot'ına scheduler durumu

**Interfaces:**
- Consumes: `omni_scheduler::Scheduler::spawn_agent(...)` (scheduler.rs), `ManagedAgent`, `AdmissionController`, `InterruptBus`
- Produces: `/omni-dashboard` — aktif/kuyruk/uyuyan ajanlar tablosu (tier machine), interrupt buton davranışı (komut argümanı: `/omni-dashboard interrupt <id>`), her ajanın durumu.

- [ ] **Step 1:** bridge `OmniSnapshot`'a `agents: Vec<AgentRow {id, tier, status, task_title}>` ekle (scheduler'dan).
- [ ] **Step 2:** `/omni-dashboard` komutu: tabloyu Message olarak basar; `interrupt <id>` alt komutu InterruptBus'a gönderir.
- [ ] **Step 3:** TUI'de test: `/omni-dashboard` → liste görünür. Commit.

### Task 3.2: Kapı — tests/multiagent_fanout.rs yeşil
- [ ] Koş: `cargo test -p omni-tests --test multiagent_fanout` → PASS/FAIL. FAIL düzelt. Commit.

### Task 3.3: Kaynak valisi + bütçe (K2/AS4) kapı doğrulaması
- [ ] `tests/interrupt_granularity.rs` koş → PASS/FAIL. FAIL düzelt. Commit.

---

# FAZ 4 — Router modları + JEP + grounding (6.5, 3.4)

**Kapsam:** round/fallback/JEP · rol→model katalogdan (AS7) · yanlışlamacı yargıç + kanıt zorunluluğu. (omni-router strategies.rs + Judge + EvidenceReplayer VAR)
**EKSİK:** router yalnız `omnitrix run`'da (fallback); TUI'de JEP/gounding hiç yok; model kataloğu omnitrix tarafında değil.

### Task 4.1: TUI'ye router modları (/omni-model, /omni-routing)
- [ ] `/omni-routing <round_robin|fallback|jep>` komutu: `omni_router::strategies::Router`'a mod değiştirir (config/routing.toml yaz). `/omni-model <rol> <model>`: rol→model atama (config/models.toml, AS7 — literal isim yok, katalogdan).
- [ ] Bridge'e `RoutingSnapshot { strategy, roles: {judge, executor, planner} }` ekle. Commit.

### Task 4.2: Kapı — provider_fallback.rs + grounding_redteam.rs yeşil
- [ ] `cargo test -p omni-tests --test provider_fallback` → PASS/FAIL.
- [ ] `cargo test -p omni-tests --test grounding_redteam` → PASS/FAIL.
- [ ] FAIL olanları düzelt (testin iddiasına sadık). Commit.

---

# FAZ 5 — Persona + tool broker zorlaması (K12, K3)

**Kapsam:** config/personas/*.toml şema + hot-reload · allowlist broker (omni-tools ToolBroker VAR) · shell parser + acil-kaçış (yargıç onaylı).
**EKSİK:** ToolBroker omnitrix run'da; TUI'de yasak-araç zorlaması pager'ın kendi tool'larına uygulanmıyor; personas config'i TUI'de yüklenmiyor.

### Task 5.1: TUI tool'larını broker'dan geçir
- [ ] `omni_bridge`'e `ToolValidator` seam: her tool call ismi broker allowlist'ten kontrol edilir; yasak ise `CommandResult::Message("yasak araç")` + olay log. Pager acp_handler/tool_call yoluna bağla.
- [ ] `config/personas/*.toml` hot-reload: `xai-fsnotify` ile izle, değişimde `/omni-personas` listesi güncellenir.
- [ ] Commit.

### Task 5.2: Kapı — tool_allowlist_redteam.rs yeşil
- [ ] `cargo test -p omni-tests --test tool_allowlist_redteam` → PASS/FAIL. FAIL düzelt (yasak araç %100 red + log, sızma 0). Commit.

---

# FAZ 6 — Uzak erişim + bildirim (K9, 6.6)

**Kapsam:** IPv6/Tailscale/Telegram/Twilio kanalları tek API üstünde · bildirim tetikleyicileri + gürültü susturma. (omni-notify VAR, DEAD)
**EKSİK:** omni-notify hiçbir yerde use edilmiyor.

### Task 6.1: omni-notify'ı warm_up'a bağla
- [ ] `NotifyDispatcher`'ı `OmnitrixContext`'e ekle: config (notify.escalation) okunur, `Broadcaster` olaylarına abone olur (AgentUpserted/TaskUpserted/Notice), Telegram/Twilio kanalları config'ten.
- [ ] `/omni-notify test` slash komutu: deneme bildirimi gönderir.
- [ ] Test: birim test (dispatcher'a sahte kanal enjekte edip tetikleyici doğrula). Commit.

---

# FAZ 7 — Araştırma motoru (K14, 6.2)

**Kapsam:** omni-research sağlayıcı-değiştirilebilir · yüzeysel/derin/okyanus · JSON+MD çıktı. (VAR, DEAD; omni-scheduler/research.rs'de DUPLIKE enum — kaldır, omni-research'ü kullan)
**EKSİK:** omni-research use edilmiyor; scheduler'da duplike ResearchMode var.

### Task 7.1: omni-research'ü canlı bağla
- [ ] `omni-scheduler/src/research.rs`'teki duplike `ResearchMode`'u kaldır; `omni_research::ResearchMode` kullan.
- [ ] `/omni-research <surface|deep|ocean> <soru>` slash komutu: `ResearchEngine::investigate` → sonuçlar scrollback'e + `research_findings`'e (omni-storage).
- [ ] Test: MCP provider mock ile unit test. Commit.

---

# FAZ 8 — Anahtar besleme hattı (K13, 6.3)

**Kapsam:** harici SQLite → oku → canlılık → canlı/ölü ayrımı → provider'a ekle; kaynak-agnostik. (omni-provider ModelIngestor/FeedSource VAR)
**EKSİK:** besleme hattı use edilmiyor.

### Task 8.1: Besleme hattını bağla
- [ ] `omni-provider::ingestion` FeedSource'ları (harici sqlite, config dosyası) → `ModelIngestor` → `KeyManager` + `HealthProbe` canlılık kontrolü → canlı/ölü bölümü (sqlite tablo). 
- [ ] `/omni-keys status` slash komutu: canlı/ölü anahtar sayısı.
- [ ] Test: geçici sqlite ile uçtan uca. Commit.

---

# FAZ 9 — Kayıt + yedekleme (AS6, AS10)

**Kapsam:** event-log + tetiklemeli medya · 3-2-1 SigV4 + şifreleme. (omni-record, omni-backup VAR, DEAD; omni-backup lib.rs'e modüller eklenmemiş — DERLENMİYOR, ilk düzeltme bu)
**EKSİK:** ikisi de use edilmiyor; omni-backup `crypto`/`sigv4` modülleri lib.rs'de eksik.

### Task 9.1: omni-backup derlenir hale + bağla
- [ ] omni-backup lib.rs'e `pub mod checksum; pub mod crypto; pub mod sigv4; pub mod snapshot; pub mod target;` ekle; `cargo check -p omni-backup` yeşil.
- [ ] `S3BackupTarget` + şifreleme ile 3-2-1 yedek: warm_up'da zamanlayıcı (config), olay tetiklemeli snapshot.
- [ ] `/omni-backup now` slash komutu. Test: unit (şifrele+şifre çöz roundtrip). Commit.

### Task 9.2: omni-record bağla
- [ ] `EventLog` + `VideoRecorder` tetiklemesi (record.video/dom/events config'i) — event-log zaten Task 1.3'te yazılıyor; video/dom tetiklemeli medya ekle.
- [ ] `/omni-record <session_id> replay` komutu. Commit.

---

# FAZ 10 — Computer-use + self-hosting + tam-otonom döngü (AS11, AS12, 6.7)

**Kapsam:** xai-computer-hub-* (Wayland+X11) · self-modify kapılı · tam-otonom döngü + sonlanma oracle'ı. (omni-tools Computer VAR, omni-core selfhost VAR, xai-computer-hub-* VAR)

### Task 10.1: Computer-use tool'unu TUI'ye bağla
- [ ] omni-tools `Computer` tool'unu shell AgentBuilder tool listesine ekle (Task 1.6 zinciri). `xai-computer-hub-core/sdk/mcp-adapter` bağla.
- [ ] Test: computer-use dokunuşu diff akışında (diff_visibility benzeri). Commit.

### Task 10.2: Tam-otonom döngü + sonlanma oracle'ı
- [ ] `/omni-autonomous <problem>` komutu: kullanıcı yalnız problemi verir; döngü: oku→parçala→kaydet→araştır(surface/deep/ocean)→depola→yığın seç→süre kararı→plan→yapı taşları→paralel/ardışık sorgula→multiajan görevlendir→doğrula→bitir veya dön. Sonlanma oracle'ı (omni-core oracle.rs:1024 VAR) görev bitti mi diye karar verir; bitmediyse döngü döner.
- [ ] Rapor: döngü diyagramı + nerede durur. Commit.

---

# KESİŞEN KAPI — Soak (7/24) + genel doğrulama

- [ ] Tüm `tests/*.rs` tek koşuda: `cargo test -p omni-tests` (8 dosya) → hepsi yeşil.
- [ ] TUI açılır, `/omni`, `/omni-dashboard`, `/omni-tasks`, `/omni-research`, `/omni-keys`, `/omni-backup`, `/omni-notify`, `/omni-autonomous` çalışır; WebUI 127.0.0.1:9876'da açık; Ctrl+C < 50ms; RSS < 250MB hedefi (omni-bench ile ölç).
- [ ] `cargo clippy --workspace -- -D warnings` temiz.
- [ ] Sonuç raporu: kapı tablosu (faz | kapı | durum | kanıt).
