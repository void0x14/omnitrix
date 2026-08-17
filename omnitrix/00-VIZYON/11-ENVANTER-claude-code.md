# 11-ENVANTER — Claude Code Sızıntı Kaynağı Envanteri

> Kaynak: `/home/void0x14/Documents/claude-code-leaks/claude-code/` (otorite varyant, 2201 dosyalık ağaç; kaynak kısmen minify — değişken adları tek harfe indirgenmiş ama schema/string'ler sağlam)
> Yöntem: kayıt noktalarının TAM enumeration'u (regex + brace-matching schema çıkarımı). Keyword örneklemesi YOK.
> Sınıflandırma: **GOOD (ÇAL)** = gerçek işlev / kullanıcı lehine · **BAD (LOBOTOMİ)** = kullanıcı ajanını kırpan, tekelleşme/ölçüm veya eksik kod · **NÖTR** = altyapı/koşullu.

## KAPSAM KANITI

| Tarama | Yöntem | Sonuç |
|---|---|---|
| Dizin ağacı | ls + find | 2201 dosya; src/ altında 40 tool dizini, 102 komut dizini/dosyası |
| Tool kayıt noktası | src/tools.ts getAllBaseTools() tam okuma | 40 tool + 18 koşullu (feature/env-gate) referans; 8 tool dizini sızıntıda YOK |
| Tool input schema | brace-matching + zod key çıkarımı (her lazySchema bloku) | 36 tool schema'sı çıkarıldı; 4 tool boş/passthrough schema |
| Komut kayıt noktası | src/commands.ts COMMANDS() + INTERNAL_ONLY_COMMANDS + her dosyada `name:` | 84 isimli komut + 31 ant-only + ~12 feature-gate'lı = ~118 slash komutu |
| Env flag | `process.env.` tam tarama (rg -o, sort -u) | **499 benzersiz** (227'si CLAUDE* önekli) |
| Feature flag | `feature('...')` tam tarama | **90 benzersiz** |
| Hook event | HOOK_EVENTS dizisi (entrypoints/agentSdkTypes.ts) | **27 event** |
| Permission modu | src/types/permissions.ts | 5 dış + 2 iç mod |

**En kritik bulgu:** Sızıntı "otorite varyant" olmasına rağmen 8 ant-only/feature-gate'lı tool'un kaynağı eksik (tools.ts'de yalnızca require referansı var: TungstenTool, MonitorTool, SendUserFileTool, PushNotificationTool, SubscribePRTool, SuggestBackgroundPRTool, VerifyPlanExecutionTool, ayrıca REPLTool.ts ve SleepTool.ts ana dosyaları). Yani sızıntı, kullanıcı ajanı görünümü verip ANTHROPIC-İÇİ (ant) katmanı bilinçli dışarıda bırakan **kısmen lobotomize bir varyanttır**; kalan çekirdek (schema'lar, hook protokolleri, permission motoru) gerçek ve eksiksizdir.

## 1. TOOL ENVANTERİ (40 kayıtlı + 18 koşullu referans)

| # | Tool (ad) | Parametreler (input schema) | Ne yapar | Permission | Side-effect | Maliyet | Karar |
|---|---|---|---|---|---|---|---|
| 1 | Agent | description, prompt, subagent_type, model(sonnet/opus/haiku), run_in_background, name, team_name, mode, isolation(worktree/remote), cwd | Alt ajan (subagent) başlatır; worktree/remote izolasyon | ask | Yeni işlem, disk/write | Yüksek (token) | GOOD — çekirdek işlev |
| 2 | AskUserQuestion | question, header, options[2-4]{label,description,preview}, multiSelect | Kullanıcıya soru sorar | allow | UI blokajı | Düşük | GOOD |
| 3 | Bash | command, description, _simulatedSedEdit{filePath,newContent} | Kabuk komutu çalıştırır; sandbox kararı, sed/sudo/readonly/destructive validasyonları | ask | Rastgele komut yürütme (en geniş yüzey) | Orta | GOOD — en kritik yüzey |
| 4 | SendUserMessage (eski Brief) | message, path, size, isImage, file_uuid | Kullanıcıya mesaj + dosya gönderir | allow | UI | Düşük | GOOD |
| 5 | Config | setting, value | Ayarları get/set (ant-only) | ask | Config yazma | Düşük | NÖTR (ant-only) |
| 6 | EnterPlanMode | message | Plan moduna geçer | ask | Akış | Düşük | GOOD |
| 7 | EnterWorktree | name, message, action, discard_changes | Git worktree yaratır | ask | Git/disk | Orta | GOOD |
| 8 | ExitPlanMode (V2) | plan, plan_version, plan_v2, tool(enum Bash), prompt | Plan onayı ister | ask | Akış | Düşük | GOOD |
| 9 | ExitWorktree | action(keep/remove), message | Worktree kapatır | ask | Git/disk | Orta | GOOD |
| 10 | Edit | file_path, old_string, new_string, replace_all | Dosya düzenler (string replasman) | ask | Disk yazma | Orta | GOOD |
| 11 | Read | file_path, offset, limit, options{sequentialNumbers} | Dosya okur (görüntü işleme dahil) | ask | Okuma | Düşük | GOOD |
| 12 | Write | file_path, content | Dosya yazar | ask | Disk yazma | Orta | GOOD |
| 13 | Glob | pattern, path, max_results, include_hidden, target_directory | Glob araması | ask | Okuma | Düşük | GOOD |
| 14 | Grep | pattern, path, glob, output_mode, substitute, include_hidden, regex | Regex arama | ask | Okuma | Düşük | GOOD |
| 15 | NotebookEdit | notebook_path, cell_id, new_source | Jupyter hücre düzenler | ask | Disk yazma | Orta | GOOD |
| 16 | WebFetch | url, prompt | URL çeker, prompt'a işletir (preapproved URL listesi) | ask | Ağ | Yüksek | GOOD |
| 17 | TodoWrite | todos | Yapılacak listesi günceller | allow | State | Düşük | GOOD |
| 18 | WebSearch | query | Web araması | ask | Ağ | Yüksek | GOOD |
| 19 | TaskStop | task_id, shell_id(deprecated) | Görevi durdurur | ask | İşlem | Orta | GOOD |
| 20 | Skill | skill, args, command_name, _PROTO_skill_name | Skill çalıştırır (inline/forked) | ask | Çalıştırma | Değişken | GOOD |
| 21 | LSP | (LSPTool.ts) | LSP sembol/doküman sorgusu (ENABLE_LSP_TOOL) | ask | Okuma | Düşük | GOOD |
| 22 | ListMcpResourcesTool | server(ops.) | MCP kaynaklarını listeler | allow | Okuma | Düşük | GOOD |
| 23 | ReadMcpResourceTool | server, uri | MCP kaynağı okur | ask | Okuma | Düşük | GOOD |
| 24 | MCPTool | passthrough (z.object({}).passthrough()) | MCP sunucu çağrısı (mcp__ öneki) | ask | Uzaktan çağrı | Değişken | GOOD |
| 25 | McpAuthTool | boş (z.object({})) | MCP OAuth akışı | allow | Auth | Düşük | NÖTR |
| 26 | StructuredOutput | passthrough | Yapılandırılmış çıktı yolu | allow | Çıktı | Düşük | GOOD |
| 27 | TaskCreate | subject, description | Görev yaratır (TodoV2) | ask | State | Düşük | GOOD |
| 28 | TaskGet | task | Görev detayı | ask | Okuma | Düşük | GOOD |
| 29 | TaskList | (liste) | Görevleri listeler | ask | Okuma | Düşük | GOOD |
| 30 | TaskOutput | (çıktı akışı) | Görev çıktısı | ask | Okuma | Düşük | GOOD |
| 31 | TaskUpdate | task_id, subject, description, status, owner, metadata | Görevi günceller | ask | State | Düşük | GOOD |
| 32 | TeamCreate | team_name, description, agent_type | Takım (swarm) yaratır | ask | State | Düşük | GOOD |
| 33 | TeamDelete | team_name | Takımı siler (aktif üye kontrolü) | ask | State | Düşük | GOOD |
| 34 | CronCreate | cron, prompt, recurring, durable | Zamanlı görev (AGENT_TRIGGERS) | ask | Scheduler | Düşük | GOOD |
| 35 | CronDelete | id | Zamanlı görevi siler | ask | Scheduler | Düşük | GOOD |
| 36 | CronList | (liste) | Zamanlı görevleri listeler | allow | Okuma | Düşük | GOOD |
| 37 | ToolSearch | query, max_results | Ertelenmiş (deferred) tool arar/seçer | allow | Okuma | Düşük | GOOD |
| 38 | PowerShell | (clmTypes + schema) | PowerShell çalıştırır | ask | Komut yürütme | Orta | GOOD |
| 39 | Sleep | SleepTool.ts sızıntıda YOK | Bekleme/dream (PROACTIVE/KAIROS) | — | Zaman | Düşük | BAD — kodu dışarıda |
| 40 | RemoteTrigger | (RemoteTriggerTool.ts) | Uzaktan tetik (AGENT_TRIGGERS_REMOTE) | ask | Ağ | Orta | NÖTR |
| 41 | REPL | REPLTool.ts YOK (sadece constants/primitiveTools) | REPL modu (ant-only) | — | — | — | BAD — kodu eksik |
| 42 | TestingPermissionTool | (testing/) | Permission test aracı (NODE_ENV=test) | ask/deny | Test | Düşük | NÖTR |

**Koşullu referanslı ama dizini sızıntıda OLMAYAN tool'lar (tools.ts'de require var, kaynak yok):** SuggestBackgroundPRTool, MonitorTool, SendUserFileTool, PushNotificationTool, SubscribePRTool, VerifyPlanExecutionTool, TungstenTool, OverflowTestTool, CtxInspectTool, TerminalCaptureTool, WebBrowserTool, SnipTool, ListPeersTool, WorkflowTool → hepsi **BAD** (lobotomize/eksik sızıntı kanıtı).
## 2. KOMUT ENVANTERİ (118 slash komutu)

### 2a. Kullanıcı komutları (84 isimli)

| # | Komut | Davranış | Karar |
|---|---|---|---|
| 1 | /add-dir | Çalışma dizini ekler | GOOD |
| 2 | /advisor | Advisor model yapılandırır | GOOD |
| 3 | /agents | Agent config'lerini yönetir | GOOD |
| 4 | /branch | Konuşmanın dalını yaratır | GOOD |
| 5 | /remote-control | Uzaktan kontrol oturumu açar | GOOD |
| 6 | /bridge-kick | Bridge hata durumu enjekte eder (test) | NÖTR |
| 7 | /brief | Brief-only mod | GOOD |
| 8 | /btw | Yan soru (ana konuşmayı kesmez) | GOOD |
| 9 | /chrome | Chrome entegrasyonu ayarları | GOOD |
| 10 | /clear | Geçmişi temizler | GOOD |
| 11 | /color | Prompt bar rengi | NÖTR |
| 12 | /commit-push-pr | Commit+push+PR | GOOD |
| 13 | /commit | Git commit | GOOD |
| 14 | /compact | Özetle sıkıştırma | GOOD |
| 15 | /config | Config paneli | GOOD |
| 16 | /context | Context kullanımını görselleştirir | GOOD |
| 17 | /copy | Sohbeti kopyalar | GOOD |
| 18 | /cost | Oturum maliyeti/süresi | GOOD |
| 19 | /desktop | Claude Desktop'a devam | GOOD |
| 20 | /diff | Değişiklikleri ve turn diff'lerini gösterir | GOOD |
| 21 | /doctor | Kurulum/settings teşhisi | GOOD |
| 22 | /effort | Model efor seviyesi | GOOD |
| 23 | /exit | REPL'den çıkar | GOOD |
| 24 | /export | Konuşmayı dosyaya/panoya dışa aktarır | GOOD |
| 25 | /extra-usage | Limit sonrası kullanım yapılandırır | GOOD |
| 26 | /fast | Hızlı mod | GOOD |
| 27 | /feedback | Geri bildirim | GOOD |
| 28 | /files | Context'teki dosyaları listeler | GOOD |
| 29 | /heapdump | JS heap dump'ı masaüstüne | NÖTR |
| 30 | /help | Yardım + komut listesi | GOOD |
| 31 | /hooks | Hook yönetimi | GOOD |
| 32 | /ide | IDE entegrasyonu | GOOD |
| 33 | /init-verifiers | Verifier başlatma | NÖTR |
| 34 | /init | Kurulum | GOOD |
| 35 | /project_areas | Proje alanları (insights) | NÖTR |
| 36 | /install-github-app | GitHub App kurar | GOOD |
| 37 | /install-slack-app | Slack App kurar | GOOD |
| 38 | /install | Kurulum | GOOD |
| 39 | /keybindings | Kısayollar | GOOD |
| 40 | /login | Giriş | GOOD |
| 41 | /logout | Çıkış | GOOD |
| 42 | /mcp | MCP sunucu yönetimi | GOOD |
| 43 | /memory | Bellek yönetimi | GOOD |
| 44 | /mobile | Mobil | GOOD |
| 45 | /model | Model seçimi | GOOD |
| 46 | /output-style | Çıktı stili | GOOD |
| 47 | /passes | Pass yönetimi | GOOD |
| 48 | /permissions | İzin yönetimi | GOOD |
| 49 | /plan | Plan modu | GOOD |
| 50 | /plugin | Plugin yönetimi | GOOD |
| 51 | /pr-comments | PR yorumları | GOOD |
| 52 | /privacy-settings | Gizlilik ayarları | GOOD |
| 53 | /rate-limit-options | Rate limit seçenekleri | GOOD |
| 54 | /release-notes | Sürüm notları | GOOD |
| 55 | /reload-plugins | Plugin yeniden yükle | GOOD |
| 56 | /remote-env | Uzak ortam env değişkenleri | GOOD |
| 57 | /web-setup | CCR remote kurulumu | GOOD |
| 58 | /rename | Oturum adı | GOOD |
| 59 | /resume | Oturum devam | GOOD |
| 60 | /review | Kod inceleme | GOOD |
| 61 | /rewind | Geri sar | GOOD |
| 62 | /sandbox | Sandbox aç/kapat | GOOD |
| 63 | /security-review | Güvenlik incelemesi | GOOD |
| 64 | /session | Oturum yönetimi | GOOD |
| 65 | /skills | Skill yönetimi | GOOD |
| 66 | /stats | İstatistik | GOOD |
| 67 | /status | Durum | GOOD |
| 68 | /statusline | Statusline | GOOD |
| 69 | /stickers | Sticker | NÖTR |
| 70 | /tag | Etiketleme | GOOD |
| 71 | /tasks | Görevler | GOOD |
| 72 | /terminal-setup | Terminal kurulumu | GOOD |
| 73 | /theme | Tema | GOOD |
| 74 | /think-back | Thinkback | GOOD |
| 75 | /thinkback-play | Thinkback oynat | GOOD |
| 76 | /ultraplan | Ultra plan (ULTRAPLAN) | GOOD |
| 77 | /upgrade | Güncelleme | GOOD |
| 78 | /usage | Kullanım | GOOD |
| 79 | /version | Sürüm | GOOD |
| 80 | /vim | Vim modu | GOOD |
| 81 | /voice | Ses modu (VOICE_MODE) | GOOD |
| 82 | /x402 | X402 ödeme akışı | NÖTR |
| 83 | /insights | Insights | NÖTR |
| 84 | /context (non-interactive) | Context dökümü (script) | GOOD |

### 2b. INTERNAL_ONLY_COMMANDS (USER_TYPE=ant, ~31) — BAD (normal kullanıcıya kapalı)

| Komut | Davranış | Karar |
|---|---|---|
| /backfill-sessions | Oturum geçmişi backfill | BAD — ant-içi |
| /break-cache | Cache kırma | BAD |
| /bughunter | Bug avcısı | BAD |
| /commit-push-pr, /commit | Git işlemleri (ant build) | NÖTR |
| /ctx_viz | Context görselleştirme | NÖTR |
| /good-claude | İyi ajan raporlama | BAD — kullanıcı davranışı ölçümü |
| /issue | Issue açma | NÖTR |
| /force-snip | Snip zorlama | NÖTR |
| /mock-limits | Limit mock'u | BAD |
| /reset-limits | Limit sıfırlama | BAD — üretici kontrol anahtarı |
| /onboarding | Onboarding | GOOD |
| /share, /summary, /teleport | Paylaşım/özet | NÖTR |
| /ant-trace, /perf-issue | Ant izleme/performans | BAD — telemetri |
| /env | Ortam dökümü | NÖTR |
| /oauth-refresh | OAuth yenileme | NÖTR |
| /debug-tool-call | Tool çağrısı debug | NÖTR |
| /agents-platform | Ajan platformu | BAD |
| /autofix-pr | PR otomatik düzeltme | NÖTR |

### 2c. Feature-gate'lı (~12) — NÖTR (hepsi koşullu, kaynak mevcut)

| Komut | Feature | Karar |
|---|---|---|
| /proactive | PROACTIVE | NÖTR |
| /brief | KAIROS_BRIEF | NÖTR |
| /assistant | KAIROS | NÖTR |
| /bridge | BRIDGE_MODE | NÖTR |
| /remoteControlServer | DAEMON+BRIDGE_MODE | NÖTR |
| /voice | VOICE_MODE | NÖTR |
| /workflows | WORKFLOW_SCRIPTS | NÖTR |
| /remote-setup | CCR_REMOTE_SETUP | NÖTR |
| /subscribe-pr | KAIROS_GITHUB_WEBHOOKS | NÖTR |
| /torch | TORCH | NÖTR |
| /peers | UDS_INBOX | NÖTR |
| /fork | FORK_SUBAGENT | NÖTR |
| /buddy | BUDDY | NÖTR |
## 3. ENV FLAG ENVANTERİ (499 benzersiz; 227 CLAUDE* önekli)

### 3a. CLAUDE_CODE_* — çekirdek (tam liste, ~200)

**GOOD (kullanıcı lehine):** CLAUDE_CODE_ACCESSIBILITY, ADDITIONAL_PROTECTION, ATTRIBUTION_HEADER, AUTO_MODE_MODEL, BASE_REF, BASH_SANDBOX_SHOW_INDICATOR, CLIENT_CERT/CLIENT_KEY/CLIENT_KEY_PASSPHRASE, COMMIT_LOG, CONTAINER_ID, DEBUG_LOG_LEVEL, DEBUG_LOGS_DIR, DEBUG_REPAINTS, DIAGNOSTICS_FILE, DISABLE_1M_CONTEXT, DISABLE_ADAPTIVE_THINKING, DISABLE_ADVISOR_TOOL, DISABLE_ATTACHMENTS, DISABLE_AUTO_MEMORY, DISABLE_BACKGROUND_TASKS, DISABLE_CLAUDE_MDS, DISABLE_CRON, DISABLE_EXPERIMENTAL_BETAS, DISABLE_FAST_MODE, DISABLE_FEEDBACK_SURVEY, DISABLE_FILE_CHECKPOINTING, DISABLE_GIT_INSTRUCTIONS, DISABLE_LEGACY_MODEL_REMAP, DISABLE_MESSAGE_ACTIONS, DISABLE_MOUSE, DISABLE_MOUSE_CLICKS, DISABLE_NONSTREAMING_FALLBACK, DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL, DISABLE_TERMINAL_TITLE, DISABLE_THINKING, DISABLE_VIRTUAL_SCROLL, DONT_INHERIT_ENV, EFFORT_LEVEL, ENABLE_FINE_GRAINED_TOOL_STREAMING, ENABLE_SDK_FILE_CHECKPOINTING, ENABLE_TASKS, FILE_READ_MAX_OUTPUT_TOKENS, FORCE_FULL_LOGO, GLOB_HIDDEN, GLOB_NO_IGNORE, GLOB_TIMEOUT_SECONDS, IDLE_THRESHOLD_MINUTES, IDLE_TOKEN_THRESHOLD, MAX_CONTEXT_TOKENS, MAX_OUTPUT_TOKENS, MAX_RETRIES, MAX_TOOL_USE_CONCURRENCY, PLUGIN_CACHE_DIR, PLUGIN_GIT_TIMEOUT_MS, PLUGIN_SEED_DIR, PROFILE_QUERY, PROFILE_STARTUP, SHELL, SHELL_PREFIX, GIT_BASH_PATH, PWSH_PARSE_TIMEOUT_MS, SUBAGENT_MODEL, SUBPROCESS_ENV_SCRUB, TMPDIR, UNATTENDED_RETRY, USE_BEDROCK/USE_VERTEX/USE_FOUNDRY, SKIP_BEDROCK_AUTH/SKIP_VERTEX_AUTH/SKIP_FOUNDRY_AUTH, WORKSPACE_HOST_PATHS, ADDITIONAL_DIRECTORIES_CLAUDE_MD

**NÖTR (altyapı):** API_BASE_URL, API_KEY_FILE_DESCRIPTOR, API_KEY_HELPER_TTL_MS, AUTO_COMPACT_WINDOW, AUTO_CONNECT_IDE, BLOCKING_LIMIT_OVERRIDE, BRIDGE_BASE_URL, CCR_MIRROR, CLIENT_APP, COORDINATOR_MODE, COWORKER_TYPE, DEBUG_LOGS_DIR, DISABLE_COMMAND_INJECTION_CHECK, DISABLE_NONESSENTIAL_TRAFFIC, DISABLE_PRECOMPACT_SKIP, DISABLE_POLICY_SKILLS, EAGER_FLUSH, EMIT_SESSION_STATE_EVENTS, EMIT_TOOL_USE_SUMMARIES, ENABLE_CFC, ENABLE_PROMPT_SUGGESTION, ENABLE_TELEMETRY, ENABLE_TOKEN_USAGE_ATTACHMENT, ENTRYPOINT, ENVIRONMENT_KIND, ENVIRONMENT_RUNNER_VERSION, EXIT_AFTER_FIRST_RENDER, EXIT_AFTER_STOP_DELAY, EXTRA_BODY, EXTRA_METADATA, GB_BASE_URL, HOST_PLATFORM, IDE_HOST_OVERRIDE, IDE_SKIP_AUTO_INSTALL, IDE_SKIP_VALID_CHECK, INCLUDE_PARTIAL_MESSAGES, IS_COWORK, JSONL_TRANSCRIPT, MCP_INSTR_DELTA, MESSAGING_SOCKET, NEW_INIT, NO_FLICKER, OAUTH_CLIENT_ID, OAUTH_REFRESH_TOKEN, OAUTH_SCOPES, OAUTH_TOKEN, OAUTH_TOKEN_FILE_DESCRIPTOR, OTEL_FLUSH_TIMEOUT_MS, OTEL_HEADERS_HELPER_DEBOUNCE_MS, OTEL_SHUTDOWN_TIMEOUT_MS, OVERRIDE_DATE, PERFETTO_TRACE, PERFETTO_WRITE_INTERVAL_S, PLAN_MODE_INTERVIEW_PHASE, PLAN_MODE_REQUIRED, PLAN_V2_AGENT_COUNT, PLAN_V2_EXPLORE_AGENT_COUNT, PLUGIN_USE_ZIP_CACHE, POST_FOR_SESSION_INGRESS_V2, PROXY_RESOLVES_HOSTS, PROVIDER_MANAGED_BY_HOST, QUESTION_PREVIEW_FORMAT, REMOTE, REMOTE_ENVIRONMENT_TYPE, REMOTE_MEMORY_DIR, REMOTE_SEND_KEEPALIVES, REMOTE_SESSION_ID, REPL, RESUME_INTERRUPTED_TURN, SAVE_HOOK_ADDITIONAL_CONTEXT, SCROLL_SPEED, SESSION_ACCESS_TOKEN, SESSIONEND_HOOKS_TIMEOUT_MS, SESSION_ID, SESSION_KIND, SESSION_LOG, SESSION_NAME, SIMPLE, SKIP_FAST_MODE_NETWORK_ERRORS, SKIP_PROMPT_HISTORY, SLOW_OPERATION_THRESHOLD_MS, SSE_PORT, STALL_TIMEOUT_MS_FOR_TESTING, STREAMLINED_OUTPUT, SYNTAX_HIGHLIGHT, SYNC_PLUGIN_INSTALL, SYNC_PLUGIN_INSTALL_TIMEOUT_MS, TAGS, TASK_LIST_ID, TERMINAL_RECORDING, TEST_FIXTURES_ROOT, TMUX_PREFIX, TMUX_PREFIX_CONFLICTS, TMUX_SESSION, TMUX_TRUECOLOR, TWO_STAGE_CLASSIFIER, USE_CCR_V2, USE_COWORK_PLUGINS, USE_NATIVE_FILE_SEARCH, USE_POWERSHELL_TOOL, VERIFY_PLAN, WEBSOCKET_AUTH_FILE_DESCRIPTOR, WORKER_EPOCH, ENABLE_XAA (BAD yönü aşağıda)

**BAD (LOBOTOMİ/tekelleşme):** ACCOUNT_TAGGED_ID, ACCOUNT_UUID, ORGANIZATION_UUID (hesap etiketleme/takip), BLOCKING_LIMIT_OVERRIDE (limit bypass üretici anahtarı), ENHANCED_TELEMETRY_BETA (derin telemetri), MANAGED_SETTINGS_PATH (kurumsal ayar zorlaması), UNDERCOVER (gizli mod), DISABLE_COMMAND_INJECTION_CHECK (güvenlik kapatılabilir), DISABLE_POLICY_SKILLS (kurumsal kırpma)

### 3b. CLAUDE_* diğer (~27)

| Env | Karar | Gerekçe |
|---|---|---|
| CLAUDE_BRIDGE_BASE_URL / OAUTH_TOKEN / SESSION_INGRESS_URL / USE_CCR_V2 | NÖTR | Bridge/CCR altyapısı |
| CLAUDE_CONFIG_DIR | GOOD | Config dizini |
| CLAUDE_JOB_DIR | NÖTR | İş dizini |
| CLAUDE_MOCK_HEADERLESS_429 | NÖTR | 429 mock (test) |
| CLAUDE_MORERIGHT | NÖTR | Moreright özelliği |
| CLAUDE_TRUSTED_DEVICE_TOKEN | BAD | Cihaz güven jetonu (cihaz bağlama) |
| CLAUDE_ENABLE_STREAM_WATCHDOG / STREAM_IDLE_TIMEOUT_MS | GOOD | Akış koruması |
| CLAUDE_ENV_FILE | GOOD | Env dosyası |
| CLAUDE_FORCE_DISPLAY_SURVEY | BAD | Anket zorlama |
| CLAUDE_INTERNAL_FC_OVERRIDES | BAD | İç feature-flag override — kullanıcı kırpım anahtarı |
| CLAUDE_LOCAL_OAUTH_API_BASE / APPS_BASE / CONSOLE_BASE | NÖTR | Local OAuth |
| CLAUDE_SESSION_INGRESS_TOKEN_FILE | NÖTR | Oturum jetonu |
| CLAUDE_REPL_MODE | NÖTR | REPL modu |
| CLAUDE_AFTER_LAST_COMPACT / AUTOCOMPACT_PCT_OVERRIDE | NÖTR | Kompakt ayarları |
| CLAUDE_AUTO_BACKGROUND_TASKS | GOOD | Arka plan görevleri |
| CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR | GOOD | Bash çalışma dizini |
| CLAUDE_DEBUG | GOOD | Debug |
| CLAUDE_AGENT_SDK_* (CLIENT_APP, DISABLE_BUILTIN_AGENTS, MCP_NO_PREFIX, VERSION) | NÖTR | SDK |
| CLAUDE_CODE_* ortak listesi (227 adet) | bkz. 3a | |

### 3c. Genel/üçüncü taraf (~245)

| Grup | Örnekler | Karar |
|---|---|---|
| Sağlayıcılar | ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN, ANTHROPIC_BASE_URL, ANTHROPIC_MODEL, ANTHROPIC_SMALL_FAST_MODEL, ANTHROPIC_BEDROCK_BASE_URL, ANTHROPIC_VERTEX_PROJECT_ID, ANTHROPIC_FOUNDRY_*, ANTHROPIC_CUSTOM_* , ANTHROPIC_DEFAULT_*_MODEL*, ANTHROPIC_UNIX_SOCKET, AWS_*, GOOGLE_CLOUD_PROJECT, VERTEX_BASE_URL, BEDROCK_BASE_URL | GOOD |
| OTEL | OTEL_EXPORTER_*, OTEL_LOGS_*, OTEL_METRICS_*, OTEL_TRACES_*, ANT_OTEL_* | NÖTR |
| Telemetri/analitik | ANT_CLAUDE_CODE_METRICS_ENDPOINT, BETA_TRACING_ENDPOINT, TEAM_MEMORY_SYNC_URL, VOICE_STREAM_BASE_URL, ENABLE_ENHANCED_TELEMETRY_BETA, ENABLE_BETA_TRACING_DETAILED, CURSOR_TRACE_ID | BAD — veri dışa akış uçları |
| Limit/abonelik | MAX_SESSIONS, MAX_SESSIONS_PER_HOUR, MAX_SESSIONS_PER_USER, DISABLE_COST_WARNINGS, DISABLE_PROMPT_CACHING*, FALLBACK_FOR_ALL_PRIMARY_MODELS, ENABLE_PROMPT_CACHING_1H_BEDROCK, CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS, SESSION_GRACE_MS | NÖTR |
| Kill-switch (kullanıcı lehine) | DISABLE_AUTO_COMPACT, DISABLE_AUTOUPDATER, DISABLE_COMPACT, DISABLE_TELEMETRY, DISABLE_ERROR_REPORTING, DISABLE_INSTALLATION_CHECKS, DISABLE_UPGRADE_COMMAND, DISABLE_LOGIN_COMMAND, DISABLE_LOGOUT_COMMAND, DISABLE_FEEDBACK_COMMAND, DISABLE_BUG_COMMAND, DISABLE_DOCTOR_COMMAND, DISABLE_EXTRA_USAGE_COMMAND, DISABLE_INSTALL_GITHUB_APP_COMMAND, DISABLE_CLAUDE_CODE_SM_COMPACT, ENABLE_CLAUDE_CODE_SM_COMPACT | GOOD — kullanıcı kapatabilir |
| Ortam tespiti | CI, GITHUB_ACTIONS, GITHUB_*, GITLAB_CI, CODESPACES, GITPOD_WORKSPACE_ID, FLY_*, VERCEL, NETLIFY, RENDER, RAILWAY_*, K_SERVICE, WEBSITE_*, KUBERNETES_SERVICE_HOST, AWS_LAMBDA_FUNCTION_NAME, AZURE_FUNCTIONS_ENVIRONMENT, DENO_DEPLOYMENT_ID, DYNO, PROJECT_DOMAIN, CF_PAGES, BUILDKITE, CIRCLECI, RUNNER_* | NÖTR |
| Terminal | TERM, TERM_PROGRAM, COLORTERM, TMUX, TMUX_PANE, KITTY_WINDOW_ID, ITERM_SESSION_ID, WSL_DISTRO_NAME, WT_SESSION, VSCODE_GIT_ASKPASS_MAIN, GNOME_TERMINAL_SERVICE, KONSOLE_VERSION, VTE_VERSION, XTERM_VERSION, ZED_TERM, SSH_CLIENT/CONNECTION/TTY, TERMINAL_EMULATOR, ConEmu*, STY, MSYSTEM, VisualStudioVersion, SESSIONNAME, ALACRITTY_LOG, BAT_THEME, TERMINATOR_UUID, TILIX_ID, BROWSER, EDITOR, VISUAL | NÖTR |
| Proxy/güven | HTTP_PROXY, HTTPS_PROXY, http_proxy, https_proxy, NO_PROXY, no_proxy, ALLOWED_ORIGINS, NODE_EXTRA_CA_CERTS, SSL_CERT_FILE, NODE_OPTIONS, UV_THREADPOOL_SIZE | GOOD |
| Sandbox/limit | IS_SANDBOX, SAFEUSER, HOST, PORT, BASH_MAX_OUTPUT_LENGTH, SCROLLBACK_BYTES, SLASH_COMMAND_TOOL_CHAR_BUDGET, TASK_MAX_OUTPUT_LENGTH, MAX_MCP_OUTPUT_TOKENS, MCP_TIMEOUT, MCP_TOOL_TIMEOUT, MAX_STRUCTURED_OUTPUT_RETRIES, MAX_THINKING_TOKENS, API_TIMEOUT_MS, API_MAX_INPUT_TOKENS, API_TARGET_INPUT_TOKENS, USE_BUILTIN_RIPGREP, EMBEDDED_SEARCH_TOOLS, ENABLE_MCP_LARGE_OUTPUT_FILES, MCP_REMOTE_SERVER_CONNECTION_BATCH_SIZE, MCP_SERVER_CONNECTION_BATCH_SIZE | NÖTR/GOOD |
| OAuth/güven | OAUTH_CLIENT_ID, OAUTH_CLIENT_SECRET, OAUTH_ISSUER, OAUTH_SCOPES, OAUTH_CALLBACK_URL, SESSION_SECRET, USE_LOCAL_OAUTH, USE_STAGING_OAUTH, AUTH_PROVIDER, AUTH_TOKEN, ADMIN_USERS, USER_TYPE, USER_HOME_BASE, MCP_CLIENT_SECRET, MCP_OAUTH_CALLBACK_PORT, MCP_OAUTH_CLIENT_METADATA_URL, MCP_XAA_IDP_CLIENT_SECRET | NÖTR (ADMIN_USERS/USER_TYPE kapıları ant-içi) |
| Test/debug | NODE_ENV, DEBUG, DEBUG_SDK, FORCE_VCR, VCR_RECORD, TEST_ENABLE_SESSION_PERSISTENCE, BUGHUNTER_DEV_BUNDLE_B64, CLAUDE_CODE_STALL_TIMEOUT_MS_FOR_TESTING, CLAUDE_CODE_EXIT_AFTER_* | NÖTR |
| Swe-bench | SWE_BENCH_INSTANCE_ID, SWE_BENCH_RUN_ID, SWE_BENCH_TASK_ID | NÖTR — değerlendirme |
| Diğer ilginç | X402_PRIVATE_KEY (ödeme), CCR_ENABLE_BUNDLE, CCR_FORCE_BUNDLE, CCR_UPSTREAM_PROXY_ENABLED, CLAUDE_CODE_PROACTIVE, CLAUDE_CODE_PROXY_RESOLVES_HOSTS, P4PORT, COREPACK_ENABLE_AUTO_PIN, MONOREPO_ROOT_DIR, WORK_DIR, GITHUB_ACTION_INPUTS/PATH, LOCALAPPDATA, APPDATA, USERPROFILE, USERNAME, USER, HOME, PATH, PWD, TEMP, TMPDIR, XDG_CONFIG_HOME, LANG, LC_ALL, LC_TIME, SHELL, HOSTNAME | NÖTR |
## 4. PERMISSION MODLARI ve HOOK EVENT'LERİ

### Permission modları (src/types/permissions.ts)

| Mod | Tip | Davranış | Karar |
|---|---|---|---|
| acceptEdits | dış | Dosya düzenlemelerini otomatik kabul eder | GOOD |
| bypassPermissions | dış | Tüm izinleri atlar | NÖTR |
| default | dış | Sorar (default ask) | GOOD |
| dontAsk | dış | Sormaz | NÖTR |
| plan | dış | Plan onayı zorunlu | GOOD |
| auto | iç (TRANSCRIPT_CLASSIFIER flag'iyle açılır) | Otomatik sınıflandırıcı | NÖTR |
| bubble | iç (user-addressable değil) | Sadece iç mod | NÖTR |

### Hook event'leri (27, HOOK_EVENTS dizisi) — hepsi GOOD (kullanıcıya uzatma gücü)

| # | Event | # | Event |
|---|---|---|---|
| 1 | PreToolUse | 15 | PermissionDenied |
| 2 | PostToolUse | 16 | Setup |
| 3 | PostToolUseFailure | 17 | TeammateIdle |
| 4 | Notification | 18 | TaskCreated |
| 5 | UserPromptSubmit | 19 | TaskCompleted |
| 6 | SessionStart | 20 | Elicitation |
| 7 | SessionEnd | 21 | ElicitationResult |
| 8 | Stop | 22 | ConfigChange |
| 9 | StopFailure | 23 | WorktreeCreate |
| 10 | SubagentStart | 24 | WorktreeRemove |
| 11 | SubagentStop | 25 | InstructionsLoaded |
| 12 | PreCompact | 26 | CwdChanged |
| 13 | PostCompact | 27 | FileChanged |
| 14 | PermissionRequest | | |

Not: sync/async hook JSON çıktı protokolleri, hookSpecificOutput, permissionUpdateSchema, promptRequestSchema (elicitation) ve hook matcher motoru sızıntıda eksiksiz mevcut (src/utils/hooks/).
## 5. FEATURE FLAG'LERİ (90 benzersiz)

| Flag | Karar | Gerekçe |
|---|---|---|
| ABLATION_BASELINE, ANTI_DISTILLATION_CC | BAD | Distilasyon/ablasyon karşıtı ölçüm |
| AGENT_MEMORY_SNAPSHOT, AGENT_TRIGGERS, AGENT_TRIGGERS_REMOTE | NÖTR | Ajan hafıza/tetik |
| ALLOW_TEST_VERSIONS, BUILDING_CLAUDE_APPS, OVERFLOW_TEST_TOOL, HARD_FAIL | NÖTR | Yapı/test |
| AUTO_THEME, AWAY_SUMMARY, HISTORY_PICKER, HISTORY_SNIP, QUICK_SEARCH, MESSAGE_ACTIONS, NATIVE_CLIPBOARD_IMAGE, TEMPLATES, TERMINAL_PANEL, VOICE_MODE | GOOD | UI/UX |
| BASH_CLASSIFIER, TREE_SITTER_BASH, TREE_SITTER_BASH_SHADOW, POWERSHELL_AUTO_MODE, SKIP_DETECTION_WHEN_AUTOUPDATES_DISABLED | GOOD | Komut güvenlik sınıflandırıcısı |
| BG_SESSIONS, DAEMON, CACHED_MICROCOMPACT, REACTIVE_COMPACT, COMPACTION_REMINDERS, PROMPT_CACHE_BREAK_DETECTION | NÖTR | Arka plan/kompakt |
| BRIDGE_MODE, CCR_AUTO_CONNECT, CCR_MIRROR, CCR_REMOTE_SETUP, DIRECT_CONNECT, SSH_REMOTE, SELF_HOSTED_RUNNER, BYOC_ENVIRONMENT_RUNNER | NÖTR | Uzak/bulut kod yürütme (CCR) |
| BUDDY, COORDINATOR_MODE, COWORKER_TYPE_TELEMETRY, EXPERIMENTAL_AGENT_TEAMS, TEAMMEM | NÖTR | Swarm/teammate |
| CHICAGO_MCP, MCP_RICH_OUTPUT, MCP_SKILLS | GOOD | MCP |
| COMMIT_ATTRIBUTION | NÖTR | Commit atıf |
| CONNECTOR_TEXT, USE_CONNECTOR_TEXT_SUMMARIZATION | NÖTR | Bağlayıcı |
| CONTEXT_COLLAPSE, TOKEN_BUDGET | NÖTR | Context yönetimi |
| DOWNLOAD_USER_SETTINGS, UPLOAD_USER_SETTINGS | BAD | Kullanıcı ayarlarının bulut senkronu |
| DUMP_SYSTEM_PROMPT | GOOD | Sistem prompt dökümü |
| ENHANCED_TELEMETRY_BETA | BAD | Derin telemetri |
| EXPERIMENTAL_SKILL_SEARCH, RUN_SKILL_GENERATOR, SKILL_IMPROVEMENT | NÖTR | Skill |
| EXTRACT_MEMORIES, FILE_PERSISTENCE, MEMORY_SHAPE_TELEMETRY | NÖTR | Bellek |
| FORK_SUBAGENT, VERIFICATION_AGENT, REVIEW_ARTIFACT, UNATTENDED_RETRY | GOOD | Doğrulama/tekrar |
| HOOK_PROMPTS, WORKFLOW_SCRIPTS | GOOD | Uzatma |
| IS_LIBC_GLIBC, IS_LIBC_MUSL | NÖTR | libc tespiti |
| KAIROS, KAIROS_BRIEF, KAIROS_CHANNELS, KAIROS_DREAM, KAIROS_GITHUB_WEBHOOKS, KAIROS_PUSH_NOTIFICATION, LODESTONE | NÖTR | Yeni nesil ajan platformu |
| MONITOR_TOOL, SHOT_STATS, SLOW_OPERATION_LOGGING, STREAMLINED_OUTPUT, TRANSCRIPT_CLASSIFIER, PERFETTO_TRACING | NÖTR | İzleme |
| NATIVE_CLIENT_ATTESTATION | BAD | Yerel istemci kanıtlama — DRM benzeri |
| NEW_INIT, PROACTIVE, TORCH, ULTRAPLAN, ULTRATHINK, UDS_INBOX, WEB_BROWSER_TOOL | NÖTR/GOOD | Özellikler |
## 6. ÖZET KARAR TABLOSU

| Kategori | Sayı | GOOD (ÇAL) | BAD (LOBOTOMİ) | NÖTR |
|---|---|---|---|---|
| Tool | 40+18 | 36 | 8+14 (eksik kod + ant-only) | 14 |
| Komut | ~118 | ~90 | ~31 (ant-only) | ~12 |
| Env flag | 499 | ~200 | ~15 | ~284 |
| Feature flag | 90 | ~30 | ~8 | ~52 |
| Hook event | 27 | 27 | 0 | 0 |
| Permission modu | 7 | 4 | 0 | 3 |
**Sonuç:** Sızıntı büyük ölçüde gerçek (GOOD) — çekirdek motor, tool schema'ları, hook protokolleri, permission motoru eksiksiz ve çalıştırılabilir.
LOBOTOMİ işaretleri: (1) 8 ant-only tool + REPLTool/SleepTool ana dosyalarının sızıntıda olmaması,
(2) CLAUDE_INTERNAL_FC_OVERRIDES, NATIVE_CLIENT_ATTESTATION, DOWNLOAD/UPLOAD_USER_SETTINGS, ACCOUNT_UUID/TAGGED_ID, UNDERCOVER, ENHANCED_TELEMETRY_BETA gibi tekelleşme/ölçüm yapıları,
(3) ADMIN_USERS ve USER_TYPE=ant kapıları tümüyle normal kullanıcıya kapalı.
Yani: çalışır bir motor + bilinçli kırpılmış ant-içi katman.
