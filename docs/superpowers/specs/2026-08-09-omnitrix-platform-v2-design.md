# Omnitrix Platform v2 — Tasarım Spesifikasyonu

**Tarih:** 2026-08-09  
**Durum:** Onaylı (kullanıcı: soru sorma, planı yaz, ajan çalışsın)  
**Otorite hiyerarşisi:** Bu spec → `docs/ARCHITECTURE.md` → mevcut kod seam'leri. Çelişkide **mevcut seam + I1–I8 invariantları** kazanır; `xai-*` vendored ağacı fork edilmez, yalnızca tanımlı seam noktalarından bağlanır.

---

## 0. Mevcut durum (kanıtlı)

| Alan | Durum | Kanıt |
|---|---|---|
| Manuel `/connect` | Tamam | `provider_picker/`, `xai-omni-keychain`, `docs/provider-connect.md` |
| Key önek tipi | Kısmi | `detect_key_type(provider_id, key)` — provider **önceden biliniyor** |
| Routing config | İskelet | `config/routing.toml`: `round_robin\|weighted\|fallback\|jep` |
| Fallback walk | Var | `xai-grok-sampler/src/retry.rs` (`FallbackRouter`) |
| Flow Governor | Kod var | `session/flow/*` (analyze→notify aşamaları) |
| AutonomousLoop | Kod var, bağlanmamış | `agent/autonomous.rs` |
| Computer use | CLI sürücü | `computer_tool.rs` (xdotool/ydotool/grim) — Codex CU değil |
| Persona katalog | 8 referans | `config/personas/*` — Ben10 tipleri yok |
| Grounding | Var | sampler grounding + flow judge |
| SQLite key feed | Yok | hedef: `.../backshoot/data/verifier.db` |

**Kritik mimari gerçek:** `crates/omni/*` crate'leri workspace'te yok. Orkestrasyon **eritilmiş** halde `crates/codegen/xai-grok-{shell,pager,tools,sampler,...}` içinde yaşıyor. Yeni iş bu seam'lere yazılır; hayali `omni-*` crate açılmaz.

---

## 1. Vizyon özeti (8 sütun)

1. **Auto-Connect** — API key yapıştır → provider + bölge otomatik → model seçimine atla. Manuel akış kalır.
2. **Routing Modes Catalog** — 9router/omnirouter kalitesinde onlarca adlandırılmış mod; CLI + TUI; her modun açıklaması, UX'i, deterministik seçici.
3. **Credential Feeder** — Tüm key'ler ölüyse `verifier.db` → `usable_credentials` (NOT: ileride özel sütun). Canlılık kontrolü → live / dead-hold.
4. **Loop Engineering** — Deterministik döngü çekirdeği (Flow Governor genişletmesi). İki tip: `full_autonomous` | `user_focused`. AI yalnızca ilerletir; aşama seçmez.
5. **Truth / Anti-sycophancy** — Yerleşik tool zorunluluğu, bash kaçış engeli, grounding required, üreten≠doğrulayan.
6. **Persona Menagerie** — Ben10 + özel isimli subagent tipleri; her biri TOML + prompt stub + allowlist; içerik sonra doldurulur.
7. **Computer Use (Codex-class)** — Linux community CU portu + mevcut `ComputerBackend` trait; Codex parity hedefi.
8. **Native Tool Superiority** — LLM tembellik yollarını (cat/grep/sed/EOF) native tool'larla kapat; codebase-learn native.

**Zorunlu görev akışı (her ajan, her döngü tipi):**  
Problem oku → parçala → liste kaydet → web research (surface|deep|ocean) → findings depola → sindir → stack öner + çürüt → süre sistemi karar verir → plan → yapı taşları → parallel query → execute/multiajan → verify → notify (telegram/sms). Kayıt: yerel + bulut 3-2-1. İzleme: realtime veya replay.

---

## 2. Tasarım ilkeleri (değişmez)

| ID | İlke |
|---|---|
| D1 | **Determinizm süreçte:** sıcaklık değil; gate, artifact, checkpoint, allowlist |
| D2 | **AI inisiyatif yok:** Flow Governor aşama seçer; ajan sadece mevcut stage tool'larıyla ilerler |
| D3 | **I5:** model adı/fiyat kodda literal değil; rol → katalog |
| D4 | **I6:** production `unwrap`/`expect`/`panic!` yok |
| D5 | **I7:** yan etki önce WAL/niyet |
| D6 | **I8:** olgusal iddia = kanıt ref |
| D7 | **Tool surface = politika:** expose edilmeyen tool çalışmaz; bash parse + deny list |
| D8 | **Yüzler ince:** TUI/CLI aynı control komutlarını çağırır |
| D9 | **Subagent-driven uygulama:** her task taze context; sequential-thinking her task başında |
| D10 | **Araştırma zorunlu task'larda:** Context7 + web MCP + paper-search (literatür) |

---

## 3. Sütun tasarımları

### 3.1 Auto-Connect

**Akış (yeni):**
```
[Auto] key paste
  → PrefixProbe (offline regex/heuristic table)
  → CandidateProviders[] ranked
  → LiveProbe her aday (region endpoints paralel, timeout 2.5s)
  → Winner {provider_id, region, base_url, auth_scheme}
  → skip Provider+BaseUrl steps
  → Model step (mevcut model_select)
  → Apply (mevcut)
```

**Manuel akış:** mevcut `ConnectStep` sırası **değişmez**.

**Yeni adımlar:**
- `ConnectStep::AutoKey` — ilk ekran iki sekme: `Auto` | `Manual`
- `ConnectStep::AutoDetecting` — spinner + aday listesi
- `ConnectStep::AutoAmbiguous` — birden fazla eşit skor → kullanıcı seçer (nadir)

**CLI:**
```
grok connect --auto --api-key sk-...
grok connect --auto --api-key-file -
# başarılı → model listesi stdout veya --model ile noninteractive apply
```

**Dosyalar:**
- `xai-omni-keychain/src/detect.rs` — saf prefix tablosu + skor
- `xai-grok-shell/src/util/provider_probe.rs` — HTTP canlılık/region
- `xai-grok-pager/.../provider_picker/` — Auto adımları
- `docs/provider-connect.md` — Auto bölümü

**Prefix tablosu (v1 minimum):**  
`sk-ant-`→anthropic, `sk-or-`→openrouter, `sk-proj-`/`sk-`→openai(+compat candidates), `AIza`→google, `gsk_`→groq, `xai-`→xai, `sk-`+deepseek host heuristics, Azure deployment keys, Bedrock-style, Together, Fireworks, Mistral, Cohere, Perplexity, SiliconFlow, DeepInfra, Groq, Cerebras, SambaNova, NVIDIA NIM, GitHub models, Cloudflare, Together, Novita, Hyperbolic, vb. — models.dev id ile hizalı.

**Region:** provider metadata'da `regions: [{id, base_url}]`; probe latency + HTTP 200/401 ayrımı (401 = key format OK ama auth fail vs wrong host).

---

### 3.2 Routing Modes Catalog

**Amaç:** 3 değil **genişletilebilir katalog**. Her mod bir `RoutingMode` kaydı:

```rust
pub struct RoutingModeDef {
    pub id: &'static str,           // "fallback-strict"
    pub family: ModeFamily,         // Balance | Failover | RoleSplit | Hybrid | Specialty
    pub title: &'static str,
    pub blurb: &'static str,        // 1 cümle TUI
    pub long_help: &'static str,    // /routing help
    pub params_schema: ModeParams,  // weights, roles, sticky_ttl, ...
    pub selector: SelectorKind,     // enum → implementasyon
}
```

**v1 aileleri ve örnek modlar (≥40 kayıt; config + built-in):**

| Family | Örnek id'ler |
|---|---|
| Balance | `rr`, `wrr`, `least-conn`, `ewma-latency`, `token-bucket-fair`, `sticky-session`, `hash-prompt` |
| Failover | `fallback-strict`, `fallback-soft`, `backup-only-on-429`, `backup-on-5xx`, `circuit-break-cascade`, `hedge-p95` |
| RoleSplit | `jep-classic`, `jep-cheap-plan`, `jep-strong-judge`, `planner-only-chain`, `executor-swarm` |
| Hybrid | `balance-then-fallback`, `jep-with-rr-executors`, `canary-10`, `shadow-mirror` |
| Specialty | `research-ocean-prefer`, `coding-long-context`, `json-strict-model`, `vision-capable-only`, `tool-call-reliable` |
| Cost | `cheapest-alive`, `budget-aware`, `quality-floor`, `deadline-aware` |
| Privacy | `local-first`, `no-train-providers`, `eu-region-only` |

**Seçici motor:** tek `RouterEngine::pick(ctx) -> EndpointHandle`.  
Modlar **strateji plug-in**; yeni mod = tablo satırı + gerekirse küçük selector fn.  
TUI: `/routing` picker (fuzzy + family filter + blurb).  
CLI: `grok routing list|set|show|explain <id>`.

**JEP varsayılan roller (değiştirilebilir, I5):**  
judge←role judge, executor←executor, planner←planner — `config/routing.toml [jep]` + persona override.

---

### 3.3 Credential Feeder (verifier.db)

**Kaynak (NOT — geçici):**  
`/home/void0x14/Documents/ihsan-agama-verilen-destek/backshoot/data/verifier.db`  
Sütun: **`usable_credentials`** (kullanıcı: sonra özel ayar/sütun gelecek).

**Tetikleyiciler:**
1. **Reaktif:** `RouterEngine` tüm endpoint'leri `Dead|CircuitOpen` görünce
2. **Pasif:** arka plan tick (varsayılan 15 dk) — config `feeder.poll_secs`
3. **Manuel:** `grok keys feed --now`

**Boru hattı:**
```
read sqlite (read-only connection)
  → parse credential blobs
  → Auto-detect (3.1)
  → live probe
  → Live → keychain category "feeder-live" + provider pool
  → Dead → keychain category "feeder-dead-hold" (silinmez; revive poll)
  → audit event
```

**Güvenlik:** DB path config'te; default path kullanıcı verdiği; sandbox dışı path için explicit allow. Key'ler düz loglanmaz (mask).

---

### 3.4 Loop Engineering

**Çekirdek:** mevcut Flow Governor **genişletilir**, ikinci bir AI-orchesrator yazılmaz.

**İki döngü tipi:**

| Tip | Girdi | Araştırma | Fan-out | Bütçe |
|---|---|---|---|---|
| `user_focused` | problem + kısıtlar + stack tercih + kabul kriteri | surface/deep | sınırlı | düşük |
| `full_autonomous` | sadece problem + istenen sonuç | deep→ocean | yüksek (scheduler tavanı) | yüksek |

**Döngü tanımı dosyası:** `config/flow/<name>.toml` + opsiyonel `config/flow/<name>.md`  
CLI: `grok loop new|run|list|validate`  
TUI: `/loop` wizard (form → TOML üretimi **deterministik şablonla**, AI metni yalnızca form alanlarını doldurur; aşama grafiğini AI çizmez).

**Aşama grafiği (zorunlu sıra, governor):**  
`Analyze → Research → Digest → StackSelect → StackVerify → Duration → Plan → Decompose → ParallelQuery → Execute → Verify → Notify`

- `Duration` kararı **ayrı süre sistemi** (`flow/duration.rs`) — AI seçmez.
- `StackVerify` çürütme döngüsü: max N tur, artifact `StackVerified`.
- Checkpoint: mevcut `flow_checkpoint` tool.
- İhlal: tool gizleme + görünmez düzeltme (mevcut gate).

**AutonomousLoop** (`agent/autonomous.rs`): Governor'ın Execute aşamasına **backend** olarak bağlanır; kendi aşama seçmez.

---

### 3.5 Truth layer (anti-yaln / anti-tembellik)

Katmanlar:
1. **Tool allowlist** persona başına (mevcut)
2. **Bash deny taxonomy** genişletme: `cat|head|tail|grep|rg|sed|awk|perl -i|python -c|tee|<<EOF` → red + "use native tool X" mesajı
3. **Escape hatch:** yalnızca `ToolHealth` %99 fail + `EmergencyTools` config + audit
4. **Grounding required** default (`routing.toml grounding = "required"`)
5. **Judge falsification** Execute→Verify kapısı
6. **Claim scanner** (sampler grounding) — kanıtsız iddia → retry/revise
7. **No self-approval:** `UserSignOff` makine kimliği reddi (oracle zaten var)

---

### 3.6 Persona Menagerie

Her subagent = `config/personas/<slug>.toml` + `prompts/<slug>.md`  
İçerik v1: **stub** (description + role + tools + temperature + routing).  
TODO checklist ayrı dosyada (aşağı §6).

**Tip listesi (slug'lar):**  
kashif, gezgin, anadolu-parsi, baykus, bal-porsugu, kartal, kusmuk, juryrigg, chamalien, jetray, insanazor, xlr8, fastrack, golge-hayalet, gri-madde, cannonbolt, guncelleme, armodrillo, ates-topu, pul-kanat, blitzwolfer, yuzen-cene, yaban-kopek, buyuk-korku, ditto, echo-echo, vahsi-asma, dort-kol, elmas-kafa, snare-oh, eye-guy, arctiguana, shocksquatch, rath, firtina-beyin, gravattack, atomix, toepick, gutrot, astrodactyl, nrg, parlak-tas, feedback, swampfire, amfibian, waterhazard, terraspin, whampire, ball-weevil, crashhopper, bullfrag, clockwork, coban-yildizi, orumcek-maymun, goop, charmcaster, blake-blossom, jet-fadil, nymphomaniac, rambo, the-flash

**Yükleyici:** mevcut persona file-watch seması (`_schema.md`).  
Scheduler: `subagent_type` → persona name eşlemesi.

---

### 3.7 Computer Use

**Hedef:** Codex Computer-Use parity (Linux community port).  
**Mevcut:** `ComputerBackend` + `EnvComputerBackend` (CLI).  
**Plan:**
1. Araştır: OpenAI CU protocol + linux port (community)
2. `ComputerBackend` impl: `CodexCuBackend` (veya community crate)
3. İzin modeli: explicit user grant + session scope
4. Kayıt: screenshot path + action log → `omni-record`/session events
5. Flow stage `Execute` tool group'a `Computer` ekli (zaten enum'da)

---

### 3.8 Native tools

**Yeni / güçlendirilecek:**
| Tool | Tembellik rakibi | Not |
|---|---|---|
| `codebase_learn` | read tüm repo | graph/index native (codebase-memory veya xai-codebase-graph) |
| `hashline_read/edit/grep` | cat/sed/grep | mevcut güçlendir |
| `structured_search` | rg rastgele | path+symbol+callgraph |
| `apply_patch` | EOF heredoc | unified diff only |
| `research` surface/deep/ocean | tarayıcı tembellik | mevcut research_tool |
| `flow_checkpoint` | AI "bitti" demesi | mevcut |
| `computer` | manuel GUI | 3.7 |

Bash tool: default **kapalı** executor dışı personlarda; açıkken taxonomy filter.

---

## 4. Görev yaşam döngüsü (zorunlu boru hattı)

```
ProblemText
  → Analyze: kelime kelime + building blocks list → artifact ProblemList (kalıcı store)
  → Research(mode): web MCPs → FindingsArchive (jsonl+index; opsiyonel media)
  → Digest: büyükse chunked read → DigestNote
  → StackSelect (AI öneri) + StackVerify (web + çürütme döngüsü)
  → DurationSystem.decide(mvp|full)  // AI DEĞİL
  → Plan → PlanDoc
  → Decompose → BuildingBlocks
  → ParallelQuery → ExecutionGraph
  → Execute (multiajan / routing mode)
  → Verify (auto + judge + optional user)
  → Notify (telegram/sms)
```

**İzleme:** SSE/WS event stream (mevcut control yüzü deseni) + session replay dosyaları.  
**Yedek:** 3-2-1 (`omni-backup` eritme raporu / session backup).

---

## 5. Fazlama (uygulama sırası)

| Faz | Ad | Çıktı | Bağımlılık |
|---|---|---|---|
| **P0** | Auto-Connect | key→provider→model | keychain + connect |
| **P1** | Routing catalog v1 (≥40 mod) + TUI/CLI | çalışan seçici | sampler fallback |
| **P2** | Credential feeder | dead→db feed | P0 detect+probe |
| **P3** | Loop engineering UX + governor bağları | loop new/run | flow/* mevcut |
| **P4** | Truth layer bash deny + grounding defaults | redteam testleri | tools bash |
| **P5** | Persona menagerie stubs | 60+ persona dosyası | schema |
| **P6** | Computer Use Codex-class | backend + docs | computer_tool |
| **P7** | Native tools (codebase_learn vb.) | tool registry | graph |
| **P8** | E2E dikey dilim + kayıt/notify | bir full_autonomous demo | P0–P7 |

Her faz **kendi plan task'larıyla** ship edilir; bir faz kırılırsa sonrakine geçilmez.

---

## 6. Persona TODO checklist (içerik sonra)

Dosya: `docs/superpowers/checklists/2026-08-09-persona-menagerie.md`  
Her persona için: `[ ] soul.md` `[ ] tools allowlist finalize` `[ ] routing` `[ ] budget` `[ ] tests`.

---

## 7. Bilinçli ertelemeler (YAGNI değil — sıralı)

- verifier.db özel sütun şeması (kullanıcı sonra)
- Bulut sync sağlayıcı seçimi (S3 uyumlu generic bırak)
- SMS sağlayıcı sözleşmesi (notify kanal trait)
- Persona full personality yazımı (kullanıcı doldurur)
- Windows CU

---

## 8. Başarı kriterleri

1. `grok connect --auto --api-key …` provider sormadan model listesine iner (veya tek model apply).
2. `/routing` ≥40 mod listeler; `fallback` ve `jep-classic` E2E çalışır.
3. Tüm key dead iken feeder live key ekler veya dead-hold'a yazar.
4. `grok loop run full_autonomous --problem "..."` governor aşamalarını atlayamaz.
5. `cat`/`grep` bash denemesi allowlist ihlaliyle reddedilir.
6. 60+ persona diskten yüklenir (file watch).
7. Computer screenshot+click Linux'ta en az bir backend ile geçer.
8. `codebase_learn` tool registry'de ve bir persona allowlist'inde.

---

## 9. Riskler

| Risk | Azaltma |
|---|---|
| Prefix çakışması (`sk-`) | LiveProbe zorunlu; ambiguous UI |
| Feeder kötü key flood | rate limit + dead-hold + category isolation |
| Otonom maliyet | user_focused default; budget envelope |
| Context patlaması (ajan) | **subagent-driven**, task brief dosyaları |
| cargo OOM | paket bazlı test; gerekirse static-only (önceki plan geleneği) |
