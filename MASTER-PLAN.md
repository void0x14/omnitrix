# MASTER-PLAN.md
## Omnitrix Tabanlı Otonom Multiagent Orkestrasyon Platformu — Mimari & Geliştirme Ana Planı

> Bu belge kod içermez. `otonom-multiagent-orkestrasyon/MASTER-PLAN.md`'deki gereksinimleri temel alır; **omnitrix'in hali hazırdaki kod tabanı üzerine inşa edilecek** şekilde mimariyi, yeni crate'leri, entegrasyon noktalarını ve aşamalı geliştirme yol haritasını tanımlar. Kod yazımı bu plan onaylandıktan sonra başlar.

---

## 0. Omnitrix Mirası — Neyi Tekrar İcat Etmiyoruz

Omnitrix (xAI Grok CLI), halihazırda 80+ Rust crate'ten oluşan, üretim seviyesinde çalışan bir kod tabanıdır. Aşağıdaki bileşenler zaten mevcuttur ve **yeniden yazılmayacak**, üzerine inşa edilecektir:

| Mevcut Crate | Rolü | Multiagent için Kullanımı |
|---|---|---|
| `xai-grok-agent` | Agent tanımı, builder, system prompt birleştirme, compaction | Her subagent tipi bu `Agent` + `AgentBuilder` üzerine kurulur |
| `xai-grok-tools` | Tool registry, implementasyonlar, bridge, retry | Native tool seti (read/write/edit/search...) doğrudan kullanılır |
| `xai-tool-runtime` | Birleşik `Tool` trait'i, dispatch, context, streaming | Tüm agent eylem yüzeyi bu trait üzerinden |
| `xai-tool-types` | Subagent tanımları (`BuiltinSubagent`: PLAN, EXPLORE, GENERAL_PURPOSE), task kill/wait araçları | Genişletilir; yeni persona tipleri eklenir |
| `xai-chat-state` | Actor tabanlı konuşma durumu, compaction, token sayımı | Her agent kendi `ChatStateActor`'üne sahip olur |
| `xai-grok-subagent-resolution` | Subagent spawn çözümleme (model, persona, capability, isolation) | Genişletilir; routing policy + capability allowlist eklenir |
| `xai-grok-sandbox` | OS sandbox (nono+bwrap), seccomp network filtreleme | Her agent sandbox profili ile çalışır |
| `xai-computer-hub-core` | Transport + ToolRegistry + CompoundResolver | Computer-use broker omurgası |
| `xai-computer-hub-sdk` | Computer Hub SDK (metric client vb.) | Doğrudan kullanılır |
| `xai-grok-memory` | Cross-session bellek (markdown + embedding + MMR) | Agent'lar arası shared context |
| `xai-grok-models` | Varsayılan model ID'leri | Provider router'a beslenir |
| `xai-grok-config` | Çok katmanlı config yükleme | Orkestrasyon config'i bu katmanlara eklenir |
| `xai-grok-hooks` | Runtime hook sistemi (session_start, pre/post_tool_use, session_end) | Interrupt + audit hook'ları eklenir |
| `xai-grok-shell` / `xai-grok-workspace` | Ana daemon/shell binary'si + workspace server | Multiagent daemon bu binary'lere entegre edilir |
| `xai-grok-pager-render` + `xai-ratatui-inline` | TUI render altyapısı (ratatui) | Multiagent izleme TUI'si aynı altyapıda |
| `xai-sqlite-journal` | SQLite WAL journal soyutlaması | Multiagent state persistansı bu temelde |

**Prensip:** Var olanı yıkma, genişlet. Her yeni özellik, mevcut trait'leri implemente eden veya mevcut struct'ları saran yeni bir crate olarak gelir.

---

## 1. Karar Özeti — Omnitrix'e Uyarlanmış Kararlar

| # | Konu | Karar | Omnitrix'te Karşılığı |
|---|------|-------|----------------------|
| D1 | Ölçek + verimlilik | OOM olmadan maksimum eşzamanlılık. RAM optimize edilecek bir metrik. | Mevcut tokio multi-thread runtime + admission control eklenecek |
| D2 | Çekirdek dil | **Rust** (stable, edition 2024) | Zaten omnitrix Rust workspace |
| D3 | Depolama | **Katmanlı (hibrit):** sıcak KV (`redb`) + kalıcı SQLite/WAL + CAS blob store | `xai-sqlite-journal` zaten WAL soyutlaması sağlıyor; `redb` yeni eklenecek; CAS blob store yeni |
| D4 | Durability | WAL + idempotent replay; anlık kapanış + bounded recovery | `xai-sqlite-journal` replay altyapısı mevcut |
| D5 | Dağıtım | Kapalı kaynak, kişisel. Ben 10 isimleri serbest. | Mevcut omnitrix lisans yapısı korunur |
| D6 | Determinizm | Kanıt-zorunlu (grounding) + judge doğrulama + ihlal → interrupt/ceza | Yeni `ork-router` + `ork-judge` crate'leri |
| D7 | Tool enforcement | Computer-use sandbox'lı capability yüzeyi; broker'dan geçer | `xai-grok-sandbox` + `xai-computer-hub-core` temel alınır, genişletilir |
| D8 | Mimari şekil | **Mevcut daemon + yeni multiagent runtime modülü.** TUI/WebUI zaten var, genişletilir. | Mevcut shell/workspace binary'lerine runtime modülü entegre edilir |

---

## 2. Hedef Mimari — Omnitrix Üzerine Katmanlama

Mevcut omnitrix mimarisini bozmadan, üzerine aşağıdaki yeni katmanlar eklenir:

```
                     ┌─── Mevcut Omnitrix ───────────────────────────────┐
                     │                                                     │
  TUI (ratatui) ──┐  │  ┌──────────────┐   ┌──────────────────────────┐   │
                  ├──▶  │  xai-grok-    │   │  xai-grok-agent          │   │
  WebUI (axum)  ──┘  │  │  shell/      │◀─▶│  (Agent, AgentBuilder)   │   │
                     │  │  workspace    │   └──────────┬───────────────┘   │
                     │  └──────┬───────┘              │                   │
                     │         │                      │                   │
                     │  ┌──────▼──────────────────────▼──────────────┐    │
                     │  │  xai-tool-runtime (Tool trait, dispatch)    │   │
                     │  └──────┬──────────────────────────────────────┘    │
                     │         │                                          │
                     │  ┌──────▼───────┐  ┌───────────────────────────┐    │
                     │  │ xai-grok-    │  │ xai-computer-hub-*        │    │
                     │  │ sandbox      │  │ (transport, registry...)   │    │
                     │  └──────────────┘  └───────────────────────────┘    │
                     │                                                     │
                     └─────────────────────────────────────────────────────┘

                     ┌─── Yeni Multiagent Katmanları ─────────────────────┐
                     │                                                     │
                     │  ┌──────────────────────────────────────────┐      │
                     │  │  ork-runtime  (Multiagent Scheduler)      │      │
                     │  │  - Hierarchical task DAG                  │      │
                     │  │  - Admission control (RAM backpressure)   │      │
                     │  │  - Context swap-out (zstd → CAS)          │      │
                     │  │  - Interrupt + Ceza kademeleri            │      │
                     │  │  - Persona catalogue (Ben 10)             │      │
                     │  └───────┬────────────────────────────────────┘      │
                     │          │                                          │
                     │  ┌───────▼────────────────────────────────────┐     │
                     │  │  ork-router  (LLM Router / JEP Motoru)      │     │
                     │  │  - RoundRobin / Weighted / Fallback         │     │
                     │  │  - Judge-Executor-Planner (JEP)             │     │
                     │  │  - Grounding (kanıt-zorunlu)               │     │
                     │  │  - Trust score tracking                    │     │
                     │  └───────┬────────────────────────────────────┘     │
                     │          │                                          │
                     │  ┌───────▼────────────────────────────────────┐     │
                     │  │  ork-provider  (Provider Health & Key Mgr)  │     │
                     │  │  - Provider tespiti + doğrulama             │     │
                     │  │  - Health prob (real-time + passive)        │     │
                     │  │  - Key yönetimi (OS keyring + age)          │     │
                     │  └───────┬────────────────────────────────────┘     │
                     │          │                                          │
                     │  ┌───────▼────────────────────────────────────┐     │
                     │  │  ork-storage  (Kalıcı State Katmanı)        │     │
                     │  │  - redb (hot KV, uçucu agent state)         │     │
                     │  │  - SQLite schema (yeni tablolar)            │     │
                     │  │  - CAS blob store (video/DOM/context dump)  │     │
                     │  │  - WAL replay (xai-sqlite-journal üstüne)   │     │
                     │  └───────┬────────────────────────────────────┘     │
                     │          │                                          │
                     │  ┌───────▼────────────────────────────────────┐     │
                     │  │  ork-notify  (Bildirim)                     │     │
                     │  │  - Telegram Bot API                         │     │
                     │  │  - Twilio SMS (opsiyonel)                   │     │
                     │  └───────┬────────────────────────────────────┘     │
                     │          │                                          │
                     │  ┌───────▼────────────────────────────────────┐     │
                     │  │  ork-record  (Kayıt Sistemi)                │     │
                     │  │  - Event log (her zaman)                    │     │
                     │  │  - DOM/ekran yakalama (tetikli)             │     │
                     │  │  - Video stream (opsiyonel)                 │     │
                     │  └───────┬────────────────────────────────────┘     │
                     │          │                                          │
                     │  ┌───────▼────────────────────────────────────┐     │
                     │  │  ork-backup  (3-2-1 Yedekleme)             │     │
                     │  │  - SQLite VACUUM INTO + blake3 doğrulama    │     │
                     │  │  - CAS blob sync (içerik-adresli push)      │     │
                     │  │  - S3-uyumlu hedef                          │     │
                     │  └────────────────────────────────────────────┘      │
                     │                                                     │
                     └─────────────────────────────────────────────────────┘
```

### 2.1 Entegrasyon Stratejisi

- **Mevcut kod değişmez.** Yeni crate'ler mevcut trait'leri implemente eder veya mevcut struct'ları wrapper'a alır.
- `xai-grok-agent::Agent` — yeni `ork-runtime::ManagedAgent` wrapper'ına alınır; ek state (parent_id, depth, persona, budget, trust_score) eklenir.
- `xai-tool-runtime::Tool` trait'i — aynen kullanılır; yeni `ork-runtime::CapabilityBroker` her tool çağrısını policy'ye göre onaylar/ret eder.
- `xai-chat-state::ChatStateActor` — her agent'a bir tane; idle agent'ın context'i zstd + CAS'e swap-out edilir.
- `xai-tool-types::BuiltinSubagent` — genişletilir; Ben 10 persona seti eklenir.
- `xai-grok-subagent-resolution` — genişletilir; routing policy + capability allowlist + budget resolution eklenir.

### 2.2 Süreç Modeli

- Mevcut tokio multi-thread runtime korunur.
- Her agent bir tokio task + kendi `ChatStateActor`'ü + bounded state.
- **Admission control:** canlı RSS ölçümü ile yeni agent spawn'ı kuyruğa alınır; idle agent context'i CAS'e swap-out edilir.
- CPU-bound işler (tokenizasyon, sıkıştırma) mevcut rayon pool'una offload.

---

## 3. Yeni Klasör Yapısı (Omnitrix Workspace'e Eklenecek)

Mevcut `crates/` altına yeni bir `ork/` (orkestrasyon) alt dizini eklenir. Mevcut hiçbir crate taşınmaz veya değiştirilmez.

```
omnitrix/
├── Cargo.toml                          # workspace kökü (yeni ork crate'leri members'a eklenir)
├── MASTER-PLAN.md                      # bu dosya
├── ...
├── crates/
│   ├── build/        ... (mevcut)      # DEĞİŞMEZ
│   ├── codegen/      ... (mevcut)      # DEĞİŞMEZ — mevcut tüm xai-* crate'leri
│   ├── common/       ... (mevcut)      # DEĞİŞMEZ
│   │
│   └── ork/                            # ═══ YENİ ═══ Multiagent orkestrasyon crate'leri
│       ├── ork-runtime/                # Multiagent scheduler, hiyerarşi, interrupt, ceza, persona
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs
│       │       ├── scheduler.rs        # Work-stealing + admission control
│       │       ├── managed_agent.rs    # xai-grok-agent::Agent wrapper'ı
│       │       ├── hierarchy.rs        # Parent-child DAG, max_depth
│       │       ├── admission.rs        # RAM backpressure, context swap-out
│       │       ├── interrupt.rs        # Interrupt kademeleri (soft→hard)
│       │       ├── penalty.rs          # Ceza sistemi (5 kademe: uyarı→karantina)
│       │       ├── persona.rs          # Ben 10 persona kataloğu
│       │       ├── budget.rs           # Token/cost/latency bütçe takibi
│       │       └── research.rs         # Araştırma modları (yüzeysel/derin/okyanus)
│       │
│       ├── ork-router/                 # LLM Router + Judge-Executor-Planner motoru
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs
│       │       ├── strategies.rs       # RoundRobin, Weighted, Fallback
│       │       ├── jep.rs              # Judge-Executor-Planner modu
│       │       ├── judge.rs            # Judge: rubric + kanıt-zorunlu puanlama
│       │       ├── planner.rs          # Planner: görev DAG üretimi
│       │       ├── executor.rs         # Executor: planı tool çağrılarıyla uygula
│       │       ├── grounding.rs        # Kanıt-zorunlu doğrulama
│       │       └── trust.rs            # Trust score tracking (model+persona)
│       │
│       ├── ork-provider/               # Provider & key yönetimi
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs
│       │       ├── detection.rs        # Provider format/prefix tespiti + doğrulama
│       │       ├── health.rs           # Health prob (real-time + passive)
│       │       ├── keyring.rs          # OS keyring + age/ChaCha20Poly1305 fallback
│       │       └── ingestion.rs        # Model/price ingestion
│       │
│       ├── ork-storage/                # Kalıcı state katmanı
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs
│       │       ├── traits.rs           # Storage trait soyutlaması
│       │       ├── redb_store.rs       # redb sıcak KV
│       │       ├── sqlite_schema.rs    # Multiagent SQLite tabloları (migration'lar)
│       │       ├── wal.rs             # WAL replay (xai-sqlite-journal üstüne)
│       │       ├── cas.rs             # İçerik-adresli blob deposu (blake3)
│       │       └── writer_actor.rs     # Tek-writer aktör (SQLite serileştirme)
│       │
│       ├── ork-notify/                 # Bildirim sistemi
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs
│       │       ├── telegram.rs         # Telegram Bot API
│       │       └── twilio.rs           # Twilio SMS (opsiyonel)
│       │
│       ├── ork-record/                 # Kayıt sistemi
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs
│       │       ├── event_log.rs        # Event log (her zaman, ucuz)
│       │       ├── screen_cap.rs       # DOM/ekran görüntüsü (tetikli)
│       │       ├── video.rs            # Video stream (opsiyonel)
│       │       └── compressor.rs       # zstd stream → CAS
│       │
│       ├── ork-backup/                 # 3-2-1 yedekleme
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs
│       │       ├── snapshot.rs         # SQLite VACUUM INTO + CAS sync
│       │       ├── checksum.rs         # blake3 doğrulama
│       │       └── target.rs           # S3-uyumlu hedef push/pull
│       │
│       ├── ork-tui/                    # Multiagent izleme TUI'si (mevcut altyapıda)
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs
│       │       ├── dashboard.rs        # Agent grid, sağlık, metrikler
│       │       ├── agent_detail.rs     # Tek agent detay görünümü
│       │       └── interrupt_ui.rs     # Interrupt/onay UI'ı
│       │
│       ├── ork-webui/                  # WebUI multiagent panelleri
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs
│       │       └── ws_handlers.rs      # WebSocket event stream handler'ları
│       │
│       └── ork-daemon/                 # Ana daemon binary'si (orkd)
│           ├── Cargo.toml              # [[bin]] target
│           └── src/
│               ├── main.rs             # Daemon başlangıç, sinyal, yaşam döngüsü
│               ├── bootstrap.rs        # Config yükleme, storage init, provider keşif
│               └── api.rs              # Control-plane API (komut bus, WS)
│
├── prompts/                            # ═══ YENİ ═══ Persona prompt şablonları
│   ├── planner.md                      # Ghostfreak (görev DAG)
│   ├── executor.md                     # Four Arms (büyük yazım)
│   ├── judge.md                        # Brainstorm (kanıt-zorunlu puanlama)
│   ├── explorer.md                     # XLR8 (hızlı keşif)
│   ├── deep_explorer.md                # Wildmutt (derin codebase understanding)
│   ├── quick_fix.md                    # Grey Matter (mikro-fix)
│   ├── watcher.md                      # Big Chill (7/24 pasif izleme)
│   └── enforcer.md                     # Heatblast (interrupt/ceza uygulayıcı)
│
├── migrations/                         # ═══ YENİ ═══ SQL şema migration'ları
│   ├── 0001_ork_providers.sql
│   ├── 0002_ork_models_health.sql
│   ├── 0003_ork_tasks_agents.sql
│   ├── 0004_ork_tool_audit.sql
│   ├── 0005_ork_penalty_trust.sql
│   ├── 0006_ork_recordings.sql
│   └── 0007_ork_backups_write_journal.sql
│
├── config/                             # ═══ YENİ ═══ Orkestrasyon config profilleri
│   └── profiles/
│       ├── low.toml                     # Düşük kaynak profili
│       ├── mid.toml                     # Varsayılan profil
│       └── high.toml                    # Yüksek ölçek profili
│
└── tests/                              # ═══ YENİ ═══ Entegrasyon + yük testleri
    ├── multiagent_fanout.rs
    ├── admission_control.rs
    ├── crash_recovery.rs
    └── provider_fallback.rs
```

**Görünürlük kuralı:** Yeni `ork-*` crate'leri mevcut `xai-*` crate'lerine bağımlılık verebilir, ancak tersi asla olmaz. Mevcut kod değişmez. `ork-tui` ve `ork-webui` yalnızca `ork-daemon` API'si ve `ork-runtime` event stream'i üzerinden iletişim kurar.

---

## 4. Provider & Key Yönetimi (`ork-provider`)

### 4.1 Provider tespiti
- Mevcut `xai-grok-models` crate'i varsayılan model ID'lerini sağlar. `ork-provider` bunun üzerine:
  - **İki aşamalı tespit:** (1) API key format/prefix heuristiği ile aday provider daralt, (2) `/models` veya metadata endpoint çağrısı ile doğrula.
  - OpenRouter, LiteLLM gibi proxy'ler de desteklenir.

### 4.2 Key saklama
- Mevcut `xai-grok-secrets` crate'i varsa kullanılır; yoksa OS keyring + `age`/XChaCha20-Poly1305 fallback.
- Bellekte `zeroize`; loglarda redaction.

### 4.3 Health prob
- **Real-time:** aktif kullanım anında 429/5xx → anında degrade + router'a bildir.
- **Passive:** düşük frekanslı health prob (config'te aralık).
- Sonuçlar SQLite `provider_health` tablosuna yazılır.

---

## 5. LLM Router / Judge Motoru (`ork-router`)

### 5.1 Routing Policy (katmanlı)
```rust
RoutingPolicy {
    strategy: RoundRobin | Weighted | Fallback | JudgeExecutorPlanner,
    fallback_chain: Vec<ProviderModel>,   // her strateji içinde geçerli
    budget: Budget { max_tokens, max_cost, max_latency_ms },
    grounding: Required | Preferred | Off,
}
```
- JEP modu içinde executor rolü kendi `Fallback` zincirine sahiptir.
- Router her çağrıda `provider_health` tablosundan canlılık okur; `down`/`quota-exhausted` olanları zincirden çıkarır.
- Retry: exponential backoff + jitter; idempotent request id.

### 5.2 Judge-Executor-Planner (JEP)
- **Planner:** mevcut `xai-tool-types::BuiltinSubagent::PLAN`'den esinlenir, genişletilir. Görevi araştırma→plan→execute döngüsüne böler, alt-görev DAG'ı üretir.
- **Executor:** planı tool çağrılarıyla uygular. Mevcut `Agent::invoke_tools()` akışını kullanır.
- **Judge:** executor çıktısını kanıta göre puanlar:
  - (a) Tool-çıktısı ile tutarlılık (grounding),
  - (b) Plan hedefine uygunluk,
  - (c) Format/şema geçerliliği,
  - (d) Güven skoru.
  - Eşik altı → reddet + geri besleme + ceza.
- Judge kararı **rubric + zorunlu kanıt alıntısı** ister; iddiayı hangi tool çıktısının desteklediğini işaretlemek zorundadır.

### 5.3 Grounding (D6)
- "Asla halüsinasyon" garanti edilemez; **kanıt-zorunlu (grounding) + judge doğrulama + ihlal → interrupt/ceza** çerçevesi.
- Judge, executor çıktısındaki her iddiayı bir tool çıktısıyla eşleştirmek zorundadır.

---

## 6. Computer-Use & Tool Enforcement (Mevcut Altyapı + Genişletme)

### 6.1 İlke
LLM ham OS/shell'e asla erişemez. Mevcut `xai-grok-sandbox` + `xai-computer-hub-core` + `xai-tool-runtime` bu ilkeyi zaten sağlar. `ork-runtime` ek olarak:

- Her subagent tipine **capability allowlist** atar.
- **Capability Broker:** her eylemi policy'ye göre onaylar/ret eder; riskli eylemler onay kapısı gerektirir.
- Agent'ın sandbox profili, personasına göre otomatik seçilir.

### 6.2 Capability modeli
```rust
Capability {
    fs: FsCap { read: bool, write: bool, exec_allowlist: Vec<PathBuf> },
    net: NetCap { http: bool, host_allowlist: Vec<String> },
    gui: GuiCap { input: bool, screencap: bool },
    process: ProcessCap { spawn_allowlist: Vec<String> },
}
```
- Mevcut `xai-grok-sandbox::ProfileName` ile eşleştirilir.
- Her persona tanımına capability seti gömülüdür (bkz. §8).

---

## 7. Native Tool Seti (Mevcut + Genişletme)

Mevcut `xai-grok-tools` zaten zengin bir tool seti sağlar. `ork-runtime` ek olarak:
- Tool çağrılarını JSON-Schema'ya göre validasyon,
- Serbest-metin "shell komutu" tespiti → reddet + ceza,
- `ToolCallContext` üzerinden capability kontrolü enjekte eder.
- Yeni araçlar (`codebase_map` genişletmesi) mevcut `Tool` trait'i implemente edilerek eklenir.

---

## 8. Multiagent Runtime, Hiyerarşi & Subagent Tipleri (`ork-runtime`)

### 8.1 Hiyerarşi & rekürsiyon
- Rekürsif görevlendirme derinliği `max_depth = 5` (config).
- Eşzamanlı agent üst sınırı: `max_active_agents` (dinamik, admission control belirler).
- Admission control: canlı RAM ölçümü (RSS + arena) → eşiğe yaklaşınca yeni spawn *kuyruğa* alınır; idle agent context'i CAS'e swap-out → RAM boşalır, gerektiğinde geri yüklenir.
- **Aktif ≠ var-olan ayrımı:** 10.000+ agent *diskte var olabilir*, ama aktif çalışan sayısı RAM'e göre dinamik tavanlanır.

### 8.2 Scheduler
- Mevcut tokio work-stealing havuzu kullanılır.
- Her agent bir tokio task + kendi `ChatStateActor`'ü.
- Idle context'ler `zstd` ile sıkıştırılır; sıcak pencere RAM'de.

### 8.3 Subagent tipleri (persona) — Ben 10 isimleri (D5)

Mevcut `xai-tool-types::BuiltinSubagent` (PLAN, EXPLORE, GENERAL_PURPOSE) genişletilir:

| Persona | Rol | Capability | Routing | Prompt |
|---------|-----|-----------|---------|--------|
| **Kaşif (XLR8)** | Hızlı geniş keşif/araştırma | fs.read, net.http, search | RoundRobin, hız-öncelikli | `prompts/explorer.md` |
| **Gezgin (Wildmutt)** | Derin codebase understanding | fs.read, codebase_map | Weighted, kalite | `prompts/deep_explorer.md` |
| **Juryrigg (Grey Matter)** | Mikro-fix / hızlı edit | fs.read/write, edit | Fallback | `prompts/quick_fix.md` |
| **İnşaatçı (Four Arms)** | Büyük yazım/refactor | fs.*, spawn_managed | JEP | `prompts/executor.md` |
| **Yargıç (Brainstorm)** | Judge rolü | fs.read (kanıt) | Judge modeli, düşük sıcaklık | `prompts/judge.md` |
| **Planlayıcı (Ghostfreak)** | Görev DAG üretimi | fs.read, codebase_map | Planner | `prompts/planner.md` |
| **Gözcü (Big Chill)** | 7/24 pasif araştırma/izleme | net.http, search | Passive, düşük bütçe | `prompts/watcher.md` |
| **Denetçi (Heatblast)** | Interrupt/ceza uygulayıcı | control-plane | — | `prompts/enforcer.md` |

Her persona: rol + capability allowlist + routing policy + sistem prompt. Mevcut `AgentBuilder` + `AgentDefinition` ile inşa edilir.

### 8.4 Interrupt & Ceza (5 kademe)
1. **Context uyarısı** (soft): geri besleme enjekte, tekrar dene.
2. **Task durdurma** (medium): mevcut alt-görev iptal, replan.
3. **Agent sonlandırma** (hard): agent kill, kaynak geri al.
4. **Güven düşürme** (kalıcı): model/persona `trust_score` düşer → router daha az yönlendirir.
5. **Karantina:** tekrarlayan ihlalde provider/model geçici devre dışı.

Tüm cezalar `penalty_log` tablosuna yazılır.

### 8.5 Araştırma modları
| Mod | Kaynak | Derinlik | Süre | Token |
|-----|--------|----------|------|-------|
| Yüzeysel | 5 | 1 | 60 sn | düşük |
| Derin | 30 | 3 | 10 dk | orta |
| Okyanus | 200+ | max_depth | bütçe-güdümlü | yüksek |

---

## 9. Veritabanı Tasarımı (`ork-storage`)

### 9.1 Katman ayrımı
- **redb (sıcak, uçucu):** aktif agent state, context pencereleri, kuyruklar. Hızlı, işlem-sonu temizlenebilir.
- **SQLite/WAL (kalıcı):** yapılandırılmış geçmiş, konfig, sağlık, kayıt metadata. Mevcut `xai-sqlite-journal` üzerine inşa, **tek writer aktör** ile serileştirilir.
- **CAS blob store:** video/DOM/context dump; SQLite yalnızca `blob_ref (blake3 hash + boyut + kodek)` tutar.

### 9.2 SQLite şema taslağı (migration'lar)
```sql
-- Provider & anahtar (0001)
providers(id PK, name, kind, base_url, created_at)
api_keys(id PK, provider_id FK, key_ref, label, status, created_at)

-- Model & sağlık (0002)
provider_health(id PK, provider_id FK, model, state, latency_ms, checked_at, detail)
models(id PK, provider_id FK, name, ctx_len, price_in, price_out, caps_json)
routing_policies(id PK, name, strategy, config_json)

-- Görev & agent hiyerarşisi (0003)
tasks(id PK, parent_id FK NULL, root_id FK, title, mode, status, depth, created_at, closed_at)
agents(id PK, task_id FK, persona, parent_agent_id FK NULL, state, sandbox_profile, created_at, ended_at)
agent_events(id PK, agent_id FK, seq, kind, payload_json, ts)
messages(id PK, agent_id FK, role, provider_model, content_ref, tokens_in, tokens_out, cost, ts)

-- Tool denetimi (0004)
tool_calls(id PK, agent_id FK, tool, args_json, result_ref, status, capability_ok, ts)
capability_audit(id PK, agent_id FK, capability, target, decision, approver, ts)

-- Denetim & güven (0005)
penalty_log(id PK, agent_id FK, level, reason, applied_by, ts)
interrupts(id PK, agent_id FK, kind, source, ts, resolved_at)
trust_scores(subject_id, subject_kind, score, updated_at)

-- Kayıt (0006)
recordings(id PK, agent_id FK, media_type, blob_ref, bytes, codec, started_at, ended_at)

-- Yedekleme & WAL kontrol (0007)
backups(id PK, scope, destination, status, checksum, size, started_at, finished_at)
write_journal(id PK, op_id UNIQUE, op_kind, payload_ref, applied BOOL, ts)
```

### 9.3 Migration
- `migrations/NNNN_isim.sql` sıralı; `schema_version` tablosu ile ileri-yalnız.
- Mevcut `xai-sqlite-journal` migration runner'ı kullanılır veya genişletilir.

### 9.4 Durability & replay
- Mevcut `xai-sqlite-journal` WAL soyutlaması temel alınır. `ork-storage` üzerine idempotent replay katmanı ekler.
- Kapanış: sinyal → yeni iş dur → mevcut WAL checkpoint → fsync → çık.
- Açılış: `write_journal.applied=false` replay → kaldığı yerden devam.

---

## 10. Kayıt Sistemi (`ork-record`)

- Varsayılan opsiyonel/tetiklemeli (her zaman video RAM'i patlatır).
- Akış: yakalama → doğrudan zstd stream → CAS dosya (RAM'de ring-buffer, KB seviyesi).
- Türler: event-log (her zaman), DOM/ekran (tetikli), video (opsiyonel).
- Metadata SQLite `recordings`; ham veri CAS'te.
- Retention: yaş/boyut tavanı → eski blob GC.

---

## 11. Bildirim & 3-2-1 Yedekleme

### 11.1 Bildirim (`ork-notify`)
- **Telegram Bot API** varsayılan; kritik olay/insan-onayı gereken interrupt → mesaj.
- **Twilio** SMS/sesli arama opsiyonel, yalnızca `notify.escalation=true` yüksek-önem olaylarında.

### 11.2 Yedekleme (`ork-backup`)
- **3-2-1:** (3) kopya = canlı + yerel snapshot + uzak; (2) farklı medya = yerel disk + bulut; (1) offsite = S3-uyumlu.
- Mekanizma: uygulama-güdümlü snapshot (SQLite `VACUUM INTO` + CAS blob sync), blake3 checksum doğrulama.
- Syncthing **kullanılmaz.**
- Hedef: kullanıcı config'te S3-uyumlu endpoint seçer.

---

## 12. Config & Kaynak Profilleri

Mevcut `xai-grok-config` katmanlarına ek olarak `config/profiles/` altında:

```toml
# config/profiles/mid.toml  (varsayılan)
[runtime]
max_active_agents = "auto"      # auto = admission control belirler
mem_high_watermark_mb = 0       # 0 = fiziksel RAM'in %80'i
swap_out_idle_ms = 30_000       # idle context'i CAS'e swap-out süresi
max_depth = 5

[router]
default_strategy = "fallback"
grounding = "required"

[record]
video = false
dom = false
events = true

[notify]
escalation = false
```

`low.toml` / `high.toml` profilleri admission-control eşiklerini ve bütçeleri ayarlar.

---

## 13. TUI & WebUI Entegrasyonu

### 13.1 TUI (`ork-tui`)
- Mevcut ratatui altyapısı (`xai-ratatui-inline`, `xai-grok-pager-render`) üzerine inşa edilir.
- Yeni paneller:
  - **Dashboard:** aktif agent grid'i, sağlık göstergeleri, kuyruk derinliği, maliyet/token metrikleri.
  - **Agent detay:** tek agent'ın task DAG'ı, tool çağrı geçmişi, context penceresi.
  - **Interrupt UI:** onay bekleyen eylemler için prompt.

### 13.2 WebUI (`ork-webui`)
- Mevcut axum + WebSocket (`tungstenite`) altyapısı kullanılır.
- `ork-daemon` control-plane API'si üzerinden TUI ile parite.

---

## 14. Geliştirme Yol Haritası (Fazlar)

Her faz çalışır bir dikey dilim üretir. Mevcut omnitrix kod tabanı değişmez.

### Faz 0 — İskelet & Entegrasyon Noktaları
- `crates/ork/` dizini oluşturulur.
- Cargo workspace `members`'a yeni crate'ler eklenir.
- `ork-daemon` binary iskeleti (config yükleme, `tracing`, sinyal).
- Mevcut `AgentBuilder` ile bir agent spawn edip `ork-runtime` wrapper'ına alma PoC.
- `ork-storage` trait tanımı + `xai-sqlite-journal` entegrasyonu.
- Config profili yükleme (`low/mid/high.toml`).

### Faz 1 — Depolama Çekirdeği (`ork-storage`)
- `redb` sıcak KV implementasyonu.
- SQLite migration'ları (0001-0007), schema init runner.
- CAS blob store (blake3 + dizin).
- Tek-writer aktör (SQLite serileştirme).
- WAL replay (idempotent `write_journal`).
- Test: crash-recovery, idempotent replay, tek-writer yük.

### Faz 2 — Provider & Router (Tek agent)
- `ork-provider`: tespit + key mgr + health prob.
- `ork-router`: RoundRobin + Fallback stratejileri.
- Mevcut `Agent` + `AgentBuilder` ile tek agent, router üzerinden uçtan uca çalışır.
- Health prob sonuçları SQLite'a yazılır, router okur.

### Faz 3 — Multiagent Runtime (`ork-runtime`)
- Scheduler: work-stealing + admission control (RAM backpressure).
- `ManagedAgent`: mevcut `Agent` wrapper'ı; parent-child DAG, depth tracking.
- Persona sistemi: `BuiltinSubagent` genişletmesi + capability allowlist + routing policy.
- Prompt şablonları (`prompts/`).
- Context swap-out: idle agent context → zstd → CAS; restore.
- Interrupt + ceza kademeleri (1-5).
- Trust score tracking.

### Faz 4 — JEP & Grounding (`ork-router` genişletme)
- Planner/Executor/Judge rolleri.
- Rubric + kanıt-zorunlu judge.
- Judge → executor geri besleme döngüsü.
- Ceza entegrasyonu (düşük puan → soft interrupt → hard kill).

### Faz 5 — TUI Dashboard (`ork-tui`)
- Dashboard paneli (agent grid, metrikler).
- Agent detay görünümü.
- Interrupt onay UI'ı.
- Mevcut TUI altyapısına entegre.

### Faz 6 — WebUI Panelleri (`ork-webui`)
- Mevcut axum + WS altyapısına multiagent event stream handler'ları.
- TUI ile parite.

### Faz 7 — Computer-Use + Kayıt (`ork-record`)
- GUI input/screencap capability (mevcut `xai-computer-hub-*` genişletmesi).
- Event log, DOM/ekran yakalama, video stream → zstd → CAS.
- Retention GC.

### Faz 8 — Bildirim + Yedekleme (`ork-notify`, `ork-backup`)
- Telegram Bot API entegrasyonu.
- Twilio opsiyonel.
- 3-2-1 snapshot + S3 push + blake3 doğrulama.

### Faz 9 — Ölçek Sertleştirme
- Yük testleri (10.000+ var-olan agent, dinamik aktif tavan).
- OOM-öncesi backpressure doğrulama.
- Bellek profilleme (`heaptrack`/`dhat`).
- Cold-start/shutdown süre ölçümü.
- Provider failover senaryoları.

---

## 15. Mevcut Crate'lere Dokunulacak Noktalar (Minimum Set)

Aşağıdaki noktalarda mevcut kod tabanına **minimum müdahale** ile entegrasyon sağlanır:

| Crate | Değişiklik | Gerekçe |
|-------|-----------|---------|
| `xai-tool-types` | `BuiltinSubagent` enum'ına yeni varyantlar eklenir (veya yeni bir enum parallel tanımlanır) | Persona kataloğu |
| `xai-grok-subagent-resolution` | `EffectiveRuntimeConfig`'e routing policy + budget + capability allowlist alanları eklenir | Subagent spawn'da routing kararı |
| `xai-grok-config` | Orkestrasyon config section'ı (`[ork]`) tanınır | Profil yükleme |
| `xai-grok-shell` / `xai-grok-workspace` | `ork-runtime` başlatma kancası eklenir | Daemon yaşam döngüsü |
| `Cargo.toml` (workspace kök) | Yeni `ork-*` crate'leri members'a eklenir | Derleme |

**Prensip:** Değişiklikler extension noktalarından (enum varyantı, config section, init hook) yapılır; mevcut davranışı kırmaz.

---

## 16. Açık Riskler & Azaltma

| Risk | Etki | Azaltma |
|------|------|---------|
| Halüsinasyon (D6) | Yanlış eylem, para kaybı | Grounding-required + judge + capability onay kapısı |
| Ölçekte RAM | OOM | Admission control + context swap-out + aktif≠var-olan ayrımı |
| SQLite tek-writer darboğazı | Yazma gecikmesi | Tek-writer aktör + redb sıcak yol + batch commit |
| Provider quota/kesinti | Görev durması | Health prob + fallback zinciri + karantina |
| Computer-use kötüye kullanımı | Sistem hasarı | Capability allowlist + sandbox profile + audit + onay kapısı |
| Yedek bütünlüğü | Veri kaybı | Uygulama-güdümlü snapshot + blake3 checksum + 3-2-1 |
| Mevcut kodu kırma | Regresyon | Mevcut test suite çalışır kalmalı; yeni crate'ler izole |

---

## 17. Sonraki Adım

Bu plan onaylanırsa **Faz 0**'dan başlanır:
1. `crates/ork/` dizini + Cargo workspace güncellemesi.
2. `ork-daemon` binary iskeleti (`main.rs`, config yükleme, tracing).
3. `ork-storage` trait tanımı + `xai-sqlite-journal` entegrasyonu.
4. PoC: mevcut `AgentBuilder` ile bir agent spawn edip `ManagedAgent` wrapper'ına alma.
5. Config profilleri (`low/mid/high.toml`).

Her faz sonunda çalışır dikey dilim teslim edilir. Herhangi bir kararı (özellikle §8 persona seti, §5 routing varsayılanları, §12 profiller) değiştirmek istersen implementasyondan önce netleştirebiliriz.
