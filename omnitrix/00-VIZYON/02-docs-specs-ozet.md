# Omnitrix — Docs & Specs Özeti

> Kaynak: `docs/` dizini (ARCHITECTURE.md, provider-connect.md, routing-modes.md, `docs/superpowers/` altındaki 11 spec/plan/checklist/prompt belgesi — toplam 14 belge, 2026-08-14 itibarıyla çıkarılmıştır).
> Otorite zinciri: `MASTER-PLAN.md` → `ALPHA-PLAN.md` → `docs/ARCHITECTURE.md` (ozet, otorite degildir). Spec'ler `docs/ARCHITECTURE.md` → mevcut kod seam'leri hiyerarşisine tabidir; çelişkide seam + I1–I8 invariantları kazanır.

---

## 0. Belge envanteri

| Belge | Tip | Feature / Amaç | Durum |
|---|---|---|---|
| `docs/ARCHITECTURE.md` | Mimari özet | Katman modeli, crate haritası, invariantlar | Otorite: MASTER-PLAN |
| `docs/provider-connect.md` | Kullanım kılavuzu | `/connect`, `grok connect`, `grok keys`, güvenlik modeli | Tamam (v1) |
| `docs/routing-modes.md` | Kullanım kılavuzu | Routing mode kataloğu CLI/TUI arayüzü | v1 (P1 sonrası) |
| `specs/2026-07-29-no-telemetry-design.md` | Spec | 6 telemetri kanalını kalıcı yok etme | Onaylı |
| `plans/2026-07-29-no-telemetry.md` | Plan | Patch uygulama + CI kapıları (I2a muafiyet, I8 gate) | Tamam |
| `specs/2026-08-09-provider-connect-design.md` | Spec | Provider bağlama sistemi (keychain, models.dev, TUI+CLI) | Onaylandı |
| `plans/2026-08-09-provider-connect.md` | Plan | 9+ task'lık implementasyon (compiler YASAK) | Tamam |
| `specs/2026-08-09-flow-governor-design.md` | Spec | Deterministik akış boru hattı denetleyicisi | Onaylandı, TAM teslim |
| `plans/2026-08-09-flow-governor.md` | Plan | Durum makinesi + sınıflandırıcı + kapı + governor + seam'ler | Tamam |
| `specs/2026-08-09-omnitrix-platform-v2-design.md` | Spec | 8 sütunlu platform v2 (Auto-Connect…Native Tools) | Onaylı |
| `plans/2026-08-09-omnitrix-platform-v2.md` | Plan | P0–P8 fazları, subagent-driven | P0–P1 tamam, P2+ açık |
| `prompts/2026-08-09-platform-v2-agent-prompt.md` | Agent prompt | Controller ajan çalışma sözleşmesi | Kullanımda |
| `plans/2026-08-06-omnitrix-harness-tamamlanmasi.md` | Plan | Harness Faz 0–10 kapılarını kapatma | Kısmen |
| `checklists/2026-08-09-persona-menagerie.md` | Checklist | 61 persona içerik TODO (soul/tools/routing/budget/test) | Açık |

---

## 1. Mimari (ARCHITECTURE.md)

### 1.1 Tez
Omnitrix bir **orkestrasyon katmanıdır**: tek-ajan runtime, LLM taşıma ve tool sistemi vendored `xai-*` ağacından gelir; omnitrix üzerine çok-ajan planlama, yönlendirme, persona, sağlayıcı-besleme, uzak erişim ve dayanıklılık ekler.

Sınır iki yönlüdür:
- `omni-*` → `xai-*`'a bağımlıdır (imza düzeyinde, MASTER-PLAN Bölüm 2 tablosu), tersi değil.
- `xai-*` **düzenlenmez** — `crates/common/` + `crates/codegen/` monorepo'dan senkronlanan vendored kod (sürüm: `SOURCE_REV`), kütüphane gibi tüketilir, fork edilmez. Bu, invariant `I2`'dir ve `bin/ci-gates.sh` ile zorlanır.

> İSTİSNA (kullanıcı onayıyla kabul edildi): No-telemetry patch'leri ve Flow Governor, `xai-*` ağacını seam noktalarından **bilerek** değiştirir. Platform v2 spec'i "hayali `omni-*` crate açılmaz; orkestrasyon eritilmiş halde `xai-grok-{shell,pager,tools,sampler}` içinde yaşar" der.

### 1.2 Katman modeli
```
Yüzler (K7/K8, durum tutmaz):   omni-webui · uzak kanallar (omni-notify)
Kontrol düzlemi (tek API, auth): omni-control
Çekirdek (tek süreç, tek durum): omni-proto + omni-core · omni-scheduler · omni-router · omni-tools · omni-agent
Temel (VENDORED, salt-tüketim):  xai-* (sampler · agent · tools · models · config)
Dayanıklılık (crash-only):        omni-storage · omni-provider
```

Okuma kuralları: (1) yüzler asla çekirdek durumu tutmaz; (2) çekirdek tek süreçte, mutasyonlar tek noktadan (`omni-core`); (3) her yan etkili işlem **önce** WAL niyet kaydı (`I7`); (4) `xai-*` en altta.

### 1.3 Crate haritası (özet)
| Crate | Sorumluluk |
|---|---|
| `omni-proto` | Kanonik durum modeli, olay/komut tipleri — tek kaynak |
| `omni-core` | Domain tipleri, ajan/görev durum makinesi, orkestrasyon çekirdeği |
| `omni-config` | Katmanlı config: env > DB > dosya |
| `omni-control` | Tek kontrol API'si: auth, SSE/WS yayını, komut girişi |
| `omni-scheduler` | `SubagentBackend`; tier geçişleri, kaynak valisi, rekürsiyon/fan-out tavanları, bütçe kalıtımı, interrupt |
| `omni-router` | Routing modları (`round_robin`/`weighted`/`fallback`/`jep`), rol→model çözümü, yanlışlamacı yargıç + grounding |
| `omni-tools` | Yetki broker'ı, diff-stream fs-shim, kod öğrenme, edit/search kablolaması |
| `omni-agent` | `xai_grok_agent::Agent` sarmalayıcısı; persona → `AgentDefinition` |
| `omni-storage` | CAS, WAL, SQLite/redb, tek-writer aktör, checkpoint |
| `omni-provider` | Sağlayıcı tespiti, keyring, health, anahtar besleme |
| `omni-backup` / `omni-record` | 3-2-1 SigV4+şifreli yedek / oturum kaydı |
| `omni-webui` / `omni-notify` / `omni-research` | SSR yüz / bildirim kanalları / sağlayıcı-değiştirilebilir araştırma |
| `omnitrix` / `omni-tests` / `omni-bench` | Binary (lazy-init) / kapı test takımı / faz kapısı ölçümü |

### 1.4 Durum akışı, yaşam döngüsü, dayanıklılık
- **Okuma:** yüz → `omni-control` → ilk anlık görüntü + olay akışı (SSE/WS). TUI ve WebUI **aynı** akışı tüketir. **Yazma:** komut → cekirdek → sonuç her iki yüze yayılır. Tek yazar: `omni-core` → `omni-storage` writer aktörü (WAL).
- **Ajan tier modeli:** `Existing → Sleeping → Queued → Active`. Aktif slot sayısı sabit değil (kaynak valisi dinamik). **Çalışan görev asla düşürülmez** — basınçta en uzun boştaki aktif ajan uykuya alınır. Spawn'da üç tavan: derinlik, seviye başına fan-out, global aktif. Bütçe ebeveynden zarfla kalıtılır. Görev ancak otomatik doğrulama + yargıç onayı + kullanıcı onayı ile `Done`.
- **Crash-only:** normal kapanış = ani ölüm. Niyet kaydı (`write_journal`, `op_id` UNIQUE, `applied=0`) → işlem → `applied=1` → kapanış O(1), flush yok → açılışta `applied=0` replay. Cold-start: ilk TUI frame'i hiçbir sağlayıcıya bağlanmadan çizilir, ısınma sonradan arka planda.

### 1.5 Invariantlar (I1–I8)
- `I1` kapi = komut + metrik + esik · `I2` tek yönlü `xai-*` bağımlılığı · `I3` tek ortak durum kaynağı · `I4` yetki tek noktada · `I5` model adı/fiyatı gömülü değil (rol→katalog) · `I6` üretim yolunda `unwrap`/`expect`/`panic!` = 0 · `I7` önce WAL niyet kaydı · `I8` her olgusal iddia kanıt referanslı.

---

## 2. Routing modları

### 2.1 Genel semantik
- Mod tanımları `config/routing_modes.toml`'dan yüklenir (family + başlık + 1 cümle blurb + uzun help + params + selector). Seçili id `config/routing.toml`'da `strategy = "<id>"` olarak P1.4 config köprüsüyle saklanır; legacy alias kabul edilir (`fallback` → `fallback-strict`).
- CLI: `grok routing list|set <id>|show|explain <id>`. `set` yalnızca kanonik katalog id'si kabul eder, bilinmeyen id dosya değişmeden hata verir.
- TUI: `/routing` veya komut paleti → fuzzy arama (id+başlık+family), Tab/Ok tuşları, alt pane blurb. `/omni-routing` legacy görev atamasıdır; `/connect` routing ile ilgisizdir.

### 2.2 Katalog v1 (≥40 mod, 7 aile)
| Family | Örnek modlar |
|---|---|
| Balance | `rr`, `wrr`, `least-conn`, `ewma-latency`, `token-bucket-fair`, `sticky-session`, `hash-prompt` |
| Failover | `fallback-strict`, `fallback-soft`, `backup-only-on-429`, `backup-on-5xx`, `circuit-break-cascade`, `hedge-p95` |
| RoleSplit | `jep-classic`, `jep-cheap-plan`, `jep-strong-judge`, `planner-only-chain`, `executor-swarm` |
| Hybrid | `balance-then-fallback`, `jep-with-rr-executors`, `canary-10`, `shadow-mirror` |
| Specialty | `research-ocean-prefer`, `coding-long-context`, `json-strict-model`, `vision-capable-only`, `tool-call-reliable` |
| Cost | `cheapest-alive`, `budget-aware`, `quality-floor`, `deadline-aware` |
| Privacy | `local-first`, `no-train-providers`, `eu-region-only` |

- **Seçici motor:** tek `RouterEngine::pick(ctx) -> EndpointHandle`; modlar strateji plug-in (yeni mod = tablo satırı + gerekirse küçük selector fn). P1.3'te uygulanan selectors: rr, wrr, fallback-strict, jep-classic, cheapest-alive, balance-then-fallback, hedge (basit), sticky-session; diğerleri ilkel bileşimler (TOML'de `selector = "fallback"` eşlemesi).
- `RouteContext`: prompt_hash, rol (judge|executor|planner), canlı endpoint'ler, bütçe anlık görüntüsü. `RouterEngine::on_result` öğrenme (latency/hata).
- **JEP varsayılan rolleri** (I5, değiştirilebilir): judge←judge, executor←executor, planner←planner (`config/routing.toml [jep]` + persona override).
- Mevcut fallback walk: `xai-grok-sampler/src/retry.rs` (`FallbackRouter`).

---

## 3. Provider bağlantı mimarisi

### 3.1 Bileşenler (4 parça)
1. **`xai-omni-keychain`** — şifreli key deposu: `~/.grok/keychain.omx` (GROK_HOME duyarlı). AES-256-GCM; anahtar = `Argon2id(master_password, salt)` (64 MiB, t=3, p=1), anahtar **asla diskte değil**; RAM'de `MasterKeyCache` 15 dk TTL, sonrası `zeroize` + kilit. API: `open/list_keys/reveal/add_key/update_key/remove_key/categories/set_default_category/export/import/borrow`. `BorrowedKey` = Drop'ta zeroize, `is_expired` TTL takibi — ajan erişimi bu yoldan.
2. **models.dev client** (`xai-grok-shell/src/util/models_dev.rs`) — `GET https://models.dev/api.json`, cache `~/.grok/models.dev.json` **24 saat TTL**; bayat+yok → ağ; ağ başarısız+bayat cache → fallback bayat cache; hiçbiri → hata. npm → ApiBackend eşlemesi: anthropic→`messages`, openai/xai→`responses`, openai-compatible→`chat_completions` (+`/v1`). Model listesi fallback zinciri: katalog → provider `/models` fetch → manuel ID.
3. **TUI wizard** (`/connect`, Ctrl+P, welcome menüsü): `ProviderConnectFlow` durum makinesi — Provider (rozet: `[key]`/`env`/yeni + 2 sabit custom satırı) → BaseUrl (custom'da) → Key (yeni/keychain/env) → Kategori → Model (canlı, rozetler) → Apply (config yazımı + switch). `/keys` = keychain yöneticisi (list/reveal/add/edit/remove/export/import/categories).
4. **CLI** — `grok connect --provider --api-key --base-url --model --keychain-id --category --no-session [--auto]`; `grok keys list|show|add|edit|remove|export|import|categories`. Çözüm önceliği: provider: flag > keychain kaydı; base URL: flag > kayıt > katalog; model: flag > kayıt > katalog.

### 3.2 `--auto` (otomatik provider tespiti)
`grok connect --auto --api-key ...` → keychain prefix detect (saf tablo, ≥25 kural: `sk-ant-`→anthropic, `sk-or-`→openrouter, `sk-proj-`/`sk-`→openai, `AIza`→google, `gsk_`→groq, `xai-`→xai, vb.) → adayları models.dev'den çöz → her adaya **canlı probe** (region endpoint'leri paralel, timeout 2.5s; HTTP 200/401 ayrımı — 401 = host doğru key yanlış) → winner {provider_id, region, base_url, auth_scheme} → model adımına atla. Eşit skor → **ambiguous** hatası (key hiçbir hata mesajına girmez); tüm probe'lar başarısız → hata. `--api-key` zorunludur; `--provider/--base-url/--keychain-id` ile çakışma deterministik parse-time hatasıdır.

### 3.3 Güvenlik modeli
| Bileşen | Değer |
|---|---|
| Dosya | `~/.grok/keychain.omx` (GROK_HOME duyarlı) |
| KDF | Argon2id (64 MiB) → 32 byte; şifre hiçbir yerde saklanmaz |
| Şifreleme | AES-256-GCM (nonce+tag+ciphertext, base64) |
| Anahtar | Asla diskte değil; her açılışta türetilir; RAM TTL 15 dk → zeroize → kilit |
| Ajan erişimi | `borrow()` → `BorrowedKey` guard (TTL, zeroize) |
| Runtime key store | Connect sonrası key process runtime store'a itilir; `config.toml`'a `api_key` **asla yazılmaz** — shell'e `KeychainCredentialProvider` (`AuthCredentialProvider` trait impl, `snapshot()`'ta borrow) enjekte edilir |
| Export | Master'dan bağımsız export şifresi: kendi salt + Argon2id + AES-256-GCM; metadata düz metin, key'ler şifreli; çakışan (kategori, provider) yalnızca `--overwrite` ile |

### 3.4 Anahtar besleme (Credential Feeder, P2)
- Kaynak (NOT — geçici sütun): `/home/void0x14/Documents/ihsan-agama-verilen-destek/backshoot/data/verifier.db`, sütun `usable_credentials` (kodda `// NOT: sütun geçici` yorumu zorunlu).
- Boru hattı: read-only SQLite → blob parse → Auto-detect (3.1) → live probe → **live** → keychain kategorisi `feeder-live` + provider pool; **dead** → `feeder-dead-hold` (silinmez, revive poll) → audit event.
- Tetikleyiciler: (1) reaktif — RouterEngine tüm endpoint'leri `Dead|CircuitOpen` görünce; (2) pasif — arka plan tick (varsayılan 15 dk, `feeder.poll_secs`); (3) manuel — `grok keys feed --now`.
- Güvenlik: DB path config'te, sandbox dışı path explicit allow; key'ler asla düz loglanmaz (mask).

---

## 4. No-Telemetry (2026-07-29)

**Amaç:** vendored `xai-*` ağacında 6 telemetri kanalını kalıcı devre dışı bırakmak; env/config/remote hiçbir ayar tekrar açamaz (kullanıcı isteği: xAI'ya veri gitmesin).

Yok edilen kanallar: (1) Mixpanel product analytics + HTTP POST'lar, (2) xai-mixpanel tüm network I/O (belt-and-braces), (3) Sentry, (4) OTLP export, (5) trace/GCS upload, (6) /feedback.

**Tasarım kararları:**
- Patch'ler doğrudan xai-* ağacına uygulanır (overlay değil); kaynaklar `third_party/no-telemetry-patches/0001…0006`'da saklanır (SHA256 referanslı).
- I2a diff taramasına **kalıcı koşulsuz muafiyet** (grep -vE): `xai-grok-telemetry/`, `xai-mixpanel/`, `xai-grok-shell/src/agent/config.rs`, `auth/credential_provider.rs`, `extensions/feedback.rs`. Diğer tüm xai-* dosyaları denetlenir.
- Yeni **I8 faz kapısı**: 6 fonksiyonun üretim yolunda literal değerini grepl'eyerek assert eder (`is_telemetry_enabled→false`, `is_session_metrics_enabled→false`, `is_trace_upload_enabled→false`, `is_feedback_enabled→false`, `is_error_reporting_disabled_sync→true`, `is_telemetry_explicitly_disabled_sync→true`). Biri değişirse kırmızı.
- **Mimari invariant bozulur ama kabul edilir** (bilinçli kullanıcı kararı): I2a muafiyeti + I8 gate bu kararı kapatır.
- Rebase prosedürü: upstream senkronu sonrası `scripts/rebase-no-telemetry.sh` patch'leri yeniden uygular, I8 doğrular.
- Doğrulama: `cargo check --workspace --all-targets`, `cargo test --workspace`, `omnitrix --version`, `OMNI_NO_TELEMETRY_PATCHES=1 bin/ci-gates.sh`, `bin/ci-gates.sh --build`.

---

## 5. Flow Governor (2026-08-09)

### 5.1 Problem ve ilkeler
Çok adımlı akış **asla AI inisiyatifinde değildir**: kararları yine AI verir, ama sonuçları durum makineleri ve deterministik, araç içine gömülü döngüler yönetir. AI adım atlarsa sistem tespit eder, **görünmez** düzeltir (direktif enjekte eder; kullanıcı yalnızca aşama geçişlerini görür). Basit görevlerde ağır akış zorunlu değil — akış seçimi de sisteme aittir. Kısıt: **Rust derleyicisi asla çalıştırılamaz**; doğrulama statik inceleme (grep, brace dengesi python'u) ile.

### 5.2 Mimarisi
Oturum başına bir `FlowGovernor` (`Arc<parking_lot::Mutex<...>>`, PlanModeTracker deseni), dört karar noktasında kilitler:
- **A. Prompt aktivasyonu** (`handle_prompt`): görevi sınıflandır, akışı seç, direktifi enjekte et.
- **B. Tool görünürlüğü** (`prepare_tool_definitions_inner`): kilitli aşamanın tool'larını LLM'den **gizle** (modele "üretemeyeceği" garanti).
- **C. Tur sonu** (`run_stop_gate`): kanıt eksikse `KeepWorking{direktif}`; `MAX_REDIRECTS_PER_STAGE = 3` aşılınca deterministik EndTurn.
- **D. Goal devam** (`run_goal_round_end`): akış aktifken AI-güdümlü goal döngüsü yerine akış karar verir.

`flow_checkpoint` aracı: deterministik el sıkışma — model aşama bitiminde çağırır; araç yalnızca `CheckpointCell`'e (Arc<Mutex> köprü) yazar, doğrulama session tarafında `on_tool_success` kancasında yapılır (2-aşamalı: pending → tekrar çağır → accept/reject+direktif).

### 5.3 Akış şablonları
- **universal** (12 aşama): analyze → research → digest → stack_select → stack_verify (çürütme döngüsü) → **duration (SİSTEM)** → plan → decompose → **parallel_query (SİSTEM)** → execute (TÜM ARAÇLAR + task, mvp'de ≤2 alt ajan) → verify (read/bash-ro/task + judge) → **notify (SİSTEM)**.
- **commit** (4 aşama): status (git salt-okunur) → stage (git add/rm --cached) → commit (git commit -m) → verify. Yasak: `--force/--hard/reset/rebase/push/cherry-pick/merge/clean/reflog delete/gc/filter-branch` (deterministik kelime matcher; CompiledPolicy ikinci savunma).
- **direct** (1 aşama): `do` (TÜM ARAÇLAR) → done; yalnızca stop gate denetimi.
- Sınıflandırıcı (AI yok): commit anahtar kelimeleri → Commit; kısa (<120) + araştırma/yazma sinyali yok → Direct; aksi halde Universal. Eşikler `config/flow/rules.toml` (gömülü varsayılan).
- `FlowDurationSystem` (AI değil): commit/direct → Mvp; universal + Autonomous mod → Full; aksi halde uzunluk ≤ `mvp_max_len` (800) → Mvp.
- `parallel_query`: `building_blocks` JSONL'indeki dosya çakışması + `depends` analizi → sistem bağımlılık grafiği (ortak dosya paylaşmayan = paralel).
- Meta grubu (ask_user_question, todo_write, update_goal, use_tool, skill) **her aşamada açıktır**.

### 5.4 Tam teslim kapsamı (hepsi uygulandı)
- **Judge (verify):** `flow/judge.rs` — judge persona'lı alt ajan system-triggered (üreten ≠ doğrulayan), `finalize_verify`; redler bütçeli, spawn hatası fail-soft.
- **Çoklu ajan execute (Full):** `flow/parallel.rs` — sistem her bloğu `executor` alt ajanı olarak görevlendirir (grup içi paralel, gruplar sıralı); `reject_execute` bütçeli.
- **Notify:** `flow/notify.rs` — telegram (Bot API), webhook, SMS (NetGSM uyumlu), çağrı (generic HTTP); `config/flow/notify.toml`; fail-soft.
- **TUI paneli:** `views/flow_detail.rs` — `F` ile Flow Governor paneli (`flow_events.jsonl`, mtime önbellekli).
- **Runtime config:** `flow/config.rs` — `$OMNITRIX_CONFIG_DIR` → `<cwd>/config/flow` → gömülü; mtime canlı reload.
- **Yerleşik araç zorunluluğu:** `cat/less/more/head/tail/grep/sed/awk/echo >/>>/rm` bash ile yasak (TÜM akışlarda).
- Kapsam dışı (bilinçli): kullanıcı "sign-off" kapısı verify'da (şu an yargıç + otomatik; kullanıcı onayı TUI/transkriptte görsel), AutonomousLoop'un diriltilmesi (yerine sistem çoklu ajan).
- Kayıt: `flow_events.jsonl` (7/24 denetim) + `events.jsonl` `flow.*` etiketleri + ACP `SessionNotification`.

---

## 6. Omnitrix Platform v2 (2026-08-09) — 8 sütun

### 6.1 Sütunlar
1. **Auto-Connect** — key paste → PrefixProbe (offline) → ranked adaylar → LiveProbe (region paralel, 2.5s) → Winner → model adımına atla. Manuel akış değişmez. CLI: `grok connect --auto --api-key [--api-key-file -]`. Yeni adımlar: `AutoKey | AutoDetecting | AutoAmbiguous`.
2. **Routing Modes Catalog** — ≥40 mod, 7 aile, `RoutingModeDef` kaydı; tek `RouterEngine::pick`; `/routing` fuzzy picker (family filter + blurb) + `grok routing *` (bkz. Bölüm 2).
3. **Credential Feeder** — verifier.db → `usable_credentials` → detect+probe → live/dead-hold kategorileri (bkz. 3.4).
4. **Loop Engineering** — Flow Governor genişletmesi (ikinci AI orchestrator YOK). İki döngü: `user_focused` (problem+kısıtlar, surface/deep, sınırlı fan-out, düşük bütçe) | `full_autonomous` (sadece problem, deep→ocean, yüksek, scheduler tavanı). Tanım: `config/flow/<name>.toml` + opsiyonel `.md`. CLI `grok loop new|run|list|validate`; TUI `/loop` — form→TOML **deterministik şablon**, AI metni yalnızca form alanlarını doldurur, aşama grafiğini çizmez. `AutonomousLoop` (`agent/autonomous.rs`) Governor'ın Execute aşamasına backend olarak bağlanır, kendi aşama seçmez.
5. **Truth layer (anti-tembellik/anti-yaln)** — (1) persona tool allowlist; (2) bash deny taxonomy: `cat|head|tail|grep|rg|sed|awk|perl -i|python -c|tee|<<EOF` → red + "native tool X kullan" mesajı; (3) escape hatch yalnızca `ToolHealth` %99 fail + `EmergencyTools` config + audit; (4) grounding required default; (5) judge falsification (Execute→Verify); (6) claim scanner (kanıtsız iddia → retry); (7) **no self-approval** — `UserSignOff` makine kimliği reddi.
6. **Persona Menagerie** — Ben10 + özel isimli 61 subagent tipi; her biri `config/personas/<slug>.toml` + `prompts/<slug>.md` (v1 stub: description+role+tools+temperature+routing). Yükleyici: mevcut persona file-watch (`_schema.md`). Rol eşleme sezgileri: juryrigg→executor, baykus→judge, firtina-beyin→planner, xlr8→executor… İçerik (soul/tools/budget/test) kullanıcı tarafından doldurulacak (checklist).
7. **Computer Use (Codex-class)** — Linux community CU portu; mevcut `ComputerBackend` + `EnvComputerBackend` (CLI) üzerine `CodexCuBackend`; izin modeli: explicit user grant + session scope; kayıt: screenshot + action log → session events; Execute aşaması tool grubuna `Computer` ekli.
8. **Native Tool Superiority** — `codebase_learn` (query/overview/symbol/deps, xai-codebase-graph), hashline_read/edit/grep güçlendirme, `structured_search` (path+symbol+callgraph), `apply_patch` (unified diff only), `research` (surface/deep/ocean), `flow_checkpoint`, `computer`. **Bash tool default KAPALI** executor dışı personlarda.

### 6.2 Zorunlu görev akışı (her ajan, her döngü)
ProblemText → Analyze (kelime kelime + building blocks → `ProblemList` artifact) → Research (web MCP'ler → `FindingsArchive` jsonl+index) → Digest (chunked → `DigestNote`) → StackSelect (AI öneri) + StackVerify (web + çürütme) → **DurationSystem.decide (AI DEĞİL)** → Plan → `PlanDoc` → Decompose → `BuildingBlocks` → ParallelQuery (sistem) → `ExecutionGraph` → Execute (multiajan/routing mode) → Verify (auto + judge + opsiyonel kullanıcı) → Notify (telegram/sms). İzleme: SSE/WS + replay; yedek: 3-2-1.

### 6.3 Tasarım ilkeleri (D1–D10)
D1 determinizm süreçte (gate/artifact/checkpoint/allowlist, sıcaklık değil) · D2 AI inisiyatif yok · D3 I5 · D4 I6 · D5 I7 · D6 I8 · D7 tool surface = politika (bash parse + deny list) · D8 yüzler ince (TUI/CLI aynı kontrol komutları) · D9 subagent-driven (her task taze context + sequential-thinking) · D10 araştırma zorunlu task'larda (Context7 + web MCP + paper-search).

### 6.4 Fazlama (P0–P8)
P0 Auto-Connect (tamam) → P1 Routing katalog + TUI/CLI (tamam) → P2 Credential feeder → P3 Loop engineering → P4 Truth layer → P5 Persona stubs → P6 Computer Use → P7 Native tools → P8 E2E dikey dilim + kayıt/notify. Bir fazın review gate'i kırmızıysa sonrakine geçilmez. Bağımlılıklar: P0.3→P2.1; P3.3 P1.3 sonrası; P4 P0 sonrası; P7 P4 sonrası.

### 6.5 Bilinçli ertelemeler (sıralı, YAGNI değil)
verifier.db özel sütun şeması · bulut sync sağlayıcı seçimi (S3 uyumlu generic) · SMS sağlayıcı sözleşmesi · persona full personality yazımı · Windows CU.

### 6.6 Riskler ve azaltmalar
Prefix çakışması (`sk-`) → LiveProbe zorunlu + ambiguous UI · feeder kötü key flood → rate limit + dead-hold + kategori izolasyonu · otonom maliyet → user_focused default + budget envelope · context patlaması → subagent-driven + task brief dosyaları · cargo OOM → paket bazlı test / static-only.

### 6.7 Controller ajan sözleşmesi (prompt)
Subagent-driven geliştirme: her task öncesi sequential-thinking (≥3), task brief + report path, implementer→reviewer→fix döngüsü, progress ledger `.superpowers/sdd/platform-v2-progress.md`, faz sırası sapmaz, kullanıcıya soru sorulmaz (belirsizlikte spec varsayılanı + ledger notu), son mesaj `PLATFORM_V2 DONE` + commit aralığı + kriter checklist.

---

## 7. Harness Tamamlanması (2026-08-06)

**Amaç:** MASTER-PLAN Faz 0–10 kapılarının tamamını kapatmak; omnitrix, xai-grok-pager TUI'si üzerine kurulu tam otonom harness olsun (çekirdek CANLI; `omnitrix run` dolambaçlı CLI'sı yok).

Üç katman tek süreçte: pager TUI = yüz · shell MvpAgent = ajan runtime (ACP) · omni-* çekirdek = harness. Entegrasyon noktaları: `app::run` (warm-up), `builtin_commands()` (slash), `event_loop.rs tokio::select!` (omni olay kanalı), `agent_rebuild.rs AgentBuilder` (tool/FS/provider), `minimal/hook.rs` (fn-pointer IoC).

Faz özeti:
- **F0** rename + yeşil workspace (kapı: version + check + clippy + I2).
- **F1** dikey dilim: `bootstrap::warm_up` TUI'ye bağlanır (lazy-init, `OMNITRIX_TRACE_STARTUP=1`), `/omni` + `/omni-tasks` slash (omni_bridge: `OnceLock<Arc<dyn OmniSnapshotProvider>>`), TUI olayları → omni-storage (`OmniEventSink`), Ctrl+C < 50ms crash-only, AgentBuilder'a `DiffShimFs` (CAS diff akışı) — **bağımlılık yönü kırılır**: shell'e `omni-tools` workspace dep eklenir (kullanıcı onayı).
- **F2** ortak durum + iki yüz: omni-control API 127.0.0.1:9876 canlı (auth token), webui router montajı, `ui_parity` kapısı.
- **F3** çok-ajan scheduler: `/omni-dashboard` (tier tablosu + interrupt), `multiagent_fanout` + `interrupt_granularity` kapıları.
- **F4** router + JEP + grounding: `/omni-routing`, `/omni-model` (rol→model, AS7), `provider_fallback` + `grounding_redteam` kapıları.
- **F5** persona + tool broker: `ToolValidator` seam (yasak tool %100 red + log), persona hot-reload (xai-fsnotify), `tool_allowlist_redteam`.
- **F6** uzak erişim + bildirim: `NotifyDispatcher` warm_up'a (Telegram/Twilio, escalation), `/omni-notify test`.
- **F7** araştırma: duplike `ResearchMode` kaldır, `omni_research` kullan, `/omni-research <surface|deep|ocean>`.
- **F8** anahtar besleme: `ModelIngestor` + `FeedSource` + `HealthProbe` canlılık → canlı/ölü, `/omni-keys status`.
- **F9** kayıt + yedek: omni-backup derlenir hale (lib.rs eksik modüller: checksum/crypto/sigv4/snapshot/target), S3BackupTarget 3-2-1 SigV4+şifreli, `/omni-backup now`; omni-record replay (`/omni-record <id> replay`).
- **F10** computer-use (xai-computer-hub-* Wayland+X11) + tam-otonom döngü: `/omni-autonomous <problem>` — oku→parçala→kaydet→araştır→depola→stack seç→süre kararı→plan→yapı taşları→paralel/ardışık→multiajan→doğrula→bitir veya dön; sonlanma oracle'ı (`omni-core oracle.rs`).
- **Kesişen kapı (Soak 7/24):** 8 kapı testi tek koşuda yeşil, tüm `/omni-*` komutları çalışır, WebUI açık, Ctrl+C < 50ms, RSS < 250MB (omni-bench), clippy temiz.

---

## 8. Persona Menagerie (checklist)

61 persona (Ben10 + özel): slug, Türkçe isim, rol tahmini (executor ağırlıklı; judge: baykus/elmas-kafa/toepick; planner: kartal/gri-madde/firtina-beyin/gravattack/clockwork/coban-yildizi/charmcaster; watcher: chamalien/golge-hayalet/eye-guy/whampire/blake-blossom/nymphomaniac; explorer: kashif/gezgin/jetray/pul-kanat/yaban-kopek/astrodactyl/amfibian/orumcek-maymun; quick_fix: xlr8/fastrack/crashhopper; enforcer: blitzwolfer/buyuk-korku/rath/bullfrag/rambo). Her satır için kutular: **S**=soul dolu · **T**=tools finalize · **R**=routing · **B**=budget · **X**=test.

Global eksikler: gerçek soul prompt'ları · codebase_learn/computer/research allowlist hizası · multiajan spawn isim→slug eşleme dokümantasyonu · TUI persona picker rozetleri/ikonlar · budget defaults (user_focused vs autonomous) · redteam: persona tool kaçışı yok.

---

## 9. Test kapıları ve doğrulama kriterleri

### 9.1 Kapı testleri (`tests/*.rs`, omni-tests)
| Test | Doğruladığı |
|---|---|
| `crash_recovery.rs` | Veri kaybı yok: niyet replay, crash-only kapanış |
| `diff_visibility.rs` | FS-shim: öncesi/sonrası CAS, +/- hunk olay akışında; dizin dışı dokunuşlar dahil |
| `grounding_redteam.rs` | Kanıt zorunluluğu, üreten ≠ doğrulayan, yanlışlamacı yargıç |
| `interrupt_granularity.rs` | Interrupt kesinliği (kaynak valisi) |
| `multiagent_fanout.rs` | Fan-out tavanları, bütçe kalıtımı |
| `provider_fallback.rs` | Fallback zinciri (round/fallback/JEP) |
| `tool_allowlist_redteam.rs` | Yasak araç %100 red + log, sızma 0 |
| `ui_parity.rs` | TUI komutu → WebUI akışında aynı StateEvent, snapshot bit-eş |

Ayrıca: `omni-bench` (cold-start <100ms ilk frame, shutdown <50ms, RSS <250MB), `bin/ci-gates.sh` (I1–I8), I8 no-telemetry gate, clippy `-D warnings`.

### 9.2 Platform v2 başarı kriterleri (spec §8 / plan P8.2)
1. `grok connect --auto --api-key …` provider sormadan model listesine iner (veya tek model apply).
2. `/routing` ≥40 mod listeler; `fallback` ve `jep-classic` E2E çalışır.
3. Tüm key dead iken feeder live key ekler veya dead-hold'a yazar.
4. `grok loop run full_autonomous --problem "..."` governor aşamalarını atlayamaz.
5. `cat`/`grep` bash denemesi allowlist ihlaliyle reddedilir.
6. 60+ persona diskten yüklenir (file watch).
7. Computer screenshot+click Linux'ta en az bir backend ile geçer.
8. `codebase_learn` tool registry'de ve bir persona allowlist'inde.

### 9.3 Doğrulama kısıtları
- **Provider-connect + flow-governor planları:** Rust compiler ÇALIŞTIRILAMAZ (cargo/rustc/clippy YASAK). Testler yazılır ama çalıştırılmaz; doğrulama statik inceleme (grep, brace-denge python scripti, imza eşleşmesi). Kullanıcı derlemeyi kendisi yapar.
- **Platform v2 planı:** derleme mümkünse `cargo test -p <crate> --lib`; OOM olursa test yaz + static kanıt + ledger notu.
- **Harness + no-telemetry planları:** derleyici serbest (`cargo check/test` normal akış).

### 9.4 Subagent rapor sözleşmesi (implementer)
`STATUS: DONE | DONE_WITH_CONCERNS | NEEDS_CONTEXT | BLOCKED` + commits + tests özeti + files touched + concerns; report dosyasına yazılır, chat'e yalnızca özet; production path'te unwrap/expect/panic yok (I6); her task sonunda conventional commit; secrets commit edilmez.
