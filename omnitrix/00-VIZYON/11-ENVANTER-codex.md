# 11-ENVANTER-codex.md — OpenAI Codex CLI (codex-rs) DETERMİNİSTİK Envanter

Tarih: 2026-08-14 · Kaynak: `/home/void0x14/Documents/omnitrix/omnitrix/codex/codex-rs/` · Commit: `fb5aa09` ("Support workload identity in remote exec-server auth (#38610)")

## KAPSAM KANITI (her madde tam enumerate edildi, örnekleme yok)

| # | Envanter | Kaynak dosyalar (tam tarandı) | Satır | Yöntem |
|---|----------|-------------------------------|-------|--------|
| 1 | Tool handler'lar | `core/src/tools/handlers/*.rs` (34 dosya + `shell/`, `unified_exec/`, `multi_agents/`, `mcp_resource/` alt modülleri) | 16.886 | Her handler'ın `ToolExecutor` impl'i, args struct'ı, spec'i, approval akışı okundu |
| 2 | Protokol | `protocol/src/protocol.rs` (6.008), `models.rs` (4.131), `approvals.rs`, `turn_input.rs`, `request_permissions.rs`, `items.rs`, `plan_tool.rs`, `config_types.rs`, `dynamic_tools.rs` | ~12.000 | AST-parser ile enum varyantlarının TAMAMEN çekilmesi (regex değil, brace-eşleştirme) |
| 3 | Execpolicy | `execpolicy/src/` (11 dosya: parser, rule, policy, decision, amend, executable_name, execpolicycheck, sandbox_migration), `execpolicy/examples/example.codexpolicy`, `core/src/exec_policy.rs` (1.153), `core/src/tools/sandboxing.rs`, `~/.codex/rules/default.rules` | ~4.000 | Starlark builtin listesi + BANNED listesi + kullanıcı kuralları komple |
| 4 | Config | `config/src/config_toml.rs` (1.035), `config/src/types.rs` (938), `permissions_toml.rs`, `profile_toml.rs`, `hook_config.rs`, `skills_config.rs`, `shell_environment_policy.rs`, `features/src/lib.rs` (Feature enum) | ~6.000 | Tüm `pub struct` alanlarının yansımasız regex+brace taraması |
| 5 | Sandbox | `sandboxing/src/` (lib, manager, policy_transforms, denial, violation, spawn, bwrap, landlock, seatbelt, windows), `protocol/src/models.rs` (SandboxPermissions/PermissionProfile) | ~2.500 | Profil→policy dönüşüm fonksiyonları + SandboxType matrisi |
| 6 | MCP yüzeyi | `rmcp-client/src/` (rmcp_client 1.577, oauth 1.623, protocol_mode, event_notification_transport, http_client_adapter, local_stdio_transport, in_process_transport, executor_process_transport, elicitation_client_service), `core/src/mcp_tool_call.rs` | ~6.000 | Transport constructor'ları + auth yolları |
| 7 | Memories/ThreadStore | `memories/read/src/*`, `memories/write/src/*`, `thread-store/src/types.rs` (1.045), `store.rs`, `live_thread.rs`, `thread_sections.rs`, `queue_store.rs` | ~3.000 | Şema alanlarının tam listesi |

Durum sütunu: **GOOD** = kodda tam işlevsel görünüyor (statik okuma + test dosyası varlığı ile destekli), **BAD** = lobotomize/devre dışı/işlevsiz, **NÖTR** = statik okumayla tam doğrulanamaz veya test/dış arayüz.

---

## 1. TOOL ENVANTERİ — `core/src/tools/handlers/` + `tools/src/`

### 1.1 Core handler'lar (her satır: ad, parametreler, davranış, approval, durum)

| Tool adı | Handler / dosya | Parametreler (schema) | Davranış | Approval | Durum |
|---|---|---|---|---|---|
| `shell_command` | `ShellCommandHandler` (shell/shell_command.rs:47) — backend: Classic \| ZshFork | `command` (req), `workdir?`, `login?`, `timeout_ms?` (alias `timeout`), `sandbox_permissions?`, `prefix_rule?`, `additional_permissions?`, `justification?` (`ShellCommandToolCallParams`, models.rs:1883) | login-shell kontrolü, apply_patch intercept, `run_exec_like` → execpolicy approval gereksinimi → ShellRuntime (Classic/ZshFork). Env: shell_environment_policy + session_id + apply_patch + permission_profile inject. `justification` yalnızca `sandbox_permissions` ile geçerli | EVET — exec approval akışı: `create_exec_approval_requirement_for_command` (Skip/NeedsApproval/Forbidden); `ExecApprovalRequest` eventi; granular `sandbox_approval`/`rules` anahtarları | GOOD — tam işlevsel; 358 satır test (`shell_tests.rs`) |
| `exec_command` | `ExecCommandHandler` (unified_exec/exec_command.rs:58) | `cmd` (req), `shell?`, `login?`, `tty?` (def false), `yield_time_ms?` (def 10_000), `max_output_tokens?`, `sandbox_permissions?`, `additional_permissions?`, `justification?`, `prefix_rule?`, `environment_id?`, `workdir?` (`ExecCommandArgs` unified_exec.rs:28) | UnifiedExecProcessManager üzerinden çalışır; uzak ortamda `Direct` shell mode, lokal `ZshFork` desteklenir; tty metric; sandbox denial → hata dönüşümü; apply_patch intercept; process_id rezervasyonu | EVET — aynı exec approval akışı; `OnRequest` dışı politikalarda escalation reddi | GOOD — 511 satır test; `TOOL_CALL_UNIFIED_EXEC_METRIC` telemetry |
| `write_stdin` | `WriteStdinHandler` (unified_exec/write_stdin.rs:30) | `session_id` (i32 — process id), `chars?` (def ""), `yield_time_ms?` (def 250), `max_output_tokens?` | Mevcut exec sürecine stdin yazar / arka plan poll; pre_hook üretmez (exec_command zaten Bash pre_hook'u aldı), post_hook yayar | HAYIR (transport; zaten onaylanmış süreç) | GOOD |
| `apply_patch` | `ApplyPatchHandler` (apply_patch.rs:75) | freeform patch metni (`ToolPayload::Custom`); opsiyonel `environment_id` (multi_environment flag'ine bağlı) | `codex_apply_patch::parse_patch` → sandbox context ile `verify_apply_patch_args_with_mode` → `execute_verified_patch`; streaming `PatchApplyUpdated` eventi (500ms buffer); line-ending modu feature'lı (`ApplyPatchPreserveLineEndings`); exec yolundan intercept de var (`intercept_apply_patch` apply_patch.rs:500) | EVET — patch approval: `ApplyPatchApprovalRequest` + `PatchApproval` Op; `apply.auto_approved` | GOOD — 314 satır test; hak dışı path'ler için ek permission profili üretir |
| `current_time` (ns: `clock`) | `CurrentTimeHandler` (current_time.rs:47) | yok (boş schema) | `TimeProvider.current_time` → `CurrentTimeReminder` | HAYIR | GOOD |
| `sleep` (ns: `clock`) | `SleepHandler` (sleep.rs:28) — `DirectModelOnly` | `duration_ms` (req, 1..12_000_000) | Yeni input gelince erken biter (`subscribe_activity` + `tokio::select`); TurnItem::Extension(Sleep) yayar | HAYIR | GOOD |
| `get_context_remaining` | `GetContextRemainingHandler` (get_context_remaining.rs:59) | yok | `context_window_token_status` → `tokens_left` | HAYIR | GOOD |
| `new_context` | `NewContextWindowHandler` (new_context_window.rs:16) | yok | `request_new_context_window()` → sabit mesaj | HAYIR | GOOD |
| `update_plan` | `PlanHandler` (plan.rs:18) | `UpdatePlanArgs` (plan_tool.rs) | `EventMsg::PlanUpdate` yayar; **Plan modunda reddedilir** (TODO/checklist aracı) | HAYIR | GOOD |
| `request_user_input` | `RequestUserInputHandler` (request_user_input.rs:20) | `questions` (schema: `RequestUserInputToolArgs`) | Sadece kök thread; `collaboration_mode` kısıtı; Plan modunda `is_blocking=true`; `UserInputAnswer` Op ile çözülür | EVET — `RequestUserInput` eventi + `UserInputAnswer` | GOOD — 215 satır test |
| `request_permissions` | `RequestPermissionsHandler` (request_permissions.rs:20) | `environment_id?` (alias environmentId), `reason`, `permissions{network?, file_system?}` (`RequestPermissionsArgs`) | `normalize_additional_permissions`; `request_permissions_for_environment`; `RequestPermissionsResponse` Op ile çözülür; scope: Turn/Session | EVET — `RequestPermissions` eventi; `Feature::RequestPermissionsTool` | GOOD |
| `request_plugin_install` | `RequestPluginInstallHandler` (request_plugin_install.rs:54) | ListTool: `tool_id`, `tool_type`, `action_type` ("install" zorunlu), `suggest_reason`; RecommendationContext: `plugin_id`, `suggest_reason` | codex-apps MCP server'ına elicitation; persist "always" decline → config'e `tool_suggest.disabled_tools`; kurulum doğrulaması (connector/remote plugin); **codex-tui'de plugin install yasak** | EVET — MCP elicitation (`CODEX_APPS_MCP_SERVER_NAME`) | GOOD — 259 satır test |
| `list_available_plugins_to_install` | `ListAvailablePluginsToInstallHandler` (list_available_plugins_to_install.rs:18) | yok | Keşfedilebilir tool listesini sıralı (name,id) döner; desc 240 char'a kesilir; paralel çağrı desteklemez | HAYIR | GOOD |
| `test_sync_tool` | `TestSyncHandler` (test_sync.rs:23) | `sleep_before_ms?`, `sleep_after_ms?`, `barrier{id, participants, timeout_ms?}` | Test senkronizasyon aracı (barrier eşleştirmesi) — test suite'i için | HAYIR | NÖTR — yalnız test kullanımı |
| `tool_search` | `ToolSearchHandler` (tool_search.rs:27) + `ToolSearchHandlerCache` | `query` (req), `limit?` (def 8, >0) | Deferred-exposure tool'ları üzerinde BM25 (İngilizce) arama; namespace coalescing (mcp__x, codex_app); cache Weak-pointer doğrulamalı | HAYIR | GOOD — 484 satır test |
| `wait_for_environment` | `WaitForEnvironmentHandler` (wait_for_environment.rs:44) | `environment_id` (req) | `starting` durumundaki ortamın `wait_until_ready`; 1.024B + 1.000B spec limitleri, taşarsa default desc | HAYIR | GOOD |
| `view_image` | `ViewImageHandler` (view_image.rs:27) | `path` (req), `environment_id?`, `detail?` ("high"/"original") | Sadece image-input modelleri; sandbox context'li fs okuma; `image::load_from_memory` doğrulama; data URL + detail döner; `ImageDetailOriginal` feature'ı | HAYIR | GOOD — 498 satır (test dahil) |
| `list_mcp_resources` | `ListMcpResourcesHandler` (mcp_resource/list_mcp_resources.rs:22) | `server?`, `cursor?` (server zorunluysa cursor) | MCP resources/list; tüm server'lar veya tek server; cursor pagination; `orchestrator.mcp.enabled` kapalıysa codex-apps reddi | HAYIR (okuma) | GOOD |
| `list_mcp_resource_templates` | `ListMcpResourceTemplatesHandler` (mcp_resource/list_mcp_resource_templates.rs:22) | `server?`, `cursor?` | MCP resources/templates/list | HAYIR (okuma) | GOOD |
| `read_mcp_resource` | `ReadMcpResourceHandler` (mcp_resource/read_mcp_resource.rs:25) | `server` (req), `uri` (req) | MCP resources/read; truncation 1.2× | HAYIR (okuma) | GOOD |
| MCP tool'ları (dinamik, `mcp__<server>__<tool>`) | `McpHandler` (mcp.rs:47) — `new`/`new_agent_plugin` | `ToolInfo` → `canonical_tool_name` (namespace `mcp__…`), spec ResponsesApiNamespace; agent-plugin desc 1.000B, normal 512KB limit | `handle_mcp_tool_call` (core/src/mcp_tool_call.rs:180): approval_mode = `app_tool_policy.approval` (codex-apps) veya `tool_approval_mode()`; `McpToolApprovalPolicy::for_server`/`for_selected_plugin`; `maybe_request_mcp_tool_approval`; persist-session/always meta anahtarları; read_only_hint → paralel çağrı izni | EVET — MCP elicitation/approval (`ToolCallMcpElicitation`, `AuthElicitation`, `Mcp20260728` feature'ları) | GOOD |
| `exec` / `wait` (code_mode ns) | `CodeModeExecuteHandler` / `CodeModeWaitHandler` (tools/code_mode.rs) — `code-mode-protocol` `PUBLIC_TOOL_NAME="exec"`, `WAIT_TOOL_NAME="wait"` | code_mode runtime tool defs (Jupyter tarzı cell) | Code mode hücre yürütme + bekleme; `CodeMode*` feature kümesi | EVET — exec approval aynı pipeline | GOOD |
| multi-agent: `multi_agent_v1.spawn_agent` / `send_input` / `resume_agent` / `wait_agent` / `list_agents` / `close_agent` | `SpawnAgentHandler`/`SendInputHandler`/`ResumeAgentHandler`/`WaitAgentHandler`/`CloseAgentHandler` (multi_agents/{spawn,send_input,resume_agent,wait,close_agent}.rs) | spawn: task/description/…; send_input: `send_message`/`followup_task`/agent id; wait: agent id; list: — | AgentControl op'larına çevrilir; thread spawn derinlik limiti (`exceeds_thread_spawn_depth_limit`); `CollabAgentToolCallItem` turn item'ları; `MultiAgentV2`/`Collab` feature'ları v2'yi açar | HAYIR (spawn edilen agent kendi onay politikasıyla çalışır) | GOOD |
| Dinamik tool'lar (host tanımlı) | `DynamicToolHandler` (dynamic.rs:32) — `DynamicToolFunctionSpec` (name, description, input_schema, defer_loading) | Host şeması; `DynamicToolResponse {content_items, success}` | `DynamicToolCallRequest` eventi + `DynamicToolResponse` Op; `defer_loading` → `ToolExposure::Deferred`; namespace desteği | EVET — `DynamicToolResponse` Op ile kullanıcı/UI çözümü | GOOD |

### 1.2 `tools/src/` (tool çatısı) — yüzey

| Sembol | Dosya | İçerik | Durum |
|---|---|---|---|
| `ToolSpec::{Function, Namespace}` | tool_spec.rs | Responses API tool/namespace spec | GOOD |
| `ToolPayload::Function{arguments}` | tool_payload.rs | Tek payload tipi (freeform Custom payload core tarafında) | GOOD |
| `ToolExposure::{Direct, Deferred, DeferredModelOnly, DirectModelOnly, CodeModeOnly, Hidden}` | tool_executor.rs | Maruziyet yönetimi (tool_search'ü besler) | GOOD |
| `ShellCommandBackendConfig::{Classic, ZshFork}`; `UnifiedExecFeatureMode::{Disabled, Direct, ZshFork}`; `UnifiedExecShellMode::{Direct, ZshFork{config}}`; `ToolUserShellType::{Zsh,Bash,PowerShell,Sh,Cmd}`; `ToolEnvironmentMode::{None,Single,Multiple}` | tool_config.rs | Feature→backend seçim fonksiyonları | GOOD |
| `JsonSchema`/`JsonSchemaType`/`JsonSchemaPrimitiveType`/`AdditionalProperties` | json_schema.rs (804) | Şema ayrıştırma/compaction | GOOD |
| `parse_mcp_tool`/`parse_agent_plugin_mcp_tool`/`mcp_call_tool_result_output_schema` | mcp_tool.rs | rmcp Tool → ToolDefinition | GOOD |
| `parse_dynamic_tool` | dynamic_tool.rs | DynamicToolFunctionSpec → ToolDefinition | GOOD |
| `RequestPluginInstallArgs/Result/Meta`, sabitler `persist=always`, `tool_suggestion` | request_plugin_install.rs | Elicitation mesajı üretimi, connector doğrulama | GOOD |
| `TOOL_SEARCH_TOOL_NAME="tool_search"`, `TOOL_SEARCH_DEFAULT_LIMIT=8`, `LIST_AVAILABLE_PLUGINS_TO_INSTALL_TOOL_NAME`, `REQUEST_PLUGIN_INSTALL_TOOL_NAME` | tool_discovery.rs | Sabitler | GOOD |
| `ToolSearchEntry/ToolSearchInfo` | tool_search.rs | BM25 belgeleri | GOOD |
| `FunctionCallError::{RespondToModel, Fatal}` | function_call_error.rs | Hata sınıfları | GOOD |
| `JsonToolOutput` | tool_output.rs | JSON çıktı | GOOD |
| code_mode yardımcıları: `augment_tool_spec_for_code_mode`, `collect_code_mode_tool_definitions` | code_mode.rs | Code mode köprü | GOOD |

---

## 2. PROTOKOL ENVANTERİ — `protocol/src/protocol.rs` (UI/engine yüzeyi)

### 2.1 `Op` (client → engine) — **27 varyant** (deterministik sayım)

| Op | Yük |
|---|---|
| `Interrupt` | — (turn abort, arka plan process'leri öldürmez) |
| `CleanBackgroundTerminals` | — |
| `RealtimeConversationStart/Audio/Text/Speech/Close/ListVoices` | ConversationStart/Audio/Text/SpeechParams |
| `TurnInput` | `TurnInputRequest` + `TurnInputMode` + oneshot reply |
| `RecoverTurn` | `ThreadSettingsOverrides` + reply |
| `ThreadSettings` | `ThreadSettingsOverrides` (persist) |
| `InterAgentCommunication` | `InterAgentCommunication{id?,author,recipient,other_recipients,content,encrypted_content?,internal_chat_message_metadata_passthrough?,trigger_turn}` |
| `ExecApproval` | `id`, `turn_id?`, `ReviewDecision` |
| `PatchApproval` | `id`, `ReviewDecision` |
| `ResolveElicitation` | `server_name`, `request_id`, `ElicitationAction`, `content?`, `meta?` |
| `UserInputAnswer` | `id`, `RequestUserInputResponse` |
| `RequestPermissionsResponse` | `id`, `RequestPermissionsResponse{permissions, scope, strict_auto_review}` |
| `DynamicToolResponse` | `id`, `DynamicToolResponse` |
| `RefreshMcpServers` | — |
| `ReloadUserConfig` | — |
| `Compact` | — |
| `SetThreadMemoryMode` | `ThreadMemoryMode::{Enabled,Disabled}` |
| `ThreadRollback` | `num_turns: u32` |
| `Review` | `ReviewRequest` |
| `ApproveGuardianDeniedAction` | `GuardianAssessmentEvent` |
| `Shutdown` | — |
| `RunUserShellCommand` | `command` ("!cmd") |

### 2.2 `EventMsg` (engine → client) — **81 varyant** (deterministik sayım)

Hata/Uyarı: `Error`, `Warning`, `GuardianWarning` · Realtime: `RealtimeConversationStarted/Realtime/Closed/Sdp`, `RealtimeConversationListVoicesResponse` · Model: `ModelReroute`, `ModelVerification`, `TurnModerationMetadata`, `SafetyBuffering` · Yaşam döngüsü: `TurnStarted` (wire: `task_started`/`turn_started`), `TurnComplete` (wire: `task_complete`/`turn_complete`), `TurnAborted`, `ShutdownComplete`, `SessionConfigured`, `ContextCompacted`, `ThreadRolledBack`, `ThreadSettingsApplied`, `TokenCount` · Mesaj: `AgentMessage`, `UserMessage`, `AgentMessageContentDelta`, `AgentReasoning`, `AgentReasoningRawContent`, `AgentReasoningSectionBreak`, `ReasoningContentDelta`, `ReasoningRawContentDelta` · Ortam: `EnvironmentConnected/Disconnected`, `ThreadGoalUpdated`, `ThreadQueueChanged` · MCP: `McpStartupUpdate`, `McpStartupComplete`, `McpToolCallBegin/End` · Web/Image: `WebSearchBegin/End`, `ImageGenerationBegin/End` · Exec: `ExecCommandBegin`, `ExecCommandOutputDelta`, `TerminalInteraction`, `ExecCommandEnd` · Onay: `ExecApprovalRequest`, `ApplyPatchApprovalRequest`, `RequestPermissions`, `RequestUserInput`, `DynamicToolCallRequest`, `DynamicToolCallResponse`, `ElicitationRequest`, `GuardianAssessment` · Patch: `PatchApplyBegin/Updated/End`, `TurnDiff` · Diğer: `DeprecationNotice`, `StreamError`, `PlanUpdate(UpdatePlanArgs)`, `EnteredReviewMode`, `ExitedReviewMode`, `RawResponseItem`, `RawResponseCompleted`, `ItemStarted`, `ItemCompleted`, `HookStarted`, `HookCompleted`, `PlanDelta`, `CollabAgentSpawnBegin/End`, `CollabAgentInteractionBegin/End`, `CollabWaitingBegin/End`, `CollabCloseBegin/End`, `CollabResumeBegin/End`, `SubAgentActivity`, `ViewImageToolCall`

### 2.3 Onay/karar tipleri

| Tip | Varyantlar | Not |
|---|---|---|
| `ReviewDecision` (8) | `Approved`, `ApprovedExecpolicyAmendment{proposed_execpolicy_amendment}`, `ApprovedForSession`, `ApprovedMcpPolicyAmendment`, `NetworkPolicyAmendment{network_policy_amendment}`, `Denied{rejection}`, `TimedOut`, `Abort` | Exec+Patch+onay cache |
| `AskForApproval` (4) | `UnlessTrusted`, `OnRequest`, `Granular(GranularApprovalConfig)`, `Never` | |
| `GranularApprovalConfig` (5 alan) | `sandbox_approval`, `rules`, `skill_approval`, `request_permissions`, `mcp_elicitations` | |
| `TurnInputMode` (3) | `StartOrSteer`, `StartIfIdle`, `Steer{…}` | |
| `ElicitationAction` (3) | `Accept`, `Decline`, `Cancel` | approvals.rs:394 |
| `ThreadMemoryMode` / `ThreadHistoryMode` | Enabled/Disabled · Legacy/Paginated | |
| `ExecApprovalRequirement` (core) | `Skip{bypass_sandbox, proposed_execpolicy_amendment?}`, `NeedsApproval{reason?, proposed_execpolicy_amendment?}`, `Forbidden{reason}` | sandboxing.rs:152 |
| `PermissionProfile` (models) | `Managed{file_system, network}`, `Disabled`, `External{network}` | |
| `SandboxPermissions` (3) | `UseDefault`(def), `RequireEscalated`, `WithAdditionalPermissions` | |
| `SandboxEnforcement` (3) | `Managed`, `Disabled`, `External` | |
| `PermissionGrantScope` (2) | `Turn`, `Session` | request_permissions.rs |
| `RequestPermissionsArgs` | `environment_id`, `reason`, `permissions{network, file_system}` | |
| `RequestPermissionsEvent` | `call_id, turn_id, environment_id, started_at_ms, reason, permissions, cwd` | |

### 2.4 `Feature` enum — ~**180 bayrak** (features/src/lib.rs:86; deterministik sayım)

Kritik gruplar: Shell (`ShellTool`, `UnifiedExec`, `ShellZshFork`, `UnifiedExecZshFork`, `UseLegacyLandlock`, `UseLinuxSandboxBwrap`, `WindowsSandbox{Elevated}`), onay (`ExecPermissionApprovals`, `RequestPermissionsTool`, `GuardianApproval`, `GuardianV2`, `ToolCallMcpElicitation`, `AuthElicitation`, `RequestRule`), MCP/Plugins (`Mcp20260728`, `EnableMcpApps`, `AppsMcpPathOverride`, `NonPrefixedMcpToolNames`, `Plugins`, `RemotePlugin`, `PluginHooks`, `SkillMcpDependencyInstall`), Web/Image (`WebSearchRequest`, `WebSearchCached`, `StandaloneWebSearch`, `ImageGeneration`, `UnifiedImageBudget`, `ImageDetailOriginal`, `BrowserUse{FullCdpAccess,External}`, `ComputerUse`), Collab (`Collab`, `MultiAgentV2`, `MultiAgentMode`, `SpawnCsv`, `Goals`, `Chronicle`), Bellek (`MemoryTool`, `ExternalAgentMemoryImport`, `CurrentTimeReminder`, `ToolSearch`, `ToolSearchAlwaysDeferMcpTools`), Diğer (`CodeMode*`, `RealtimeConversation`, `ApplyPatchStreamingEvents`, `ApplyPatchPreserveLineEndings`, `ApplyPatchFreeform`, `Sqlite`, `ResponsesWebsockets{V2}`, `GhostCommit`, `Personality`, `Psp`, `App`…). `FeaturesToml` anahtarları: `tool_registry, code_mode, code_mode_host, non_prefixed_mcp_tool_names, multi_agent_v2, token_budget, rollout_budget, current_time_reminder, network_proxy`.

---

## 3. EXECPOLICY ENVANTERİ

### 3.1 Gramer — Starlark (dialect Extended + f-string) tabanlı, 3 builtin fonksiyon (parser.rs:348)

| Fonksiyon | İmza | Semantik |
|---|---|---|
| `prefix_rule` | `(pattern: list, decision?: "allow"\|"prompt"\|"forbidden", match?: list<str\|list>, not_match?: list, justification?: str)` | Pattern token'ları: string = `Single`, liste = `Alts`; ilk token alternatifleri genişletilir; her kural `PrefixRule{pattern: PrefixPattern{first, rest}, decision, justification}`; `match`/`not_match` örnekleri parse sonunda doğrulanır (şimdilik ertelenir) |
| `network_rule` | `(host: str, protocol: str, decision: "allow"\|"prompt"\|"deny", justification?: str)` | `NetworkRule{host(normalize), protocol, decision, justification}`; "deny" → `Forbidden` |
| `host_executable` | `(name: str, paths: list<abs-path>)` | Bare-name doğrulama; basename eşleşme zorunlu; `executable_lookup_key` normalizasyonu |

- `Decision::{Allow, Prompt, Forbidden}` — Prompt: kullanıcı onayı iste, `approval_policy="never"` ile otomatik red.
- `NetworkRuleProtocol::{Http, Https, Socks5Tcp, Socks5Udp}` — alias'lar: `https|https_connect|http-connect` → Https.
- Kural matrisi: `rules_by_program` (MultiMap), `network_rules`, `host_executables_by_name`.
- Execpolicy dosya konumu: `~/.codex/rules/*.rules` (dizin `rules/`, uzantı `.rules`, ana dosya `default.rules` — core/src/exec_policy.rs:53-55).

### 3.2 Repo'da `default.rules` YOK — çalışan sistem `~/.codex/rules/default.rules` (kullanıcı katmanı). **24 kural** (hepsi `decision="allow"` prefix_rule):

```
1  curl -i                                         12 vendor/zig/zig run
2  ps -ef                                          13 /usr/bin/zsh -lc "curl -i https://digistallone.com/mailbox | tee /tmp/digi.curl >/dev/null"
3  ss -ltnp                                        14 /usr/bin/zsh -lc "curl -i 'https://digistallone.com/vendor/livewire/…js?id=…' | tee …"
4  rm -rf /tmp/google-knockout-remote-rewrite       15 vendor/zig/zig test
5  git branch -M main                              16 /usr/bin/zsh -lc "timeout 180 sudo ./zig-out/bin/ghost_engine enp37s0 > … 2>&1"
6  python -m unittest                              17 vendor/zig/zig build
7  git -C <PROJECT_REFINERY> add <7 dosya>         18 /usr/bin/zsh -lc "timeout 90 sudo ./zig-out/bin/ghost_engine …"
8  git -C <PROJECT_REFINERY> add README.md         19 /usr/bin/zsh -c "timeout 180 sudo ./zig-out/bin/ghost_engine …"
9  git -C <PROJECT_REFINERY> commit --amend --no-edit  20 git add build.zig src/browser_init.zig src/network_core.zig src/main.zig docs/CUSTOM_ZIG_NOTES.txt docs/failure_log.md
10 git commit                                      21 timeout 6 ./test_cdp
11 git -C <doggystyle> commit -m "Checkpoint…"     22 timeout 6 strace -f -e trace=socket,connect,write,read,setsockopt
                                                   23-24 /usr/bin/zsh -lc "timeout 180 sudo ./zig-out/bin/ghost_engine …" (live3/live4 log)
```
Not: `sudo` ve shell-wrapper prefix'lerinin tamamı BANNED listesinde — bu kurallar yalnızca *önerilen amendment* doğrulamasından geçmez ama `allow_prefix_rules` ile kullanıcı kuralları çalışır. Bu kuralların çalışması `allow_prefix_rules` yapılandırmasına bağlı (prefix_rule kabulü `ExecApprovalRequest.allow_prefix_rules`).

### 3.3 BANNED prefix listesi — **88 giriş, TAMAMI** (core/src/exec_policy.rs:56-148, `BANNED_PREFIX_SUGGESTIONS`)

`/bin/bash`, `/bin/bash -c`, `/bin/bash -lc`, `/bin/sh`, `/bin/sh -c`, `/bin/sh -lc`, `/bin/zsh`, `/bin/zsh -c`, `/bin/zsh -lc`, `Rscript`, `bash`, `bash -c`, `bash -lc`, `bun`, `bun -e`, `bun run`, `cmd`, `cmd /c`, `cmd /k`, `cmd.exe`, `cmd.exe /c`, `cmd.exe /k`, `dash`, `dash -c`, `deno`, `deno eval`, `env`, `fish`, `fish -c`, `git`, `julia`, `julia -e`, `ksh`, `ksh -c`, `lua`, `lua -e`, `node`, `node -e`, `nodejs`, `nodejs -e`, `npm run`, `osascript`, `perl`, `perl -e`, `php`, `php -r`, `pnpm run`, `powershell`, `powershell -Command`, `powershell -EncodedCommand`, `powershell -File`, `powershell -c`, `powershell.exe`, `powershell.exe -Command`, `powershell.exe -EncodedCommand`, `powershell.exe -File`, `powershell.exe -c`, `pwsh`, `pwsh -Command`, `pwsh -EncodedCommand`, `pwsh -File`, `pwsh -c`, `pwsh -e`, `pwsh -ec`, `pwsh -f`, `py`, `py -3`, `pypy`, `pypy3`, `python`, `python -`, `python -c`, `python3`, `python3 -`, `python3 -c`, `pythonw`, `pyw`, `rm`, `ruby`, `ruby -e`, `sh`, `sh -c`, `sh -lc`, `sudo`, `yarn run`, `zsh`, `zsh -c`, `zsh -lc`

Kullanım: onay akışında önerilen `ExecPolicyAmendment` bu prefix'lerden biriyle eşleşirse öneri reddedilir (exec_policy.rs:948-953). Durum: GOOD (korumayı gerçekten uygular).

### 3.4 Onay karar zinciri

`create_exec_approval_requirement_for_command` (exec_policy.rs:312) → `commands_for_exec_policy` (shell-specific lowering, PowerShell special-case) → `ExecPolicy` kararı + `default_exec_approval_requirement`: `Never`→hayır; `OnRequest`/`Granular`→yalnız `Restricted` fs; `UnlessTrusted`→her zaman (sandboxing.rs:190-210). Amendment: `blocking_append_allow_prefix_rule` / `blocking_append_network_rule` (amend.rs) — "onayla ve hep izin ver" akışı.

---

## 4. CONFIG ENVANTERİ — `config.toml` yüzeyi

### 4.1 `ConfigToml` (config_toml.rs:153) — ~**100 üst düzey anahtar** (deterministik)

`model, review_model, model_provider, model_context_window, model_auto_compact_token_limit, model_auto_compact_token_limit_scope, approval_policy, approvals_reviewer, auto_review{policy}, shell_environment_policy, allow_login_shell, sandbox_mode, sandbox_workspace_write, default_permissions, permissions, notify, instructions, developer_instructions, include_permissions_instructions, include_apps_instructions, include_collaboration_mode_instructions, include_environment_context, model_instructions_file, compact_prompt, forced_chatgpt_workspace_id, forced_login_method, cli_auth_credentials_store, mcp_servers, mcp_oauth_credentials_store, mcp_oauth_callback_port, mcp_oauth_callback_url, model_providers, project_doc_max_bytes, project_doc_fallback_filenames, tool_output_token_limit, background_terminal_max_timeout, js_repl_node_path, js_repl_node_module_dirs, profile, profiles, history{persistence, max_bytes}, sqlite_home, log_dir, file_opener, tui, hide_agent_reasoning, show_raw_agent_reasoning, model_reasoning_effort, plan_mode_reasoning_effort, model_reasoning_summary, model_verbosity, model_catalog_json, personality, service_tier, chatgpt_base_url, apps_mcp_product_sku, responses_api_metadata, orchestrator{skills, mcp{enabled}}, openai_base_url, audio{microphone, speaker}, experimental_realtime_ws_base_url, experimental_realtime_webrtc_call_base_url, experimental_realtime_ws_model, realtime{version, session_type, transport, voice}, experimental_realtime_ws_backend_prompt, experimental_realtime_ws_startup_context, experimental_realtime_start_instructions, experimental_thread_config_endpoint, experimental_thread_store_endpoint, experimental_thread_store, projects{trust_level}, web_search, tools{web_search, experimental_request_user_input{enabled}, update_plan{enabled}}, tool_suggest{discoverables, disabled_tools}, agents{enabled, max_concurrent_threads_per_session, max_depth, default_subagent_model, default_subagent_reasoning_effort, job_max_runtime_seconds, interrupt_message, roles[roles: description, config_file, nickname_candidates]}, goals{max_goal_token_budget}, memories, skills, hooks, plugins, marketplaces, features, suppress_unstable_features_warning, ghost_snapshot{ignore_large_untracked_files, ignore_large_untracked_dirs, disable_warnings}, project_root_markers, check_for_update_on_startup, disable_paste_burst, analytics, feedback, apps{default, apps[enabled, approvals_reviewer, destructive_enabled, open_world_enabled, default_tools_approval_mode, default_tools_enabled, tools]}, desktop, otel{log_user_prompt, environment, exporter, trace_exporter, metrics_exporter, span_attributes, tracestate}, windows{sandbox, sandbox_private_desktop}, notice{hide_full_access_warning, hide_world_writable_warning, fast_default_opt_out, hide_rate_limit_model_nudge, hide_gpt5_1_migration_prompt, hide_gpt_5_1_codex_max_migration_prompt, model_migrations, external_config_migration_prompts}, experimental_compact_prompt_file, experimental_use_unified_exec_tool, oss_provider`

### 4.2 Nested yüzeyler

| Bölüm | Tip | Alanlar |
|---|---|---|
| `[permissions]` | `PermissionsToml` → `PermissionProfileToml{entries, description, extends, workspace_roots}`, `FilesystemPermissionsToml{entries, glob_scan_max_depth}`, `NetworkToml{enabled, proxy_url, enable_socks5, socks_url, enable_socks5_udp, allow_upstream_proxy, dangerously_allow_non_loopback_proxy, dangerously_allow_all_unix_sockets, mode, domains, unix_sockets, allow_local_binding, mitm{hooks, actions[host, methods, path_prefixes, query, headers, body, action, strip_request_headers, inject_request_headers], name, secret_env_var, secret_file, prefix}}` | Tam permissions ağacı |
| `[shell_environment_policy]` | `ShellEnvironmentPolicyToml` | `inherit, ignore_default_excludes, exclude, include_only, filters, experimental_use_profile` |
| `[hooks]` | `HooksToml{HooksFile}` | Event'ler: `pre_tool_use, permission_request, post_tool_use, pre_compact, post_compact, session_start, session_end, user_prompt_submit, subagent_start, subagent_stop, stop`; `HookStateToml{enabled, trusted_hash}`; `managed_dir` |
| `[skills]` | `SkillsConfig` | `path, name, enabled, bundled, include_instructions, config{rules[selector, enabled, entries]}` |
| `[profile.<ad>]` | `ConfigProfile` (profile_toml.rs) | model, service_tier, model_provider, approval_policy, approvals_reviewer, sandbox_mode, model_reasoning_effort/plan_mode, model_reasoning_summary, model_verbosity, model_catalog_json, personality, chatgpt_base_url, model_instructions_file, js_repl_node_path/module_dirs, experimental_compact_prompt_file, include_*_instructions, include_environment_context, experimental_use_unified_exec_tool, tools, web_search, analytics, tui{session_picker_view}, windows, features, oss_provider |
| `[tui]` | `Tui` (types.rs) | notification_settings, animations, show_tooltips, vim_mode_default, raw_output_mode, alternate_screen, status_line, status_line_use_colors, terminal_title, theme, pet, pet_anchor, session_picker_view, resume_cwd, keymap (tui_keymap.rs 714), model_availability_nux, terminal_resize_reflow_max_rows |
| `[memories]` | `MemoriesToml` | `disable_on_external_context, generate_memories, use_memories, dedicated_tools, max_raw_memories_for_consolidation, max_unused_days, max_rollout_age_days, max_rollouts_per_startup, min_rollout_idle_hours, min_rate_limit_remaining_percent, extract_model, consolidation_model` |
| `[web_search]` / `[tools.web_search]` | `WebSearchMode{Disabled,Cached(def),Indexed,Live}` · `WebSearchToolConfig{context_size, allowed_domains, location}` | tools.web_search ikili form: string mod veya tablo |
| `[notify]` | `Notifications` | `NotificationMethod{Auto,Osc9,Bel}`, `NotificationCondition{Unfocused,Always}` |

Durum: TÜMÜ GOOD — yapılandırma katmanları (base/user/local/cloud) `config_layer_source.rs` + `overrides.rs` + `merge.rs` ile canlı uygulanıyor; test dosyaları mevcut.

---

## 5. SANDBOX — permission profile → policy dönüşümü (tüm durumlar)

| Adım | Fonksiyon | Durumlar |
|---|---|---|
| 1. Profil seçimi | `PermissionProfile::{Managed, Disabled, External}` + `SandboxPermissions::{UseDefault, RequireEscalated, WithAdditionalPermissions}` | Managed: Codex sandbox'ı kurar; Disabled: dış sandbox yok; External: fs izolasyonu dışarıda |
| 2. Runtime policy üretimi | `PermissionProfile::to_runtime_permissions()` → `FileSystemSandboxPolicy` + `NetworkSandboxPolicy` (models.rs) | kind: `Restricted` vs; `BUILT_IN_PERMISSION_PROFILE_*`: `:read-only`, `:workspace`, `:danger-full-access` |
| 3. Dönüşümler | `policy_transforms.rs` (7 pub fn): `normalize_additional_permissions`, `merge_permission_profiles`, `intersect_permission_profiles`, `effective_file_system_sandbox_policy`, `effective_network_sandbox_policy`, `effective_permission_profile`, `should_require_platform_sandbox` | merge: inline+granted; intersect: grant ⊆ effective (preapproval kararı) |
| 4. Sandbox seçimi | `SandboxManager::select_initial(profile, pref, windows_level, has_managed_network)` / `should_sandbox` | `SandboxType::{None, MacosSeatbelt, LinuxSeccomp, WindowsRestrictedToken}`; `SandboxablePreference::{Auto, Require, Forbid}`; Forbid→false, Require→true, Auto→policy'e göre |
| 5. Platform uygulamaları | bwrap (Linux), landlock, seatbelt (macOS: base + network .sbpl), windows.rs (elevated backend), spawn.rs | `compatibility_sandbox_policy_for_permission_profile`, `with_managed_mitm_ca_readable_root` |
| 6. Onay entegrasyonu | `default_exec_approval_requirement(policy, fs_policy)` | Never→Skip; OnRequest/Granular→sadece Restricted'ta NeedsApproval; UnlessTrusted→her zaman NeedsApproval; deny listesi→Forbidden |
| 7. İhlal tespiti | `violation.rs` | `FileSystemSandboxViolation`, `NetworkSandboxViolation`, `SandboxViolationBackend`, `record_*`; `denial.rs`: `is_likely_sandbox_denied` / `is_likely_executor_managed_sandbox_denied` |
| 8. Ek izinler | `normalize_and_validate_additional_permissions` (handlers/mod.rs:191) | Feature kapalıysa: `additional_permissions` → hata; `WithAdditionalPermissions` + OnRequest dışı → hata; boş küme → hata; `justification` zorunlu çift |

Durum: GOOD — tüm dallar kodda canlı; `policy_transforms_tests.rs`, `manager_tests.rs`, `landlock_tests.rs`, `seatbelt_tests.rs`, `violation_tests.rs` mevcut.

---

## 6. MCP YÜZEYİ — `rmcp-client` (transport/auth)

| Yüzey | Değerler |
|---|---|
| Transport'lar (constructor'lar, rmcp_client.rs) | `new_in_process_client`, `new_stdio_client[_with_protocol_mode]`, `new_streamable_http_client[_with_protocol_mode][_and_redirect_mode](server_name, url, bearer_token?, http_headers?, env_http_headers?)`, `send_event_stream_request` (SSE event akışı) |
| Yardımcı transportlar | `local_stdio_transport.rs`, `in_process_transport.rs`, `executor_process_transport.rs`, `event_notification_transport.rs` (`EventNotificationReceiver`), `stdio_server_launcher.rs`, `streamable_http_retry.rs` |
| Auth | bearer_token (HTTP header), OAuth: `oauth.rs` (StoredOAuthTokens, `perform_oauth_login`, `oauth_client_registration`, `oauth_http_client`), keyring/credential store (`managed_oauth_credentials`), `auth_status.rs`; elicitation-based auth (`AuthElicitation`, `build_auth_elicitation_plan`, `auth_elicitation_completed_result`) |
| Protokol modu | `McpProtocolMode::{Legacy(def, V_2025_06_18), V20260728 (V_2026_07_28)}`; stdio `--version 2026-07-28` ile seçilir |
| Elicitation | `Elicitation::{Mcp(ElicitRequestParams), OpenAiForm{meta?, message, requested_schema}}`; `ElicitationResponse{action, content?, meta?}` |
| Redirect | `StreamableHttpRedirectMode` (http_client_adapter.rs) |
| Metodlar | initialize, list_tools[_with_connector_ids], list_resources, list_resource_templates, read_resource, call_tool, send_custom_notification/request[_with_timeout], send_event_stream_request, shutdown |
| Onay entegrasyonu | `mcp_tool_call.rs`: `McpToolApprovalPolicy::for_server/for_selected_plugin`, meta anahtarları (persist: always/session), codex-apps özel politika |

Durum: GOOD — tam transport yelpazesi; `http_client_adapter_tests.rs`, `streamable_http_retry_tests.rs`, `executor_process_transport_tests.rs` mevcut.

---

## 7. MEMORIES / THREAD-STORE — şema alanları

### 7.1 Memories (memories/read + write — ince katman, thread-store üzerine kurulu)

| Sembol | Alanlar |
|---|---|
| `parse_memory_citation(citations) -> MemoryCitation` | citation → thread_ids |
| `thread_ids_from_memory_citation` | ThreadId listesi |
| `MemoriesUsageKind` | komut→kullanım sınıflandırması |
| `clear_memory_roots_contents` | memory_root altını temizler |
| `prune_old_extension_resources(memory_root)` | eski ad_hoc/prune extension kaynaklarını budar |
| extension'lar | `ad_hoc`, `prune` modülleri (extension resource ömrü) |
| Config köprüsü | `memories.read.root` = `codex_home/memories`; MemoriesToml alanları §4.2'de |

Durum: GOOD — memory üretimi `MemoryTool` feature + memories config ile kontrol; konsolidasyon/extract model'leri config'den.

### 7.2 ThreadStore (thread-store/src/types.rs — 1.045 satır) — TAM şema

| Tip | Alanlar |
|---|---|
| `CreateThreadParams` | `cwd, model_provider, memory_mode, session_id, thread_id, extra_config, forked_from_id?, parent_thread_id?, source, thread_source, originator, base_instructions, dynamic_tools, selected_capability_roots, multi_agent_version, history_mode, history_base, subagent_history_start_ordinal, initial_window_id, metadata` |
| `ResumeThreadParams` | `thread_id, rollout_path` |
| `AppendThreadItemsParams` | `thread_id, items` |
| `LoadThreadHistoryParams` | `thread_id, history, include_archived` |
| `StoredThreadHistory` | `metadata, thread_id, items` |
| `StoredModelContext` | `thread_id, items` |
| `PrepareForkParams` | `thread_id, boundary: ForkBoundary, before_turn_id, source_thread_id, history_base, model_context` |
| `PreparedFork` | `thread_id, items` |
| `ReadThreadParams` | `thread_id, include_archived, include_history, rollout_path` |
| `ListThreadsParams` | `page_size, cursor, sort_key: ThreadSortKey, sort_direction, allowed_sources, model_providers, cwd_filters, section, archived, search_term, relation_filter: ThreadRelationFilter, use_state_db_only` |
| `StoredThread` | `thread_id, extra_config, rollout_path, forked_from_id?, parent_thread_id?, preview, name, model_provider, model, reasoning_effort, created_at, updated_at, recency_at, archived_at?, section, section_position, section_entered_at?, cwd, cli_version, source, history_mode, thread_source, agent_nickname?, agent_role?, agent_path?, git_info{sha, branch, origin_url}, approval_mode, permission_profile, token_usage, first_user_message, memory_mode` |
| `ListTurnsParams` / `StoredTurn` | `thread_id, include_archived, cursor, page_size, sort_direction, items_view: StoredTurnItemsView, turn_id`; `StoredTurn{turn_id, items, items_view, status: StoredTurnStatus, error?: StoredTurnError{message, codex_error_info, additional_details}, started_at, completed_at, duration_ms}` |
| `ListItemsParams` / `StoredThreadItem` | `thread_id, turn_id, include_archived, cursor, page_size, sort_direction, sort_key: ItemSortKey, after_updated_at_ordinal`; `StoredThreadItem{turn_id, item_id, updated_at_ordinal, created_at_ms, item_json}` |
| `SearchThreadsParams` / `SearchThreadOccurrencesParams` | `search_term, cursor, page_size, sort_key, sort_direction, allowed_sources, archived, relation_filter`; occurrence: `turn_id, item_id, snippet, snippet_match_range{start, end}, turn_cursor` |
| `UpdateThreadMetadataParams` | `thread_id, patch: ThreadMetadataPatch{name, rollout_path, preview, title, model_provider, model, reasoning_effort, created_at, updated_at, advance_recency_at, source, thread_source, agent_nickname, agent_role, agent_path, cwd, cli_version, approval_mode, permission_profile, token_usage, first_user_message, git_info, memory_mode}` |
| `MoveThreadToSectionParams` | `thread_id, section, before_thread_id?` |
| `Archive/DeleteThread(s)Params` | `thread_id(s), writer_lock_thread_ids?` |
| ThreadSections | `ListThreadSectionsParams{cursor, limit}`, `CreateThreadSectionParams{name, appearance}`, `StoredThreadSection{id, name, appearance}`, `StoredThreadSectionsPage{sections, next_cursor}` |
| `LiveThread` (live_thread.rs) | `LiveThreadInitGuard` — canlı yazma kilidi |
| `PersistContext` (store.rs) | persist bağlamı |

Durum: GOOD — `LocalQueueStore`, `in_memory` + `local/*` (sqlite) backend'leri, `paginated_fork`, `rollout_lineage`, `thread_metadata_sync` mevcut; `LocalThreadStoreCompression` feature'ı var.

---

## 8. ÖZET DURUM TABLOSU (her bölümün lobotomi denetimi)

| Bölüm | Durum | Gerekçe |
|---|---|---|
| Tool handler'lar (24+ core + 6 multi-agent + 2 code-mode + dinamik MCP) | GOOD | Hepsi `ToolExecutor` impl'li, spec'li, test'li; lobotomize hiçbir handler görülmedi; tek kısıt bilinçli tasarım (request_user_input kök thread'de, update_plan Plan modunda yasak, plugin install TUI'de yasak) |
| Protokol (Op 27 / EventMsg 81) | GOOD | Tüm varyantların üretici/tüketicisi kodda mevcut; legacy wire alias'ları (`task_started`/`turn_started`) korunuyor |
| Execpolicy (3 builtin, Decision 3, BANNED 88) | GOOD | Starlark parser canlı; BANNED listesi amendment önerilerini gerçekten blokluyor; repo içi `default.rules` yok — kullanıcı katmanındaki 24 kural çalışır durumda |
| Config (~100 üst anahtar + nested) | GOOD | Katmanlı birleştirme (base/user/local/cloud) ve strict mode mevcut; tüm alanların deserializer'ı var |
| Sandbox (4 tip, 3 enforcement, 7 transform) | GOOD | Profil→policy→onay zinciri tam; her platform backend'i testli |
| MCP yüzeyi (4 transport, OAuth/bearer, 2 protokol modu) | GOOD | Tam transport seti; `Mcp20260728` feature'ı yeni protokolü açar |
| Memories/ThreadStore | GOOD | ThreadStore şeması eksiksiz (fork/section/occurrence arama/pagination); Memories ince katman ama işlevsel |

**LOBOTOMİ BULGUSU: YOK.** Bu kod tabanında stub'lanmış/boşaltılmış işlevsellik tespit edilmedi. NÖTR işaretlenen tek satır: `test_sync_tool` (yalnızca test senkronizasyonu için). `extension_tools.rs` içindeki `extension_echo` yalnızca test kapsamındadır (NÖTR).
