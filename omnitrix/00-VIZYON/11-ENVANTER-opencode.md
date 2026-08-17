# 11 — ENVANTER: opencode 1.18.18 (kaynak ağaç)

> Kaynak: `/home/void0x14/Documents/omnitrix/omnitrix/opencode/packages/` (opencode monorepo, `packages/opencode/package.json` → `"version": "1.18.18"`)
> Tarih: 2026-08-14 · Yöntem: **satır-satır TAM enumerate** — keyword grep + örnekleme YOK.

## 0. Kapsam Kanıtı (enumerate edilen dosyalar)

| Katman | Dosya | Kanıt |
|---|---|---|
| Tool registry | `src/tool/registry.ts` | 450 satır; 18 builtin tool init (satır 204–222), koşullu dahil etme (243), plugin/custom tool yükleme (178–199) |
| Tool'lar | `src/tool/{shell,read,write,edit,glob,grep,task,todo,webfetch,websearch,skill,apply_patch,question,plan,invalid,code-mode,lsp,truncate,external-directory}.ts`, `src/tool/shell/{id,prompt}.ts` | Her dosya baştan sona okundu; parametre şemaları `Schema.Struct` bloklarından |
| Komut registry | `src/command/index.ts` + `src/command/template/{initialize,review}.txt` | 178 satır; Default.INIT/REVIEW + config/MCP/skill kaynakları |
| Config şeması | `src/config/config.ts` + `packages/core/src/v1/config/{config,agent,command,permission}.ts` | 681 satır + 4 şema dosyası |
| Permission | `src/permission/{index,evaluate,arity}.ts` + `core/src/v1/permission.ts` + `schema/src/v1/permission.ts` | Rule/Request/Reply/Event tam tipleri |
| Agent'lar | `src/agent/agent.ts` + `src/agent/prompt/{compaction,explore,summary,title}.txt` + `generate.txt` | 453 satır; 7 yerleşik agent (satır 140–265) |
| Plugin hook'ları | `src/plugin/{index,shared,loader}.ts` + `packages/plugin/src/index.ts` | `Hooks` arayüzü (22 hook) |
| Runtime bayrakları | `src/effect/runtime-flags.ts` + `core/src/flag/flag.ts` | 57 env anahtarı |
| Env taraması | `grep process.env` → `opencode/src`, `core/src`, `cli/src` | 60+ benzersiz değişken |

Permission action kullanım kanıtı (grep): `read`×5, `edit`×5, `todowrite`×3, `external_directory`×3, `task`×2, `question`×2, `doom_loop`×2, `websearch`, `webfetch`, `skill`, `plan_exit`, `plan_enter`, `lsp`, `grep`, `glob`, `bash`, `workflow_tool_approval`.

**Değerlendirme ölçeği**: **ÇAL (GOOD)** = deterministik, kaynak-doğrulanabilir, omnitrix yeniden-kullanımına uygun. **LOBOTOMİ (BAD)** = otomasyonda güvenlik/telemetri/özerklik riski — kapalı tutulmalı (omnitrix tool-broker allowlist felsefesiyle çelişir). **NÖTR** = ikisi de değil / deneysel / koşullu.

---

## 1. Tool Envanteri (`src/tool/`)

Registry sırası (`registry.ts:226-244`); `permission action` = `ctx.ask({permission})` değeri.

| # | Tool ID | Parametreler (tip — anlam) | Davranış | Permission | Side-effect | Durum |
|---|---|---|---|---|---|---|
| 1 | `invalid` | `tool: string`, `error: string` | "Do not use"; geçersiz çağrıyı hata metniyle döndürür | — (ask yok) | yok | NÖTR |
| 2 | `question` | `questions: Question.Prompt[]` (mutable array) | Kullanıcıya soru sorar, cevapları prompt'a enjekte eder; yalnızca app/cli/desktop'ta açık (`registry.ts:202`) | `question` (default: deny) | interaktif UI; otomasyonda bloke | LOBOTOMİ (otomasyonda kilit noktası) |
| 3 | `bash` | `command: string` (zorunlu), `timeout?: PositiveInt` (ms), `workdir?: string` | tree-sitter parse (bash/PS/cmd), dosya-path taraması (rm/cp/mv/… setleri), çıktı truncate → `Global.Path.tmp`, `shell.env` hook, abort/timeout kill | `bash` + `external_directory` | süreç çalıştırma, dosya/ağ yan etkisi, keyfi kod | LOBOTOMİ (omnitrix personada bash kapalı — 04-raporuyla uyumlu) |
| 4 | `read` | `filePath: string`, `offset?: NonNegativeInt` (1-bazlı), `limit?: NonNegativeInt` (default 2000) | Dosya/dizin listeleme, binary-engel (ext+sniff, 50 KB cap), image/PDF → base64 attachment, LSP warm-up, `<system-reminder>` enjeksiyonu | `read` (`*.env`/`*.env.*` → ask) | salt okur; LSP touchFile | ÇAL |
| 5 | `glob` | `pattern: string`, `path?: string` (cwd default) | ripgrep glob, limit 100, mutlak yol çıktısı | `glob` | salt okur | ÇAL |
| 6 | `grep` | `pattern: string` (regex), `path?: string`, `include?: string` (örn. `*.js`) | ripgrep, limit 100, satır bazlı eşleşme çıktısı | `grep` | salt okur | ÇAL |
| 7 | `edit` | `filePath: string`, `oldString: string`, `newString: string`, `replaceAll?: boolean` | 10 aşamalı replacer zinciri (Simple→LineTrimmed→BlockAnchor→Whitespace→Indent→Escape→Trimmed→Context→Multi), Levenshtein eşikleri, BOM koruma, dosya lock (semaphore), format + LSP diag, watcher/FileSystem event | `edit` | dosya yazma + event + LSP | ÇAL (güçlü; omnitrix hashline alternatifi) |
| 8 | `write` | `content: string`, `filePath: string` | Tam dosya yazımı, BOM tespiti, format, watcher event, LSP diag, diğer dosyalar için 5-dosya diag cap | `edit` | dosya yazma | ÇAL |
| 9 | `task` | `description: string` (3–5 kelime), `prompt: string`, `subagent_type: string`, `task_id?: string` (devam), `command?: string`, `background?: boolean` (deneyeysel) | Subagent oturumu açar; `subagent_depth` limiti; çocuğa todowrite/task deny + `primary_tools` deny; background modunda job + otomatik sonuç enjeksiyonu | `task` | yeni oturum + model çağrısı | ÇAL |
| 10 | `webfetch` | `url: string`, `format?: text\|markdown\|html` (default markdown), `timeout?: number` (max 120 sn) | HTTP GET, 5MB cap, Accept-header formatı, Cloudflare 403 retry, image → attachment, HTML→markdown (turndown) | `webfetch` | ağ erişimi | LOBOTOMİ (ağ) |
| 11 | `todowrite` | `todos: Todo.Info[]` (mutable) | Oturum todo durumunu günceller; tam liste değişimi | `todowrite` | oturum durumu | ÇAL |
| 12 | `websearch` | `query: string`, `numResults?: number` (default 8), `livecrawl?: fallback\|preferred` (default fallback), `type?: auto\|fast\|deep` (default auto), `contextMaxCharacters?: number` (default 10000) | Provider seçimi: `OPENCODE_WEBSEARCH_PROVIDER` override → exa/parallel bayrakları → session checksum ile deterministik seçim; exa/parallel HTTP çağrısı (25 sn timeout) | `websearch` | ağ + harici API (EXA_API_KEY/PARALLEL_API_KEY) | LOBOTOMİ (ağ) |
| 13 | `skill` | `name: string` | SKILL.md + dizin dosyalarını (limit 10) prompt'a enjekte eder | `skill` | okur | ÇAL |
| 14 | `apply_patch` | `patchText: string` | `*** Begin Patch` formatı; add/update/delete/move; önce doğrulama (parse + hunk), sonra uygulama; BOM/format/LSP | `edit` | çoklu dosya yazma/silme/taşıma | ÇAL |
| 15 | `execute` (deneysel code-mode) | `code: string` | Sınırlı yorumlayıcıda orkestrasyon scripti; MCP child-tool çağrıları (per-tool ask), `tool.execute.before/after` hook'ları; progress/abort | `<mcp-tool-key>` (dinamik) | MCP çağrıları | NÖTR (deneysel, `OPENCODE_EXPERIMENTAL_CODE_MODE`) |
| 16 | `lsp` (deneysel) | `operation: Literals(ops)`, `filePath: string`, `line: Int ≥1`, `character: Int ≥1`, `query?: string` | LSP işlemleri (workspaceSymbol vb.) | `lsp` | salt okur | NÖTR (deneysel, default kapalı) |
| 17 | `plan_exit` (plan modu) | `{}` (boş) | Plan dosyası tamam → kullanıcıya onay sorusu → "build" agent'a geçer + sentetik user part enjekte eder | `plan_exit` | oturum agent değişimi | ÇAL (plan mode) |

**Dinamik/ek tool kaynakları** (registry.ts:178-199): (a) `.opencode/{tool,tools}/*.{js,ts}` — custom dosya tool'ları (`namespace_id`); (b) `plugin.tool` tanımları (Zod veya legacy JSON Schema args); (c) MCP katalog tool'ları. Permission: tool adına göre wildcard. Durum: NÖTR (dosya), LOBOTOMİ (harici MCP).

**Koşullu filtreler** (`registry.ts:286-298`): `websearch` yalnızca `providerID===opencode` veya exa/parallel bayrağıyla; `apply_patch` yalnızca `gpt-*` modellerinde (`usePatch`); o zaman `edit`/`write` gizlenir.

---

## 2. Komut Envanteri (`src/command/`)

Kaynak: `command/index.ts:70-152` — registry deterministik: 2 yerleşik + config + MCP prompt'ları + skill'ler.

| Komut | Kaynak | Açıklama | Hints | Subtask | Durum |
|---|---|---|---|---|---|
| `init` | yerleşik (`initialize.txt`) | "guided AGENTS.md setup" (worktree yolu enjekte) | `$1..` / `$ARGUMENTS` | hayır | ÇAL |
| `review` | yerleşik (`review.txt`) | "review changes [commit\|branch\|pr], defaults to uncommitted" | `$1..` / `$ARGUMENTS` | **evet** | ÇAL |
| `config.command.*` | kullanıcı config | `template` (zorunlu) + `description?/agent?/model?/variant?/subtask?` | template'ten türetilir | config'e bağlı | NÖTR |
| `mcp.prompts()` | MCP sunucuları | prompt adı; `$1..$n` argümanları; template lazily resolve edilir | argüman sayısı | hayır | LOBOTOMİ (harici) |
| `skill.all()` | skill dizinleri | skill adı; `<built-in>` dışındakilere "Base directory" eklenir | yok | hayır | ÇAL |

Not: `review` command'ında `subtask: true` — subagent üzerinde çalışır. TUI'deki `/init /review` + adlandırılmış komutlar bu registry'den beslenir.

---

## 3. Config Şeması (`core/src/v1/config/config.ts` + `src/config/config.ts`)

### 3.1 Üst seviye anahtarlar (`ConfigV1.Info`)

| Anahtar | Tip | Varsayılan | Anlam | Durum |
|---|---|---|---|---|
| `$schema` | string? | `https://opencode.ai/config.json` (eksikse yazılır) | şema referansı | NÖTR |
| `shell` | string? | platform | bash tool + terminal için shell; yazılırken `""` silinir | ÇAL |
| `logLevel` | `DEBUG\|INFO\|WARN\|ERROR`? | INFO | log seviyesi | ÇAL |
| `server` | Server? | — | `opencode serve`/web için sunucu config (port, cors, vs.) | NÖTR |
| `command` | Record<string, Command.Info> | {} | slash komut tanımları | ÇAL |
| `skills` | Skills.Info? | — | ekstra skill klasör yolları | ÇAL |
| `references` / `reference` | Reference.Info? | — | adlandırılmış git/yerel dizin referansları (`reference` **deprecated**) | NÖTR |
| `watcher` | {ignore?: string[]}? | — | dosya izleyici ignore desenleri | ÇAL |
| `snapshot` | boolean? | **true** | snapshot kaydı; false → undo/redo dosya değişiklikleri çalışmaz | ÇAL |
| `plugin` | Plugin.Spec[]? | [] | plugin spec listesi (npm/path/URL) | NÖTR |
| `share` | `manual\|auto\|disabled`? | manual | paylaşım davranışı | LOBOTOMİ (telemetri/paylaşım) |
| `autoshare` | boolean? | — | **deprecated** → `share`; `true` ise `share:"auto"` türetilir | LOBOTOMİ |
| `autoupdate` | boolean \| `"notify"`? | — | otomatik güncelleme | LOBOTOMİ |
| `disabled_providers` | string[]? | [] | otomatik yüklenen provider'ları kapatır | ÇAL |
| `enabled_providers` | string[]? | — | set edilirse YALNIZCA bunlar | ÇAL |
| `model` | string? | — | `provider/model` formatı | ÇAL |
| `small_model` | string? | — | başlık üretimi vb. küçük model | ÇAL |
| `default_agent` | string? | build | primary olmalı; geçersizse build | ÇAL |
| `subagent_depth` | NonNegativeInt? | **1** | subagent iç içe derinlik (1 = subagent subagent açamaz) | ÇAL |
| `username` | string? | `os.userInfo().username` | görünen kullanıcı adı | NÖTR |
| `mode` | Record<string, Agent.Info>? | — | **deprecated** → `agent`; `mode:"primary"` enjekte edilir | NÖTR |
| `agent` | Record<string, Agent.Info> | {} | agent tanımları (bkz. 3.2) | ÇAL |
| `provider` | Record<string, Provider.Info>? | — | custom provider + model override'ları | ÇAL |
| `mcp` | Record<string, MCP.Info \| {enabled: boolean}>? | — | MCP sunucu config'leri | LOBOTOMİ (harici sunucu) |
| `formatter` | false \| true \| object? | — | formatlayıcılar (biome/oxfmt/prettier…) | ÇAL |
| `lsp` | false \| true \| object? | — | LSP sunucuları | NÖTR |
| `instructions` | string[]? | [] | AGENTS.md dışı talimat dosyaları (merge'de Set-birleşimi) | ÇAL |
| `layout` | Layout? | stretch | **deprecated** — her zaman stretch | NÖTR |
| `permission` | Permission.Info? | — | bkz. 3.4 | ÇAL |
| `tools` | Record<string, boolean>? | — | kısayol: `true→allow`, `false→deny`; write/edit/patch → `edit` anahtarına | ÇAL |
| `attachment` | Attachment.Info? | — | görsel boyut limitleri/resize | ÇAL |
| `enterprise` | {url?: string}? | — | enterprise URL | LOBOTOMİ (bağlantı) |
| `tool_output` | {max_lines?, max_bytes?}? | 2000 / 51200 | tool çıktısı truncate eşikleri → truncation dizinine yazılır | ÇAL |
| `compaction` | {auto?, prune?, tail_turns?, preserve_recent_tokens?, reserved?}? | auto:true, prune:false | context dolunca otomatik sıkıştırma | ÇAL |
| `experimental` | Experimental? | — | bkz. 3.5 | NÖTR |

### 3.2 Agent anahtarları (`ConfigAgentV1.Info`)

| Anahtar | Tip | Anlam | Durum |
|---|---|---|---|
| `model` | string? | `provider/model` | ÇAL |
| `variant` | string? | model varyantı (yalnızca agent modeliyle) | ÇAL |
| `temperature` / `top_p` | Finite? | sampling | ÇAL |
| `prompt` | string? | sistem prompt (yerleşikler prompt dosyası referansı) | ÇAL |
| `tools` | Record<string, boolean>? | **deprecated** → permission'a normalize (write/edit/patch→edit) | NÖTR |
| `disable` | boolean? | yerleşik agent'ı devre dışı bırakır (örn. `build`) | ÇAL |
| `description` | string? | ne zaman kullanılacağı | ÇAL |
| `mode` | `subagent\|primary\|all`? | rol; yeni agent default "all" | ÇAL |
| `hidden` | boolean? | @ menüsünden gizler | ÇAL |
| `options` | Record<string, any>? | bilinmeyen anahtarlar buraya düşer (provider options) | NÖTR |
| `color` | #hex \| tema rengi? | UI rengi | NÖTR |
| `steps` / `maxSteps` | PositiveInt? | agentic iterasyon tavanı (`maxSteps` deprecated) | ÇAL |
| `permission` | Permission.Info? | agent'a özgü kurallar | ÇAL |

### 3.3 Komut anahtarları (`ConfigCommandV1.Info`)

`template: string` (zorunlu) · `description?` · `agent?` · `model?` · `variant?` · `subtask?: boolean`

### 3.4 Permission config (`ConfigPermissionV1`)

- `Action = "ask" | "allow" | "deny"`
- Değer: tek string (= `"*"` için) veya `Record<pattern, action>` (pattern `~/`/`$HOME` genişletilir)
- Bilinen anahtarlar: `read, edit, glob, grep, list, bash, task, external_directory, todowrite, question, webfetch, websearch, lsp, doom_loop, skill` + serbest ekler

### 3.5 Deneysel anahtarlar (`experimental.*`)

| Anahtar | Tip | Anlam | Durum |
|---|---|---|---|
| `disable_paste_summary` | boolean? | yapıştırma özetini kapatır | NÖTR |
| `batch_tool` | boolean? | batch tool'u açar | NÖTR |
| `openTelemetry` | boolean? | AI SDK telemetry span'ları | LOBOTOMİ (telemetri) |
| `primary_tools` | string[]? | yalnızca primary agent'lara açık tool'lar; subagent'a deny eklenir | ÇAL |
| `continue_loop_on_deny` | boolean? | deny'de agent döngüsünü sürdürür | ÇAL |
| `mcp_timeout` | PositiveInt? | MCP istek timeout'u (ms) | ÇAL |
| `policies` | Policy[]? | provider erişimi gibi kaynaklara policy | ÇAL |

---

## 4. Permission Action'ları (`core/src/v1/permission.ts` + `schema/src/v1/permission.ts` + `src/permission/`)

| Öğe | Tanım | Durum |
|---|---|---|
| `Action` | `allow \| deny \| ask` | ÇAL |
| `Rule` | `{permission: string, pattern: string, action: Action}` | ÇAL |
| `Ruleset` | Rule[]; değerlendirme: `findLast` (en son kural kazanır) + `Wildcard.match`; varsayılan `ask` | ÇAL |
| `Request` | `{id: per_…, sessionID, permission, patterns[], metadata, always[], tool?}` | ÇAL |
| `Reply` | `once \| always \| reject` (+ `message?` feedback) | ÇAL |
| `Approval` | `{projectID, patterns[]}` | ÇAL |
| Event'ler | `permission.asked`, `permission.replied` | ÇAL |
| Hatalar | `DeniedError` (kural var), `RejectedError` (kullanıcı reddi), `CorrectedError` (feedback'li red), `NotFoundError` | ÇAL |
| `reject` yan etkisi | aynı session'daki tüm pending istekler iptal edilir | ÇAL |
| `always` yan etkisi | `approved` listesine `{permission, pattern, allow}` eklenir; aynı session'da kapsanan pending'ler otomatik onaylanır | ÇAL |
| `disabled()` | `edit` → edit/write/apply_patch; `read` → list_mcp_resources/resource_templates/read_mcp_resource; pattern `*`+deny → tool gizlenir | ÇAL |
| Kullanımdaki action anahtarları (grep kanıtı) | `read, edit, glob, grep, bash, task, webfetch, websearch, skill, todowrite, question, external_directory, lsp, doom_loop, plan_enter, plan_exit, list` + MCP tool anahtarları + `workflow_tool_approval` | ÇAL |
| Varsayılan kural seti (`agent.ts:119-136`) | `*: allow`, `doom_loop: ask`, `external_directory: ask` (+ whitelist: truncation dir, tmp, skill/reference dirs), `question: deny`, `plan_enter/plan_exit: deny`, `read: "*.env"→ask, "*.env.*"→ask, "*.env.example"→allow` | ÇAL |

---

## 5. Agent Tipleri (`src/agent/agent.ts`)

| Agent | Mode | Native | Hidden | Model/Prompt | Permission özellikleri | Durum |
|---|---|---|---|---|---|---|
| `build` | primary | ✓ | — | — | defaults + `question: allow`, `plan_enter: allow` | ÇAL |
| `plan` | primary | ✓ | — | — | defaults + `question/plan_exit: allow`, `task: deny`, `edit: "*"→deny` (yalnızca `.opencode/plans/*.md` + global plans izinli), `external_directory: plans/* → allow` | ÇAL |
| `general` | subagent | ✓ | — | — | defaults + `todowrite: deny` | ÇAL |
| `explore` | subagent | ✓ | — | prompt `explore.txt` | `"*": deny` + `grep/glob/list/bash/webfetch/websearch/read: allow` + `external_directory` read-only whitelist | ÇAL |
| `compaction` | primary | ✓ | ✓ | prompt `compaction.txt` | defaults + `"*": deny` | ÇAL |
| `title` | primary | ✓ | ✓ | prompt `title.txt`, `temperature: 0.5` | defaults + `"*": deny` | ÇAL |
| `summary` | primary | ✓ | ✓ | prompt `summary.txt` | defaults + `"*": deny` | ÇAL |

Notlar: (a) `Agent.Info` şeması: name, description?, mode, native?, hidden?, topP?, temperature?, color?, permission, model?{modelID,providerID}, variant?, prompt?, options, steps? (agent.ts:35-55). (b) Kullanıcı agent'ları default `mode:"all"`, `native:false`; `disable:true` yerleşiği siler. (c) `Truncate.GLOB` her agent'a zorla `external_directory: allow` eklenir. (d) `default_agent` seçimi: primary + hidden olmayan. (e) `generate()`: `PROMPT_GENERATE` ile yeni agent JSON üretimi (temperature 0.3, OpenTelemetry opsiyonel, openai-oauth istisnası).

---

## 6. Plugin Hook'ları (`packages/plugin/src/index.ts` → `Hooks`)

| Hook | İmza (input → output) | Tetiklenme yeri | Durum |
|---|---|---|---|
| `dispose` | () → Promise<void> | oturum sonunda | ÇAL |
| `event` | {event: Event} | her legacy event (directory eşleşmesi) | ÇAL |
| `config` | Config | plugin yükleme sonunda, tüm plugin'ler | ÇAL |
| `tool` | Record<string, ToolDefinition> | tool kayıtları | ÇAL |
| `auth` | AuthHook (oauth/api methods) | auth akışları (codex, copilot, gitlab, poe, cloudflare, azure, do, snowflake, xai) | NÖTR |
| `provider` | ProviderHook {id, models?} | provider model listesi | ÇAL |
| `chat.message` | {sessionID, agent?, model?, messageID?, variant?} → {message, parts} | yeni mesaj | ÇAL |
| `chat.params` | {sessionID, agent, model, provider, message} → {temperature, topP, topK, maxOutputTokens, options} | LLM parametreleri | ÇAL |
| `chat.headers` | aynı input → {headers} | LLM HTTP başlıkları | ÇAL |
| `permission.ask` | Permission → {status: ask\|deny\|allow} | her izin sorusu | ÇAL |
| `command.execute.before` | {command, sessionID, arguments} → {parts} | slash komut öncesi | ÇAL |
| `tool.execute.before` | {tool, sessionID, callID} → {args} | tool çağrısı öncesi (code-mode child dahil) | ÇAL |
| `shell.env` | {cwd, sessionID?, callID?} → {env} | bash tool env'si | ÇAL |
| `tool.execute.after` | {tool, sessionID, callID, args} → {title, output, metadata} | tool çağrısı sonrası | ÇAL |
| `experimental.chat.messages.transform` | {} → {messages} | mesaj dönüşümü | NÖTR |
| `experimental.chat.system.transform` | {sessionID?, model} → {system} | sistem prompt dönüşümü (agent generate'de de) | NÖTR |
| `experimental.provider.small_model` | {provider} → {model?} | small model override | NÖTR |
| `experimental.session.compacting` | {sessionID} → {context[], prompt?} | compaction prompt'u | NÖTR |
| `experimental.compaction.autocontinue` | {sessionID, agent, model, provider, message, overflow} → {enabled} | sentetik "continue" turu | NÖTR |
| `experimental.text.complete` | {sessionID, messageID, partID} → {text} | text part tamamlama | NÖTR |
| `tool.definition` | {toolID} → {description, parameters} | LLM'e giden tool tanımı (registry.ts:313) | ÇAL |

Yerleşik (internal) plugin'ler (`plugin/index.ts:66-84`): CodexAuth, CopilotAuth, Modal, GitlabAuth, PoeAuth, CloudflareWorkers, CloudflareAIGateway, Azure, DigitalOcean, SnowflakeCortex, XaiAuth. `OPENCODE_DISABLE_DEFAULT_PLUGINS` ile kapatılır.

---

## 7. Env Değişkenleri (tam liste)

### 7.1 opencode çekirdek (flag.ts + runtime-flags.ts + config.ts)

| Değişken | Anlam | Durum |
|---|---|---|
| `OPENCODE_CONFIG` | ek config dosyası yolu | ÇAL |
| `OPENCODE_CONFIG_DIR` | config dizini | ÇAL |
| `OPENCODE_CONFIG_CONTENT` | config JSON içeriği (string) | ÇAL |
| `OPENCODE_PERMISSION` | JSON permission merge | ÇAL |
| `OPENCODE_DISABLE_PROJECT_CONFIG` | proje config'ini kapatır | ÇAL |
| `OPENCODE_DISABLE_AUTOCOMPACT` | compaction.auto=false | ÇAL |
| `OPENCODE_DISABLE_PRUNE` | compaction.prune=false | ÇAL |
| `OPENCODE_DISABLE_AUTOUPDATE` / `OPENCODE_ALWAYS_NOTIFY_UPDATE` | güncelleme kontrolü | LOBOTOMİ (dış ağ) |
| `OPENCODE_DISABLE_TERMINAL_TITLE`, `OPENCODE_SHOW_TTFD` | terminal başlığı/ölçüm | NÖTR |
| `OPENCODE_DISABLE_MODELS_FETCH`, `OPENCODE_MODELS_URL`, `OPENCODE_MODELS_PATH` | model listesi kaynağı | NÖTR |
| `OPENCODE_DISABLE_MOUSE` | TUI fare | NÖTR |
| `OPENCODE_FAKE_VCS` | VCS taklidi | NÖTR |
| `OPENCODE_SERVER_PASSWORD` / `OPENCODE_SERVER_USERNAME` | HTTP sunucu kimlik doğrulaması | NÖTR |
| `OPENCODE_DISABLE_FFF`, `OPENCODE_EXPERIMENTAL_DISABLE_FFF` | fast-file-search kapatma (win default) | NÖTR |
| `OPENCODE_DB` | veritabanı yolu | ÇAL |
| `OPENCODE_WORKSPACE_ID`, `OPENCODE_EXPERIMENTAL_WORKSPACES` | workspace | NÖTR |
| `OPENCODE_EXPERIMENTAL` (master) + `OPENCODE_EXPERIMENTAL_*` alt bayrakları | deneysel özellikler (FILEWATCHER, REFERENCES, BACKGROUND_SUBAGENTS, LSP_TOOL, OXFMT, PLAN_MODE, CODE_MODE, EVENT_SYSTEM, ICON_DISCOVERY, WEBSOCKETS, NATIVE_LLM, OUTPUT_TOKEN_MAX, BASH_DEFAULT_TIMEOUT_MS, LSP_TY, COPY_ON_SELECT) | NÖTR |
| `OPENCODE_TUI_CONFIG` | TUI config yolu | ÇAL |
| `OPENCODE_PURE` | plugin'leri kapatır (pure mod) | ÇAL |
| `OPENCODE_PLUGIN_META_FILE` | plugin meta dosyası | NÖTR |
| `OPENCODE_CLIENT` | `cli\|app\|desktop\|...` (default cli) — question tool'u etkiler | ÇAL |
| `OPENCODE_AUTO_SHARE` | otomatik paylaşım | LOBOTOMİ |
| `OPENCODE_DISABLE_DEFAULT_PLUGINS`, `OPENCODE_DISABLE_EMBEDDED_WEB_UI`, `OPENCODE_DISABLE_EXTERNAL_SKILLS`, `OPENCODE_DISABLE_LSP_DOWNLOAD` | devre dışı bırakma bayrakları | ÇAL |
| `OPENCODE_DISABLE_CLAUDE_CODE` / `_PROMPT` / `_SKILLS` | claude-code prompt/skill engeli | ÇAL |
| `OPENCODE_ENABLE_EXA`, `OPENCODE_EXPERIMENTAL_EXA`, `OPENCODE_ENABLE_PARALLEL`, `OPENCODE_EXPERIMENTAL_PARALLEL`, `OPENCODE_WEBSEARCH_PROVIDER` | websearch provider seçimi | NÖTR |
| `OPENCODE_ENABLE_EXPERIMENTAL_MODELS`, `OPENCODE_ENABLE_QUESTION_TOOL` | özellik açma | NÖTR |
| `OPENCODE_AUTO_HEAP_SNAPSHOT`, `OPENCODE_GIT_BASH_PATH` | hata ayıklama | NÖTR |
| `OPENCODE_LOG_LEVEL`, `OPENCODE_PRINT_LOGS`, `OPENCODE_DIRECT_TRACE`, `OPENCODE_DISABLE_CHANNEL_DB`, `OPENCODE_PID` | loglama | NÖTR |
| `OPENCODE_API_KEY`, `OPENCODE_AUTH_CONTENT`, `OPENCODE_ACP_PROFILE`, `OPENCODE_CONSOLE_TOKEN`, `OPENCODE_REPO_CLONE_GITHUB_BASE_URL` | kimlik doğrulama/ACP | NÖTR |
| `OPENCODE_TEST_HOME`, `OPENCODE_TEST_MANAGED_CONFIG_DIR` | test | NÖTR |
| `OTEL_EXPORTER_OTLP_ENDPOINT/HEADERS`, `OTEL_RESOURCE_ATTRIBUTES` | OpenTelemetry export | LOBOTOMİ (telemetri) |

### 7.2 Provider/ortam

`EXA_API_KEY`, `PARALLEL_API_KEY`, `GITLAB_INSTANCE_URL`, `GITLAB_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`, `CLOUDFLARE_API_KEY`, `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_GATEWAY_ID`, `CF_AIG_TOKEN`, `AZURE_COGNITIVE_SERVICES_RESOURCE_NAME`, `AZURE_RESOURCE_NAME`, `AWS_PROFILE`, `AWS_REGION`, `AWS_BEARER_TOKEN_BEDROCK`, `AWS_CONTAINER_CREDENTIALS_FULL_URI`, `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`, `GCLOUD_PROJECT`, `GCP_PROJECT`, `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`, `GOOGLE_VERTEX_PROJECT`, `GOOGLE_VERTEX_LOCATION`, `VERTEX_LOCATION`, `SNOWFLAKE_CORTEX_PAT`, `SNOWFLAKE_CORTEX_TOKEN`, `MODAL_PROXY_TOKEN`, `DOTNET_CLI_HOME`, `AICORE_DEPLOYMENT_ID`, `AICORE_RESOURCE_GROUP`, `AICORE_SERVICE_KEY` — hepsi NÖTR/LOBOTOMİ (harici sağlayıcı kimlikleri; omnitrix tarafında yalnızca allowlist dışı kullanım).

### 7.3 Sistem

`PATH`, `PWD`, `HOME`, `SHELL`, `TERM`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `COMSPEC`, `PATHEXT`, `XDG_CONFIG_HOME`, `VSCODE_EXTENSIONS`, `AGENT`, `OPENCODE` — ÇAL (shell ortamı).

---

## 8. Özet Değerlendirme

| Skala | Sayı | Örnekler |
|---|---|---|
| **ÇAL (GOOD)** | 25+ | read/glob/grep/edit/write/task/todowrite/skill/apply_patch; init/review komutları; permission motoru (Rule+Wildcard+Reply); 7 yerleşik agent; tool_output/compaction/subagent_depth/snapshot; shell.env/tool.definition/tool.execute.* hook'ları |
| **LOBOTOMİ (BAD)** | 12 | bash (keyfi kod), webfetch/websearch (ağ), question (interaktif), share/autoshare/autoupdate (telemetri/dış), mcp (harici sunucu), enterprise.url, openTelemetry+OTEL_*, provider env kimlikleri, autoupdate |
| **NÖTR** | 15+ | execute/lsp (deneysel), plan_exit, mode/layout/reference/tools (deprecated), experimental.*, auth plugin'leri, TUI bayrakları |

**Omnitrix için çıkarımlar**: (1) Tool-broker allowlist'i `read,glob,grep,edit,write,todowrite,skill,apply_patch` + `task` ile sınırlanabilir; `bash/websearch/webfetch/question` reddedilmeli. (2) `subagent_depth`, `primary_tools`, `tool_output`, `compaction`, `snapshot` hazır güvenlik/verim anahtarlarıdır. (3) Permission motoru zaten "son kural kazanır + wildcard + ask-default" modelini taşır — omnitrix `capability_audit` fikriyle uyumlu değil (deny yerine ask öncelikli), adaptasyon gerekir. (4) Plugin hook'ları (22 adet) orkestrasyon katmanının izleme/kısıtlama noktaları olarak doğrudan yeniden kullanılabilir.
