# Claude Code — Feature Ekstraksiyon Raporu (SIZINTI KAYNAK KODU TABANLI)

Kaynak: `/home/void0x14/Documents/claude-code-leaks/` — 2026-03-31 tarihli GERÇEK TypeScript kaynağı (package.json: `"version": "0.0.0-leaked"`, `"main": "src/entrypoints/cli.tsx"`). Bu rapor RE makalelerine değil, doğrudan dosyalara dayanır; her iddia `dosya_yolu:satır` ile kanıtlıdır.

## 0. Varyant haritası

| Dizin | Dosya | İçerik | Otorite? |
|---|---|---|---|
| `claude-code/` | 2173 | **En eksiksiz.** `src/` (tam ağaç: assistant, bootstrap, bridge, buddy, cli, commands, components, constants, context, coordinator, entrypoints, hooks, ink, keybindings, memdir, migrations, moreright, native-ts, outputStyles, plugins, query, remote, schemas, screens, server, services, shims, skills, state, tasks, tools, types, upstreamproxy, utils, vim, voice) + `prompts/` (17 md rehber) + `web/` + `mcp-server/` + `docker/` + `agent.md` + `Skill.md` | ✅ **OTORİTE** |
| `claude-code-source-code/` | 1940 | `src/` + `docs/en` (sızdıranın analiz dokümanları: telemetry, hidden features, undercover mode, killswitches, roadmap). Dosya bazında `claude-code/src` ile sadece trailing-newline farkı (md5 aynı: `query.ts` = `4b897311…`) | Aynı snapshot |
| `claude-code-base/` | 1905 | `src/` klasörü **düzleştirilmiş** (assistant/, bootstrap/, cli/, services/… kökte). İçerik birebir aynı | Aynı snapshot |
| `claw-code/` | 109 | **Python portu** ("Python port with Rust on the way") — üçüncü taraf türevi, Claude Code'un kendi kodu değil | ❌ Analiz dışı |

Sonuç: 3 büyük varyant aynı sürümün farklı paketlemeleri; `claude-code/` tüm referansların kaynağıdır.

## 1. Kimlik

- **Stack:** Bun + TypeScript, `bun:bundle` (scripts/build-bundle.ts), `feature()` build-time flag'leri (external build'lerde DCE), `MACRO.*` compile-time sabitleri, React + **özel Ink fork'u** (`src/ink/` — kendi reconciler/renderer/optimizer'ı: `frame.ts`, `optimizer.ts`, `node-cache.ts`, `render-to-screen.ts`).
- **Agent loop:** `src/query.ts` (1730 satır tek monolit) — state makinesi `src/query/transitions.ts` (20+ terminal/continue reason: `tool_use`, `blocking_limit`, `prompt_too_long`, `max_output_tokens_recovery`, `token_budget_continuation`, `aborted_streaming`…). DI tasarımı `src/query/deps.ts` (callModel/microcompact/autocompact enjekte edilebilir — test için). Streaming: `queryModelWithStreaming` (`src/services/api/claude.ts`) + `StreamingToolExecutor` (paralel tool çalıştırma, `tengu_streaming_tool_execution2` gate).
- **Gates:** GrowthBook + Statsig migrasyonu (`src/services/analytics/growthbook.ts:804` `checkStatsigFeatureGate_CACHED_MAY_BE_STALE`); 60+ compile-time feature flag (KAIROS×154, TRANSCRIPT_CLASSIFIER×107, TEAMMEM×51, VOICE_MODE×46, BASH_CLASSIFIER×45, CONTEXT_COLLAPSE×20, CACHED_MICROCOMPACT×12, REACTIVE_COMPACT×4…).
-- **Sürüm notu:** `version: "0.0.0-leaked"` — ancak iç yorumlarda BQ tarihleri ve PR numaraları var (örn. `autoCompact.ts:68-69` "BQ 2026-03-10: 1,279 sessions had 50+ consecutive failures… wasting ~250K API calls/day").

## 2. EN İYİ feature'lar (ÇAL listesi — kod referanslı)

### 2.1 Beş katmanlı compaction sistemi — DOĞRULANDI (hepsi gerçek, artı 2 yeni mekanizma)

1. **Auto-full compact** — `src/services/compact/autoCompact.ts`:
   - Eşik: `getAutoCompactThreshold()` = `effectiveContextWindow − 13_000` (`AUTOCOMPACT_BUFFER_TOKENS`, satır 62). `getEffectiveContextWindowSize()` = contextWindow − `min(maxOutput, 20_000)` (p99.99 özet çıktısı 17.387 token olduğu için, satır 28-49).
   - Uyarı bandı 20K, blocking limit = effectiveWindow − 3K (`MANUAL_COMPACT_BUFFER_TOKENS`), env override'ları: `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`, `CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE`, `DISABLE_AUTO_COMPACT`.
   - **Circuit breaker:** `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3` — irrecoverable prompt_too_long'da API'yi dövme (satır 67-70, gerçek BQ verisiyle kanıtlı).
   - **Recursion guard:** `querySource === 'session_memory' || 'compact'` fork'ları için tetiklenmez (deadlock koruması, satır 169-173).
   - **Snip koordinasyonu:** `snipTokensFreed` — history-snip sonrası stale usage düzeltmesi (satır 164-167, 225).
2. **Session-memory compact (LLM'siz, cross-session)** — `src/services/compact/sessionMemoryCompact.ts`: autoCompact'tan ÖNCE denenir (`autoCompact.ts:287-310`); `DEFAULT_SM_COMPACT_CONFIG = { minTokens: 10_000, minTextBlockMessages: 5, maxTokens: 40_000 }`; başka session'dan alınmış özet yeniden kullanılır → **LLM çağrısı tamamen atlanır**. GrowthBook: `tengu_sm_compact_config`. 
3. **Microcompact (kısmi, LLM'siz)** — `src/services/compact/microCompact.ts`:
   - `COMPACTABLE_TOOLS` allowlist'i (Read/Bash/Grep/Glob/WebFetch/WebSearch/Edit/Write — satır 50-59).
   - `estimateMessageTokens()`: 4/3 güvenlik payıyla rough tahmin (satır 182-185); görsel başına ~2000 token sabit.
   - **Time-based MC:** `timeBasedMCConfig.ts` — son asistan mesajından 60 dk geçtiyse cache zaten soğuk → eski tool sonuçlarını temizle (`tengu_slate_heron`, keepRecent: 5).
   - **CACHED_MICROCOMPACT:** cache-editing API (`cache_edits` + `cache_reference`) ile mesaj içeriğini DEĞİŞTİRMEDEN tool sonuçlarını siler → prompt cache prefix'i korunur, sıfır token maliyeti. Count-based trigger/keep GrowthBook'tan. Forked-agent/ana-thread ayrımı (satır 250-260).
4. **API-native context management** — `src/services/compact/apiMicrocompact.ts`: `clear_tool_uses_20250919` (varsayılan trigger 180K / hedef 40K) ve `clear_thinking_20251015` (1 saatten eski thinking'ler temizlenir, 1 turn korunur). Tool-clearing stratejileri sadece `USER_TYPE=ant` için (satır 85-91).
5. **Sub-agent compact** — `src/services/compact/compact.ts` (1706 satır): `compactConversation()` forked compact agent çalıştırır; `promptCacheSharingEnabled` 3P'de varsayılan **true** (satır 431-435) → alt süreç ana konuşmanın cache'ini paylaşır. `MAX_PTL_RETRIES=3`, streaming retry 2, compact sonrası re-injection: son 5 dosya × 5K token (`POST_COMPACT_MAX_FILES_TO_RESTORE`, `POST_COMPACT_MAX_TOKENS_PER_FILE`).
-- **YENİ (RE'de yoktu):** **REACTIVE_COMPACT** (API 413 prompt-too-long'a tepkisel compact, `tengu_cobalt_raccoon`) ve **CONTEXT_COLLAPSE** (90% commit / 95% blocking-spawn kademeli context yönetimi — `src/services/contextCollapse/`). İkisi de autoCompact'ı devre dışı bırakıp kendi akışını kuruyor (`autoCompact.ts:189-223`).

### 2.2 Prompt-cache mühendisliği — en olgun kısım

- `cache_control: { type: 'ephemeral', ttl: '1h' }` (`src/services/api/claude.ts:365-371`, `should1hCacheTTL`), her istekte TEK cache marker kuralı (satır 3078).
- **Static/dynamic system prompt ayrımı:** `src/constants/prompts.ts:114` `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` — statik bölüm (intro/system/doing-tasks/actions/using-your-tools/tone/output-efficiency) cache'lenir; dinamik bölümler `systemPromptSection()` registry'siyle yönetilir (`session_guidance`, `memory`, `env_info`, `language`, `output_style`, `mcp_instructions`, `scratchpad`, `frc`, `summarize_tool_results`, `token_budget`, `brief`). `DANGEROUS_uncachedSystemPromptSection` — MCP bağlantıları cache kırıp gece yarısı acil durumlar için (satır 32-35).
- **Cache-break dedektörü:** `src/services/api/promptCacheBreakDetection.ts` — system/tools/per-tool schema hash'leri, `cache-control` hash'i (scope/TTL flip'leri), cache-break diff'ini `~/tmp/cache-break-XXXX.diff`'e yazar. BQ yorumu: "77% of tool breaks per-tool schema changes" — yani AgentTool/SkillTool dinamik listeleri cache'i kıran 1 numaralı sebep.
- **MCP instructions delta:** `mcp_instructions_delta` attachment'ları — geç MCP bağlantısında per-turn recompute yerine delta (cache busting önleniyor).

### 2.3 Token budget modu (üretkenlik ayarı)

- `src/query/tokenBudget.ts`: `checkTokenBudget()` — `COMPLETION_THRESHOLD = 0.9`, diminishing-returns detektörü (3 devam + <500 token/check artışı = dur), "+500k / 2M / 1B token" hedeflerinde otomatik devam mesajları (`getBudgetContinuationMessage`). System prompt bölümü TOKEN_BUDGET gate'inde (prompts.ts:507-517).

### 2.4 Tool sistemi + permission engine

- `src/Tool.ts`: pipeline `validateInput → checkPermissions → ask/deny/allow`; `ToolUseContext`; `CanUseToolFn`.
- `src/utils/permissions/` — 20+ dosya: `permissions.ts`, `bashClassifier.ts` + `yoloClassifier.ts` (iki aşamalı Bash tehlikelilik sınıflandırması), `dangerousPatterns.ts`, `permissionRuleParser.ts` + `shellRuleMatching.ts` (glob kural eşleme), `shadowedRuleDetection.ts`, `denialTracking.ts`, `pathValidation.ts`, `bypassPermissionsKillswitch.ts`, `PermissionMode.ts` (default/plan/acceptEdits/bypassPermissions…).
- **Dosya okuma bütçeleri:** `src/tools/FileReadTool/limits.ts` — `maxSizeBytes = 256 KB` (stat öncesi), `maxTokens = 25_000` (çıktı sonrası throw; yorum: truncate yerine throw → tool error ~100 bayt vs ~25K token kazası). Env: `CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS`, GB: `tengu_amber_wren`.
- **Tool sonuç limitleri:** `src/constants/toolLimits.ts` — `DEFAULT_MAX_RESULT_SIZE_CHARS = 50_000`, `MAX_TOOL_RESULT_TOKENS = 100_000`, `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS = 200_000`, `TOOL_SUMMARY_MAX_LENGTH = 50`; `src/utils/toolResultStorage.ts` frozen/fresh seleksiyonu (bütçe üstünde en eskileri değiştir).
- **Tool listing delta:** `src/tools/AgentTool/prompt.ts:50-55` — agent listesi tool description yerine `agent_listing_delta` attachment'ına taşınıyor (schema değişimi = cache bust, bunu önlemek için).

### 2.5 Subagent / multi-agent

- `src/tools/AgentTool/` (Task tool): `runAgent.ts` (maxTurns, `max_turns_reached` attachment), `forkSubagent.ts` (context taşıma), `resumeAgent.ts`, `loadAgentsDir.ts` (kullanıcı tanımlı agent'lar), `agentMemory.ts` — **kalıcı alt-ajan belleği**: `user` (`~/.claude/agent-memory/`), `project` (`.claude/agent-memory/`), `local` (`.claude/agent-memory-local/`) scope'ları, path traversal korumalı.
- Görev yönetimi API'si: TaskCreate/Get/List/Output/Stop/Update + TeamCreate/Delete (TEAMMEM×51), `SendMessageTool` (ajan-a devam), `src/tasks/` (LocalAgentTask, LocalMainSessionTask — Ctrl+B arka plan, 's'/'a' task id prefix'leri), coordinator (`src/coordinator/`, COORDINATOR_MODE×32), Buddy (BUDDY×16), in-process teammates.
- `DEFAULT_AGENT_PROMPT` (prompts.ts:758): "Complete the task fully—don't gold-plate, but don't leave it half-done… concise report" — subagent sözleşmesi.

### 2.6 Stop hooks & otomasyon

- `src/query/stopHooks.ts` (474 satır): session-end stop hook'ları — prompt suggestion (`executePromptSuggestion`), auto-dream (`executeAutoDream`), subagent stop-hook kilit yönetimi, `tengu_pre_stop_hooks_cancelled` telemetrisi. Hooks altyapısı: `src/utils/hooks/` (execAgentHook, execPromptHook, execHttpHook, sessionHooks, hookHelpers, hooksConfigManager…).

### 2.7 Maliyet takibi

- `src/cost-tracker.ts` + `src/costHook.ts`: per-call maliyet, session cost restore (`restoreCostStateForSession`), `formatTotalCost()`; `src/services/claudeAiLimits.ts` (limit yönetimi), `src/services/mockRateLimits.ts` (test). `src/services/toolUseSummary/` (tool-use özet üretici — haiku ile özetleme).

## 3. Dezavantajlar (LOBOTOMİ listesi — kod kanıtlı)

1. **Tek monolit monolitikliği:** `src/query.ts` 1730 satır + `src/screens/REPL.tsx` **5006 satır** — tek bileşen; state makinesi 20+ geçiş reason'u ile birbirine dolanmış (reactive compact × context collapse × session memory × snip × recompaction chain etkileşimleri yorumlarda "race"/"would deadlock"/"nuking granular context" olarak itiraf ediliyor). omnitrix için: query loop'u olay kaynağı (event-sourced) küçük redüktörler yap.
2. **Experiment flag kaosu:** 60+ `feature()` gate; KAIROS tek başına 154 çağrı; `USER_TYPE === 'ant'` internal-only kod yolları kodun her yerine serpiştirilmiş (numeric_length_anchors, cached microcompact, API clear-tool stratejileri, "undercover mode"…). External kullanıcı daha az özellik + daha az cache optimizasyonu alıyor. Flag'ler hem compile-time (bun:bundle DCE) hem runtime (GrowthBook/Statsig fetch) — iki eksenli karmaşa.
3. **Telemetri yüzeyi geniş:** `src/services/analytics/datadog.ts:13` — `https://http-intake.logs.us5.datadoghq.com/api/v2/logs`, **public client token** (`DD-API-KEY` header'ı), 100'lük batch / 15s flush, event allowlist'i (`DATADOG_ALLOWED_EVENTS`). `firstPartyEventLoggingExporter.ts` (OTLP) + `metadata.ts` (envContext: model, betas, version, session, userBucket). Nüans: 3P provider kullanıcıları (Bedrock/Vertex/Foundry) ve non-production env göndermiyor (satır 164-172); MCP/model adları kardinalite için normalleştiriliyor. Yine de: her tengu_* event'i + `getUserBucket()` + per-session fingerprint = omnitrix'te tek kanal + tek anahtar + yokluk modu.
4. **Bellek sızıntı geçmişi kodda gömülü:** `query.ts:587-594` yorumu — `createDumpPromptsFetch` closure'ları her istekte ~700KB request body tutuyordu; uzun session'da **~500MB retention** → tek closure'a sabitlendi. `autoCompact.ts:100-110` — `CONTEXT_COLLAPSE` için "module-level state shared across forks destroys the MAIN thread's committed log" (fork'lar arası paylaşılan module-level mutable state). İnk tarafında node-cache/optimizer var ama `fpsMetrics` + 5006 satırlık REPL + her token'da state güncellemesi eski RE iddiasıyla uyumlu.
5. **Estimate'e dayalı context yönetimi:** `tokenCountWithEstimation` + 4/3 padding (microCompact.ts:182-185) + `snipTokensFreed` yamaları — kompakt eşiği kaba tahminle çalışıyor; 3'lü circuit breaker ve `prompt_too_long` retry'ları (MAX_PTL_RETRIES=3) bu belirsizliğin yaması. Cache-break detektörünün varlığı bile cache kırılımlarının kronik sorun olduğunun kanıtı (77% tool-schema kaynaklı).
6. **Cache TTL pazarı:** `ephemeral` 1h TTL (claude.ts:365-371) + time-based MC'nin 60 dk eşiği — saat bazlı cache kaderi; 1 saatlik boşlukta her turn tam fiyat. omnitrix: 5m TTL stratejisi veya istemci tarafı cache-referans yönetimi.
7. **Kontrol dışı büyüme:** 2173 dosyalık repo'da tek komut başına dosya sayısı şişkin (commands/ 100+), Bridge/Voice/Remote/Desktop/web/coordinator gibi 6 paralel ürün yüzeyi aynı koda gömülü — kapsam disiplini omnitrix için ders.

## 4. Sistem prompt'u mühendisliği özeti (altın madeni)

Kaynak: `src/constants/prompts.ts` (915 satır) + `systemPromptSections.ts`.

- **Statik çekirdek (cache'li):** `getSimpleIntroSection` → "You are an interactive agent… Use the instructions below and the tools available to you to assist the user" + `CYBER_RISK_INSTRUCTION` ("NEVER generate or guess URLs unless confident they help with programming"). `getSimpleSystemSection` (permission modları, `<system-reminder>` etiket uyarısı, **prompt-injection uyarısı**: "flag it directly to the user", hooks bölümü, **"The system will automatically compress prior messages… your conversation is not limited by the context window"** — kompaktı modele söz olarak satıyor). `getSimpleDoingTasksSection` (minimalizm doktrini: "Don't add features… beyond what was asked", "Default to writing no comments", "no speculative abstractions", "verify it actually works: run the test, execute the script" + ant-only **false-claims karşıtı paragraf** — "Never claim 'all tests pass' when output shows failures"). `getActionsSection` ("reversibility and blast radius", "Authorization stands for the scope specified, not beyond"). `getSimpleToneAndStyleSection` + `getOutputEfficiencySection` + ant-only `numeric_length_anchors` ("≤25 words between tool calls, ≤100 words final" — sayısal çıpa ile %1.2 çıktı token tasarrufu iddiası).
- **Dinamik bölümler (registry, cache'li/uncache'li ayrımıyla):** `session_guidance` (skill komutları), `memory` (`loadMemoryPrompt()` → CLAUDE.md + memdir), `env_info` (cwd, git repo, worktree uyarısı, platform, shell, OS, model adı/ID, knowledge cutoff, "latest model family" pazarlama, ürün yüzeyi: CLI/desktop/web/IDE), `language`, `output_style`, `mcp_instructions` (veya delta attachment), `scratchpad` (scratchpad kullanım talimatları), `frc` (`getFunctionResultClearingSection` — FRC sözleşmesi), `summarize_tool_results`, `token_budget`, `brief` (KAIROS).
- **KAIROS varyantı:** `prompts.ts:471-484` — "You are an autonomous agent. Use the available tools to do useful work." + CYBER_RISK + reminders + memory + env; proaktif ajan modu.
- **Per-tool prompt'lar:** her tool'un kendi `prompt.ts`'i var (BashTool/prompt.ts: git talimatları, background çalıştırma notu, commit/PR skill yönlendirmesi; AgentTool/prompt.ts: agent listing formatı, "provide a complete task description", concurrency notu).

## 5. omnitrix için öneri

**ÇAL (birebir):**
1. **5 katmanlı compact hiyerarşisini kopyala** — sıralama: session-memory (LLM'siz) → time-based MC (60dk/keep 5) → cached microcompact (cache_edits) → auto-full (window−13K, circuit breaker 3, recursion guard) → sub-agent compact (cache sharing). Eşikler `autoCompact.ts` + `sessionMemoryCompact.ts`'ten aynen.
2. **Prompt cache statik/dinamik bölünmesi** (`SYSTEM_PROMPT_DYNAMIC_BOUNDARY` + section registry + `DANGEROUS_uncached` sınıfı) + cache-break dedektörü (hash karşılaştırmalı, diff dosyalı).
3. **Tool sonucu bütçeleri:** 50K char/100K token/200K per-message limitleri + frozen/fresh seçimi (`toolLimits.ts`), Read için 256KB/25K token + offset/limit nudge.
4. **Token budget modu** (0.9 eşiği + diminishing-returns stop'u) ve **subagent kalıcı belleği** (scope'lu agent-memory dizinleri).
5. **Stop-hook zinciri** (session-end otomasyonu) + **kost tracker** per-call maliyet.
6. **Sistem prompt paragrafları** (özellikle "doing tasks" minimalizm doktrini, false-claims karşıtı raporlama kuralı, CYBER_RISK, kompakt güvencesi).

**YAPMA (kanıtlı):**
- 5006 satırlık REPL + 1730 satırlık query monoliti → küçük modüller, olay kaynağı (event sourcing) redüktörleri.
- 60+ feature flag + ant-only kod yolları → omnitrix herkese aynı, flag sayısı <10.
- `DD-API-KEY` public client token'lı Datadog hattı → tek telemetri kanalı + tek kapatma anahtarı + veri yokluk modu.
- Fork'lar arası paylaşılan module-level mutable state → process izolasyonu veya explicit snapshot.
- Cache'e hour-TTL'e mahkumiyet → kısa TTL'li istemci tarafı cache-referans yönetimi.
- 6 paralel ürün yüzeyi (bridge/voice/remote/web/coordinator/desktop) tek koda → önce çekirdek, yüzeyler plugin.
