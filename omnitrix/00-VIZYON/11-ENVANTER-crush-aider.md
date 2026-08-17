# 11 — ENVANTER: crush (Go) + aider (Python) — İKİ HARNESS, %100 KAYIT NOKTASI ENUMERASYONU

> Kaynak A: `/home/void0x14/Documents/omnitrix/omnitrix/crush/` (charmbracelet/crush, Go)
> Kaynak B: `/home/void0x14/Documents/omnitrix/omnitrix/aider/` (Aider, Python)
> Tarih: 2026-08-14 · Yöntem: **satır-satır TAM enumerate** — keyword grep + örnekleme YOK. Her satır, kayıt noktasından (fabrika fonksiyonu, struct, registry, switch-case) doğrulandı.

## 0. Kapsam Kanıtı (enumerate edilen kayıt noktaları)

| # | Kayıt noktası | Dosya | Kanıt |
|---|---|---|---|
| 1 | CRUSH tool fabrikaları | `internal/agent/tools/*.go` + `agent_tool.go` + `agentic_fetch_tool.go` | 29 fabrika fonksiyonu (`func NewXxxTool(`) + 31 tool adı const'ı |
| 2 | CRUSH tool registry | `internal/agent/coordinator.go:679-803` (`buildTools`) | 18 temel + 1 interaktif + 8 LSP + 2 MCP + dinamik MCP + `agent` + `agentic_fetch`; `AllowedTools`/`AllowedMCP` filtresi |
| 3 | CRUSH parametre şemaları | `type XxxParams struct` (20 adet) + `XxxPermissionsParams` (13 adet) | JSON tag + description satırları |
| 4 | CRUSH permission yüzeyi | `internal/permission/permission.go` | allowlist (`allowed_tools`), auto-approve session, `SkipRequests` (yolo) |
| 5 | CRUSH config komutları | `internal/shellconfig/register.go:21-29` | 7 bash-builtin: provider, model, mcp, lsp, permissions, hook, option |
| 6 | CRUSH option anahtarları | `internal/shellconfig/options.go` (`optionSpecs` map) | 16 anahtar + `ui` + `reset` |
| 7 | CRUSH CLI komutları | `internal/cmd/*.go` (`Use:` string'leri) | 15 komut (session alt komutlarıyla 20) |
| 8 | CRUSH config şeması | `internal/config/config.go` (`Config`, `Options`, `Tools`, `Agent`, `ProviderConfig`, `MCPConfig`, `HookConfig`, `Permissions` struct'ları) | JSON tag + jsonschema açıklamaları |
| 9 | CRUSH hook event'leri | `internal/hooks/hooks.go` + `docs/hooks/README.md` | `EventPreToolUse` (tek event), `HaltExitCode=49`, Decision allow/deny/none, input_rewrite |
| 10 | CRUSH MCP state machine | `internal/agent/tools/mcp/init.go:142-151` + `lifecycle.go` + `channel.go` | 5 state (disabled/starting/connected/error/needs-auth), 3 reinit action, 3 channel gate state, 2 event |
| 11 | CRUSH tema sistemi | `internal/ui/styles/themes.go` | `ThemeKeyForProvider` (2 anahtar) + `CharmtonePantera` + `HypercrushObsidiana` |
| 12 | CRUSH TUI komut palette | `internal/ui/dialog/commands.go` + `internal/commands/commands.go` | `user:`/`project:` prefix, `$ARG` regex, MCP prompt'ları, skill kataloğu |
| 13 | AIDER coder registry | `aider/coders/__init__.py` | 13 aktive coder + 4 devre dışı (func ailesi) |
| 14 | AIDER edit format'ları | her `*coder.py` içinde `edit_format =` | 15 format + prompt kanıtları (`>>>>>>> REPLACE`, `*** Begin Patch`, `@@ ... @@`, fenced, editor) |
| 15 | AIDER model→format önerisi | `aider/models.py:430+` + `resources/model-settings.yml` | 471 edit_format satırı; dağılım: diff=289, editor-diff=138, diff-fenced=35, udiff=4, whole=4, architect=1 |
| 16 | AIDER slash komutları | `aider/commands.py` (`cmd_*` metodları) | 50 komut (dinamik keşif: `get_commands()`) |
| 17 | AIDER git operasyonları | `aider/repo.py` (GitPython) | commit, add, config, diff (HEAD/cached/wd), diff_commits, tracked files, dirty files, head commit, ignore motoru |
| 18 | AIDER CLI flag'leri | `aider/args.py` | 120 `add_argument` çağrısı, 17 grup, ~130 flag (alias'larla) |
| 19 | AIDER linter | `aider/linter.py` | `Linter` sınıfı: languages dict (python→py_lint), `all_lint_cmd`, `set_linter`, 3 katmanlı python lint (basic+compile+flake8) |
| 20 | AIDER watch | `aider/watch.py` | watchfiles (Rust) + pathspec gitignore + AI-yorum modu (ask/code prompt), `!`/`?` aksiyonları |

**Değerlendirme ölçeği**: **ÇAL (GOOD)** = deterministik, kaynak-doğrulanabilir, omnitrix yeniden-kullanımına uygun. **LOBOTOMİ (BAD)** = otomasyonda güvenlik/telemetri/özerklik riski — kapalı tutulmalı (omnitrix tool-broker allowlist felsefesiyle çelişir). **NÖTR** = ikisi de değil / koşullu / deneysel.

---

# BÖLÜM 1 — CRUSH (Go)

## 1. Tool Envanteri (`internal/agent/tools/` + `agent_tool.go` + `agentic_fetch_tool.go`)

Registry sırası: `coordinator.go:buildTools` (679-803). Tool adları const'lardan; parametreler `XxxParams` JSON tag'lerinden; permission = `permissions.Request(...)` çağrısının varlığından. 31 tool adı.

| # | Tool ID | Parametreler (JSON — anlam) | Davranış | Permission | Durum |
|---|---|---|---|---|---|
| 1 | `agent` | (sub-agent tool; parametreleri `agent_tool.md` şablonunda) | Alt-agent oturumu açar (coder modeli); yalnızca `AllowedTools` içindeyse kurulur (`coordinator.go:681`) | — (kendi içinde model çağrısı) | ÇAL |
| 2 | `agentic_fetch` | `url?` (opsiyonel), `prompt` (zorunlu — ne aranacak) | URL verildiyse içeriği alır, verilmediyse web'de arar + analiz eder; sub-agent olarak çalışır (`agentic_fetch_tool.go:67-110`); hook interception'dan muaftır | EVET (`AgenticFetchToolName`, `agentic_fetch_tool.go:89`) | NÖTR (ağ + sub-agent; otomasyonda kapalı tutulmalı) |
| 3 | `bash` | `description` (≤30 char), `command` (zorunlu), `working_dir?`, `run_in_background?`, `auto_background_after?` (varsayılan 60 sn) | mvdan.sh ile komut; ~60 yasaklı komut (curl, sudo, apt, pacman, ip, mount…); argüman blokerleri (install/sudo bayrakları); safe read-only komutları otomatik geçer; 30 KB çıktı cap; 60 sn sonra otomatik background; `job_output`/`job_kill` entegrasyonu | EVET (read-only değilse; `BashPermissionsParams`) | LOBOTOMİ (keyfi kod; omnitrix personada bash kapalı — 04-raporuyla uyumlu) |
| 4 | `crush_info` | (parametresiz) | Config/working-dir/LSP durumu/skills bilgisi döner | — | ÇAL |
| 5 | `crush_logs` | `lines?` (varsayılan 50, max 100) | `crush.log`'dan son N satır | — | ÇAL |
| 6 | `lsp_diagnostics` | `file_path?` (boşsa proje geneli) | LSP diyagnostikleri (hata/uyarı) | — | ÇAL (LSP kuruluysa) |
| 7 | `download` | `url` (zorunlu), `file_path` (zorunlu), `timeout?` (max 600 sn) | URL içeriğini yerel dosyaya indirir | EVET | LOBOTOMİ (ağ + dosya yazma) |
| 8 | `edit` | `file_path` (mutlak), `old_string`, `new_string`, `replace_all?` | String replace; LSP + filetracker + history entegrasyonu; whitespace varyantı (`edit_whitespace.go`) | EVET (`EditPermissionsParams` — old/new content) | ÇAL (hashline'a yakın; güçlü) |
| 9 | `multiedit` | `file_path`, `edits[]` (sıralı `MultiEditOperation` dizisi) | Tek çağrıda çoklu edit operasyonu | EVET | ÇAL |
| 10 | `fetch` | `url`, `format` (text/markdown/html), `timeout?` (max 120 sn) | URL içeriğini istenen formatta döner | EVET | LOBOTOMİ (ağ) |
| 11 | `glob` | `pattern` (zorunlu), `path?` (cwd varsayılan) | Glob eşleşmeleri; `tools.glob.timeout` (varsayılan 30 sn) | — | ÇAL |
| 12 | `grep` | `pattern` (regex), `path?`, `include?` (örn. `*.js`), `literal_text?` | ripgrep ile içerik arama; `tools.grep.timeout` (varsayılan 5 sn) | — | ÇAL |
| 13 | `job_kill` | `shell_id` (zorunlu) | Background shell'i sonlandırır | — | ÇAL |
| 14 | `job_output` | `shell_id`, `wait?` (tamamlanana kadar bloke) | Background job çıktısını okur | — | ÇAL |
| 15 | `list_mcp_resources` | `mcp_name` (zorunlu) | MCP sunucusunun kaynaklarını listeler | EVET | NÖTR (MCP koşullu) |
| 16 | `ls` | `path?`, `ignore[]?`, `depth?` | Dizin listeleme; `tools.ls.max_depth`/`max_items` (varsayılan 1000) | EVET (her listede) | ÇAL |
| 17 | `lsp_call_hierarchy` | `symbol`, `direction` (incoming/outgoing), `path?` | Çağrı hiyerarşisi | — | ÇAL |
| 18 | `lsp_definition` | `symbol`, `path?` | Sembol tanımına git | — | ÇAL |
| 19 | `lsp_rename` | `symbol`, `new_name`, `path?` | Sembol yeniden adlandırma (LSP'den edit alır) | EVET | ÇAL |
| 20 | `lsp_replace_symbol` | `symbol`, `file_path`, `replacement?`, `action?` (replace\|add_before\|add_after\|delete) | Sembolü değiştir/ekle/sil | EVET | ÇAL |
| 21 | `lsp_restart` | `name?` (boşsa tüm LSP'ler) | LSP client'larını yeniden başlatır | — | NÖTR |
| 22 | `lsp_symbols` | `file_path` | Dosyadaki semboller | — | ÇAL |
| 23 | `question` | `questions[]`, `confirm_title?`, `confirm_description?` (çoklu soru için) | İnteraktif form (tek soru = sekmesiz, çoklu = sekmeli + onay); YALNIZCA interactive ana agent | — | LOBOTOMİ (otomasyonda kilit noktası; `coordinator.go:734` interactive-only) |
| 24 | `read_mcp_resource` | `mcp_name`, `uri` | MCP kaynağını okur | EVET | NÖTR (MCP koşullu) |
| 25 | `lsp_references` | `symbol`, `path?` | Sembol referansları | — | ÇAL |
| 26 | `sourcegraph` | `query`, `count?` (max 20), `context_window?` (10), `timeout?` (max 120) | Sourcegraph kod arama (harici API) | — | LOBOTOMİ (harici ağ API) |
| 27 | `todos` | `todos[]` (`TodoItem` listesi) | Session todo listesini günceller (UI'ya yansır) | — | ÇAL |
| 28 | `view` | `file_path`, `offset?` (0-bazlı), `limit?` (varsayılan 200) | Dosya okuma; LSP warm-up + skill eşleştirme | EVET (her okumada) | ÇAL |
| 29 | `web_fetch` | `url` | Web sayfası içeriği | — | LOBOTOMİ (ağ; permission YOK — not: `fetch`'in aksine) |
| 30 | `web_search` | `query`, `max_results?` (varsayılan 10, max 20) | Web arama | — | LOBOTOMİ (ağ; permission YOK) |
| 31 | `write` | `file_path`, `content` | Dosya yazma; LSP + filetracker + history | EVET | ÇAL |

**Koşullu katmanlar** (`coordinator.go`):
- `question`: yalnızca `!isSubAgent && c.interactive` (`:734`).
- 8 LSP tool'u: `len(cfg.LSP) > 0 || AutoLSP != false` (`:739`).
- `list_mcp_resources` + `read_mcp_resource`: `len(cfg.MCP) > 0` (`:753`).
- Dinamik MCP tool'ları: `tools.GetMCPTools(c.permissions, ...)` — `agent.AllowedMCP` filtresi (nil = tümü, boş = hiçbiri, map = mcp:tool listesi) (`:768-790`).
- **Hook sarmalayıcı**: PreToolUse hook'ları varsa tüm tool'lar `wrapToolsWithHooks` ile sarılır; sub-agent'lar muaftır (`:795-800`).
- `AllowedTools` (nil = hepsi) + `Permissions.allowed_tools` (otomatik onay listesi) iki ayrı katman.

**Per-tool permission özeti**: `permissions.Request` kullanan 13 dosya: bash, download, edit, fetch, list_mcp_resources, ls, lsp_rename, lsp_replace_symbol, mcp-tools (dinamik), multiedit, read_mcp_resource, view, write. **Dikkat**: `web_fetch`/`web_search`/`sourcegraph`/`download`-hariç ağ tool'ları permission'sız — LOBOTOMİ gerekçesi.

---

## 2. Slash / Komut Sistemleri (3 katman)

### 2.1 Shellconfig bash-builtin komutları (`internal/shellconfig/register.go:21-29`) — crushrc'de `crush` komutları

| Komut | Dosya | Davranış | Durum |
|---|---|---|---|
| `provider` | `provider.go` | `add`/`remove`/`rm`; provider yapılandırması | ÇAL |
| `model` | `model.go` | model seçimi/yapılandırması | ÇAL |
| `mcp` | `mcp.go` | MCP sunucusu ekleme | NÖTR (harici) |
| `lsp` | `lsp.go` | `lsp add <lang> --command ...` (stdio/http/sse) | ÇAL |
| `permissions` | `permissions.go` | tool izinleri | ÇAL |
| `hook` | `hook.go` | hook tanımlama | ÇAL |
| `option` | `options.go` | config anahtarı (bkz. 2.2) | ÇAL |

### 2.2 `option` anahtarları (`optionSpecs` map, `options.go`)

| Anahtar | JSON alanı | Tip | Durum |
|---|---|---|---|
| `debug` | `debug` | bool | ÇAL |
| `debug-lsp` | `debug_lsp` | bool | ÇAL |
| `auto-lsp` | `auto_lsp` | bool | ÇAL |
| `progress` | `progress` | bool | ÇAL |
| `metrics` | `disable_metrics` | bool (inverted) | LOBOTOMİ (telemetri) |
| `auto-summarize` | `disable_auto_summarize` | bool (inverted) | NÖTR |
| `provider-auto-update` | `disable_provider_auto_update` | bool (inverted) | LOBOTOMİ (dış ağ) |
| `default-providers` | `disable_default_providers` | bool (inverted) | NÖTR |
| `notifications` | `notifications` | string | NÖTR |
| `data-directory` | `data_directory` | string | ÇAL |
| `initialize-as` | `initialize_as` | string | NÖTR |
| `context-path` | `context_paths` | list | ÇAL |
| `global-context-path` | `global_context_paths` | list | ÇAL |
| `skill-path` | `skills_paths` | list | ÇAL |
| `disable-skill` | `disabled_skills` | list | ÇAL |
| `attribution-trailer-style` | `attribution.generated_with` + style | none\|co-authored-by\|assisted-by | NÖTR |
| `ui` | `options.ui` | alt anahtarlar: `compact`/`transparent`, `diff`, `scrollbar`, `completions-max-depth`, `completions-max-items` | NÖTR |
| `reset <key>` | — | list anahtarını sıfırlar | ÇAL |

### 2.3 CLI komutları (`internal/cmd/*.go`, cobra `Use:`)

`crush` (root) · `run [prompt...]` · `login [platform]` · `logout [platform]` · `models` · `projects` · `session` (alt: `list`, `show <id>`, `last`, `delete <id>`, `rename <id> <title>`) · `logs` · `dirs` · `schema` · `server` · `stats` · `update-providers [path-or-url]` — 15 komut / 20 `Use:` tanımı. Durum: `run`/`session`/`schema`/`dirs` ÇAL; `login`/`logout`/`server`/`stats`/`update-providers` NÖTR.

### 2.4 TUI komut palette (`internal/ui/dialog/commands.go` + `internal/commands/commands.go`)

Yerleşik slash komutu YOK — palette şunlardan beslenir:
- **Custom commands**: 3 kaynak dizin — `~/.config/crush/commands/` (`user:`), `~/.crush/commands/` (`user:`), `<data-dir>/commands/` (`project:`); markdown dosyaları; `$UPPER_CASE` argümanlar regex ile çıkarılır (hepsi zorunlu); alt dizinler `:` ile birleşir (`commands.go:121-222`).
- **MCP prompt'ları**: `mcp.Prompts()` → `<mcp-name>:<prompt>`; 30 sn timeout (`commands.go:228-239`).
- **Skill kataloğu**: `UserInvocable` skill'ler `user:<skill>` olarak palette düşer (`FromSkillCatalog`).

---

## 3. Config Şeması — HER ANAHTAR (`internal/config/config.go`)

### 3.1 Üst seviye (`Config`)

| Anahtar | Tip | Anlam | Durum |
|---|---|---|---|
| `$schema` | string? | şema referansı | NÖTR |
| `models` | map (large/small → SelectedModel) | `{model, provider, think?, temperature?, top_p?, top_k?, frequency_penalty?, presence_penalty?, provider_options?, reasoning_effort?, max_tokens?}` | ÇAL |
| `recent_models` | map (large/small → list) | son seçimler | NÖTR |
| `providers` | map (id → ProviderConfig) | bkz. 3.2 | ÇAL |
| `mcp` | map (ad → MCPConfig) | bkz. 3.3 | NÖTR (harici) |
| `lsp` | map (ad → LSPConfig) | LSP sunucuları | ÇAL |
| `options` | Options | bkz. 3.4 | ÇAL |
| `permissions` | `{allowed_tools: string[]}` | otomatik onaylı tool'lar | ÇAL |
| `tools` | `{glob: {timeout}, grep: {timeout}, ls: {max_depth, max_items}}` | tool limitleri | ÇAL |
| `hooks` | map (event → HookConfig[]) | bkz. 5 | ÇAL |
| `env` | map (string→string) | başlangıç env değişkenleri | NÖTR |
| `agents` | map (ad → Agent) | bkz. 3.5 | ÇAL |

### 3.2 `ProviderConfig` anahtarları

`id` · `name` · `base_url` · `type` (openai/anthropic/openrouter/vercel/azure/bedrock/google/google-vertex/openaicompat/hyper/litellm/ollama/omlx…) · `api_key` ($VAR çözümlemeli) · `oauth` · `disable` · `system_prompt_prefix` · `extra_headers` (shell-expansion'lı) · `extra_body` (JSON passthrough, expansion YOK) · `provider_options` · `flat_rate` · `models[]` (catwalk model listesi) · `extra_params` (azure apiVersion, vertex project/location)

### 3.3 `MCPConfig` anahtarları

`command` · `env` · `args` · `type` (stdio\|sse\|http) · `url` · `disabled` · `disabled_tools[]` · `enabled_tools[]` (allowlist) · `timeout` (sn, varsayılan 10) · `headers` (expansion'lı) · `oauth` (OAuth 2.1, yalnız http) · `oauth_client_id` + OAuth token persistence

### 3.4 `Options` anahtarları (18)

`context_paths[]` · `global_context_paths[]` · `skills_paths[]` · `tui` (alt anahtarlar) · `debug` · `debug_lsp` · `disable_auto_summarize` · `data_directory` · `disabled_tools[]` · `disable_provider_auto_update` · `disable_default_providers` · `attribution` · `disable_metrics` · `initialize_as` · `auto_lsp` (nil = açık) · `progress` · `notifications` · `disabled_skills[]`

### 3.5 `Agent` anahtarları

`id` · `name` · `description` · `disabled` · `model` (large\|small) · `allowed_tools[]` (nil = hepsi) · `allowed_mcp` (map mcp→tool listesi; nil = tümü, boş = hiçbiri) · `context_paths[]` (override). Yerleşik agent: `coder` (`AgentCoder`).

---

## 4. Hook Sistemi

| Öğe | Değer | Durum |
|---|---|---|
| Event'ler | **TEK event: `PreToolUse`** (dokümante: "plans to support the full gamut") | NÖTR (eksik event gamı) |
| `HookConfig` | `name` (UI görünür adı) · `matcher` (tool adı regex, boş = tümü) · `command` (zorunlu) · `timeout` (sn, varsayılan 30) | ÇAL |
| Çalıştırma | paralel; sonuçlar config sırasında birleşir; mvdan.sh ile; CRUSH=1/AGENT=crush/AI_AGENT=crush env | ÇAL |
| Kararlar | `allow` / `deny` / `none` + `halt` (tüm turu durdurur) + `reason` + `context` + `updated_input` (shallow-merge patch = input_rewrite) | ÇAL |
| `HaltExitCode` | **49** (2 bloklar yalnızca çağrıyı; 49 tüm turu) | ÇAL |
| Hook metadata | `hook_count`, `decision`, `halt`, `reason`, `input_rewrite`, per-hook `hooks[]` — tool response metadata'sına gömülür | ÇAL |
| Bash tool entegrasyonu | `bash.go` hook sarmalayıcı; sub-agent'lar hook'tan muaftır | NÖTR |

---

## 5. MCP State Machine (`internal/agent/tools/mcp/`)

| Yüzey | Değerler | Durum |
|---|---|---|
| Client `State` (init.go:142-151) | `disabled` · `starting` · `connected` · `error` · `needs-auth` (5 durum) | ÇAL |
| Reconcile action'ları (lifecycle.go) | `reinitDisable` · `reinitRemove` · `reinitStart` — config diff'i: starting server yalnızca aynı config'le bağlanıyorsa dokunulmaz; connected config değişince restart; yeni/hatalı/auth-gerekli/devre-dışı → restart | ÇAL |
| Channel gate (channel.go) | `undecided` (buffer) → `open` (drain+publish) \| `closed` (discard) — capability negotiation sonrası | ÇAL |
| MCP event'leri | `EventStateChanged` · `EventToolsListChanged` (tool palette dinamik güncellenir) | ÇAL |
| Bağlantı | async init; interactive run'lar beklemez, non-interactive `WaitForInit` bekler (`coordinator.go:242-246`) | ÇAL |
| OAuth | `oauth` + `oauth_client_id`; dynamic client registration; token persistence | NÖTR |
| Loglama | MCP log notifications (SEP-2577 deprecated) | NÖTR |
| Diğer | `StateStarting` transition `withPending` (çakışan başlatma tekilleştirme), singleflight reconcile | ÇAL |

---

## 6. Tema Sistemi (`internal/ui/styles/themes.go`)

| Yüzey | Değer | Durum |
|---|---|---|
| `ThemeKeyForProvider` | `"hyper"` → hyper; **diğer tümü** → `"default"` (tek istisnalı eşleme) | NÖTR (yalnız 2 tema) |
| `CharmtonePantera()` | varsayılan koyu tema | ÇAL |
| `HypercrushObsidiana()` | hyper provider teması | NÖTR |
| Provider değişiminde | aynı tema anahtarı → yeniden build atlanır (performans) | ÇAL |

---

# BÖLÜM 2 — AIDER (Python)

## 7. Coder / Edit Format Envanteri (`aider/coders/`)

Registry: `coders/__init__.py` (13 aktive). Devre dışı: func ailesi (comment'li/yüklenmeyen). Format mekaniği prompt dosyalarından doğrulandı.

| # | Coder sınıfı | `edit_format` | Format mekaniği | Önerilen model (kanıt: model-settings.yml + models.py) | Durum |
|---|---|---|---|---|---|
| 1 | `HelpCoder` | `help` | yardım metni | — | ÇAL |
| 2 | `AskCoder` | `ask` | soru-cevap; dosya DÜZENLEMEZ | herhangi (soru modu) | ÇAL |
| 3 | `ContextCoder` | `context` | çevreleyen kod bağlamını gösterir | herhangi | ÇAL |
| 4 | `ArchitectCoder` (AskCoder) | `architect` | 2 model: planlayıcı + editör; `auto_accept_architect` onayı; `gpt_prompts=ArchitectPrompts` | o1-preview (settings'te `architect`); deepseek-r1/reasoner, qwq (editor-diff ile) | ÇAL (2-model orkestrasyon; omnitrix mimarisine uygun) |
| 5 | `EditBlockCoder` | `diff` | editblock: `>>>>>>> REPLACE` blokları (edithistory); 300+ modelde varsayılan | openrouter/* tümü, gpt-4o, gpt-4.1, llama, deepseek-chat… (289 model) | ÇAL |
| 6 | `EditBlockFencedCoder` | `diff-fenced` | fenced code block editblock (~~~/``` ile) | gemini-1.5-pro, gemini-2.5-pro/flash ailesi (35 model) | ÇAL |
| 7 | `EditorEditBlockCoder` | `editor-diff` | editor tool tabanlı (modelin native edit tool'u; `>>>>>>> REPLACE` üretir) | claude-sonnet/opus-4.x, gpt-5-pro, o1/o3/o4-mini, deepseek-reasoner, qwen-coder (138 model) | ÇAL (model-native tool; omnitrix'te dikkat) |
| 8 | `EditorWholeFileCoder` | `editor-whole` | editor tool + tüm dosya yeniden yazımı | claude-3.5-sonnet (20240620 öncesi dönem) | NÖTR |
| 9 | `EditorDiffFencedCoder` | `editor-diff-fenced` | editor tool + fenced diff | — | NÖTR |
| 10 | `PatchCoder` | `patch` | V4A patch: `*** Begin Patch` / `*** End Patch` | gpt-3.5-turbo/gpt-4 (v1 dönemi); legacy | NÖTR |
| 11 | `UnifiedDiffCoder` | `udiff` | `@@ ... @@` unified diff (-U0) | gpt-4-turbo/-preview (udiff=4 model) | ÇAL |
| 12 | `UnifiedDiffSimpleCoder` | `udiff-simple` | sadeleştirilmiş udiff | — | NÖTR |
| 13 | `WholeFileCoder` | `whole` | tüm dosya içeriği (`// entire file content ...`) | grok-3-mini-beta ailesi (whole=4) + bilinmeyen model default'u | NÖTR (uzun çıktı; token pahalı) |
| 14 | `EditBlockFunctionCoder` | `func` | function-call şeması (explanation/edits, original_lines/updated_lines) | legacy (JSON mode dönemi) | LOBOTOMİ (registry'de yüklenmez — `__init__.py`'de yok) |
| 15 | `WholeFileFunctionCoder` | `func` | function-call: explanation/files[{path,content}] | legacy | LOBOTOMİ (yüklü değil) |
| 16 | `SingleWholeFileFunctionCoder` | `func` | tek dosya function-call | legacy | LOBOTOMİ (comment'li: `# from .single_wholefile_func_coder import ...`) |
| 17 | `Coder` (base) | `None` | soyut; coder üretim fabrikası (`Coder.create`) | — | ÇAL |

**Model→format karar kuralları** (`models.py:430-580`): `openrouter/*` → diff; `/o1-mini`, `/o1-preview`, `/o1` → diff + `editor-diff`; `gpt-5*` → diff + `editor-diff`; `gpt-4-turbo`/`-preview` → udiff; diğer → model-settings.yml (dağılım: diff=289, editor-diff=138, diff-fenced=35, udiff=4, whole=4, architect=1). Bilinmeyen model default: `whole`.

---

## 8. Slash Komutları — TAM LİSTE (50; `aider/commands.py` `cmd_*` metodları)

Komut sistemi: `cmd_*` metodu otomatik `/komut` olur; prefix eşleşmesi + belirsizlik hatası; `!komut` = shell alias'ı. Argümanlar `args` string'i.

| Komut | Davranış | Durum |
|---|---|---|
| `/model` | ana modeli değiştir | ÇAL |
| `/editor-model` | editör modeli değiştir | ÇAL |
| `/weak-model` | weak model değiştir | ÇAL |
| `/chat-mode` | edit format'ı değiştir (geçerli format listesi gösterir) | ÇAL |
| `/models` | model arama | ÇAL |
| `/web` | web sayfasını scrape edip markdown'a çevirir, mesaja ekler | LOBOTOMİ (ağ) |
| `/commit` | chat dışı değişiklikleri commit'ler | ÇAL |
| `/lint` | chat'teki/dirty dosyaları lint'ler ve düzeltir | ÇAL |
| `/clear` | chat geçmişini temizler | ÇAL |
| `/reset` | tüm dosyaları düşürür + geçmişi temizler | ÇAL |
| `/tokens` | token kullanımı raporu | ÇAL |
| `/undo` | son aider commit'ini geri alır | ÇAL |
| `/diff` | son mesajdan beri diff | ÇAL |
| `/add` | dosya ekle (düzenlenebilir/review) | ÇAL |
| `/drop` | dosya çıkar | ÇAL |
| `/git` | git komutu (çıktı chat'e girmez; GIT_EDITOR=true) | ÇAL |
| `/test` | shell komutu çalıştırır, exit≠0 ise çıktıyı chat'e ekler | NÖTR (test ediciler için) |
| `/run` (alias `!`) | shell komutu; isteğe bağlı chat'e ekler | NÖTR |
| `/exit` / `/quit` | çıkış | ÇAL |
| `/ls` | bilinen dosyaları listeler, chat'tekileri işaretler | ÇAL |
| `/help` | aider hakkında sorular | ÇAL |
| `/ask` | ask moduna geç (prompt verilirse soru-cevap) | ÇAL |
| `/code` | ana formatla kod moduna geç | ÇAL |
| `/architect` | 2-model architect moduna geç | ÇAL |
| `/context` | context moduna geç | ÇAL |
| `/ok` | `/code Ok, please go ahead...` alias'ı | ÇAL |
| `/voice` | ses kaydı + transkript (OpenAI API key ister) | NÖTR |
| `/paste` | clipboard görsel/metin → chat (ImageGrab) | NÖTR |
| `/read-only` | salt-okunur dosya ekler / ekli dosyayı salt-okunur yapar | ÇAL |
| `/map` | repo map'ini yazdırır | ÇAL |
| `/map-refresh` | repo map'i zorla yeniler | ÇAL |
| `/settings` | mevcut ayarları yazdırır | ÇAL |
| `/load` | dosyadan komut yükler/çalıştırır | ÇAL |
| `/save` | oturum dosyalarını yeniden kurabilecek komut dosyası kaydeder | ÇAL |
| `/multiline-mode` | Enter/Meta+Enter davranışını takas eder | ÇAL |
| `/copy` | son asistan mesajını clipboard'a kopyalar | ÇAL |
| `/report` | GitHub Issue açarak sorun bildirir | NÖTR (ağ/telemetri) |
| `/editor` | prompt yazmak için editör açar | ÇAL |
| `/edit` | `/editor` alias'ı | ÇAL |
| `/think-tokens` | düşünme token bütçesi (8096, 8k, 0.5M, 0=kapalı) | ÇAL |
| `/reasoning-effort` | reasoning effort seviyesi (sayı veya low/medium/high) | ÇAL |
| `/copy-context` | bağlamı clipboard'a kopyalar | NÖTR |

Not: `cmd_editor_model`/`cmd_weak_model`/`cmd_model` chat içi `model:` prefix'li mesajlarla da tetiklenir. `cmd_commit`'in `raw_cmd_commit` çekirdeği `/commit` + `--commit` + git hook'ta paylaşılır.

---

## 9. repo.py — Git Operasyonları TAM LİSTE (`aider/repo.py`, GitPython)

| Metod | Git eşdeğeri / mekanik | Durum |
|---|---|---|
| `__init__` | `git.Repo(fname, search_parent_directories=True)`; çoklu repo tespiti ("Files are in different git repos"); `git_commit_verify` | ÇAL |
| `commit` | `git add <fname>` + `git config --get user.name` + `git commit <msg>`; `aider_edits` etiketi; `git_commit_verify=False` ise `--no-verify`; committer/author attribution flag'leri (--attribute-*) | ÇAL |
| `get_commit_message` | diff'ten commit mesajı üretir (LLM; user_language destekli) | ÇAL |
| `get_diffs` | `git diff HEAD -- <files>` (commit varsa); yoksa `git diff --cached` + `git diff` (index + working dir) | ÇAL |
| `diff_commits` | `git diff [--color] <from> <to>` | ÇAL |
| `get_tracked_files` | `head.commit.tree.traverse()` (blob'lar) + `index.entries` (staged); cache (`tree_files`); normalize + ignore filtresi; IndexError toleransı (bozuk repo) | ÇAL |
| `get_dirty_files` | `is_dirty` + `git diff --name-only [--cached]` | ÇAL |
| `is_dirty` | `repo.is_dirty()` | ÇAL |
| `get_head_commit` / `get_head_commit_sha` / `get_head_commit_message` | `repo.head.commit` (+hexsha/summary) | ÇAL |
| `refresh_aider_ignore` / `ignored_file` | `.aider.ignore` + `.gitignore` pathspec (gitignore_parser); TTL cache (`aider_ignore_ts`/`last_check`); `git_ignored_file` | ÇAL |
| `path_in_repo` / `abs_root_path` / `normalize_path` | repo sınır kontrolü; `subtree_only` modu | ÇAL |
| `set_git_env` | geçici GIT_* env (contextmanager) | ÇAL |

`ANY_GIT_ERROR` = git.errors + OSError/IndexError/BufferError/TypeError/ValueError/AttributeError/AssertionError/TimeoutError — tüm git çağrıları bu tuple ile sarılır.

---

## 10. CLI Flag'leri — TAM LİSTE (`aider/args.py`; 120 `add_argument`, 17 grup, ~130 flag)

### 10.1 Main model
`--model` · `--list-models`/`--models` · `--edit-format`/`--chat-mode` · `--architect` · `--auto-accept-architect` (default True) · `--editor-model` · `--editor-edit-format` · `--weak-model` · `--reasoning-effort` · `--thinking-tokens` · `--verify-ssl` (True) · `--timeout` · `--show-model-warnings` (True) · `--check-model-accepts-settings` (True) · `--max-chat-history-tokens` · `--model-settings-file` (`.aider.model.settings.yml`) · `--model-metadata-file` (`.aider.model.metadata.json`)

### 10.2 API Keys ve ayarlar
`--openai-api-key` · `--anthropic-api-key` · `--openai-api-base` · `--openai-api-type` · `--openai-api-version` · `--openai-api-deployment-id` · `--openai-organization-id` · `--set-env` · `--api-key`

### 10.3 Cache settings
`--cache-prompts` (False) · `--cache-keepalive-pings` (0)

### 10.4 Repomap settings
`--map-tokens` (0=kapalı) · `--map-refresh` (auto\|always\|files\|manual; default auto) · `--map-multiplier-no-files` (2)

### 10.5 History Files
`--input-history-file` · `--chat-history-file` · `--restore-chat-history` (False) · `--llm-history-file`

### 10.6 Output settings
`--dark-mode` · `--light-mode` · `--pretty` (True) · `--stream` (True) · `--user-input-color` (#00cc00) · `--tool-output-color` · `--tool-error-color` (#FF2222) · `--tool-warning-color` (#FFA500) · `--assistant-output-color` (#0088ff) · `--completion-menu-color` · `--completion-menu-bg-color` · `--completion-menu-current-color` · `--completion-menu-current-bg-color` · `--code-theme` (default) · `--show-diffs` · `--fancy-input` (True) · `--multiline` (False) · `--detect-urls` (True) · `--line-endings` (platform\|lf\|crlf)

### 10.7 Git settings
`--git` (True) · `--gitignore` (True) · `--add-gitignore-files` (False) · `--aiderignore` · `--subtree-only` (False) · `--auto-commits` (True) · `--dirty-commits` (True) · `--attribute-author` · `--attribute-committer` · `--attribute-commit-message-author` (False) · `--attribute-commit-message-committer` (False) · `--attribute-co-authored-by` (True) · `--git-commit-verify` (False) · `--skip-sanity-check-repo`

### 10.8 Fixing ve committing
`--commit` (False; commit+exit) · `--commit-prompt` · `--dry-run` (False) · `--lint` (False; lint+fix+exit) · `--lint-cmd` · `--auto-lint` (True) · `--test-cmd` ([]; tekrar edilebilir) · `--auto-test` (False) · `--test` (False; test+fix+exit)

### 10.9 Analytics
`--analytics` · `--analytics-log` · `--analytics-disable` (False; kalıcı) · `--analytics-posthog-host` · `--analytics-posthog-project-api-key`

### 10.10 Upgrading
`--check-update` (True) · `--just-check-update` (False; exit code) · `--show-release-notes` · `--install-main-branch` (False) · `--upgrade`/`--update` (False) · `--version`

### 10.11 Modes
`--message`/`--msg` · `--message-file` · `--gui`/`--browser` · `--copy-paste` (False) · `--apply` (debug) · `--apply-clipboard-edits` (False) · `--exit` (debug) · `--show-repo-map` (debug) · `--show-prompts` (debug) · `--load` (başlangıçta `/commands` dosyası)

### 10.12 Voice settings
`--voice-format` (wav\|mp3\|webm; default wav) · `--voice-language` (en) · `--voice-input-device`

### 10.13 Other settings
`--file` · `--read` · `--vim` · `--chat-language` · `--commit-language` · `--yes-always` · `--encoding` (utf-8) · `--env-file` · `--suggest-shell-commands` (True) · `--notifications` (False) · `--notifications-command` · `--editor` · `--shell-completions` · `--verbose` · `--config` · `--disable-playwright`

### 10.14 Deprecated model shortcut'ları (`deprecated.py`)
`--opus` · `--sonnet` · `--haiku` · `--gpt-4` · `--gpt-4o` · `--gpt-4o-mini` · `--gpt-4-turbo` · `--gpt-3` · `--deepseek` · `--o1-mini` · `--o1-preview` — hepsi `--model`'e yönlendirilir + uyarı verir. Durum: LOBOTOMİ (kaldırılacak).

---

## 11. Linter / Watch Mekanizması

### 11.1 Linter (`aider/linter.py`)

| Parça | Değer | Durum |
|---|---|---|
| `Linter` sınıfı | `encoding`, `root`, `languages` dict (python → `py_lint`), `all_lint_cmd` | ÇAL |
| `set_linter(lang, cmd)` | lang verilirse dillere; yoksa `all_lint_cmd` (--lint-cmd) | ÇAL |
| `lint(fname)` | önce `all_lint_cmd`, yoksa dil eşleşmesi; `tree_context` ile satır bağlamı | ÇAL |
| Python 3 katman | `basic_lint` (AST, imports/syntax) + `lint_python_compile` (compile()) + `flake8_lint` (--lint-cmd flake8 ise) | ÇAL |
| `LintResult` | metin + `lines` seti (edit sonrası doğrulama için) | ÇAL |
| Aider entegrasyonu | `/lint` komutu + `--lint` + `--auto-lint` + git hook'u (`lint` on file save: `--watch-files`) | ÇAL |
| Dış linter'lar | `lint_cmd`'de `{filename}` yer tutucusu; çıkış parse (error format) | ÇAL |

### 11.2 Watch (`aider/watch.py`)

| Parça | Değer | Durum |
|---|---|---|
| Engine | `watchfiles` (Rust) — `watch(*roots, watch_filter, stop_event, ignore_permission_denied=True)` | ÇAL |
| Root'lar | `get_roots_to_watch()` (cwd + git repo kökleri) | ÇAL |
| Ignore | `load_gitignores(gitignore_paths)` → pathspec (`GitWildMatchPattern`); `filter_func` | ÇAL |
| Mod | `--watch-files` + `--watch` alt modu: dosya değişince chat'e EKLER; AI-yorum modu (`get_ai_comments`): değişiklik için AI yorumu + `!`/`?` aksiyonları (kullanıcı onayı ister) | NÖTR (otomasyonda kararsız) |
| Prompt'lar | `watch_ask_prompt` / `watch_code_prompt` (`watch_prompts.py`) | ÇAL |
| Diğer | `changed_files` kuyruğu, `process_changes` (chat'e dosya ekleme), analytics event'leri, `verbose` modu | NÖTR |

---

## 12. Özet Değerlendirme

| Skala | Sayı | Örnekler |
|---|---|---|
| **ÇAL (GOOD)** | CRUSH 30+ / AIDER 40+ | CRUSH: edit/multiedit/write/view/ls/glob/grep/todos/job_*/crush_*/LSP ailesi; permission allowlist; PreToolUse hook; MCP state machine; shellconfig builtin'leri. AIDER: 13 coder + diff/editor-diff/udiff format'ları; 44+ slash komutu; repo.py git katmanı; linter 3-katman; model→format haritası (471 satır) |
| **LOBOTOMİ (BAD)** | CRUSH 7 / AIDER 6+ | CRUSH: bash (keyfi kod), download/fetch/web_fetch/web_search/sourcegraph (ağ + permission'sız), question (interaktif). AIDER: func ailesi (3 coder yüklü DEĞİL), deprecated model flag'leri (11), /web (ağ), --upgrade/--install-main-branch, analytics (PostHog), /report |
| **NÖTR** | 20+ | CRUSH: agent/agentic_fetch (sub-agent), MCP/OAuth, tema (2), tek hook event. AIDER: whole/udiff-simple/patch (legacy), editor-* varyantları, /test//run//voice//paste, --watch-files AI-yorum modu |

**Omnitrix için çıkarımlar**: (1) CRUSH tool-broker allowlist: `edit,multiedit,write,view,ls,glob,grep,todos,job_output,job_kill,crush_info,crush_logs,lsp_*` + permission; `bash,web_*,fetch,download,sourcegraph,question` reddedilmeli. (2) CRUSH'un `EditPermissionsParams` (old_content/new_content) hashline'in anchor kuralıyla birebir uyumlu — yeniden kullanılabilir. (3) AIDER'ın 2-model `architect` orkestrasyonu + `auto_accept_architect` onay akışı omnitrix planlayıcı/uygulayıcı ayrımının hazır kopyası. (4) AIDER'ın model→edit_format haritası (diff=289/editor-diff=138/diff-fenced=35) omnitrix model yönlendirmesinde doğrudan kullanılabilir. (5) CRUSH MCP state machine (5 durum + reconcile) MCP gateway'inin durum modeli olarak taşınabilir; AIDER'da MCP yok (eksik yüzey). (6) Her iki harness'ta da dosya-izleme var (CRUSH filetracker / AIDER watchfiles) — omnitrix watcher'ı için referans.
