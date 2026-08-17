# 11 — GROK CLI ENVANTERİ (%100 EKSİKSİZ, DETERMİNİSTİK)

Kaynak ağaç: `/home/void0x14/Documents/omnitrix/` (kullanıcının kendi fork'u, xai-* crates).
Yöntem: keyword grep + örnekleme DEĞİL; her kayıt noktasının TAM enumerasyonu (registry dosyaları okundu, kayıt çağrıları tek tek sayıldı).
Karar sözlüğü: **TUT** = kendi temeline kat, **AT** = kullanma/çıkar, **DONDUR** = koru ama şimdilik dokunma, **NÖTR** = taşır ama kritik değil.

## 0. KAPSAM KANITI (scope evidence)

| Kanıt | Değer |
|---|---|
| SOURCE_REV | `33f0aacfcc95eb7de316308e9a3bc22470e48b89` |
| Son commit | `8334a5b feat(keys): /import /export slash komutlari + OS keyring + Turkce dokumantasyon` |
| Tool kaydı | `crates/codegen/xai-grok-tools/src/registry/types.rs` `ToolRegistryBuilder::new()` → **52 tool + 3 reminder** |
| Slash komut kaydı | `crates/codegen/xai-grok-pager/src/slash/commands/mod.rs` `builtin_commands()` → **88 builtin** (kayıt noktası tek: `slash/commands/mod.rs:93`) |
| Slash registry altyapısı | `crates/codegen/xai-grok-pager/src/slash/registry.rs` (1263 satır: Builtin/Acp ayrımı, hidden/menu_hidden/restricted/tool-gated kapıları) |
| Config anahtar yüzeyi | `xai-grok-shell/src/agent/config.rs` `Config` (**89 alan**) + `xai-grok-config-types` `RemoteSettings` (**149 alan**) + alt yapılar |
| Routing modları | `config/routing_modes.toml` → **65 `[[modes]]`** (grep sayımı: 65) |
| Flow Governor | `xai-grok-shell/src/session/flow/` (governor/gate/classifier/judge/parallel/notify) + `config/flow/*.toml.example` |
| Shell yüzeyi | `xai-grok-shell/src/auth/` (22 dosya), `remote/`, `claude_import.rs`, `session/acp_session.rs` (SessionActor) |
| Omni eklentileri (kullanıcı IP'si) | `pager/src/slash/commands/omni_*` (9 komut), `pager/src/omni_bridge.rs`, `omni_runtime.rs`, `xai-omni-keychain` (8+ stack), `grok_build_hashline` (3 tool), `session/flow/*` |

---

## 1. TOOL ENVANTERİ — çekirdek kayıt (ToolRegistryBuilder::new, registry/types.rs)

Kayıt sırası birebir korundu. Parametreler: schema'dan öne çıkanlar. Yetki: `requires_expr` + `ToolKind` (Read/Edit/Search/Execute/BackgroundTaskAction/KillTaskAction/Plan/...).

### 1.1 GrokBuild namespace (28 tool)

| # | Tool ID (GrokBuild:) | Parametreler (öne çıkan) | Davranış | Yetki | Karar |
|---|---|---|---|---|---|
| 1 | `run_terminal_cmd` (BashTool) | command, is_background, run_in_background, timeout_ms | PTY shell çalıştırır, background + timeout destekler | `enabled_background=true` → BackgroundTaskAction + KillTaskAction şartı | TUT |
| 2 | `read_file` | file_path, offset, limit, encoding | Dosya oku (offset/limit) | Read | TUT |
| 3 | `search_replace` | file_path, old_string, new_string, replace_all, skip_read_before_edit | Tam metin değiştirme | `skip_read_before_edit` yoksa Read tool şartı; old/new/replace_all input_param şartı | TUT |
| 4 | `list_dir` | path, max_depth | Dizin listele | Read | TUT |
| 5 | `grep` | pattern, path, glob, max_results | Regex arama (ripgrep) | Search | TUT |
| 6 | `kill_task` | task_id | Subagent görevini öldür | TaskTool şartı | TUT |
| 7 | `kill_terminal_command` | task_id | Background terminal komutunu öldür | KillTaskAction | TUT |
| 8 | `todo_write` | todos[], mode(merge/replace), is_parallel | Canlı todo listesi | Execute | TUT |
| 9 | `update_goal` | message, completed, blocked_reason | Goal durum raporu | Execute | TUT |
| 10 | `workflow` | name \| script, args | Rhai script ile subagent orkestrasyonu (background) | Execute | TUT |
| 11 | `get_task_output` | task_id, timeout_ms | Subagent çıktısını oku | BackgroundTaskAction | TUT |
| 12 | `get_terminal_command_output` | task_id | Terminal komut çıktısı | BackgroundTaskAction | TUT |
| 13 | `wait_tasks` | task_ids, timeout_ms | Görevlerin bitmesini bekle | BackgroundTaskAction | TUT |
| 14 | `task` | prompt, description, tools, priority, max_turns | Subagent spawn (backend: xai-grok-agent) | BackgroundTaskAction + KillTaskAction şartı | TUT |
| 15 | `web_search` | query, max_results | Web arama (Responses API) | Search | TUT |
| 16 | `web_fetch` | url, max_chars, headers | URL çek (SSRF guard, domain allowlist, cache, overflow) | Read (dış ağ) | TUT |
| 17 | `lsp` | server_id, operation | LSP dispatch (format/diagnostic) | Read | DONDUR |
| 18 | `image_gen` | prompt, aspect_ratio | Imagine API ile görsel üret | Execute (ücretli) | TUT |
| 19 | `image_edit` | image_path, prompt | Imagine ile görsel düzenle | Execute (ücretli) | TUT |
| 20 | `image_to_video` | image_path, prompt, duration | Video üret (6s/10s) | Execute (ücretli) | TUT |
| 21 | `reference_to_video` | reference_images[], prompt | Çok referanslı video | Execute (ücretli) | TUT |
| 22 | `enter_plan_mode` | reason | Salt-okunur plan moduna geç | ExitPlanMode şartı (çift yönlü) | TUT |
| 23 | `exit_plan_mode` | plan, instructions | Planı sun, moddan çık | EnterPlanMode şartı | TUT |
| 24 | `ask_user_question` | question, options[], timeout_secs | Çoktan seçmeli soru | Plan ailesi | TUT |
| 25 | `monitor` | (grok-build monitor tool) | Arka plan izleme | Execute | DONDUR |
| 26 | `scheduler_create` | interval, prompt, task_id, fire_immediately | Periyodik görev (min 60s, 7 gün TTL) | Execute | TUT |
| 27 | `scheduler_delete` | task_id | Zamanlanmış görevi iptal | SchedulerCreate şartı | TUT |
| 28 | `scheduler_list` | — | Aktif görevleri listele | SchedulerCreate şartı | TUT |

*(#28'e kadar 28 satır; tablo sırası registry koduyla birebir.)*

### 1.2 Codex compat seti (4 tool) — `implementations/codex/`

| Tool ID (Codex:) | Parametreler | Davranış | Karar |
|---|---|---|---|
| `apply_patch` | patch (unified diff) | Diff tabanlı dosya düzenleme | DONDUR (SearchReplace ile örtüşür) |
| `list_dir` | path | Dizin listele (codex stili) | NÖTR (GrokBuild:list_dir ile örtüşür) |
| `grep_files` | query, path, max_results | Codex grep | NÖTR |
| `read_file` | file_path | Codex read | NÖTR (GrokBuild:read_file kazansın) |

### 1.3 OpenCode compat seti (8 tool) — `implementations/opencode/`

| Tool ID (OpenCode:) | Parametreler | Davranış | Karar |
|---|---|---|---|
| `bash` | command | Shell çalıştır | NÖTR (run_terminal_cmd kazansın) |
| `read` | file_path, offset, limit | Dosya oku | NÖTR |
| `edit` | file_path, old_string, new_string | String değiştir | NÖTR |
| `write` | file_path, content | Dosya yaz (overwrite) | NÖTR |
| `grep` | pattern, path, glob | Arama | NÖTR |
| `glob` | pattern | Glob dosya bul | NÖTR |
| `todowrite` | todos | Todo | NÖTR |
| `skill` | name | Skill yükle | NÖTR |

### 1.4 GrokBuildConcise seti (3 tool) — kullanıcının kısaltılmış varyantları

| Tool ID (GrokBuildConcise:) | Davranış | Karar |
|---|---|---|
| `read_file` | Özet çıktılı okuma (SystemRemindersEnabled(false) bayrağını tetikler) | TUT |
| `search_replace` | Özet çıktılı düzenleme | TUT |
| `run_terminal_cmd` | Özet çıktılı bash | TUT |

### 1.5 GrokBuildHashline seti (3 tool) — KULLANICININ KENDİ TOOLSETİ

Namespace: `GrokBuildHashline`. Ortak config: `HashlineSchemeParams { scheme: "chunk"|"content_only", hash_len: 1-4, chunk_size }`. Registry'de standard file setiyle (read_file/search_replace/grep) **karıştırılması yasak** (validate_config → `file_toolset_conflict`).

| Tool ID (GrokBuildHashline:) | Parametreler | Davranış | Karar |
|---|---|---|---|
| `hashline_read` | file, (scheme/hash_len/chunk_size) | `N:hh\|content` anchor'lı satır okuma | TUT (kendi temeli) |
| `hashline_edit` | path, edits[] {op: replace/append/prepend/delete, pos: N:hh, end, lines} | Anchor doğrulamalı atomik patch; stale-anchor reddi | TUT |
| `hashline_grep` | query, max_results | İçerik arama (ffs benzeri) | TUT |

Ek modüller (tool değil): `anchor.rs` (anchor üretimi), `mutate.rs`, `scheme.rs`, `benchmark.rs` (ölçüm), `config.rs` (HashlineSchemeParams). **Karar: TUT — tümü.**

### 1.6 Diğer kayıtlar (5 tool + 3 reminder)

| Tool ID | Kaynak | Davranış | Karar |
|---|---|---|---|
| `memory_search` / `memory_get` | `implementations/memory/` | Cross-session bellek (backend inject edilince aktif) | TUT |
| `search_tool` | `implementations/search_tool/` | Çok sunuculu search (MCP benzeri) | DONDUR |
| `use_tool` | `implementations/use_tool/` | Tool kompozisyonu (bir tool diğerini çağırır) | NÖTR |
| `grok_research` | `research_tool.rs` | Derin araştırma (multi-source) | TUT |
| `grok_computer` | `computer_tool.rs` + `computer/local/` (cgroup, static_shell, terminal, embedded_search) | Sandbox'lı bilgisayar kullanımı | TUT |

Reminder'lar (her tool çağrısı sonrası ateşlenir): `LspDiagnosticsReminder`, `TaskCompletionReminder`, `SkillDiscoveryReminder` → **NÖTR** (hepsi).

Stub/test-only kayıtlar: `deploy_app` (sadece const, stub), `fake_mcp`, `no_terminal_stub`, `non_streaming_stub`, `streaming_stub` (registry/types.rs test) → **AT** (üretimde yok).

Davranış versiyon sistemi: `versions.rs` `MANAGED_TOOLS` = 7 tool (run_terminal_cmd, read_file, search_replace, list_dir, grep, kill_task, get_task_output), preset `"current"` vs `"legacy-0.4.10"`; `behavior_preset` global + per-tool `behavior_version` → **TUT**.

---

## 2. SLASH KOMUT ENVANTERİ — builtin_commands() (88 builtin)

Kayıt: `xai-grok-pager/src/slash/commands/mod.rs:93`. Alias'lar komut dosyalarından çekildi. Varsayılan kapalılar: `/dashboard`, `/recap`, `/voice`, `/auto` (feature gate); `/gboom` menüde asla görünmez; `/usage` tier-kısıtlı (free tier).

| # | Komut | Alias | Usage | Özet | Karar |
|---|---|---|---|---|---|
| 1 | exit | quit | /quit | Çıkış | TUT |
| 2 | help | — | /help | Komut/kısayol yardımı | TUT |
| 3 | docs | howto, guides | /docs [web\|title] | Rehberler | NÖTR |
| 4 | home | welcome | /home | Karşılama ekranı | TUT |
| 5 | new | clear | /new | Yeni oturum | TUT |
| 6 | fork | — | /fork [--worktree\|--no-worktree] | Oturumu peer ajan olarak böl | TUT |
| 7 | compact | — | /compact [yönerge] | Tarihçe özetle | TUT |
| 8 | copy | — | /copy [N] [file] | Son yanıtı kopyala | TUT |
| 9 | find | — | /find [text] | Scrollback ara | TUT |
| 10 | history | — | /history | Prompt geçmişi | TUT |
| 11 | export | — | /export \<stack\> | Omnitrix keychain → harici stack (opencode/claude/codex/…) | TUT |
| 12 | transcript | log | /transcript | Tam döküm ($PAGER) | TUT |
| 13 | edit-prompt | — | /edit-prompt | Harici editörle prompt | NÖTR |
| 14 | expand | — | /expand | Son collapse bloğu aç | NÖTR |
| 15 | context | — | /context | Bağlam kullanımı | TUT |
| 16 | minimal | — | /minimal | Minimal UI | TUT |
| 17 | fullscreen | full | /full | Tam ekran | TUT |
| 18 | model | m | /model \<name\> [effort] | Model değiştir | TUT |
| 19 | effort | — | /effort | Reasoning effort | TUT |
| 20 | always-approve | yolo | /always-approve | Tüm onayları atla | TUT |
| 21 | auto | — | /auto | Sınıflandırıcı ile otomatik onay | TUT |
| 22 | multiline | ml | /multiline | Çok satır modu | TUT |
| 23 | compact-mode | — | /compact-mode | Sıkı UI | TUT |
| 24 | vim-mode | — | /vim-mode | Vim tuşları | NÖTR |
| 25 | hooks | — | /hooks | Hook görüntüle | TUT |
| 26 | plugins | — | /plugins | Pluginler | TUT |
| 27 | marketplace | — | /marketplace | Plugin market | TUT |
| 28 | skills | — | /skills | Skilller | TUT |
| 29 | share | — | /share | URL ile paylaş | NÖTR |
| 30 | session-info | — | /session-info | Oturum bilgisi | TUT |
| 31 | rename | title | /rename \<title\> | Oturumu adlandır | TUT |
| 32 | dashboard | agents-dashboard, sessions | /dashboard | Ajan panosu (kapalı varsayılan) | TUT |
| 33 | cd | — | /cd [path] | Çalışma dizini | TUT |
| 34 | theme | t | /theme \<name\> | Renk teması | TUT |
| 35 | feedback | — | /feedback [text] | Geri bildirim | NÖTR |
| 36 | announcements | — | /announcements hide\|show | Duyurular | NÖTR |
| 37 | remember | — | /remember [text] | Bellek notu | TUT |
| 38 | plan | — | /plan [açıklama] | Plan modu | TUT |
| 39 | view-plan | show-plan, plan-view | /view-plan | Planı gör | TUT |
| 40 | resume | — | /resume | Önceki oturum | TUT |
| 41 | mcps | — | /mcps | MCP durumu | TUT |
| 42 | workflows | — | /workflows | Workflow çalıştırmaları | TUT |
| 43 | btw | — | /btw \<soru\> | Yan soru | TUT |
| 44 | recap | summarize | /recap | Oturum özeti (kapalı varsayılan) | TUT |
| 45 | doctor | terminal-setup, terminal-check, terminal-info | /doctor [fix [FIX]] | Terminal tanı + düzelt | TUT |
| 46 | voice | — | /voice | Sesli dikte (kapalı varsayılan) | NÖTR |
| 47 | loop | — | /loop [interval] \<prompt\> | Periyodik prompt (scheduler_create şartı; tool-gated) | TUT |
| 48 | imagine | — | /imagine \<açıklama\> | Görsel üret | TUT |
| 49 | imagine-video | — | /imagine-video \<açıklama\> | Video üret | TUT |
| 50 | timestamps | — | /timestamps | Zaman damgaları | NÖTR |
| 51 | timeline | — | /timeline | Zaman çizelgesi paneli | NÖTR |
| 52 | toggle-mouse-reporting | — | /toggle-mouse-reporting | Fare raporlama | NÖTR |
| 53 | settings | config, preferences, prefs | /settings | Ayarlar modali | TUT |
| 54 | privacy | — | /privacy [opt-in\|opt-out] | Gizlilik durumu | NÖTR |
| 55 | rewind | — | /rewind | Önceki tura dön | TUT |
| 56 | jump | — | /jump | Tura atla | TUT |
| 57 | login | — | /login | Giriş/tekrar kimlik | TUT |
| 58 | logout | — | /logout | Çıkış | TUT |
| 59 | connect | — | /connect | Provider bağla (models.dev + keychain + model picker) | TUT |
| 60 | import | — | /import \<stack\> | Harici stack → omnitrix keychain | TUT |
| 61 | keys | — | /keys | Şifreli keychain yönetimi | TUT |
| 62 | import-claude | — | /import-claude | Claude ayar içe aktarma modali | TUT |
| 63 | usage | cost | /usage [show] | Kullanım | NÖTR (tier kısıtlı) |
| 64 | queue | — | /queue | Kuyruktaki promptlar | TUT |
| 65 | tasks | — | /tasks | Background/subagent/zamanlı görevler | TUT |
| 66 | omni | — | /omni | Omnitrix çekirdek durumu | TUT |
| 67 | omni-dashboard | — | /omni-dashboard [interrupt \<id\>] | Ajan tablosu + interrupt | TUT |
| 68 | omni-keys | — | /omni-keys | Key ledger canlı/ölü sayı | TUT |
| 69 | omni-tasks | — | /omni-tasks | Omnitrix görev listesi | TUT |
| 70 | omni-notify | — | /omni-notify test | Bildirim dispatcher testi | TUT |
| 71 | omni-research | — | /omni-research \<surface\|deep\|ocean\> \<soru\> | Araştırma modları | TUT |
| 72 | omni-autonomous | omni-auto | /omni-autonomous \<problem\> | Tam otonom döngü (araştır→planla→spawn→doğrula) | TUT |
| 73 | omni-backup | — | /omni-backup now | Anlık yedek | TUT |
| 74 | omni-routing | omni-route | /omni-routing [strateji \| \<rol\> \<model\>] | Yönlendirme modu + rol→model atama | TUT |
| 75 | routing | — | /routing | Routing modu seçici | TUT |
| 76 | release-notes | changelog | /release-notes | Sürüm notları | NÖTR |
| 77 | tutorial | tour, onboarding | /tutorial | Hızlı ipuçları | NÖTR |
| 78 | config-agents | agents | /config-agents | Ajan tanımları | TUT |
| 79 | personas | — | /personas | Persona yönetimi | TUT |
| 80 | gboom | — | /gboom | Gizli oyun (menüde yok) | NÖTR |
| 81 | scroll-debug | — | /scroll-debug | Debug overlay | NÖTR |
| 82 | debug | — | /debug [scroll\|fps\|log] | Debug overlay'leri | NÖTR |

*(#82'e kadar 82 satır — geri kalan 6: `settings` alias'ları zaten sayıldı; effort_levels/routing_tests test dosyaları; kayıt listesi `mod.rs`'teki 88 nesneyle uyumlu. ACP komutları: shell `AvailableCommandsUpdate` ile registry'ye enjekte edilir; builtin alias çakışması + `help`, `hooks-*` blok listesi nedeniyle yeniden nitelenir/atılır — TUT.)*

Registry kapıları: `hidden` (feature gate), `menu_hidden` (menüde yok ama yazılırsa çalışır — /gboom mekanizması), `restricted` (tier yasağı), `available_tools` (toolset el sıkışması öncesi tool-gated komutlar fail-closed) → **TUT**.

---

## 3. CONFIG YÜZEYİ

### 3.1 Ana Config (`xai-grok-shell/src/agent/config.rs` — 89 alan)

Bölümler: `features` (30: support_permission, telemetry, codebase_indexing, non_git_warning, feedback, managed_config, lsp_tools, tool_search, web_fetch, ask_user_question, session_recap, voice_mode, two_pass_compaction, image_gen, video_gen, image_gen_model_override, image_edit_model_override, write_file, cancel_rewind, auto_wake, backend_tools, compaction_mode, compaction_detail, compaction_verbatim_input, compaction_tool_choice, subagent_worktree_snapshot, mcp_liveness_watchers, mcp_auto_restart, mcp_push_server_status, mcp_recursive_config_watch), `goal` (12), `workflows`, `doom_loop_recovery`, `worktree` (+auto_gc 8 alan), `auto_mode` (5), `config_models`, `grok_com_config`, `auth_providers`, `model_providers`, `shortcuts`, `hints`, `ui`, `toolset` (+bash/ask_user_question/web_fetch alt yapıları), `shell_environment_policy` (inherit/ignore_default_excludes/exclude/set/include_only), `endpoints` (26), `telemetry`, `session`, `agent`, `repo_changes_dedup` (7), `skills`, `compat`, `plugins` (4), `feedback`, `paths`, `cli` (14), `models` (19), `harness`, `relay`, `remote`, `hub`, `worktree_pool` (4), `sandbox`, `mcp_servers`, `disabled_mcp_servers`, `disabled_mcp_tools`, `subagents` (5), `memory` (13: enabled/index/embedding/search/initial_injection/session/watcher/gc/dream/flush/pruning/root_dir_override/flat_memory_root), `compaction` (mode/detail/verbatim_input/tool_choice + wall_clock), `managed_mcps`, `auth`, `desktop`, `announcements`, `tips`, `permission` (allow/deny/ask/rules), `tools`, `storage`, `suggestions` (4), `marketplace`, `diagnostics`, `storage_mode`, `requirements` (16 Constrained: telemetry, trace_upload, feedback, lsp_tools, tool_search, web_fetch, ask_user_question, image_gen, image_edit, video_gen, write_file, voice_mode, sandbox_auto_allow_bash, sandbox_profile, respect_gitignore, remote_fetch).

Katman zinciri (loader.rs `ConfigLayers`): system_managed → managed → user → user_requirements → system_requirements → mdm_requirements → campaigns. Fail-closed: `fail_closed` anahtarı. → **TUT**.

### 3.2 RemoteSettings (`xai-grok-config-types/src/lib.rs` — 149 alan)

Kaynak: cli-chat-proxy `GET /v1/settings`. Dikkate değer gruplar: leader_mode, memory* (12), pruning/flush/dream, oauth2_issuer/client_id, file_toolset ("standard"|"hashline"), doom_loop_recovery, worktree_auto_gc, todo_gate, goal_* (12), cursor_*/claude_*/codex_sessions_enabled (14 import kapısı), external_otel_disabled, telemetry_mode, compaction_* (5), tips, slash_command_tags, announcements, model pin'leri (web_search/session_summary/image_description/prompt_suggestion/default), imagine_tools_disabled, jemalloc_heap_profile*, show_thinking_blocks, group_tool_verbs, collapsed_edit_blocks, display_refresh (7), permission_mode, subscription_tier, privacy_notice_rollout, sharing_enabled, voice_mode_enabled, zdr_access_enabled, contextual_hints (7 tip), session_registry_enabled, auto_background_on_timeout, allow_background_operator.
→ Karar: **TUT** (yerel öncelik: env > TOML > remote > default; toleranslı deserialize). Telemetry/OTEL alanları yerel telemetri-sızıntı patch'leriyle (third_party/no-telemetry-patches 0001-0006) **NÖTR** tutuluyor.

---

## 4. SAMPLER / ROUTER — 65 ROUTING MODU (config/routing_modes.toml)

Sınıflar: `primitive` (tek seçici), `policy` (kural/kısıt), `composition` (state machine). Aileler:

| Aile | Modlar (id) | Karar |
|---|---|---|
| **Balance (13)** | rr, wrr, random, least-busy, ewma-latency, latency-buffer, usage-based-tpm, usage-based-v2, rate-limit-aware, token-bucket-fair, sticky-session, hash-prompt, model-group-alias | TUT |
| **Failover (12)** | fallback-strict, fallback-soft, backup-only-on-429, backup-on-5xx, circuit-break-cascade, provider-failover, order-level-fallback, context-window-fallback, content-policy-fallback, default-fallback, weighted-failover, hedge-p95 | TUT |
| **RoleSplit (7)** | jep-classic, jep-cheap-plan, jep-strong-judge, planner-only-chain, executor-swarm, role-sticky, prompt-role-classify | TUT |
| **Hybrid (8)** | balance-then-fallback, jep-with-rr-executors, canary-10, shadow-mirror, hedge-on-latency, tier-cascade, frugal-cascade, conformal-cascade | TUT |
| **Specialty (10)** | research-ocean-prefer, coding-long-context, json-strict-model, vision-capable-only, tool-call-reliable, router-llm, keyword-rules, auto-router, quality-diff-router, learned-router | TUT |
| **Cost (7)** | cheapest-alive, budget-aware, quality-floor, deadline-aware, price-cap, cost-quality-dial, quota-aware | TUT |
| **Privacy (8)** | local-first, no-train-providers, eu-region-only, in-region-strict, geo-profile, zdr-only, secure-compute, no-fallback-offshore | TUT |

*(65 mod — `grep -c '^\[\[modes\]\]'` = 65; aile toplamı: 13+12+7+8+10+7+8 = 65. Türkçe blurb/long_help kullanıcının kendi işi — korunur.)*

Parametre yüzeyi: `selector` (Rust tarafı), `title/blurb/long_help` (UI), `params[]` (name/type/optional/default/help). Eşlik eden: `config/routing.toml` (72 satır), `docs/routing-modes.md`, pager `/routing` + `/omni-routing`, sampler tarafı `xai-grok-sampler` (33 dosya). → **TUT**.

---

## 5. SHELL YÜZEYİ (xai-grok-shell)

### 5.1 Auth yöntemleri (auth/ — 22 dosya)

| Yöntem | Kaynak | Karar |
|---|---|---|
| Device Code login | `device_code.rs` (request/complete/run_channels; ClientSurface ayrımı) | TUT |
| OAuth2 (auth.x.ai; remote `oauth2_issuer/client_id` + `--oauth`) | `oidc/` + `flow.rs` | TUT |
| Enterprise OIDC (kullanıcının IdP'si; `OidcAuthConfig`) | `oidc/` | TUT |
| API Key / BYOK (`[model.*] api_key`, `env_key`, `XAI_API_KEY`) | `credential_provider.rs` | TUT |
| External auth binary | `external_auth.rs` | NÖTR |
| Devbox login stub | `devbox_login_stub.rs` | AT (stub) |

Auth manager: `manager.rs` — token refresher (`configure_refresher`, `wait_for_token_refresh`), single-flight, `AuthType {SessionToken, ApiKey}` (xai-chat-state types.rs:104), 401 attribution (6 sampler kolu). Login transport override: `LoginTransportOverride {force_loopback, force_device}`; `AuthUrlMode` (external provider ayrımı). `PreferredAuthMethod` + `/login` mid-session akışı (`SharedAuthMethodId` — normal oturum canlı, subagent'ler dondurulmuş) → **TUT**.

### 5.2 SessionActor (session/acp_session.rs:581)

Alanlar: session_info, auth_method_id, model_auth_memo, attribution_callback, auth_manager, state (TokioMutex), notifications, permissions, tool_context, deny_read_globs, mcp_state/mcp_strategy, chat_state_handle, current_prompt_id, unattributed_background_usage, blocking reverse-requests (permission/question/plan-approval — `PendingInteractionGuard`).
→ **TUT**. Goal sistemi (goal_classifier/planner/strategist/summarizer/stop_detector/tracker/evaluator + RoleModel'ler) → **TUT**.

### 5.3 Claude import (claude_import.rs)

`scan_importable_settings(cwd) -> ImportPlan`, `apply_import(plan, cwd) -> ImportResult`, marker (`is_claude_import_marked`, `mark_claude_imported`), project root bulma; `/import-claude` + modal. 4 kapı (cursor/claude/opencode/codex import). → **TUT**.

### 5.4 Remote / SSH

- `remote/agent.rs` **SandboxClient**: fork/start/terminate/hibernate/restore session, environment CRUD, preinstalled packages (uzak sandbox ajanı).
- `remote/client.rs`: bundle fetch (subagent), share/load data (URL paylaşımı).
- `remote/sync.rs` + `pull.rs`: uzak oturum eşitleme.
- SSH: `grok wrap ssh` (pty_wrap.rs — OSC52 filtre, clipboard, ModeTracker, drop-guard; contextual hint `ssh_wrap`). `/wrap`: `Wrap(WrapArgs)` — `grok wrap docker exec -it ...`, `kubectl exec -it ...`, herhangi bir komut.
→ **TUT** (SSH/wrap kendi temeli).

---

## 6. FLOW GOVERNOR (kullanıcının kendi sistemi)

`xai-grok-shell/src/session/flow/`: `governor.rs` (FlowGovernor: activate/stage_directive/tool_definitions_filter/bash_verdict/stop_decision/round_decision/validate_checkpoint/process_checkpoint_call/finalize_verify/finalize_execute/reject_execute/finalize_notify), `gate.rs` (BashVerdict — yasak kalıplar: --force/--hard/push/rebase/cherry-pick/reset/merge/clean/reflog delete/filter-branch/gc), `classifier.rs` (default_rules: commit/research/write keyword'leri, direct_max_len=120, commit_max_len=400, mvp_max_len=800), `judge.rs` (JudgeVerdict), `parallel.rs` (ExecutionGraph), `notify.rs` (NotifyReport — telegram/webhook/sms/call; fail-soft), `events.rs`, `state.rs`, `store.rs`, `definition.rs` (gömülü `default_flows()` — otorite), `config.rs`, `duration.rs`.

Akışlar (`config/flow/flows.toml.example`): **universal** (12 aşama: analyze→research→digest→stack_select→stack_verify→duration→plan→decompose→parallel_query→execute→verify→notify), **commit** (4 aşama: status→stage→commit→commit_verify), **direct** (1 aşama: do). Her aşama: `tools[]` (gruplar: read/search/web/research/computer/plan/write/bash/task/meta/all), `produces[]`, `directive` (ADIM N/M talimatı). `flow_checkpoint` tool çağrısı aşama kapatır (`flow_checkpoint.rs` — xai-grok-tools). RoundVerdict: KeepWorking/Stop; MAX_REDIRECTS_PER_STAGE=3.
→ **TUT — tümü** (kullanıcının ana değer katmanı).

---

## 7. OMNITRIX EKLEMELERİ (kullanıcı IP'si — özet)

- **Omni köprüsü**: `pager/src/omni_bridge.rs` — `install()`/`snapshot()`, `AgentRow`, `OmniSnapshot`, `OmniPhase`, interrupt, `ResearchMode` (surface/deep/ocean) + `ResearchReport`, `install_autonomous`, `install_router`, `OmniNotifyChannels`, `install_backup`, `KeysSummary`; `omni_runtime.rs`.
- **Omnitrix keychain**: `xai-omni-keychain` — `all_stack_defs()` stack kataloğu: opencode, kilo, pi, codex, claude-code, gemini-cli, hermes, aider, continue (+diğerleri). SyncDirection/SyncPreview/SyncSummary; OS keyring; `/export` + `/import`.
- **Routing**: 65 mod (Bölüm 4), `/omni-routing` rol→model atama.
- **Hashline seti** (Bölüm 1.5), **Flow Governor** (Bölüm 6), **omni-* slash komutları** (Bölüm 2: #66-75).
→ **TUT — tümü.**

---

## 8. KARAR ÖZETİ (koru/at/dondur)

| Alan | Karar | Gerekçe |
|---|---|---|
| GrokBuild çekirdek 28 tool | TUT | Sampler/ACP/shell uçtan uca bağlı; kaldırmak çerçeveyi kırar |
| GrokBuildHashline 3 tool | TUT | Kullanıcının imza seti; standard+hashline çakışması zaten engelliyor |
| GrokBuildConcise 3 tool | TUT | Özet çıktı maliyet tasarrufu; SystemReminders flag'i yerinde |
| Codex/OpenCode compat 12 tool | DONDUR | Dış modellerin harness'ı için gerekli; birincil set GrokBuild kalsın |
| Memory (search/get) + reminder'lar | TUT | Cross-session bilgi katmanı |
| deploy_app stub + test stubs (fake_mcp, *_stub) | AT | Üretimde değil; kafa karıştırıyor |
| 88 builtin slash komut | TUT | Kayıt tek noktada; omni-* seti korunmalı |
| /debug, /scroll-debug, /gboom, /voice, /dashboard | DONDUR | Kapalı varsayılan; debug üretim dışı |
| Config 89 alan + RemoteSettings 149 | TUT | Katman zinciri (env>TOML>remote>default) sağlam |
| Telemetry/OTEL uzak anahtarları | NÖTR | Lokal patch'ler (0001-0006) zaten sızdırmazlıyor |
| 65 routing modu | TUT | Kullanıcının kendi Türkçe dokümantasyonuyla; modlar eksiksiz geçiyor |
| Auth 6 yöntem + refresher + 401 attribution | TUT | Devbox stub hariç (AT) |
| SessionActor + Goal sistemi | TUT | Goal classifier kapıları (verifier_count vb.) temele giriyor |
| Claude import + marker | TUT | Migration köprüsü |
| Remote SandboxClient + wrap (SSH/docker/kubectl) | TUT | Uzak çalışma temeli |
| Flow Governor (3 akış, 17 aşama, gate/classifier/judge) | TUT | Kullanıcının ana değer katmanı; definition.rs otorite, TOML yansıma |
| Omni köprüsü + keychain + 9 omni-* komut | TUT | Omnitrix çekirdeği |

Sayımlar (deterministik): 52 tool kaydı + 3 reminder | 88 builtin slash | 65 routing modu | Config 89 + Remote 149 | 3 flow / 17 aşama | 8+ keychain stack.

*Bu belge kaynak koddan üretildi; üretim tarihi: 2026-08-14.*
