# 04 — Config & Persona Envanteri

> Kaynak: `config/`, `prompts/`, `README.md`, `CONTRIBUTING.md`, `SECURITY.md`
> Kapsam: persona kataloğu, routing modları, model kataloğu, Flow Governor, sistem prompt özleri, proje vizyonu, güvenlik duruşu
> Tarih: 2026-08-14

---

## 1. Operasyonel Vizyon (Özet)

Omnitrix bir **orkestrasyon katmanıdır**: tek-ajan çalışma zamanını, LLM taşımasını ve tool sistemini vendored `xai-*` ağacı sağlar; omnitrix bunların üzerine **çok-ajan planlama, yönlendirme, persona, sağlayıcı-besleme, uzak erişim ve dayanıklılık** ekler.

Temel ilkeler:

- **Tek yönlü bağımlılık (I2):** `omni-*` → `xai-*`; `xai-*` VENDORED ve düzenlenmez (CI diff görürse kapı kırmızı).
- **Model adı kodda gömülü değil (I5):** persona → `role` → katalog (`config/models.toml` + gömülü `DEFAULT_MODELS_JSON`) çözümü.
- **Yetki tek noktada (I4):** tool broker (K3 allowlist); listeye girmeyen her çağrı reddedilir ve `capability_audit`'e yazılır.
- **Akış AI insiyatifinde DEĞİLDİR:** aşama sırası, kapılar, kanıt doğrulama, ihlal düzeltmesi sistem durum makinelerine aittir (Flow Governor).
- **Derlemesiz hot-reload:** her persona diske ayrı bir dosya; `xai-fsnotify` dizini izler, dosya değişince defter tazelenir (Faz 5).
- **Persona kimlikleri Ben 10 uzaylılarıyla eşlenir** (Wildmutt, Heatblast, Four Arms, XLR8, Brainstorm, Ghostfreak, Grey Matter, Big Chill).

---

## 2. Persona Envanteri (`config/personas/*.toml`)

Sema: `_schema.md` (MASTER-PLAN 11.1). Zorunlu alanlar `name` + `description`; `system_prompt` = dosya referansı (persona dizinine göre çözülür); model `role` üzerinden katalogdan çözülür (I5).

| Persona | Dosya | Ben 10 Karşılığı | Rol | Temperature | Bütçe (max_cost) | Routing | max_depth | Yazma Yetkisi | Ağ Yetkisi |
|---|---|---|---|---|---|---|---|---|---|
| planner | planner.toml | Ghostfreak | `planner` | 0.1 | 5.0 | `jep` | 3 | YOK | YOK |
| executor | executor.toml | Four Arms | `executor` | 0.2 | 20.0 | `jep` | 2 | **VAR** (tek kişi) | YOK |
| judge | judge.toml | Brainstorm | `judge` | 0.0 | 3.0 | `jep` | 1 | YOK | YOK |
| explorer | explorer.toml | XLR8 | `summary` | 0.2 | 2.0 | `round_robin` | 1 | YOK | VAR (read-only) |
| deep_explorer | deep_explorer.toml | Wildmutt | `planner` | 0.1 | 12.0 | `weighted` | 2 | YOK | YOK |
| enforcer | enforcer.toml | Heatblast | `judge` | 0.0 | 3.0 | `jep` | 1 | YOK | YOK |
| quick_fix | quick_fix.toml | Grey Matter | `executor` | 0.1 | 4.0 | `fallback` | 1 | VAR (dar) | YOK |
| watcher | watcher.toml | Big Chill | `web_search` | 0.2 | 2.0 | `fallback` | 1 | YOK | VAR (read-only) |
| faz5_probe | faz5_probe.toml | — (kapı kanıtı) | `executor` | 0.2 | — | `jep` | 3 | YOK | YOK |

### Yetki matrisi

- **Yazma yetkisi yalnızca `executor` ve `quick_fix`'te açık** (`write`, `hashline_edit`). Tüm diğer persona'lar salt okurdur (allowlist: `hashline_read`, `hashline_grep`).
- `shell`/`bash` **hiçbir personada açık değildir**; açılması yargıç onayına bağlıdır (Faz 5 kapsamı).
- `enforcer`'ın interrupt/karantina yetkisi bir model aracı değildir: kararlar **kontrol düzlemi** (`omni-control`) API'sinden yürür; allowlist'i salt okunur kalır.
- `faz5_probe`: derlemesiz yükleme kanıtı olarak yazılmış geçici kapı ölçüm personası (ölçüldükten sonra silinebilir).

### JEP rol dağılımı

- Rol katalogu (geçerli anahtarlar): `judge · executor · planner · summary · web_search`
- `planner` → `jep` (planlayıcı), `executor` → `jep` (uygulayıcı), `judge` → `jep` (yargıç), `explorer` → `summary` (en düşük gecikmeli bağlam), `watcher` → `web_search` (arama ağırlıklı bağlam).

---

## 3. Routing (`config/routing.toml` + `config/routing_modes.toml`)

### 3.1 Ana config (`routing.toml`)

| Alan | Değer | Anlam |
|---|---|---|
| `strategy` | `fallback` | Varsayılan mod. Canonical id veya legacy alias (`fallback→fallback-strict`, `round_robin→rr`, `weighted→wrr`, `jep→jep-classic`). Bilinmeyen değer sessiz fallback değil — köprü hata döner. |
| `grounding` | `required` | Kanıt kipi (10.3): `required` \| `preferred` \| `off`. |
| `health_db` | (yorumlu) | `provider_health` SQLite dosyası; verilmezse canlılık filtresi uygulanmaz. |
| `[jep]` | `planner/executor/judge` | Rol → rol eşlemesi; değerler ROL ADIDIR, model adı değil (I5). |
| `[circuit_breaker]` | opsiyonel | window_secs, min_samples, error_rate_threshold, open_secs, half_open_max_probes. |
| `[budget]` | opsiyonel | max_tokens, max_cost, max_latency_ms (AS4). |

Aday zincir (`[[provider]]` blokları): sıra önemlidir (fallback yukarıdan aşağı iner); her blokta `role` (tercih edilen, katalogdan çözülür) veya doğrudan `model`; `weight` yalnızca `weighted` modunda anlamlı.

### 3.2 Mod Kataloğu (`routing_modes.toml`) — 60+ mod, 7 aile

Şema: `id` (unique) · `family` · `class` (primitive/policy/composition) · `selector` (SelectorKind eşlemesi) · `title/blurb/long_help` · opsiyonel `params`.

| Aile | Sayı | Öne çıkan modlar |
|---|---|---|
| **Balance** | ~13 | `rr`, `wrr`, `random`, `least-busy`, `ewma-latency`, `latency-buffer`, `usage-based-tpm/v2`, `rate-limit-aware`, `token-bucket-fair`, `sticky-session`, `hash-prompt`, `model-group-alias` |
| **Failover** | ~12 | `fallback-strict/soft`, `backup-only-on-429`, `backup-on-5xx`, `circuit-break-cascade`, `provider-failover`, `order-level-fallback`, `context-window-fallback`, `content-policy-fallback`, `default-fallback`, `weighted-failover`, `hedge-p95` |
| **RoleSplit** | 7 | `jep-classic`, `jep-cheap-plan`, `jep-strong-judge`, `planner-only-chain`, `executor-swarm`, `role-sticky`, `prompt-role-classify` |
| **Hybrid** | 8 | `balance-then-fallback`, `jep-with-rr-executors`, `canary-10`, `shadow-mirror`, `hedge-on-latency`, `tier-cascade`, `frugal-cascade`, `conformal-cascade` |
| **Specialty** | 10 | `research-ocean-prefer`, `coding-long-context`, `json-strict-model`, `vision-capable-only`, `tool-call-reliable`, `router-llm`, `keyword-rules`, `auto-router`, `quality-diff-router`, `learned-router` |
| **Cost** | 7 | `cheapest-alive`, `budget-aware`, `quality-floor`, `deadline-aware`, `price-cap`, `cost-quality-dial`, `quota-aware` |
| **Privacy** | 8 | `local-first`, `no-train-providers`, `eu-region-only`, `in-region-strict`, `geo-profile`, `zdr-only`, `secure-compute`, `no-fallback-offshore` |

Katalog **veridir**: model adı veya fiyat literal'i içermez; değerler çalışma zamanında kullanıcı/endpoint verisinden gelir (I5).

---

## 4. Model Kataloğu (`config/models.toml` — AS7)

- Dosya **tamamen boş bırakılabilir**: boşken her rol `xai-grok-models::DEFAULT_MODELS_JSON` içindeki gömülü varsayılanlardan çalışma zamanında çözülür.
- Çözüm sırası (rol R için): 1) `[roles] R` → 2) `default` → 3) gömülü JSON'un role özel alanı (`web_search`/`session_summary`) → 4) gömülü JSON'un default alanı. Boş string `""` = "ayarlanmamış" → bir üst katmana düşer.
- Geçerli rol anahtarları (başkası hata): `judge · executor · planner · summary · web_search` — hepsi şu an boş (katalog çözümü).
- Geniş `[models."<id>"]` bloğuyla yeni model ekleme / metadata zenginleştirme (provider, context_window, api_backend).
- Okuyucu köprü: `crates/codegen/xai-grok-sampler/src/routing_config.rs` (`parse_models_config`, `parse_routing_config`, `resolve_strategy`).

---

## 5. Flow Governor (`config/flow/`)

Çalışma zamanı yükleme post-MVP; şu an şablonlar ve kurallar `xai-grok-shell/src/session/flow/` içinde **gömülü varsayılanlar** olarak yaşar. Örnekler bunların insan-okur TOML yansımasıdır.

**Çekirdek ilkeler (ASLA ihlal etme):**
1. Akış AI insiyatifinde değildir — durum makineleri + `flow_checkpoint` aracı.
2. Kilitli aşama araçları modelin tool listesinde hiç yoktur.
3. Erken bitirme reddedilir — stop gate `KeepWorking{direktif}` döner (aşama başına en fazla 3 düzeltme).
4. Kararları AI verir, sonuçları sistem yönetir (örn. duration kararı sistemindir).
5. Düzeltmeler görünmezdir; ihlaller `flow_events.jsonl`'e düşer.
6. Basit görevler ağır akışa zorlanmaz (`direct`).

**Akışlar:**

| Akış | Aşama sayısı | Aşamalar |
|---|---|---|
| `universal` | 12 | analyze → research → digest → stack_select → stack_verify → duration → plan → decompose → parallel_query → execute → verify → notify |
| `commit` | 4 | status → stage → commit → commit_verify (Bash'te `--force/--hard/reset/rebase/push/cherry-pick/merge/clean/reflog delete/filter-branch/gc` REDDEDİLİR) |
| `direct` | 1 | do |

**Araç grupları:** Read, Search, Write, Bash, Web, Research, Computer, Plan, Task + **Meta (HER AŞAMADA AÇIK)**: `ask_user_question, todo_write, update_goal, use_tool, skill, memory_search, memory_get`. `execute`/`do` aşamalarında "All".

**Sınıflandırıcı kuralları (rules.toml.example):** `commit_keywords` ("commit", "git commit"...), `research_keywords` ("araştır", "nedir", "kıyasla"...), `write_keywords` ("yaz", "ekle", "düzelt", "fix"...); eşikler: `direct_max_len=120`, `commit_max_len=400`, `mvp_max_len=800`.

**Bildirim kanalları (notify.toml.example):** Telegram bot, generic webhook, SMS (NetGSM), sesli çağrı (NetGSM). Hepsi `enabled=false` varsayılan; kanal hatası akışı asla düşürmez (fail-soft).

---

## 6. Sistem Prompt Özleri (`prompts/*.md`)

| Persona | Öz (1-2 cümle) |
|---|---|
| **deep_explorer** (Wildmutt) | Derin statik kod analizi: bağımlılık grafiği, veri akışı, trait/interface hiyerarşisi, kod kokusu ve mimari pattern tespiti. Kalite > hız; kod DEĞİŞTİRMEZ, yalnızca kanıtlı analiz raporu üretir; "bence" yasak, 50+ tool çağrısı yasak. |
| **explorer** (XLR8) | Codebase'i hızlı tarar (max 15 tool çağrısı): dizin yapısı, kilit dosyalar, pattern'ler, kısa özet. Geniş-sığ tarama; derin analiz deep_explorer'ın işi; kod kalitesi yargısı yasak. |
| **planner** (Ghostfreak) | Görevi atomic alt-adımlara böler, bağımlılık zinciri (A→B→C, A‖B paralel) çıkarır, somut dosya/satır referanslı kanıt-güdümlü JSON DAG üretir (`{"dag":[...]}`). Yalnızca plan üretir, uygulama executor'a gider; max_depth=5. |
| **executor** (Four Arms) | Planner'ın DAG'ını adım adım uygular; büyük yazım/refactor yapar. Plana sadakat, yazmadan önce hedefi oku, refactor'da tüm referansları güncelle, tek görevde 500+ satır değişiklik yasak, max 5 alt-executor. |
| **judge** (Brainstorm) | Executor çıktısını 4 eksenli rubric ile puanlar (Grounding 0-25 · Plan uyumu 0-25 · Format 0-25 · Güven 0-25); toplam <70 → REDDET. Kanıt-zorunlu grounding: her iddia tool çıktısına bağlanır; JSON geri bildirim formatı. |
| **enforcer** (Heatblast) | Capability ihlallerini denetler, 5 kademeli ceza uygular: 1-Soft uyarı → 2-Task durdurma → 3-Agent sonlandırma → 4-Güven düşürme → 5-Karantina (yalnızca insan onayı kaldırır). Sıfır tolerans, kanıt-güdümlü (audit log zorunlu), orantılı, adil; yalnızca kontrol düzlemi. |
| **quick_fix** (Grey Matter) | 1-5 satırlık mikro-fix (import hatası, tip uyuşmazlığı, kırık referans); minimal müdahale, test-korumalı; 3 çağrıda çözemediği hatayı executor'a devreder; max 10 tool çağrısı, API kontratı değiştirmek yasak. |
| **watcher** (Big Chill) | 7/24 pasif izleme: kaynakları periyodik kontrol eder, değişiklik/fırsat/tehdit tespitini düşük maliyetle raporlar. Tur başına max 5 tool çağrısı, turlar arası min 60 sn, sinyal/gürültü filtresi, asla yazma/aktif keşif. |

---

## 7. Proje Tanımı, Kapılar ve Kullanım (README.md)

### Tanım
Orkestrasyon katmanı. 4 katman: **Yüzler** (omni-webui, uzak kanallar) → **Kontrol düzlemi** (omni-control: tek API, auth zorunlu, SSE/WS) → **Çekirdek** (omni-proto/core, omni-scheduler, omni-router, omni-tools, omni-agent) → **Temel** (xai-* VENDORED) + **Dayanıklılık** (omni-storage, omni-provider). 19 `omni-*` crate.

### Kullanım (tek binary)
`omnitrix` (TUI) · `--version` · `key add` (yalnızca doğrulanan anahtar; şifreli keychain `~/.grok/keychain.omx`, AES-256-GCM + Argon2id, master password RAM'de 15 dk TTL) · `task` (WAL niyet kaydı, op_id döner) · `run` (uçtan uca) · `rss`. Provider bağlama: `/connect` TUI wizard veya `grok connect --provider/--api-key/--base-url/--model`.

### Config katmanları (AS8) — öncelik yüksekten düşüğe
1. **Env override** (`OMNITRIX_*`, yol ayracı `__`) → 2. **DB runtime** (`config_kv` tablosu, çalışırken etkili) → 3. **Dosya varsayılan** (`config/*.toml` + profil).

### Kapılar (invariantlar, hepsi CI'da bloklayıcı)
- `bash bin/ci-gates.sh` (I1/I2a/I2b/I4/I5/I7/IP + I6 raporu), `cargo test --workspace`, `cargo run -p omni-bench` (cold-start/shutdown/RSS eşikleri).
- **I1** kapı = komut+metrik+eşik · **I2** tek yönlü xai bağımlılığı · **I3** tek ortak durum kaynağı · **I4** yetki tek noktada (tool broker) · **I5** model adı/fiyatı gömülü değil · **I6** prod yolunda `unwrap/expect/panic!` = 0 · **I7** her yan etkili işlem önce WAL · **IP** `dunce::canonicalize`.
- Kapı testleri: `crash_recovery` · `diff_visibility` · `ui_parity` · `multiagent_fanout` · `provider_fallback` · `grounding_redteam` · `tool_allowlist_redteam` · `interrupt_granularity`.

### Katkı ve lisans
Apache-2.0 (birinci taraf). Dış katkı kabul edilmez (aşağıya bakınız).

---

## 8. Güvenlik Duruşu (SECURITY.md + CONTRIBUTING.md)

- **Güvenlik raporları:** HackerOne programı üzerinden (`https://hackerone.com/x`). Güvenlik açıkları için halka açık GitHub issue açılmaz.
- **Katkı politikası:** Bu depo **harici pull request veya talep dışı patch kabul etmez**; SpaceXAI yazılımı dahili geliştirir, halka açık ağaç kaynak şeffaflığı + yerel derleme içindir. CLA sunulmaz.
- **Ek güvenlik katmanları (config'den):** anahtar yalnızca şifreli keychain'de; runtime key RAM'de (15 dk TTL); `local-first`/`zdr-only`/`eu-region-only`/`no-fallback-offshore` routing modları; enforcer'ın 5 kademeli ceza/karantina sistemi; capability audit logları.

---

## 9. En Kritik Bulgular

1. **Model adı hiçbir yerde gömülü değil (I5):** tüm sistem rol→katalog çözümüne dayanır; `models.toml` boş bırakılabilir, `routing.toml` yalnızca rol adları taşır.
2. **Yazma yetkisi 2 personada yoğunlaşmış:** `executor` (bütçe 20.0) ve `quick_fix` (4.0); diğer 7 persona salt okurdur ve kabuk/ag kapalıdır — enforcer'ın ceza yetkisi bile salt model aracı değil, kontrol düzlemi API'sidir.
3. **Akış deterministiktir:** 12 aşamalı `universal` akışında aşama kapıları, kanıt doğrulama ve `KeepWorking` düzeltmeleri sisteme aittir; AI yalnızca `flow_checkpoint` ile kapatır, erken bitirme reddedilir.
4. **JEP üçgeni:** planner (0.1, yazar-ama-salt-okur) → executor (0.2, tek yazıcı) → judge (0.0, 4 eksenli rubric + kanıt zorunluluğu, <70 reddet) + enforcer (0.0, 5 kademeli ceza) üstünde 60+ routing modu (Balance/Failover/RoleSplit/Hybrid/Specialty/Cost/Privacy) çalışır.
