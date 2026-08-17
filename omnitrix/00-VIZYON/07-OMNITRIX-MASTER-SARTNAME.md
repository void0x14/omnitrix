# 07 — OMNITRIX MASTER ŞARTNAME (NİHAİ İNŞA ŞARTNAMESİ)

> Tarih: 2026-08-14 · Girdiler: 05-SENTEZ (15 alan kararı) · 06-MODUL-ESLEME (modül haritası) · 08-ALGORITMA-EXTRACT (mekanizmalar) · 09-MANEVRA-TEKNOLOJILERI (SumTree/diff/walker) · 10-BENCHMARK-VE-AKADEMI (Terminal-Bench + Complexity Trap/Root Theorem/Governance Decay/GEPA) · 11-ENVANTER ×6 (claude-code, opencode, codex, crush+aider, grok-cli, pi+omp+hermes+dsh) · claude-code/SIZINTI-DOCS-EKSTRAKT (KAIROS, killswitch).
> Statü: İNŞA EKİBİNİN TEK KAYNAĞI. Bu belgedeki her rakam bir eşiktir; her eşik bir kapıdır; her kapı CI'da bloklayıcıdır (Bölüm 10). Belgeyle çelişen kod = hata, inisiyatif değil.

---

## 1. KİMLİK + MUTLAK KURALLAR

**Kimlik:** Omnitrix = tek kullanıcı için, sonsuz ölçek hedefli, tek binary'den yönetilen otonom kodlama ajanı. 12 harness'in envanterinden "çal ve buda" (Bölüm 9 lobotomi kanıtı), kullanıcının kendi grok-build fork'u (xai-* crates) üzerine.

Aşağıdaki 10 kural **mutlak eşiklerdir** — ölçülebilir, ihlali faz kapısını geçemez:

| # | Kural | Ölçülebilir eşik | Kaynak |
|---|-------|------------------|--------|
| K1 | **Soğuk başlatma 1s bandı; sıcak cache/daemon YASAK** | cold < **400ms** / sıcak < **100ms** (omni-bench, faz bütçesi Bölüm 8); hızın kaynağı yerel indeksler + lazy-init, arka plan işleri değil | 05:13, 06:6, 08 |
| K2 | **İş kutsal** — çalışan görev asla düşürülmez | 250MB darboğazında swap/kuyruklama serbest; "bitmiş işe kaynak yakmak yasak" (K11); derinlik ≤5, fan-out ≤8 | 05:15/12, 06:2 |
| K3 | **Sandbox YOK** — izolasyon = geçici git branch | codex sandbox backend (seatbelt/landlock/bwrap/MITM CA) çalışma modeline GİRMEZ; yalnız permission-profile kavramı alınır; xai-grok-sandbox CI hattı olarak dondurulur | 05:6, 06:3 |
| K4 | **Determinizm akışta (D2)** — akışa AI karar vermez | Flow Governor durum makineleri + classifier + judge karar verir; AI yalnız sonuç üretir; şablon/zarf/kapı sisteme aittir | 05:2, 06:5 |
| K5 | **Sıfır telemetri / sıfır egress** | tek anahtar, varsayılan kapalı, opt-in; no-telemetry = **kod silme** (patch değil); envelope verisi bile yok; proxy-bypass/NoKeyLog tarzı denetim engeli bulunamaz | 05:11, 06:5 |
| K6 | **Ultra bellek verimliliği + 10 bin eşzamanlı ajan** | RSS < **250MB** (omni-bench); Existing≈0 RAM (DB satırı) → Sleeping≈200B → Queued → Active≈200KB; N aktif donanıma göre, kod tavanlamaz | 05:12, 06:6 |
| K7 | **Trilyon satır kod ölçeği** | kalıcı sembol grafiği YOK (kullanıcı reddi); soru-güdümlü kanıt + hashline anchor atıf; scan'ler bounded (derinlik/entry limitleri, codex disiplini); repo map hash'i asla dışarı çıkmaz | 06:6, 05:4 |
| K8 | **Anahtarlar RAM'de + zeroize + fail-closed** | keychain AES-256-GCM + Argon2id (64 MiB) + zeroize + 15dk RAM TTL + OS keyring; anahtarlar asla diskte değil, config'e `api_key` yazılmaz; üçüncü-taraf sızmış-anahtar hattı kurulmaz (K13) | 05:8, 06:5 |
| K9 | **Bildirim disiplini** | tüm kanallar `enabled=false` varsayılan; SMS/çağrı yalnız yüksek-önem eşiğinde; kanal hatası akışı düşürmez (fail-soft) | 06 |
| K10 | **Dil politikası** | Rust çekirdek (xai-* imza düzeyinde tüketim, tokio, jemalloc, tek binary) + Zig bağımsız native'ler (walker/scan/bench — sıfır runtime, C ABI); Go/Python runtime'a GİRMEZ (yalnız araç üretimi) | 05:1, 06:1 |

---

## 2. DÖNGÜ MÜHENDİSLİĞİ

Ana döngü = **8 faz, sıralı, her faz deterministik**. Girdi/çıktı/guard aşağıdadır; faz geçişleri yalnız guard sonucuyla. AI inisiyatifi yalnız P3'ün içindedir (K4).

### 2.1 Fazlar

| Faz | Ad | Girdi | Çıktı | Guard |
|-----|----|-------|-------|-------|
| P0 | **INGEST** | prompt kuyruğu | birleşik turn | **Fold** (`drainQueueForStep`: meşgulken gelen promptlar tek step'e birleşir — crush); deterministik iptal `cancelMark`/`acceptSeq`; imza-bazlı loop detection: SHA-256 imza aynı prompt 5×/10 step → zorla durdur |
| P1 | **CONTEXT** | mesajlar + snapshot | model-input (system + tool defs + messages) | **O(1) compaction guard** (Bölüm 4 L1) — her assistant mesajından SONRA, API-raporlu usage esas; yeniden enjeksiyon listesi (Bölüm 4.6) |
| P2 | **GATE** | model-input + tool kataloğu | filtrelenmiş tool listesi + permission map | deny→gizleme (model yasak tool'u GÖRMEZ); precompiled Map (derleme zamanı, regex değil); NFKC/confusable normalizasyon; doom-loop sayaçları (Bölüm 5) |
| P3 | **STEP** | prompt + tool defs | assistant mesajı (text/thinking/tool_use) | token budget (usable ≥ 0); streaming watchdog (stall); 401 attribution; max_steps |
| P4 | **EXEC** | tool_use blokları (sıralı) | tool_result'lar | **tek executor** (SDK yalnız transport — opencode providerExecuted hatası alınmaz); crash-only niyet (Bölüm 2.4); timeout kill; heartbeat 128 kayıtta bir (pi-walker); tool çıktısı cap 30KB |
| P5 | **VERIFY** | sonuçlar | doğrulama kararı | grounding kapısı: kanıtsız iddia %100 red (üreten ≠ doğrulayan); ihlal → interrupt + trust düşüşü → karantina; yanlış-iddia karşıtı katman (Capybara v8 dersi: %29-30 FC) |
| P6 | **PERSIST** | turn kaydı | WAL commit + trajectory append | tek yazıcı aktör; önce niyet kaydı, kapanış O(1), açılışta replay (I7); event sourcing YOK (Bölüm 6) |
| P7 | **RENDER/NOTIFY** | event stream | TUI diff + notify | 16ms throttle (input öncelikli, throttle'dan muaftır); synchronized output; notify fail-soft (K9) |

### 2.2 Doom-loop guard (üç katman)

1. **Kural katmanı:** 3× aynı tool+input → onay iste (opencode `doom_loop` permission action).
2. **İmza katmanı:** SHA-256 prompt imzası 5×/10 step → zorla durdur (crush).
3. **Akış katmanı:** `MAX_REDIRECTS_PER_STAGE = 3` (Flow Governor); aşama aşılırsa RoundVerdict=Stop + kullanıcıya rapor.

### 2.3 Heartbeat / stall

- Streaming watchdog canlıdır (CC'nin ölü watchdog'u kopyalanmaz, mekanizma alınır): akışta N ms sessizlik → heartbeat ihlali.
- **30s stall tespiti, ≤10 retry** (cursor checkpoint disiplini); 10 retry sonrası checkpoint'e dön + kullanıcıya rapor.
- `CLAUDE_ENABLE_STREAM_WATCHDOG` dersi: watchdog kod değil, akış durum makinesinin girdisi.

### 2.4 Interrupt semantiği — crash-only niyet

- Interrupt = turn abort (codex `Interrupt` Op: arka plan süreçleri öldürmez).
- **Yarım tool = crash-only:** her tool çağrısı başlamadan ÖNCE WAL'a niyet kaydı yazılır (turn_id, tool_call_id, intent, args_hash, ts). Açılışta replay: niyet var + sonuç yok → iş yeniden başlatılır (bitmiş sonuç tekrar yazılmaz). Rollback yok; eksikler yeniden istenir (hashline patcher disiplini).
- Kapanış O(1): yalnız son satır commit; açılışta tüm replay tek geçişte.

### 2.5 Flow Governor entegrasyonu (K4 zorunlu kılımı)

- Akışlar (xai-grok-shell/session/flow — TAM teslim korunur): **universal** (12 aşama: analyze→research→digest→stack_select→stack_verify→duration→plan→decompose→parallel_query→execute→verify→notify), **commit** (4 aşama: status→stage→commit→commit_verify), **direct** (1 aşama: do).
- Her aşama: `tools[]` (gruplar), `produces[]`, `directive` (ADIM N/M). Aşama `flow_checkpoint` tool çağrısıyla kapanır.
- P3'ün model seçimi, P2'nin tool listesi, P5'in yargıç kararı akış durumundan türetilir — asla model teklifinden.
- Acil kaçış: K3 (Bölüm 5.6) — yargıç onaylı, loglu, tek kullanımlık.

---

## 3. BİRLEŞİK TOOL SETİ

Tek tool broker (I4). Aşağıdaki **30 birleşik tool** = nihai allowlist. Her satır: ad · birleşik parametre şeması · hangi harness'ten ne alındı · yetki sınıfı.

**Yetki sınıfları:** R1=okuma (ask'sız, env dosyaları ask) · W1=yazma (ask) · X1=çalıştırma (ask + K3) · N1=ağ (varsayılan kapalı, opt-in) · T1=görev/çoklu-ajan (ebeveyn bütçe zarfı) · M1=bellek (allow) · P1=plan (allow) · G1=system (allow).

### 3.1 Okuma (R1)

| Tool | Birleşik şema | Alınan | Sınıf |
|------|---------------|--------|-------|
| `hashline_read` | `file`, `scheme(chunk\|content_only)`, `hash_len(1-4)`, `chunk_size` → `N:hh\|content` | omp hashline anchor (satır imzası); grok GrokBuildHashline (kendi temeli); hermes read_file (limit/offset) | R1 |
| `read_file` | `file_path`, `offset`(1-bazlı), `limit`(≤2000), `encoding` | grok read_file; opencode read (offset 1-bazlı, limit 2000, binary-engel 50KB); CC Read; crush view (LSP warm-up) | R1 |
| `list_dir` | `path`, `max_depth`, `max_items`(≤1000), `ignore[]` | grok list_dir; crush ls; pi ls | R1 |
| `glob` | `pattern`, `path`, `include_hidden` | opencode glob (limit 100); crush glob (timeout 30s); omp pi-walker (scan cache paylaşımlı) | R1 |
| `grep` | `pattern`(regex), `path`, `glob`, `include`, `output_mode(content\|count\|files)`, `max_results` | grok grep; opencode grep; crush grep (timeout 5s); omp in-process ripgrep: **2 geçişli plan** (normal → iri 4MiB mmap), eşik 256 dosyada paralel, erken-çıkış bütçesi, heartbeat 128 | R1 |
| `lsp` | `server_id`, `operation`(14 op: diagnostics/definition/references/hover/symbols/rename/rename_file/code_actions/type_definition/implementation/status/reload/capabilities/request) | crush 8 LSP tool; omp 14 LSP op; grok lsp → **DONDUR** (kurulu LSP yoksa LSP_UNAVAILABLE) | R1 |
| `repo_map` | `query\|overview\|symbol\|deps`, `path` | aider: tree-sitter tags + personalized pagerank (chat ×50, ident ×10, `_` ×0.1, sqrt frekans) + binary-search bütçe (±%15) + mtime önbellek; grok `codebase_learn`; codex bounded keşif (derinlik/entry limitleri). **Hash asla dışarı çıkmaz** (K7) | R1 |

### 3.2 Düzenleme (W1)

| Tool | Birleşik şema | Alınan | Sınıf |
|------|---------------|--------|-------|
| `hashline_edit` | `path`, `edits[{op(replace\|append\|prepend\|delete), pos:N:hh, end, lines}]` | omp hashline: content-hash anchor (xxHash32 & 0xffff), stale-anchor reddi, seen-lines (cap 40/512), fail-closed recovery (uniform offset + komşu doğrulama + duplicate kuralı), noop reddi; **aider fallback zinciri**: birebir → whitespace → ellipsis → edit-distance + **git cherry-pick 3-way merge** (search metni sapmışsa bile uygulanır) + did-you-mean | W1 |
| `search_replace` | `file_path`, `old_string`, `new_string`, `replace_all`, `skip_read_before_edit` | grok search_replace (skip_read şartı); CC Edit; opencode edit 10-aşamalı replacer (Levenshtein eşikleri, BOM koruma, dosya lock); crush edit (whitespace varyantı) | W1 |
| `write` | `file_path`, `content` | opencode write (BOM tespiti, 5-dosya diag cap); CC Write; pi write (file-mutation-queue); omp write (xd://) | W1 |
| `apply_patch` | `patchText`(V4A: `*** Begin Patch`) | opencode apply_patch (önce doğrulama sonra uygulama); codex apply_patch (sandbox context'i ALINMAZ, parse/verify zinciri alınır) — yalnız `gpt-*` model aileleri (opencode koşulu korunur) | W1 |
| `multiedit` | `file_path`, `edits[]`(sıralı op dizisi) | crush multiedit (tek çağrıda çoklu operasyon) | W1 |

### 3.3 Çalıştırma (X1)

| Tool | Birleşik şema | Alınan | Sınıf |
|------|---------------|--------|-------|
| `run_terminal_cmd` | `command`, `is_background`, `timeout_ms`, `workdir`, `login` | grok run_terminal_cmd (PTY, background, timeout); crush bash dersleri: ~60 yasaklı komut, argüman blokerleri, 30KB çıktı cap, 60s sonra auto-background, job entegrasyonu. **Bu, bash'in TEK halefidir** (K3: bash tool'u kapalı) | X1 |
| `job_output` / `job_kill` / `job_list` | `task_id` / `shell_id`, `wait?` | crush job_*; deepseek job_* (kind-agnostic); grok terminal task_* | T1 |
| `wait_tasks` | `task_ids`, `timeout_ms` | grok wait_tasks; deepseek (job bekleme) | T1 |

### 3.4 Plan (P1)

| Tool | Birleşik şema | Alınan | Sınıf |
|------|---------------|--------|-------|
| `enter_plan_mode` / `exit_plan_mode` | `reason` / `plan`, `instructions` | grok (çift yönlü şart); CC (V2); opencode plan_exit (plan dosyası → build agent + sentetik user part) | P1 |
| `ask_user_question` | `question`, `options[2-4]{label,description,preview}`, `multiSelect`, `timeout_secs` | grok; CC (2-4 seçenek, preview) — yalnız kök thread (codex kısıtı) | P1 |
| `todo_write` | `todos[]`, `mode(merge\|replace)`, `is_parallel` | grok todo_write; opencode todowrite (tam liste değişimi); crush todos | P1 |
| `update_goal` | `message`, `completed`, `blocked_reason` | grok; deepseek create/get/update_goal (3-round blocked alt sınırı) | P1 |

### 3.5 Görev / çoklu-ajan (T1 — derinlik ≤5, fan-out ≤8, bütçe ebeveyn zarfı)

| Tool | Birleşik şema | Alınan | Sınıf |
|------|---------------|--------|-------|
| `task` | `prompt`, `description`(3-5 kelime), `subagent_type`, `task_id`(devam), `output_schema`, `schema_mode`, `isolated`(worktree), `priority`, `max_turns` | opencode task (subagent_depth, primary_tools deny); omp task (izole worktree fan-out + **schema-doğrulamalı typed sonuç**); grok task (xai-grok-agent); codex spawn (spawn grafı DB'de restore) | T1 |
| `send_message` / `interrupt_agent` / `list_agents` | `agent_id`, `message` / `agent_id` / — | deepseek subagent-control seti; grok task kontrolü | T1 |
| `workflow` | `name \| script`, `args` | grok workflow (Rhai, background); deepseek ralph (foreground round-based) — **DONDUR** | T1 |
| `scheduler_create` / `scheduler_delete` / `scheduler_list` | `interval`(≥60s, 7 gün TTL), `prompt`, `task_id`, `fire_immediately` | grok scheduler; CC CronCreate dersi (durable); deepseek schedule (after_seconds/fixed-rate) | T1 |

### 3.6 Web (N1 — varsayılan kapalı, opt-in; K9)

| Tool | Birleşik şema | Alınan | Sınıf |
|------|---------------|--------|-------|
| `web_search` | `query`, `max_results`(≤20) | grok web_search (Responses API); opencode websearch dersi: provider seçimi deterministik (session checksum); dsh (provider değiştirilebilir seam) | N1 |
| `web_fetch` | `url`, `max_chars`, `headers` | grok web_fetch: **SSRF guard + domain allowlist + cache + overflow**; CC WebFetch (preapproved URL listesi) | N1 |
| `grok_research` | `mode(surface\|deep\|ocean)`, `query` | grok research_tool (multi-source) | N1 |

### 3.7 Bellek (M1) ve MCP

| Tool | Birleşik şema | Alınan | Sınıf |
|------|---------------|--------|-------|
| `memory` | `operations[{action(add\|replace\|remove), content?, old_text?}], target(memory\|user)` | hermes: **atomik batch + §-ayraç + 2200/1375 char limit + secret redaction**; grok memory_search/get (cross-session, sqlite) | M1 |
| `retain` / `recall` | `items[{content, context?}]` / `query` | omp (kalıcı bellek yazma/arama); reflect ALINMAZ (LLM sentez gereksiz) | M1 |
| `skill` | `name` | opencode skill (SKILL.md + dizin, limit 10); grok skill; **lazy loading** (pi: kontekste yalnız açıklama, tam şema `/skill:name` ile) | M1 |
| `skill_manage` | `action(create\|edit\|patch\|delete\|archive\|restore)`, `name`, `content`, `pin?` | hermes skill_manage (frontmatter doğrulama, ≤15KB, güvenlik taraması); omp manage_skill | M1 |
| `mcp__<server>__<tool>` | (dinamik; passthrough) | xai-grok-mcp (rmcp 2.1 karantina, OAuth dedup, credential store — **artifact dondur**); opencode 5 durumlu status machine + ToolListChanged canlı refresh + alt süreç temizliği; crush generation-bazlı stale-connect koruması; **tünel YOK** — MCP trafiği kendi sunucundan geçmez | N1 |

### 3.8 System / capability seam (G1 — deepseek deseni, KULLANICININ ÖZEL TOOL'LARI İÇİN)

- **Tool slot protokolü:** her tool bir paket + `seam` bildirimidir: `tool_register { id, schema, seams: [ctx.*], verify: true }`. Seam'ler (deepseek capability-seams örnek alınır, 55 service): `ctx.llm, ctx.fs, ctx.shell, ctx.subprocess, ctx.tools, ctx.sessions, ctx.sessionQuery, ctx.skills, ctx.subagents, ctx.jobs, ctx.goals, ctx.compaction, ctx.web, ctx.workflowEngine, ctx.terminals, ctx.lsp, ctx.codeRuntime, ctx.tokenMeter, ctx.toolResultPruner, ctx.userQuestions, ctx.approval, ctx.systemPrompt, ctx.storage, ctx.credentials, ctx.settings, ctx.attachments`.
- **verify-tool-catalog gate (ZORUNLU):** her tool gerçek context'te boot edilir, JSON schema üretir; belgelenmeden registrant edilemez (deepseek disiplini). CI'da çalışır.
- **Guard plug-in'leri (zorunlu):** `guard/repeat-tool-reminder` (loop-hygiene) + `guard/timeout-policy` (tool-timeout) — her slot kaydında otomatik sarılır.
- **YASAK seam'ler:** `ctx.dynamicCordisRunner` / self-modification seam'leri (Bölüm 9 — dsh cordis_*).
- Wire/tip katmanı: xai-tool-types / xai-tool-protocol / xai-tool-runtime (artifact).
- `capability_audit` + `session_event_read/search/trace` (deepseek trajectory sorgusu) G1 sınıfındadır.

### 3.9 REDDEDİLEN TOOLLAR (tam liste + gerekçe)

| Tool (kaynak) | Ret gerekçesi |
|---------------|---------------|
| `bash` (opencode/crush/pi/dsh/hermes) | K3: keyfi kod; tek halef `run_terminal_cmd` + yargıç onaylı acil kaçış |
| `question` (opencode), `ask` (omp) | Otomasyonda kilit noktası; `ask_user_question` yeterli |
| `web_search`/`web_fetch` (opencode/crush/omp 23 sağlayıcı/dsh) | N1 varsayılan kapalı; tek implementasyon xai-grok (SSRF guard'lı) |
| `download` (crush), `fetch` (crush), `sourcegraph` (crush) | Ağ + dosya yazma / harici API, permission'sız yüzey |
| `computer` (omp), `computer_use` (hermes), `computer` (grok) | OS kontrolü — K5 yüzeyi; feature-gate dışı |
| `browser` (omp), `browser_cdp`/`browser_exec` (hermes) | Raw CDP/anti-bot riski; N1 opt-in değil |
| `eval` (omp), `run_code`/`execute_code` (dsh/hermes) | Kod runtime — tek binary vizyonuna aykırı, güvenlik yüzeyi |
| `github` (omp, 11 op) | Platform bağımlı; git2 + branch-izolasyonu zaten yeterli |
| `security_scan` (omp) | Codex Security backend bağımlılığı |
| `image_gen`/`image_edit`/`image_to_video`/`reference_to_video` (grok), `bfl_flux3_*`/`video_*` (hermes) | Medya üretimi — çekirdek vizyon dışı; feature-gate |
| `cordis_*` 7 tool + `dynamicCordisRunner` (dsh) | **Self-modification** — ajan kendi runtime'ına plugin yükleyemez; D2 ihlali |
| `deploy_app` stub, `fake_mcp`, `no_terminal_stub`, `non_streaming_stub`, `streaming_stub` (grok) | Stub/test-only |
| `tool_search` (opencode/CC/codex), `use_tool` (grok), `list_available_plugins_to_install`/`request_plugin_install` (codex) | Deferred exposure; plugin kurulumu kullanıcıya, tool'a değil |
| `Sleep` (CC KAIROS) | Otonom pacing — Flow Governor aşamaları pacing'i zaten verir; kod sızıntıda da yok |
| `RemoteTrigger`/`PushNotification`/`SubscribePR`/`SendUserFile`/`Monitor`/`Snip`/`Workflow`/`TerminalCapture` (CC) | KAIROS otonom tetik seti — D2 ihlali (akışa AI karar vermez); kullanıcıya sormadan push/webhook yok |
| `TeamCreate`/`TeamDelete` (CC) | Swarm — omni-scheduler tier modeli bunun yerini alır |
| `Config` (CC, ant-only), `TestingPermissionTool` | ant-only kapı / test-only |
| `request_permissions`/`wait_for_environment` (codex) | sandbox backend gerektirir (K3) |
| `write_stdin`/`exec_command` (codex) | sandbox/PTY arka planına gömülü; run_terminal_cmd kapsar |
| `delegate_task` (hermes), `subagent`+fork (dsh) | `task` tool'u tek kapıdır (çift yol yok) |
| `kanban_*` (hermes, 14 tool), `feishu_*`/`yb_*`/`discord` (hermes) | Platform gürültüsü — tek kullanıcıya değersiz |

---

## 4. CONTEXT MOTORU (omni-compact)

Tek modül, tek eşik noktası, **capability seam** (backend değiştirilebilir — deepseek deseni). API-raporlu usage esas; lokal tahmin yalnız fallback (formül: `rough = ceil(len/4)`; json/jsonl/jsonc `/2`; görüntü sabit 2000; güvenlik payı ×4/3 — CC `tokenEstimation.ts`).

### 4.1 Katman hiyerarşisi — tam sıra ve eşikler

| Kat | Mekanizma | Eşik | Kaynak |
|-----|-----------|------|--------|
| L0 | **Pin katmanı** (kısıtlardan ve kutsal bağlamdan ayrı saklanır) | Constraint Pinning: güvenlik kısıtları compaction'a ASLA girmez; pin'li bloklar her istekte AYNI KONUMDA (cache hit için) | Governance Decay 2606.22528 |
| L1 | **O(1) guard** — her assistant mesajından SONRA tek kontrol | `usable(model) = max(0, context − reserved)`; `reserved = min(20_000, maxOutputTokens)`; `count = usage.total`; `count ≥ usable → L3`'e atla (L2 prune'ı L3 öncesi de çalışabilir, maliyet O(1) kopya) | opencode overflow.ts; **pi #6339 runaway dersi kapatılır** (eşik yalnız run sınırında DEĞİL) |
| L2 | **Prune pası** (LLM'siz, compaction'dan ucuz) | `PRUNE_MINIMUM=20_000`, `PRUNE_PROTECT=40_000`, `TOOL_OUTPUT_MAX_CHARS=2_000`, `PRUNE_PROTECTED_TOOLS=["skill"]`; son 2 turn korunur; önceki compaction'ı GEÇME; budanan çıktı serialize'de `[Old tool result content cleared]`; kazanç <20k ise diske YAZMA (noise yok) | opencode compaction.ts |
| L3 | **Microcompact — zaman-tabanlı (masking-first)** | gap ≥ **60dk** (son assistant timestamp); `keepRecent = max(1, 5)`; `COMPACTABLE_TOOLS = {read, grep, glob, web_fetch, web_search, search_replace, write, run_terminal_cmd}`; temizlenen metin: `[Old tool result content cleared]`; cache-break dedektörüne "düşüş bizim" işareti | CC microCompact.ts; **Complexity Trap 2508.21433: masking summarization'a eşit çözüm, YARI maliyet** |
| L4 | **Cached MC** (yalnız cache-destekli provider) | `trigger 180k / clear_at_least 140k / keep 40k`; `cache_edits {type:delete, cache_reference}` bloğu SON user mesajına; PINNED bloklar orijinal konumlarına; model-kısıtı + ana thread kısıtı (fork sonuçları silinemez); `skipCacheWrite` fire-and-forget ayrımı | CC claude.ts/cachedMicrocompact |
| L5 | **Full/partial compaction** (LLM'li — SON çare) | tetikleyici: L1 overflow; **eşik %89** (CC auto-compact) veya explicit; kesim: pi `findCutPoint` + opencode `select` | aşağıda |

### 4.2 L5 mekaniği (pi + opencode birleşimi)

- **Budçe:** `keepRecentTokens = 20_000` (pi DEFAULT), `reserveTokens = 16_384`; turn-ölçekli bütçe `preserve_recent = clamp(usable × 0.25, 2_000, 15_000)` (opencode) — ikisi de korunur: tail-turn bütçesi opencode, entry kesimi pi.
- **Kesim:** `findCutPoint`: sondan geriye token biriktir; bütçeyi aşan ilk mesajda geçerli kesim noktasına kay; user mesajı değilse **turn'ü böl** (split-turn) → prefix ayrı özetlenir, suffix korunur; `retainedTail` zinciri: kuyruk sonraki compaction'da da korunur (opencode `tail_start_id` işaretinden eksiksiz).
- **Özet bütçeleri:** history `maxTokens = min(floor(0.8 × reserve), model.maxTokens)`; turn-prefix ayrı `min(floor(0.5 × reserve), …)`.
- **Iteratif merge (pi):** ilk compaction `SUMMARIZATION_PROMPT`, sonrakiler `UPDATE_SUMMARIZATION_PROMPT` — önceki özet `<previous-summary>` olarak gömülür. Yapılandırılmış şablon (pi): `Goal / Constraints / Progress(Done,In-Progress,Blocked) / Key Decisions / Next Steps / Critical Context` — **exact path/function/error korunur**; sonra `formatFileOperations(readFiles, modifiedFiles)`.
- **Replay (opencode):** overflow ise compaction'dan önceki son non-compaction user mesajı replay'e alınır; media part'ları `[Attached <mime>: <name>]` metnine indirilir; auto-continue sentetik turu ("Continue if you have next steps…") yalnız açıkça etkinse.
- **Özet LLM çağrıları:** `cacheRetention="none"` + yeni session id (özetler cache'e yazamaz); retry'li.
- **Cross-session cache:** ayrı katman — önceki oturum özetleri başlangıca isteğe bağlı enjekte (CC dersi: KV cache'i öldürmemek için compaction değil, statik ek).

### 4.3 Akademik takviyeler (zorunlu)

1. **Masking-first (Complexity Trap, 2508.21433):** eski tool sonuçlarını ÖZETLEMEDEN ÖNCE maskele (L3 varsayılandır); LLM-summarization yalnız L5'te ve eşiklenmiş seçenektir. Hibrit (masking+summarization) yapılmaz — ölçüm hibritin tek başına masking'den −%7 olduğunu gösterdi. Ölçüm: omni-bench compaction grid'inde maliyet/görev yarısı (masking-only) ≥ summarization-only çözüm oranı.
2. **Constraint Pinning (Governance Decay, 2606.22528):** kısıtlar L0'da; compaction özetine kısıt girmesine İZİN YOK. Kapı testi: ConstraintRot tarzı 1323-episode benzeri kendi red-team'inde compaction sonrası ihlal oranı **%0** (benchmark: %30→%0).
3. **Root Theorem (2604.20874):** tek yönetim ilkesi = sinyal-token oranı. (a) Gating **fidelity eşiğine** bağlı, token/cache maliyetine değil; (b) append-only sistemler sonlu zamanda çöker → **homeostatik kalıcılık: accumulate → compress → rewrite → shed**; (c) compactor kendi kanalında çalıştığı için **harici doğrulama kapısı zorunlu**: L5 özetleri P5 yargıcından geçer (kayıp/kısıt-silme denetimi); (d) shed = arşiv CAS'a, konuşmadan tamamen çıkar.
4. **CWL dersi (2606.11213):** 80M token / 89 ardışık görevde degradasyon yok hedefi; ölçüm kapısına yazılır (Bölüm 10 F2).

### 4.4 Compaction sonrası yeniden enjeksiyon listesi (SABİT sıra)

1. L0 pin'li kısıtlar (Governance Decay koruması)
2. Son-okunan dosyalar (`readFiles`, mtime sıralı, en fazla 8 — hunk anchor'lı)
3. Aktif plan + `flow_checkpoint` durumu (aşama/adım işareti)
4. Hook sonuçları (PreToolUse matcher'lı, en son N)
5. Frozen memory snapshot (MEMORY.md/USER.md — Bölüm 7)
6. `modifiedFiles` izleri (hashline anchor'ları)
7. Routing/persona bağlamı (JEP rolü, model ataması)

---

## 5. YETKİ MOTORU (omni-permission)

Tek nokta tool broker (I4). Precompiled Map — tüm kurallar derleme zamanında derlenir, runtime'da regex yok.

### 5.1 Kural modeli

| Öğe | Spesifikasyon | Kaynak |
|-----|---------------|--------|
| Rule | `{permission, pattern, action: allow\|deny\|ask}` | opencode core/v1/permission.ts |
| Ruleset | değerlendirme: **`findLast` kazanır** (son kural) + wildcard; varsayılan `ask` | opencode |
| Sıra | `defaults → agent → user` (user en yüksek) | opencode + codex katman zinciri |
| Reply | `once \| always \| reject(+message)`; `reject` → aynı session'daki tüm pending istekler iptal; `always` → approved listesine yazılır, kapsanan pending otomatik onaylanır | opencode |
| ExecPolicy DSL | Starlark (Extended + f-string): `prefix_rule(pattern, decision, match, not_match, justification)`, `network_rule(host, protocol, decision)`, `host_executable(name, paths)`; **Decision sıralaması `Allow < Prompt < Forbidden`** (max birleştirme); `match`/`not_match` örnekleri parse sonunda doğrulanır (dosya:satır:sütun ile) | codex execpolicy parser.rs |
| BANNED | **88 prefix** tam liste (bash/sh/zsh/curl kombinasyonları, `sudo`, `git` tek başına, `rm`, node/python/npm/pnpm/yarn, powershell ailesi …) — execpolicy amendment önerilerinde TAM eşleşme → öneri reddi | codex exec_policy.rs:56-148 |
| Dosya | `rules/*.rules` + `default.rules` + `config/personas/*.toml` allowlist'leri; katman: env > TOML > remote > default (remote = kullanıcı yerel dosyası, satıcı değil) | codex + grok |

### 5.2 Normalizasyon (zorunlu)

- **NFKC normalizasyon:** kural eşleşmesinden önce tüm komut/pattern tokenları NFKC'ye çevrilir.
- **Confusable engelleme:** Unicode confusable eşleşmeleri (CC #29489 bypass'ı) kural motoruna girmeden reddedilir; bypass testi = kapı testi (Bölüm 10 F3).
- **Network host normalizasyonu:** trim → `://`/`/`/`?`/`#` reddi → `[v6]` çöz → tek `:port` sıyır → sondaki `.`'ler + lowercase → boşsa reddet → **`*` wildcard yasak** → whitespace reddi (codex rule.rs).

### 5.3 deny → gizleme

- Pattern `*` + deny → tool tanımı **modelin gördüğü listeden kaldırılır** (opencode `disabled()`). Model yasak tool'u denemez; prompt boşa gitmez.
- Gizleme, gösterme'den önce gelir; `capability_audit` gizlenenlerin tam listesini loglar.

### 5.4 onay → kurala dönüşme

- Onaylanan komut → `Allow` `prefix_rule` amendment'ı olarak `rules/`'e yazılır (codex `blocking_append_allow_prefix_rule`).
- **Güvence:** `prefix_rule_would_approve_all_commands` kontrolü — önerilen prefix TÜM komutları Allow yapıyorsa amendment reddedilir; BANNED listesinde TAM eşleşme → red.
- Kurala dönüşme yalnız kullanıcı onayıyla (AI onayı onay yaratmaz).

### 5.5 Doom-loop + reddetme

- 3× aynı tool+input → onay iste (doom_loop action).
- SHA-256 imza 5×/10 step → zorla durdur (Bölüm 2.2).
- Reddetme gerekçesi modele geri döner (DeniedError/RejectedError/CorrectedError ayrımı — opencode).

### 5.6 K3 bash taksonomisi + yargıç onaylı acil kaçış

| Sınıf | İçerik | Davranış |
|-------|--------|----------|
| K1 | Salt-okunur allowlist (git status/diff/log, ls, ps, ss, stat, test — crush safe read-only seti) | `run_terminal_cmd` içinde otomatik geçer |
| K2 | YASAK kalıplar: `--force`, `--hard`, `push`, `rebase`, `cherry-pick`, `reset`, `merge`, `clean`, `reflog delete`, `filter-branch`, `gc` (Flow Governor gate.ts) + BANNED 88 prefix | her durumda red |
| K3 | **Acil kaçış:** bash benzeri yetki gerektiren tek seferlik komut | yargıç onayı (P5) + kullanıcı onayı + log + 24s TTL; her kullanım trajectory'de |

Kural: bash hiçbir personada açık değildir (K3 bile kişiye açık değil, olaya açıktır). Permission modları: `default` (ask) · `acceptEdits` · `plan` (CC üçlüsü); `yolo/always-approve` yalnız tek kullanıcı + loglu (crush yolo'su sessiz değildir; opencode `continue_loop_on_deny` kapalı).

---

## 6. KALICILIK (omni-storage)

### 6.1 Depolama hiyerarşisi

| Katman | Mekanizma | Detay |
|--------|-----------|-------|
| WAL | crash-only niyet kaydı | her tool çağrısından ÖNCE `(turn_id, tool_call_id, intent, args_hash, ts)`; sonuç gelince commit satırı; kapanış O(1); açılışta tek geçiş replay (Bölüm 2.4) |
| CAS | content-addressed blob store | SHA-256 adresli redb; blob'lar ana mesaj akışından AYRI; aynı içerik tek kopya; checkpoint/resume blob'ları burada |
| SQLite | tek writer aktör (rusqlite bundled, CVE-patched) | **tek message tablosu: append-only; part'lar JSON kolonda; event sourcing YOK**; fork grafı (codex ThreadSpawnEdgeStatus deseni) aynı DB'de |
| Trajectory | append-only session log | system prompt + reasoning + tool call + context enjeksiyonu kaydı; **resume/fork/search/replay aynı stream**; sorgu tool'ları: `session_event_read/search/trace` (Bölüm 3.8) |
| hermes uyumu | atomik + fcntl lock | memory yazımları dosya kilidi + atomik (SQLite şart değil); WAL ile uyumlu |

### 6.2 Alan-alan şema

| Entity | Depo | Anahtar | Yazma senaryosu | Kurtarma |
|--------|------|---------|-----------------|----------|
| Session | SQLite `sessions` | `session_id` | P6 commit | crash → son commit'e dön |
| Thread | SQLite `threads` (codex `StoredThread` alanları: forked_from_id, parent_thread_id, git_info{sha,branch,origin_url}, approval_mode, permission_profile, memory_mode…) | `thread_id` | spawn/restore | restart'ta restore |
| Turn | SQLite `turns` (`StoredTurnStatus`: başarı/hata + duration) | `turn_id` | P6 | replay |
| Item/Part | SQLite `items` — part'lar JSON kolonda | `(turn_id, item_id, updated_at_ordinal)` | P6 tek yazıcı | append-only |
| Message | SQLite tek tablo (append-only) | `(session_id, seq)` | P6 | replay |
| Blob | CAS redb | `sha256(content)` | yazma öncesi | dedupe, eksik → yeniden iste |
| Checkpoint | CAS + `checkpoints` tablosu | `(thread_id, resume_token)` | 30s stall / interrupt / kill | `resumeAction` ≤10 retry (cursor) |
| Spawn grafı | SQLite `thread_edges` | `(parent, child, status)` | spawn anında | restart'ta restore (codex) |
| Trajectory | JSONL append (CAS'ta) | `(session_id, seq)` | P6 | replay/fork/search |
| Memory | dosya (MEMORY.md/USER.md) | `(user, memory\|user)` | anında atomik yazım (hermes) | frozen-snapshot enjeksiyon |
| Kural/onay | `rules/*.rules` + `permission.toml` | kural kimliği | onay→kural (5.4) | derleme zamanı Map |

### 6.3 Kurallar

1. Yazma sırası: **niyet → çalıştır → commit**; hiçbir yazım bu sırayı bozamaz.
2. Tek writer aktör (tokio mutex) — çift yazıcı çakışması imkânsız.
3. Event sourcing YOK; part'lar JSON kolonda (basitlik, tek tablo).
4. Büyük context parçaları ana akıştan ayrı (CAS blob referansı).
5. DB yazımları **buffer + toplu flush** (crush delta-yazma darboğazı dersi — her token'da write YOK).
6. Kapanış O(1), açılış replay O(WAL uzunluğu).

---

## 7. BELLEK & ÖĞRENME (omni-memory)

### 7.1 Çalışma modeli (frozen-snapshot)

- MEMORY.md / USER.md session başında **donmuş** system prompt'a enjekte edilir (hermes deseni → prefix cache korunur).
- Session içi yazım diske **anında** (atomik + fcntl lock) ama prompt'a dokunmaz; sonraki oturumda görünür.
- **Her oturumda LLM çağrısı YOK** (codex "her oturum konsolidasyon" maliyeti alınmaz).
- Secret redaction: bellek yazım hattında sır kalıpları (API key, token) reddedilir.
- Olay-güdümlü, küçük: önemli kararlar/todo'lar, tam metin değil.

### 7.2 Nudge

| Nudge | Eşik | Kaynak |
|-------|------|--------|
| Bellek kaydı | her **10** user turn (tool kullanımında sayaç sıfırlanır) | hermes `memory.nudge_interval` |
| Skill-creation | her **15** tool iterasyonu | hermes `skills.creation_nudge_interval` |
| Turn-sonrası arka plan review | ayrı fork DEĞİL — düşük maliyetli model, kendi tool'ları üzerinden | hermes ~30K token/event fork maliyeti ALINMAZ |

### 7.3 Curator (gece işi)

- Konsolidasyon **gece/çapraz-oturum** slotunda (omni-scheduler gece işi); lease yerine **dosya kilidi** (codex 2 fazlı pipeline fikri: rollout → konsolidasyon).
- Skill lifecycle: `active → stale → archived` (kullanım kaydı tabanlı; kullanılmış skill silinmez — curator yanlış-silme riski).
- Verifier kapılı promotion (ASG-SI dersi): bir skill/öğrenme, **replay + kontrat kontrollerinden geçmeden yükselmez**; audit-log ile yeniden üretilebilir ölçüm.

### 7.4 GEPA persona-prompt evrimi (gece işi, uzun dönem)

- Persona/skill prompt'ları (61 hedef, `config/personas/*.toml` + `prompts/*.md`) GEPA ile evrimleşir (Stanford/UCB 2507.19457; hermes atfı: ICLR 2026 Oral).
- Neden: GRPO'dan ort. +%6, en çok +%20, **35×'e kadar az rollout**, $2–10/optimizasyon.
- Koşullar: yalnız gece slotu; öneri diff'i önce verifier (P5 yargıcı), sonra `capability_audit`; insan onayı şartsız ama her öneri trajectory'de; evrim determinizmi bozamaz (K4 — evrim prompt'a dokunur, akışa değil).

---

## 8. BELLEK/RAM + SOĞUK BAŞLATMA BÜTÇESİ

### 8.1 RSS hedefleri (toplam < 250MB — omni-bench Soak kapısı)

| Modül | RSS hedefi | Not |
|-------|-----------|-----|
| omni-core (loop + session actor) | ≤ 40MB | panic=unwind daemon + per-agent `catch_unwind` (AS5) |
| omni-compact | ≤ 10MB | tek eşik noktası |
| omni-edit/search native'ler (Zig) | ≤ 15MB | walker cache 16 root × TTL 1s; DirScratch havuzu |
| omni-repomap | ≤ 20MB | mtime önbellekli; kalıcı grafik YOK (K7) |
| omni-permission | ≤ 5MB | precompiled Map |
| omni-storage | ≤ 20MB | buffer + toplu flush |
| omni-provider (keychain + models.dev) | ≤ 10MB | Argon2id 64MiB yalnız unlock sırasında, sonra zeroize |
| omni-router | ≤ 10MB | 65 mod, JEP |
| omni-scheduler | ≤ 15MB + per-agent | Active ≈ 200KB bağlam; Sleeping ≈ 200B metadata + CAS pointer; Existing ≈ 0 (DB satırı) |
| omni-mcp | ≤ 30MB | rmcp 2.1; canlı refresh |
| omni-control | ≤ 5MB | tek API, iki yüz |
| omni-tui | ≤ 40MB | hedef 10–20MB sınıfı: SumTree + FlatStorage + diff render (09); GPU/React-Ink YOK |
| omni-notify | ≤ 2MB | fail-soft |
| omni-record | ≤ 5MB | trajectory append |
| omni-memory | ≤ 5MB | frozen-snapshot = tek okuma |

Bellek disiplini: release'de overflow-checks + debug-assertions açık, thin LTO, CGU=1, jemalloc (grok-cli). Bellek sızıntısı = Soak kapısı (7/24, müdahale gerektiren hata = 0).

### 8.2 Soğuk başlatma kırılımı (toplam cold < 400ms / warm < 100ms)

| Faz | Cold bütçe | Mekanizma |
|-----|-----------|-----------|
| Binary load + mmap | ≤ 100ms | tek binary, lazy sections |
| Config + requirements (Ed25519 imzalı) + rules Map derleme | ≤ 40ms | precompiled, TOML tek parse |
| Keychain açılış (Argon2id 64MiB) | ≤ 120ms | unlock + zeroize; OS keyring önce denenir |
| Tool registry (lazy) + skill kataloğu (yalnız açıklama) | ≤ 60ms | lazy-init; crush `buildTools` per-run rebuild'i ALINMAZ |
| İlk TUI frame (sağlayıcısız) | ≤ 40ms | pager; ısınma arka planda (grok dersi) |
| İlk model çağrısı | arka planda | sağlayıcı hazır olmadan arayüz yanıtlar |
| **Toplam** | **< 400ms** | Soğuk başlatma = her komutta ölçülür |

Sıcak < 100ms (her komut): walker/scan cache (TTL 1s), models.dev 24s TTL + bayat-fallback, diff render (değişen satırlar), tool cache. Hızın kaynağı yerel indeksler + Zig native'ler + lazy-init — sıcak daemon YASAK (K1).

---

## 9. LOBOTOMİ KAYIT DEFTERİ (12 harness — "çal ve buda" kanıtı)

| # | Harness | ALINAN (çalıntı) | ATILAN (lobotomize) | NEDEN |
|---|---------|------------------|---------------------|-------|
| 1 | **Claude Code** | 5'li compaction hiyerarşisi (microcompact/cached MC/API eşikleri), permission modları, hook 27 event, tool schema'ları, AskUserQuestion, stream watchdog | 1P telemetry (Datadog çift hattı + `OTEL_LOG_TOOL_DETAILS` backdoor), remote managed settings (accept-or-die), uzaktan killswitch mimarisi, React/Ink TUI (578 useState, 200-400MB), `.claude.json` 3.1GB tek dosya, Unicode confusable bypass'ı (#29489), ant-only katman, undercover mode, KAIROS otonom tetik seti (Sleep/PushNotification/SubscribePR), Buddy gamification, repo URL hash'i gönderimi | K5 (telemetri/egress), K1 (bellek/startup), 5.2 (normalizasyon), D2 (akış otonomisi), tek kanal + tek anahtar ilkesi |
| 2 | **opencode** | Ruleset (findLast/wildcard/default-ask/always→session), deny→gizleme, doom-loop action, prune pası (20k/40k), tail-turn select + splitTurn, 7 yerleşik agent, plugin 22 hook, tool_output/compaction/snapshot anahtarları | Effect-TS her yerde + tool yürütmesinin AI SDK'ya delegasyonu (çift senkron kaynağı), v1/v2 çift compaction yığını, Bun startup, share/autoshare/autoupdate, openTelemetry+OTEL_*, enterprise.url, bash/websearch/webfetch/question tool'ları, MCP harici sunucu trafiği | K1 (startup), tek eşik noktası (4), N1/K3 (tool sınıfları), tünel YOK |
| 3 | **codex** | execpolicy DSL (Starlark + BANNED 88 + örnek doğrulama + amendment güvencesi), permission profile kavramı (sandbox mekanizması DEĞİL), ThreadStore trait'i + spawn grafı, Op/Event protokol ayrımı (27/81), request_user_input kök-thread kısıtı, bounded capability keşfi | 3 platform sandbox (seatbelt/landlock/bwrap + MITM CA + yardımcı binary), Responses API'ye gömülü model bağımlılığı (resume/compact), 3 ayrı compaction dosyası (2000+ satır, test edilemez), repo Merkle hash'inin satıcıya itilmesi, request_permissions/wait_for_environment sandbox araçları, plugin install yüzeyi | K3 (sandbox yok), K7 (hash dışarı çıkmaz), sağlayıcı bağımsızlığı |
| 4 | **crush** | Fold'lu prompt kuyruğu (drainQueueForStep), deterministik iptal (cancelMark/acceptSeq), SHA-256 loop detection, buildTools filtreleri, job_*/todos, MCP 5 durum + generation guard, bash güvenlik seti (60 yasaklı komut, 30KB cap, auto-background) | Go runtime (ikinci runtime = ikinci toolchain), yolo/skip mode, her run'da provider/tool rebuild + MCP WaitForInit zorunluluğu, web_fetch/web_search/sourcegraph/download (permission'sız ağ), tek PreToolUse hook, tema sistemi (2 tema) | K10 (dil), K3/K5 (yetki), K1 (startup), hook gamı eksik |
| 5 | **aider** | Edit fuzzy fallback zinciri + git cherry-pick 3-way merge + did-you-mean, repo map (pagerank + binary-search ±%15 + mtime cache), model→format haritası (471 satır), 2-model architect orkestrasyonu, polyglot grid kültürü, linter 3-katman | Python runtime, PostHog analytics (--analytics-posthog-*), --upgrade/--install-main-branch, /report, func ailesi (3 coder registry'de yüklü değil), 11 deprecated model flag'i, /web, 3.1GB .aider tarzı geçmiş riski | K10 (dil), K5 (telemetri), ağ yüzeyi |
| 6 | **grok-cli (kendi)** | Sampler 3-katmanlı actor + router 65 mod + 401 attribution, keychain, models.dev, Flow Governor (3 akış/17 aşama), SessionActor + goal sistemi, 88 slash komut, omni-* eklemeleri — TAM teslim korunur | xai-grok-telemetry/mixpanel/secrets/update/announcements/campaigns crates (SİL — patch değil kod silme), gcloud-storage/upload/export_github/hub_server/diag_server/daemonize, eski `sampling/` ikizi, gix hattı (3 crate), mermaid/resvg/dagre zinciri (TUI'de yoksa), pdf_oxide/computer feature'a, devbox stub + test stubs, versiyon kaosu (3 hat → 0.1.0 + git describe) | K5 (telemetri), tek versiyon hattı, en pahalı render zinciri |
| 7 | **pi** | Minimal ReAct (alt-1K system prompt), lazy skills, iteratif merge özeti + findCutPoint + split-turn + retainedTail, diff render (3 aşamalı, 16ms, synchronized output, cursor delta), 7 minimal tool, 29 extension event | `reserveTokens` sessizce etkisiz (#6339 runaway — mid-turn boşluğu), MCP yok, permission sistemi yok, 65+ tema şişkinliği, compaction eşiği yalnız run sınırında | O(1) guard zorunluluğu (4.1), yetki motoru boşluğu |
| 8 | **oh-my-pi** | Hashline (anchor hash + patch dili + stale-anchor reddi + seen-lines + fail-closed recovery), in-process natives (pi-walker cache + 2 geçişli grep + 256 eşiği), task fan-out (izole worktree + typed sonuç), 14 LSP op, 28 DAP op, hub mesajlaşması | N-API/JS katmanı (Zig native tercih edilir), browser/computer yüzeyleri, web_search 23 sağlayıcı şişkinliği, eval tool, github 11 op, security_scan, 65+ tema + 90 docs sayfası öğrenme yükü, compaction reserveTokens boşluğu (pi ile ortak) | K10 (dil), güvenlik yüzeyleri (3.9), tek sağlayıcı hattı |
| 9 | **deepseek-harness** | Capability seam mimarisi (55 service) + verify-tool-catalog gate, continuable subagent + kontrol tool seti (send_message/interrupt/list), job_* birleşik kontrol, append-only trajectory + session_event_* sorguları, guard plug-in'leri (loop-hygiene + timeout), typed schema doğrulama | `cordis_*` 7 tool + dynamicCordisRunner (self-modification — ajan kendi runtime'ına plugin yükleyemez), vm sandbox plugin yükleme, v0.1.0-rc kalitesi (uyumsuzluk garantisi), Web UI merkezli approval (terminal-first değil), 105 paket yüzeyi (omnitrix tek binary) | D2 (kendi kendini değiştirme), K10 (tek binary), terminal-first |
| 10 | **cursor-cli** | Checkpoint/resume protokolü (SHA-256 CAS + resumeAction + 30s stall + ≤10 retry), interactionQuery side-channel (PreToolUse/PostToolUse tek stream'de) | **Ghost mode yalanı** (envelope açık: hostname, OS, RPC span ağacı), saf röle (offline yok), sürüm tabanı kilidi, cloud devir | K5 (envelope verisi bile yok), yerel-first |
| 11 | **warp** | BlockList/SumTree veri modeli (O(log n) viewport, zero-height gizleme, Arc path-copying), FlatStorage (paketli buffer + 24B/satır indeksi + RLE stil, 10-100×), isTelemetryEnabled gerçekten kapatıyor + yayınlanmış telemetry tablosu (ders) | GPU wgpu (Vulkan zorunlu — Linux'ta kırılgan), `/analytics/block` (her komut+çıktı+cwd), **HTTP_PROXY bypass + NoKeyLog** (denetim engeli), opt-out restart'ta geri açılıyor, server-side routing (hangi model neyi gördü bilinmez) | K5 (denetim engeli imkânsız), K1 (GPU zinciri), BYOK istemci-tarafı routing |
| 12 | **hermes** | Frozen-snapshot memory (donmuş enjeksiyon + iç yazım prompt'a dokunmaz), nudge sayacı (10/15), skill lifecycle (active/stale/archived) + curator, atomik batch memory op (2200/1375), webhook toolset daraltması (injection savunması), GEPA/DSPy evrim hattı (ICLR 2026 Oral) | ~30K token/event arka plan review fork maliyeti (ayrı ajan yerine düşük maliyetli model + kendi tool'ları), dar 2200/1375 limitler (büyütülür ama redaction korunur), curator yanlış-silme riski (kullanılmış skill silinmez), 20+ platform tool'u (feishu/yuanbao/discord/kanban/HASS), browser_cdp/browser_exec raw CDP, terminal backend çeşitliliği (Docker/SSH/Daytona/Modal) | 7.3 (curator disiplini), tek kullanıcı (platform gürültüsü), N1 |

---

## 10. FAZ KAPILARI (I1 disiplini)

Her inşa fazı = **komut + metrik + eşik**, CI'da bloklayıcı (omni-bench, Zig). Kapı testleri (grok-cli envanterinden, kalıcı): `crash_recovery, diff_visibility, ui_parity, grounding_redteam, tool_allowlist_redteam, provider_fallback, multiagent_fanout, interrupt_granularity`.

| Faz | İçerik | Komut | Metrik | Eşik |
|-----|--------|-------|--------|------|
| F0 | Artifact dondurma: sampler + keychain + models.dev + mcp + config + router + Flow Governor | `omni-bench artifacts` | dondurulmuş dosya seti hash doğrulaması (git describe + SOURCE_REV) | 0 dosya farkı; imza kontrolü geçti |
| F1 | Çekirdek döngü (P0-P7) + tool broker | `omni-bench cold-start` · `omni-bench shutdown` | cold < 400ms · sıcak < 100ms · shutdown < 50ms · RSS < 250MB · crash_recovery | eşiklerin tamamı; 3/3 çalıştırma tekrarı |
| F2 | Context motoru (L0-L5) | `omni-bench compaction-grid` | masking-first: maliyet/görev yarısı ≥ summarization-only çözüm oranı; L1 guard 0 runaway (pi #6339 testi); ConstraintRot benzeri red-team: compaction sonrası ihlal **%0**; 80M token/89 görev degradasyonu yok | tüm alt testler |
| F3 | Yetki motoru | `omni-bench tool_allowlist_redteam` · NFKC/confusable bypass testi · deny→gizleme doğrulaması (model görünür tool listesinde gizlenen YOK) | 0 bypass · 0 ihlal | 0 hata |
| F4 | Kalıcılık (CAS+WAL+trajectory+checkpoint) | `omni-bench crash_recovery` · `omni-bench interrupt_granularity` | kill sonrası replay %100 · yarım tool yeniden başlatılır · kapanış O(1) · 30s stall + ≤10 retry | %100 recovery |
| F5 | Bellek & öğrenme (frozen-snapshot + nudge + curator + GEPA gece işi) | `omni-bench memory-golden` | nudge eşikleri (10/15) · her oturumda 0 LLM çağrısı · skill promotion verifier kapılı · GEPA önerileri trajectory'de + verifier'dan geçti | 0 kural ihlali |
| F6 | TUI (blok modeli + diff render) | `omni-bench ui_parity` · `omni-bench diff_visibility` | SumTree viewport O(log n) · frame başına yazım = değişim · 16ms throttle, input muaf · RSS ≤ 40MB | parity %100 |
| F7 | Çoklu-ajan (scheduler tier + kontrol seti + typed sonuç) | `omni-bench multiagent_fanout` | derinlik ≤5 · fan-out ≤8 · bütçe ebeveyn zarfı · çalışan görev asla düşmez (K2) · interrupt granüler | 0 görev kaybı |
| F8 | Ölçek + dayanıklılık | `omni-bench scale-100` · Soak 7/24 | 100 eşzamanlı ajan stabil · Existing→Sleeping→Queued→Active geçişleri · 10k Existing DB'de · RSS 250MB altı | Soak: müdahale gerektiren hata = **0**; provider_fallback 3/3 |

Her fazda ayrıca: `grounding_redteam` (kanıtsız iddia %100 red), `provider_fallback` (fallback walk, 401 attribution), `verify-tool-catalog` (her tool boot + JSON schema, Bölüm 3.8). Derleyici yasağı yok — **ölçüm her zaman gerçektir** (aider/benchmark kültürü).

---

*Kaynaklar: 05-SENTEZ-feature-matrix.md · 06-MODUL-ESLEME.md · 08-ALGORITMA-EXTRACT.md · 09-MANEVRA-TEKNOLOJILERI.md · 10-BENCHMARK-VE-AKADEMI.md · 11-ENVANTER-claude-code/opencode/codex/crush-aider/grok-cli/pi-omp-hermes-dsh · claude-code/SIZINTI-DOCS-EKSTRAKT.md · hermes/RAPOR.md (GEPA). Bu şartname 05 ve 06'nın karar seviyesi, 08-10'un mekanizma seviyesi ve 11'lerin kanıt seviyesinin TEK birleşim noktasıdır.*
