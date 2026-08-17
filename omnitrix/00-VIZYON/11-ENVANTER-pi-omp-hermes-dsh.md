# 11 — ENVANTER: pi · oh-my-pi · hermes · deepseek-harness

> Tarih: 2026-08-14 · Yöntem: tool registry/katalog dosyalarının **tamamı** enumerate edildi (keyword örnekleme yok).
> Yargı etiketi: **GOOD** = çalışır / değerli · **BAD** = lobotomi (eksik/bozuk/tehlikeli) · **NÖTR** = taşınabilir ama tartışmalı.

## 0. Kapsam kanıtı (per-harness sayım)

| Harness | Kaynak | Sayım | Doğrulama noktası |
|---|---|---|---|
| pi | `pi/packages/coding-agent/src/core/tools/index.ts:83` | **7 built-in tool** + 22 slash komut + 29 extension event + ~45 config anahtarı | `ToolName` union: `read, bash, edit, write, grep, find, ls`; `BUILTIN_SLASH_COMMANDS`; `ExtensionAPI.on()` overloadları |
| oh-my-pi | `oh-my-pi/packages/coding-agent/src/tools/builtin-names.ts:1` | **29 built-in + 3 hidden tool** (32), 14 LSP op, 28 DAP op, 70 slash komut, 16 `://` şema, 3 magic keyword, 49 settings anahtarı | `BUILTIN_TOOL_NAMES` (29) + `HIDDEN_TOOL_NAMES` (yield/goal/think); `lsp/types.ts:10`; `tools/debug.ts` case'leri; `internal-urls/*-protocol.ts` `scheme=` |
| hermes | `hermes/hermes-agent/tools/*.py` `registry.register(...)` | **87 kayıtlı tool** (46 dosyada), 24 gateway platform + plugin'ler, ~20 config bloğu, skill lifecycle: active/stale/archived | `grep -A8 registry.register` → `name=` unique = 87; `gateway/config.py:317 Platform` enum; `tools/skill_usage.py:53` |
| deepseek-harness | `deepseek-harness/docs/tool-catalog.md` | **52 katalog satırı / 53 model-görünür tool adı** + 55 cordis service seam + 105 config'li paket | `tool-catalog.md` `###` başlıkları; `capability-seams.md` `svc_` node'ları; `config-catalog.md` `##` başlıkları |

---

# 1. pi (earendil-works/pi)

Kaynak: `pi/packages/` — agent (harness core), coding-agent (CLI/TUI/extension API), ai (provider), protocol (RPC), server, session-backends, telemetry, tui.

## 1.1 Built-in tool'lar — 7 adet (tamamı GOOD)

| Tool | Parametre (schema) | Davranış | Yargı |
|---|---|---|---|
| `read` | `path` | Dosya/URL okuma; `TruncationOptions` (maxBytes/maxLines), image mime algılama, binary tespiti | **GOOD** — satır numarası + özet çıktı, truncate head/tail |
| `bash` | `command`, `timeout?`, env | Bash yürütme; `BashSpawnHook` ile spawn interception, PTY desteği, timeout kill | **GOOD** — `bash-executor.ts` ayrı, extension hook'lu |
| `edit` | `path`, `oldString`, `newString`, fuzzy | Diff tabanlı düzenleme (`edit-diff.ts`); LineEnding koruma, fuzzy match | **GOOD** — diff önizleme + hata mesajları |
| `write` | `path`, `content` | Dosya yazma; `file-mutation-queue` ile ardışık yazım kuyruğu | **GOOD** |
| `grep` | `pattern`, path | ripgrep tabanlı içerik arama; structured çıktı, .gitignore saygısı | **GOOD** |
| `find` | glob | Dosya adı arama (glob) | **GOOD** |
| `ls` | `path` | Dizin listeleme | **GOOD** |

> Not: `core/tools/edit-diff.ts`, `image.ts`, `truncate.ts`, `output-accumulator.ts` tool DEĞİL, yardımcı modüldür. MCP tool yok (extension ile eklenir). `defaultTools` ayarı boşsa aktif set: `read, bash, edit, write` (`core/sdk.ts:252`).

## 1.2 Slash komutları — 22 adet (tamamı GOOD)

`core/slash-commands.ts:20` — `settings, model, scoped-models, export, import, share, copy, name, session, changelog, hotkeys, fork, clone, tree, trust, login, logout, new, compact, resume, reload, quit` — hepsi çalışır; skill'ler `/skill:name` olarak register olur (`docs/skills.md:75`), `!` bash kısayolu ayrı.

## 1.3 Extension API event'leri — 29 adet (tamamı GOOD)

`core/extensions/types.ts` `ExtensionAPI.on()`: `project_trust, resources_discover, session_start, session_info_changed, session_before_fork, session_compact, session_shutdown, session_before_tree, session_tree, context, before_provider_headers, after_provider_response, before_agent_start, agent_start, agent_end, agent_settled, turn_start, turn_end, message_start, message_update, message_end, tool_execution_start, tool_execution_update, tool_execution_end, model_select, thinking_level_select, tool_call, tool_result, user_bash, input` (30 satır; `session_before_switch` ayrıca tanımlı ama `on()` overload listesinde yok).

Ek extension API yüzeyi: `registerTool`, `registerCommand`, `registerProvider`, `registerNativeProvider`, `setSessionName/getSessionName`, `newSession/switchSession/fork`, `ui.dialog` (select/confirm/input), `setWidget`, `onTerminalInput`, compaction özelleştirme (`custom-compaction`), bash spawn hook, markdown transform. Örnekler `examples/extensions/` (90+ dosya).

## 1.4 Config anahtarları — `core/settings-manager.ts` (tamamı GOOD)

Üst seviye: `setupVersion, autoResume, shellPath, extensions, enabledModels, disabledProviders, disabledExtensions, modelRoleStorage, modelRoles, modelTags, modelProviderOrder, cycleOrder, symbolPreset, colorBlindMode, showHardwareCursor, defaultThinkingLevel, hideThinkingBlock, proseOnlyThinking, omitThinking, externalThinking, inlineToolDescriptors, includeModelInPrompt, includeWorkspaceTree, personality, temperature, topP, topK, minP, presencePenalty, repetitionPenalty, textVerbosity, steeringMode, followUpMode, interruptMode, doubleEscapeAction, treeFilterMode, autocompleteMaxVisible, emojiAutocomplete, readLineNumbers` (49 UI anahtarı).

Nested bloklar: `compaction` (enabled/reserveTokens/keepRecentTokens), `branchSummary` (reserveTokens/skipPrompt), `retry` (enabled/maxRetries/baseDelayMs/provider), `images` (showImages/imageWidthCells/clearOnShrink/autoResize/blockImages), `thinkingBudgets` (minimal/low/medium/high), `markdown` (codeBlockIndent/mermaid), `terminal` (transport), `warnings`, `packages` (npm/git kaynak), `enableSkillCommands`.

---

# 2. oh-my-pi (can1357 fork — pi-mono üstü)

Kaynak: `oh-my-pi/packages/coding-agent/src/`.

## 2.1 Built-in tool'lar — 29 görünür + 3 hidden (RAPOR "31" der; kaynak 32)

`tools/builtin-names.ts:1` `BUILTIN_TOOL_NAMES` (29) + `HIDDEN_TOOL_NAMES` = `yield, goal, think` (3).

| # | Tool | Parametre (özet) | Davranış | Yargı |
|---|---|---|---|---|
| 1 | `read` | `path` (local/internal URI/URL, inline selector) | Dosya/URL/dizin okuma; markit dönüşümleri, PDF/SQLite/archive; `memory:// skill://` URI; conflict `⚠ N` rozeti | **GOOD** |
| 2 | `bash` | `command, env?, timeout?, cwd?, pty?, async?` | In-process brush bash; PTY (sudo/ssh), async job → `hub` job sistemi | **GOOD** |
| 3 | `edit` | hashline modu `{input}` + replace modu `{path, old_string, new_string, replace_all}` + apply-patch | **Content-hash anchor'lı düzenleme; stale anchor REDDEDİLİR** (hashline); fuzzy; diff; snapshot store | **GOOD** — en güçlü özellik |
| 4 | `ast_grep` | `pat, lang?, skip?` | AST pattern araması (tree-sitter/pi-ast) | **GOOD** |
| 5 | `ast_edit` | `ops[{pat,out}], path?` | AST şablonuyla yapısal yeniden yazım | **GOOD** |
| 6 | `ask` | `label, description?, preview?` | Kullanıcıya yapılandırılmış seçenek picker (turn ortasında) | **GOOD** |
| 7 | `debug` | `program?, args?, cwd?` + **28 op** (aşağıda) | DAP sürüşü: lldb/dlv/debugpy; frame okuma, goroutine yürüyüşü, pause/inspect | **GOOD** |
| 8 | `eval` | `lang, code, timeout?, reset?, title?` | Kalıcı Python/Bun/Julia/Ruby kernel'ları; loopback tool köprüsü | **GOOD** |
| 9 | `github` | `op` (11 op: repo_view/file_read/pr_create/pr_checkout/pr_push/search_issues/search_prs/search_code/search_commits/search_repos/run_watch), repo/branch/pr/title/body/labels… | GH entegrasyonu; PR checkout, action run watch | **GOOD** |
| 10 | `glob` | pattern | In-process pi-walker glob (scan cache paylaşımlı) | **GOOD** |
| 11 | `grep` | pattern | In-process ripgrep | **GOOD** |
| 12 | `lsp` | **14 op** (aşağıda) | LSP mux: diagnostics/definition/references/hover/symbols/rename/… | **GOOD** |
| 13 | `inspect_image` | path | Görsel inceleme; vision-capable modelde otomatik gizleme | **GOOD** |
| 14 | `browser` | `action(open/close/run), name?, url?, code?, dialogs?` + app{path,cdp_url,relay,args,target} | Headless Chromium + stealth (13 stealth patch) + kullanıcı tab'ı relay | **NÖTR** — güçlü ama ağır/riskli |
| 15 | `computer` | `run, timeout?` | Native masaüstü kontrolü (işletim sistemi tıklama/yazma) | **NÖTR** — riskli yüzey |
| 16 | `checkpoint` | `goal` | Soruşturma checkpoint kaydı | **GOOD** |
| 17 | `rewind` | `report` | checkpoint'e geri sarma (snapshot store) | **GOOD** |
| 18 | `security_scan` | `action` (9 op: preflight/start/status/cancel/validate/cloud_scans/cloud_start/cloud_status/cloud_pull), target_kind, include/exclude_paths, base/head_revision | Native güvenlik taraması (Codex Security uyumlu); SARIF import/export | **GOOD** |
| 19 | `task` | `task, agent?, name?, outputSchema?, schemaMode?, isolated?, effort?` / batch `{context, tasks[]}` | İzole worktree'de paralel subagent fan-out; typed schema-doğrulamalı sonuç; revive/kill | **GOOD** |
| 20 | `hub` | `op` (12 op: send/wait/inbox/list/jobs/cancel/start/ps/logs/stop/restart/describe), to/message/await/from | Agent'lar arası mesajlaşma + background job yönetimi + irc | **GOOD** |
| 21 | `todo` | `op` (init/start/done/rm/drop/block/unblock/append/view), list[{phase,items}] | Phased todo listesi | **GOOD** |
| 22 | `web_search` | `query, recency?, limit?, max_tokens?, temperature?, num_search_results?` | 23 sağlayıcı zinciri (Exa/Tavily/Perplexity/…) | **NÖTR** — sağlayıcı çokluğu şişkinlik |
| 23 | `write` | `path, content` | Dosya yazma; `xd://` transport köprüsü | **GOOD** |
| 24 | `memory_edit` | — | Memory dosyası düzenleme | **GOOD** |
| 25 | `retain` | `items[{content, context?}]` | Kalıcı bellek yazma | **GOOD** |
| 26 | `recall` | `query` | Bellek arama | **GOOD** |
| 27 | `reflect` | `query, context?` | Bellek üzerinde düşünme (LLM sentez) | **GOOD** |
| 28 | `learn` | `memory, context?, skill?{name, body?}` | Ders/skill öğrenme | **GOOD** |
| 29 | `manage_skill` | `name, description?, body?, action?` | Skill CRUD (SKILL.md) | **GOOD** |
| 30 | `yield` (hidden) | — | Agent kontrolünü bırakma | **GOOD** |
| 31 | `goal` (hidden) | — | Goal durumu | **GOOD** |
| 32 | `think` (hidden) | — | Dış düşünce desteği | **GOOD** |

Legacy alias: `search→grep`, `find→glob`. MCP tool'lar `mcp__<server>_<tool>` prefix'li.

## 2.2 LSP operasyonları — 14 adet (`lsp/types.ts:10`)

`diagnostics, definition, references, hover, symbols, rename, rename_file, code_actions, type_definition, implementation, status, reload, capabilities, request`
→ `rename` = `workspace/willRenameFiles` üzerinden re-export/barrel/alias güncelleme; linter client'lar: biome/swiftlint/LSP-linter. **TAMAMI GOOD.**

## 2.3 DAP operasyonları — 28 adet (`tools/debug.ts` case'leri)

`launch, attach, set_breakpoint, remove_breakpoint, set_instruction_breakpoint, remove_instruction_breakpoint, data_breakpoint_info, set_data_breakpoint, remove_data_breakpoint, continue, step_over, step_in, step_out, pause, evaluate, stack_trace, threads, scopes, variables, disassemble, read_memory, write_memory, modules, loaded_sources, custom_request, output, terminate, sessions`
→ Adapter'lar: lldb, dlv (Go), debugpy (Python), rdbg (Ruby). **TAMAMI GOOD.**

## 2.4 Slash komutları — 70 üst seviye (top-level, `builtin-*.ts`)

`force, live, pause, quit` (control) · `todo, session, jobs, usage, stats, changelog, hotkeys, tools, context, extensions, agents, branch, fork, tree, login, logout, mcp` (session; mcp'nin 18 subcommand'ı: add/list/remove/test/reauth/unauth/enable/disable/smithery-search/smithery-login/smithery-logout/reconnect/reload/resources/prompts/notifications/help) · `ssh, new, fresh, clear, drop, compact, shake, handoff, resume, btw, tan, omfg, retry, debug, memory, rename, move, add-dir, remove-dir, dirs, exit` (lifecycle) · `security, settings, setup, plan, plan-review, vibe, goal, guided-goal, loop, queue, model, switch, fast, computer, vision, prewalk` (modes) · `advisor, export, dump, share, collab, join, leave, browser, copy` (collaboration) · `marketplace, plugins, reload-plugins` (marketplace). **TAMAMI GOOD.**

## 2.5 İç `://` şemaları — 16 adet (`internal-urls/`)

Kayıtlı 15: `agent://, artifact://, history://, issue://, pr://, local://, mcp://, memory://, omp://, rule://, security://, skill://, ssh://, vault://, xd://` (`*-protocol.ts` `readonly scheme =` ×15) + **`conflict://`** (merge URL — `write({path:"conflict://<id>"})` ile çözümleme, `read conflict://N/<scope>`). **TAMAMI GOOD.**

## 2.6 Magic keywords — 3 adet (`modes/magic-keywords.ts`)

`ultrathink` (derin düşünce modu), `orchestrate` (çoklu ajan orkestrasyonu), `workflowz` (workflow tetikleme) — standalone prose olarak tespit, gradient highlight. **GOOD.**

## 2.7 Sistem mekaniği (tema sayısı DEĞİL)

| Mekanik | Kaynak | Yargı |
|---|---|---|
| Hashline patch (content-hash anchor, stale reddi) | `edit/hashline/` | **GOOD** — omnitrix'e port adayı #1 |
| In-process natives (pi-natives: ripgrep/glob/walker + brush bash + 58 CLI aracı) | `packages/natives/` | **GOOD** |
| Advisor watchdog (ikinci model her turn'ü okur) | `advisor/` | **GOOD** |
| Rulebook (regex eşleşmesinde mid-token abort + kural enjeksiyonu) | `capability/` | **GOOD** |
| Task fan-out + Agent Hub (Alt+A) | `task/`, `tools/hub/` | **GOOD** |
| Collab relay (QR, join, read-only link) | `collab/` | **GOOD** |
| Memory backend'ler: local SQLite / Mnemopi / Hindsight | `memory-backend/`, `mnemopi/`, `hindsight/` | **GOOD** |
| 8 format native config okuma (Cursor MDC, Cline, Codex, Copilot) | `discovery/` | **GOOD** |
| Provider: 60+, per-role fallback, round-robin credential | `config/`, `registry/` | **NÖTR** — şişkin ama işlevsel |
| snapcompact (bitmap kontekst sıkıştırma) | `packages/snapcompact/` | **GOOD** |
| `omp acp` (Agent Client Protocol) | `modes/acp/` | **GOOD** |

**BAD (LOBOTOMİ):** compaction `reserveTokens` default set edilmemiş (davranış belirsiz); 65+ tema + ~90 docs sayfası öğrenme yükü; browser/computer yüzeyleri anti-bot kötüye kullanım riski; benchmark iddiaları yalnız edit formatına odaklı (uçtan uca kanıt yok).

## 2.8 Config anahtarları — `config/settings-schema.ts` (49 üst seviye)

`setupVersion, auth.broker.url, auth.broker.token, autoResume, shellPath, extensions, enabledModels, disabledProviders, disabledExtensions, modelRoleStorage, modelRoles, modelTags, modelProviderOrder, cycleOrder, symbolPreset, colorBlindMode, showHardwareCursor, defaultThinkingLevel, hideThinkingBlock, proseOnlyThinking, omitThinking, externalThinking, inlineToolDescriptors, includeModelInPrompt, includeWorkspaceTree, personality, temperature, topP, topK, minP, presencePenalty, repetitionPenalty, textVerbosity, steeringMode, followUpMode, interruptMode, doubleEscapeAction, treeFilterMode, autocompleteMaxVisible, emojiAutocomplete, readLineNumbers` + gruplar: appearance/model/interaction/context/memory/files/shell/tools/tasks/providers. **TAMAMI GOOD.**

---

# 3. hermes (NousResearch/hermes-agent)

Kaynak: `hermes/hermes-agent/` — Python core + TypeScript UI.

## 3.1 Kayıtlı tool'lar — 87 adet (`tools/*.py` `registry.register(name=...)`)

| # | Tool | Parametre (özet) | Yargı |
|---|---|---|---|
| 1 | `read_file` | `path, offset?, limit?` (max 2000 satır/~100K char, next_offset; ipynb/docx/xlsx/PDF dönüşüm) | **GOOD** |
| 2 | `write_file` | `path, content, cross_profile?` — hash doğrulamalı `verified:true`, syntax check | **GOOD** |
| 3 | `patch` | `mode(replace\|patch), path, old_string, new_string, replace_all?, patch?` — 9 fuzzy strateji + V4A | **GOOD** |
| 4 | `search_files` | `pattern, target(content\|files), path?, output_mode?, max_results?, include_patterns?, exclude_patterns?` | **GOOD** |
| 5 | `terminal` | `command, background?, timeout?, workdir?, pty?` — VM/SSH/Docker/Daytona/Modal/Singularity backend | **GOOD** |
| 6 | `process` | — process ağacı yönetimi | **GOOD** |
| 7 | `read_terminal` | — GUI terminal görüntüsü | **GOOD** |
| 8 | `close_terminal` | — | **GOOD** |
| 9 | `open_preview` | — | **GOOD** |
| 10 | `read_window_below` | — | **GOOD** |
| 11 | `focus_pane` | — | **GOOD** |
| 12 | `web_search` | `query, limit?` (5-100) | **GOOD** |
| 13 | `web_extract` | `url` — markdown, PDF, 15K char bütçe + spill | **GOOD** |
| 14 | `vision_analyze` | `image_path` | **GOOD** |
| 15 | `image_generate` | — | **GOOD** |
| 16-21 | `bfl_flux3_text_to_video, bfl_flux3_image_to_video, bfl_flux3_keyframes_to_video, bfl_flux3_video_continuation, bfl_flux3_get_result, bfl_flux3_prompting_guide` | FLUX3 video üretimi | **GOOD** |
| 22 | `video_generate` | — | **GOOD** |
| 23 | `video_analyze` | — | **GOOD** |
| 24 | `xai_video_edit` | — | **GOOD** |
| 25 | `xai_video_extend` | — | **GOOD** |
| 26-38 | `browser_navigate, browser_snapshot, browser_click, browser_type, browser_scroll, browser_back, browser_press, browser_get_images, browser_vision, browser_console, browser_cdp, browser_dialog, browser_exec` | Camoufox/CDP/browser-use backend'leri | **NÖTR** |
| 39 | `text_to_speech` | — elevenlabs/gemini/openai/xai | **GOOD** |
| 40 | `skills_list` | — | **GOOD** |
| 41 | `skill_view` | `name` | **GOOD** |
| 42 | `skill_manage` | `action(create/edit/patch/delete/archive/restore), name, content, old_string, new_string, replace_all?, file_path?, pin?` | **GOOD** — frontmatter doğrulama, ≤15KB, güvenlik taraması |
| 43 | `todo` | `todos[{id,content,status}], merge?` | **GOOD** |
| 44 | `memory` | `operations[{action(add/replace/remove), content?, old_text?}], target(memory\|user)` — atomik batch, §-ayraçlı, 2200/1375 char limit | **GOOD** — en değerli pattern |
| 45 | `session_search` | — | **GOOD** |
| 46 | `clarify` | — | **GOOD** |
| 47 | `execute_code` | `code` (Python) | **GOOD** |
| 48 | `delegate_task` | — child agent spawn | **GOOD** |
| 49 | `cronjob` | `action(create/list/update/pause/resume/remove/run), schedule?, prompt?` | **GOOD** |
| 50-53 | `ha_list_entities, ha_get_state, ha_list_services, ha_call_service` | Home Assistant (HASS_TOKEN gated) | **GOOD** |
| 54-67 | `kanban_show, kanban_list, kanban_complete, kanban_block, kanban_request_review, kanban_request_changes, kanban_heartbeat, kanban_comment, kanban_create, kanban_link, kanban_unblock, kanban_attach, kanban_attach_url, kanban_attachments` | Kanban koordinasyon (worker modunda gated) | **GOOD** |
| 68 | `computer_use` | macOS cua-driver gated | **NÖTR** |
| 69 | `project_list, project_create, project_switch` | GUI-only toolset | **GOOD** |
| 70 | `discord, discord_admin` | — | **GOOD** |
| 71 | `feishu_doc_read` | — | **GOOD** |
| 72-75 | `feishu_drive_list_comments, feishu_drive_list_comment_replies, feishu_drive_reply_comment, feishu_drive_add_comment` | — | **GOOD** |
| 76 | `react_to_message` | — | **GOOD** |
| 77 | `x_search` | — | **GOOD** |
| 78-82 | `yb_query_group_info, yb_query_group_members, yb_send_dm, yb_search_sticker, yb_send_sticker` | Yuanbao platform araçları | **GOOD** |
| 83 | `setup_mcp` | — | **GOOD** |
| 84 | `read_preview` | — | **GOOD** |

> Toolset'ler: `_HERMES_CORE_TOOLS` (web+terminal+file+vision+browser+skills+planning+delegation+cron+HASS+kanban) CLI/Telegram/Discord ortak; `_HERMES_WEBHOOK_SAFE_TOOLS` (web_search/web_extract/vision_analyze/clarify) webhook'lar için daraltılmış — prompt injection'a karşı kasıtlı kısıtlama. **GOOD** (webhook kısıtlaması örnek savunma).
> **BAD (LOBOTOMİ):** `terminal` backend çeşitliliği (Docker/SSH/Daytona/Modal) config karmaşıklığı getirir; `browser_cdp`/`browser_exec` raw CDP yüzeyi tehlikeli; 87 tool'un ~20'si platforma özel (feishu/yuanbao/discord) → genel amaç için gürültü.

## 3.2 Gateway platformları — 24 built-in + plugin

`gateway/config.py:317` `Platform` enum: `local, telegram, discord, whatsapp, whatsapp_cloud, slack, signal, mattermost, matrix, homeassistant, email, sms, dingtalk, api_server, webhook, msgraph_webhook, feishu, wecom, wecom_callback, weixin, bluebubbles, qqbot, yuanbao, relay` — plugin platformlar (irc vb.) `_missing_()` ile dinamik eklenir. **TAMAMI GOOD** (platform_registry plugin sistemi: register/deferred/unregister).

## 3.3 Config anahtarları — `cli-config.yaml.example` üst seviye bloklar

`database, runtime, model, kanban, terminal, browser, tool_loop_guardrails, compression, prompt_caching, memory, session_reset, max_concurrent_sessions, group_sessions_per_user, streaming, skills, agent, platform_toolsets, stt, code_execution, delegation, display, telemetry, updates`

Öne çıkanlar (GOOD):
- `memory.nudge_interval` (10; her N user turn'de kaydetme hatırlatması, tool kullanımında sıfırlanır), `memory.flush_min_turns` (6; exit/reset'te flush), `memory.memory_char_limit` (2200) / `user_char_limit` (1375)
- `skills.creation_nudge_interval` (15; her N tool iterasyonda skill yaratma nudgesi), `skills.external_dirs`
- `session_reset.mode` (none/idle/daily/both) + `idle_minutes`/`at_hour`
- `agent.max_turns` (500), `gateway_timeout`, `gateway_turn_lease_timeout`, `session_stall_timeout`, `restart_drain_timeout`
- `streaming.enabled` (edit-message streaming, Telegram MarkdownV2)
- `compression.context_timeout_seconds` (120) / `context_total_ceiling_seconds` (600)
- `platform_toolsets`, `max_concurrent_sessions`, `group_sessions_per_user` (güvenli default)

## 3.4 Skill yaşam döngüsü durumları — 3 adet

`tools/skill_usage.py:53` → `STATE_ACTIVE="active"`, `STATE_STALE="stale"`, `STATE_ARCHIVED="archived"` — kullanım kaydı tabanlı; curator (archive/prune, "absorbed into" sınıflandırma, kullanılmış skill silinmez); skill'ler `~/.hermes/skills/<category>/<name>/SKILL.md` (agentskills.io standardı). **GOOD.**

**BAD (LOBOTOMİ):** background_review fork'u her turn sonrası ~30K token/event maliyeti; nudge interval küçükse sürekli review birikir; `compaction.reserveTokens` pi'la aynı boşluğu taşır.

---

# 4. deepseek-harness (dsh — DeepSeek AI)

Kaynak: `deepseek-harness/docs/tool-catalog.md` (resmi, verify-tool-catalog gate'li) + `capability-seams.md` + `config-catalog.md`.

## 4.1 Tool kataloğu — 52 satır / 53 model-görünür ad

| # | Tool adı | Paket | Seam (Requires) | Yargı |
|---|---|---|---|---|
| 1 | `ask_user_question` | `dsh-tool-ask-user` | `ctx.tools, ctx.userQuestions` | **GOOD** |
| 2 | `run_code` | `dsh-tools` | `ctx.tools, ctx.codeRuntime, ctx.systemPrompt` — mode: code | **GOOD** |
| 3 | `exit_plan_mode` | `dsh-plan-mode` | `ctx.tools, ctx.systemPrompt, ctx.userQuestions` | **GOOD** |
| 4 | `bash` | `dsh-tool-bash` | `ctx.tools, ctx.shell, ctx.shellEnv, ctx.jobs` | **GOOD** |
| 5 | `pwsh` | `dsh-tool-pwsh` | aynı shell seam, Windows | **GOOD** |
| 6-12 | `cordis_define, cordis_run, cordis_stop, cordis_undefine, cordis_inspect_list, cordis_inspect_query, cordis_inspect_self` | `dsh-tool-cordis` | `ctx.tools, ctx.dynamicCordisRunner` — vm sandbox'lı self-modification, shipped tree'de YOK | **BAD** — açık opt-in, runtime'da plugin yükleme riski |
| 13 | `bash` (persistent) | `dsh-tool-bash-persistent` | `ctx.tools, ctx.terminals` — owner-izole PTY | **GOOD** |
| 14 | `str_replace_editor` | `dsh-tool-str-replace-editor` | `ctx.tools, ctx.fs` | **GOOD** |
| 15-18 | `edit, read, read_image, write` | `dsh-tool-fs` | `ctx.tools, ctx.fs, ctx.systemPrompt, ctx.attachments, ctx.llm` — read-before-write policy ayrı plugin | **GOOD** |
| 19-20 | `glob, grep` | `dsh-tool-fs-search` | `ctx.tools, ctx.subprocess` — packaged ripgrep | **GOOD** |
| 21-26 | `terminal_open, terminal_read, terminal_send, terminal_signal, terminal_list, terminal_close` | `dsh-tool-terminal` | `ctx.tools, ctx.terminals, ctx.jobs` | **GOOD** |
| 27-29 | `create_goal, get_goal, update_goal` | `dsh-tool-goal` | `ctx.tools, ctx.agents, ctx.goals` — 3-round blocked alt sınırı, direct-human root | **GOOD** |
| 30-32 | `schedule_create, schedule_delete, schedule_list` | `dsh-schedule` | `ctx.tools, ctx.sessions, sessionPersistence` — after_seconds/absolute/fixed-rate | **GOOD** |
| 33 | `lsp` | `dsh-tool-lsp` | `ctx.tools, ctx.lsp` — provider'sız LSP_UNAVAILABLE hatası | **GOOD** |
| 34 | `ralph` | `dsh-tool-ralph` | `ctx.tools, ctx.workflowEngine, ctx.subagents` — foreground round-based workflow | **GOOD** |
| 35 | `skill` | `dsh-tool-skill` | `ctx.tools, ctx.agents, ctx.skills` | **GOOD** |
| 36-40 | `session_event_read, session_event_search, session_event_trace, session_search, session_trace` | `dsh-tool-session-query` | `ctx.tools, ctx.sessionQuery` — append-only trajectory sorgusu | **GOOD** — "Every Run is Traceable" |
| 41 | `subagent` (+alias `subagent_fork`) | `dsh-tool-subagent` | `ctx.tools, ctx.subagents` — continuable background | **GOOD** |
| 42-44 | `interrupt_agent, list_agents, send_message` | `dsh-tool-subagent-control` | `ctx.tools, ctx.subagents, ctx.agents, ctx.sessionProjections` | **GOOD** |
| 45 | `report` | `dsh-tool-subagent-report` | `ctx.subagents` — child-scoped, parent session'a user mesajı | **GOOD** |
| 46-48 | `job_kill, job_list, job_output` | `dsh-tool-jobs` | `ctx.tools, ctx.jobs` — kind-agnostic job kontrolü | **GOOD** |
| 49 | `todo_write` | `dsh-tool-todo` | session-owned state | **GOOD** |
| 50 | `workflow` | `dsh-tool-workflow` | `ctx.tools, ctx.workflowEngine` | **GOOD** |
| 51-52 | `web_fetch, web_search` | `dsh-tool-web` | `ctx.tools, ctx.web` — provider değiştirilebilir | **GOOD** |

## 4.2 Cordis service seam eşlemeleri — 55 service (`capability-seams.md` svc_ node'ları)

| Seam (ctx.*) | Service Definition | Provider (örnek) | Consumer (örnek) | Yargı |
|---|---|---|---|---|
| `ctx.llm` | `packages/llm/llm` | `llm-deepseek`, `llm-pi-ai`, `llm-replay` | `agent-loop`, `compaction-basic` | **GOOD** |
| `ctx.shell` | `packages/shell/shell` | `bash-local`, `bash-sandbox`, `pwsh-local`, `pwsh-sandbox` | `tool-bash`, `tool-pwsh` | **GOOD** |
| `ctx.fs` | `packages/fs/fs` | `fs-local`, `fs-sandbox`, `fs-e2b` | `tool-fs`, `tool-str-replace-editor` | **GOOD** |
| `ctx.subprocess` | `packages/subprocess/subprocess` | `subprocess-local`, `subprocess-e2b` | `tool-fs-search` | **GOOD** |
| `ctx.tools` | `packages/core/tools` | — | tüm tool'lar | **GOOD** |
| `ctx.sessions` / `ctx.sessionPersistence` | `packages/core/session` / `session-persistence` | `session-persistence-jsonl`, `sqlite` | `schedule`, `session-query` | **GOOD** |
| `ctx.sessionQuery` | `session-query/session-query` | `session-query-sqlite` | `tool-session-query` | **GOOD** |
| `ctx.attachments` | `attachment/attachment` | `attachment-local` | `tool-fs` (read_image) | **GOOD** |
| `ctx.credentials` | `credentials/credentials` | `credentials-local` | host-apiproxy | **GOOD** |
| `ctx.settings` | `settings/settings` | `settings-file` | — | **GOOD** |
| `ctx.storage` / `ctx.storageDomain` | `storage/storage` | `storage-json`, `storage-sqlite` | workspace | **GOOD** |
| `ctx.skills` | `skill/skill` | `skill-filesystem` | `tool-skill` | **GOOD** |
| `ctx.subagents` | `subagent/subagent` | `subagent-in-process-*`, `-acp`, `-claude-code`, `-codex`, `-dsh-sdk` | `tool-subagent*` | **GOOD** |
| `ctx.jobs` | `jobs/jobs` | `jobs-local` | `tool-jobs`, `tool-bash` | **GOOD** |
| `ctx.goals` | `goal/goal` | `goal-round-driver` | `tool-goal`, `command-goal` | **GOOD** |
| `ctx.compaction` | `compaction/compaction` | `compaction-basic`, `compaction-tool-result-pruner` | `command-compact` | **GOOD** |
| `ctx.web` | `web/web` | `web-fetch-http`, `web-search-deepseek/exa/perplexity` | `tool-web` | **GOOD** |
| `ctx.workflowEngine` | `workflow/workflow` | `workflow-worker-thread` | `tool-workflow`, `tool-ralph` | **GOOD** |
| `ctx.terminals` | `terminal/terminal` | `terminal-bash` | `tool-terminal`, `tool-bash-persistent` | **GOOD** |
| `ctx.lsp` | `lsp/lsp` | `lsp-stdio` | `tool-lsp` | **GOOD** |
| `ctx.codeRuntime` | `code-runtime/code-runtime` | `code-runtime-worker-thread` | `run_code` | **GOOD** |
| `ctx.sandbox` / `ctx.sandboxPolicy` | `sandbox/sandbox` | `sandbox-local` (landlock native) | bash-sandbox, fs-sandbox | **GOOD** |
| `ctx.userQuestions` / `ctx.approval` | `interaction/user-questions` | `user-approval`, `permission-presets` | `tool-ask-user`, `plan-mode` | **GOOD** |
| `ctx.agentLoop` / `ctx.agents` | `core/agent-loop` / `core/agent` | `headless`, `agent-spine-demo` | — | **GOOD** |
| `ctx.systemPrompt` | `core/system-prompt` | — | tüm tool'lar | **GOOD** |
| `ctx.tokenMeter` / `ctx.toolResultPruner` | `llm/token-meter` / `compaction-tool-result-pruner` | — | replay/compaction | **GOOD** |
| `ctx.typert` / `ctx.typertGateway` | `typert/*` | `typert-loader`, `api-gateway` | sdk | **GOOD** |
| `ctx.cordisInspect` / `ctx.dynamicCordisRunner` | `extensions/cordis-host-runner` | vm sandbox | `tool-cordis` | **BAD** — self-modification |
| `ctx.settings` alt: `ctx.directoryPicker`, `ctx.webServer`, `ctx.sessionTitle`, `ctx.sessionProjections`, `ctx.messageFeedback`, `ctx.workspaceRegistry`, `ctx.sessionReferenceResolver`, `ctx.agentPresets`, `ctx.commands`, `ctx.agentDefaultModel`, `ctx.spillStore`, `ctx.invariants`, `ctx.sessionTelemetry`, `ctx.e2b`, `ctx.shellEnv`, `ctx.clientModules`, `ctx.apiProxy`, `ctx.permissionPresets`, `ctx.planMode` | ilgili paketler | ilgili provider'lar | ilgili consumer'lar | **GOOD** |

**BAD (LOBOTOMİ):** 4.1'deki `cordis_*` set + `dynamicCordisRunner` — ajan kendi runtime'ına plugin yükleyebilir (sandbox şartıyla bile yüksek risk); v0.1.0-rc.5 resmi uyumsuzluk garantisi; Web UI merkezli approval akışı terminal-first değil.

## 4.3 Config kataloğu — 105 paket (`config-catalog.md`)

Loadable (config'li) 105 paket + "no-config" loadable plugin'ler (agent, api-gateway, client-*, command-compact, command-feedback, command-goal, commands, fs-observation-policy, goal-round-driver, llm, lsp, schedule, session, session-checkpoint-policy, session-log-export, session-projection, session-stats, skill-badge, storage, subagent, subprocess-local, terminal, tool-ask-user, tool-call-timeout-policy, tool-cordis, tool-subagent-control, user-questions, workspace) + 15 soyut seam paketi (attachment/code-runtime/compaction/credentials/fs/host-directory-picker/jobs/sandbox/session-persistence/session-query/settings/shell/spill/subprocess/workflow). Guard paketleri: `guard/repeat-tool-reminder` (loop-hygiene) + `guard/timeout-policy` (tool-timeout). **GOOD** — verify-tool-catalog gate'i her tool'u gerçek context'te boot eder.

---

## 5. Özet karşılaştırma (tek satır her harness)

| Harness | Tool yüzeyi | Ayırt edici | Genel yargı |
|---|---|---|---|
| pi | 7 tool, 22 cmd, 29 event | Minimal, temiz, extension-first | **GOOD** — hepsi çalışır, eksik: MCP/lsp/dap yok |
| oh-my-pi | 32 tool, 14 LSP, 28 DAP, 70 cmd, 16 URI | Hashline + natives + advisor | **GOOD** (çekirdek) / **BAD** (şişkinlik, compaction boşluğu) |
| hermes | 87 tool, 24 platform | Memory nudge + skill lifecycle + webhook daraltması | **GOOD** (çekirdek) / **BAD** (platform gürültüsü, review maliyeti) |
| deepseek-harness | 53 tool, 55 seam, 105 paket | Capability seam + trajectory | **GOOD** (mimari) / **BAD** (self-modification, rc kalitesi) |
