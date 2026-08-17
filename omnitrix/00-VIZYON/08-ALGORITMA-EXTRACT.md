# 08 — ALGORİTMA EXTRACT (İnşa Şartnamesi)

> Karar seviyesi: `05-SENTEZ-feature-matrix.md`. Bu belge, o matristeki özelliklerin **inşa edilebilir mekanizma** tanımlarıdır: girdi/çıktı, adım adım algoritma (pseudokod), tüm sabitler/eşikler, kenar durumlar ve kaynak dosya:satır kanıtları.
>
> Kapsam (6 kaynak):
> 1. Claude Code microcompact (istemci + API katmanı)
> 2. opencode compaction (prune + tail-turn select)
> 3. codex execpolicy DSL
> 4. aider repo map (pagerank + binary-search bütçe)
> 5. oh-my-pi hashline (anchor hash + patch dili + stale-anchor reddi)
> 6. pi cut-point (findCutPoint ailesi)

---

## 1. Claude Code Microcompact

### 1.1 Genel bakış — iki ayrı mekanizma

`microcompactMessages(messages, toolUseContext, querySource)` API çağrısından **önce** çalışır (`microCompact.ts:253-293`). Karar sırası:

1. **Zaman-tabanlı MC** önce denenir ve tetiklenirse **kısa devre** yapar (döner, başka yola geçilmez). Gerekçe: son assistant mesajı çok eskiyse sunucu prompt cache'i çoktan soğumuştur; prefix zaten yeniden yazılacak → eski tool sonuçlarını şimdi temizlemek rewrite edilecek boyutu küçültür.
2. Zaman-tabanlı tetiklenmezse **Cached MC** denenir (feature flag `CACHED_MICROCOMPACT`, ant-only, model desteği, ana thread kısıtı).
3. Hiçbiri yoksa mesajlar olduğu gibi döner (legacy microcompact yolu silinmiş; `tengu_cache_plum_violet` her zaman true).

### 1.2 Girdi / Çıktı

- **Girdi:** `messages: Message[]`, `toolUseContext?`, `querySource?` (`'repl_main_thread'` veya `'repl_main_thread:outputStyle:<style>'`)
- **Çıktı:** `{ messages, compactionInfo?: { pendingCacheEdits?: { trigger:'auto', deletedToolIds, baselineCacheDeletedTokens } } }` (`microCompact.ts:215-220`)
  - Zaman-tabanlı yol: `messages` içeriği **mutasyona uğrar** (content temizlenir).
  - Cached MC yolu: `messages` **değişmez**; API katmanına `cache_edits` bloğu kuyruğa alınır.

### 1.3 COMPACTABLE_TOOLS (hangi tool sonuçları sıkıştırılabilir)

`microCompact.ts:41-50`:

```
{ FileRead, ...SHELL_TOOL_NAMES, Grep, Glob, WebSearch, WebFetch, FileEdit, FileWrite }
```

Not: `FileEdit`/`FileWrite` cached-MC kaydında yer alır; API tarafı stratejide ayrıca ele alınır (§1.6).

### 1.4 Token tahmin formülü

**Ham formül** (`tokenEstimation.ts:203-210`):

```
roughTokenCountEstimation(content, bytesPerToken=4) = round(len(content) / 4)
bytesPerTokenForFileType: json/jsonl/jsonc → 2, diğer → 4
```

**Mesaj düzeyi** (`estimateMessageTokens`, `microCompact.ts:164-205`): her content bloğu için:

| Blok | Katkı |
|------|-------|
| text | rough(content.text) |
| tool_result (string) | rough(content) |
| tool_result (array) | text → rough; image/document → **2000** (sabit); diğer → 0 |
| image/document | **2000** (sabit) |
| thinking | rough(block.thinking) — JSON wrapper/signature SAYILMAZ |
| redacted_thinking | rough(block.data) |
| tool_use | rough(name + jsonStringify(input)) — id ve wrapper sayılmaz |
| diğer (server_tool_use vs.) | rough(jsonStringify(block)) |

**Güvenlik payı:** toplam `× 4/3` ile çarpılır, `ceil` (`microCompact.ts:203-204`). Neden: yaklaşıksal olduğu için muhafazakâr ol.

`calculateToolResultTokens(block)` (`microCompact.ts:138-157`): content yoksa 0.

### 1.5 Zaman-tabanlı microcompact (time-based MC)

**Config** (`timeBasedMCConfig.ts:18-34`), GrowthBook key `tengu_slate_heron`:

```
{ enabled: false (varsayılan), gapThresholdMinutes: 60, keepRecent: 5 }
```

**Tetikleyici** (`evaluateTimeBasedTrigger`, `microCompact.ts:422-444`):

```
girdi: messages, querySource
1. config = getTimeBasedMCConfig()
2. değilse null:  !config.enabled
                 || !querySource                  // /context, /compact, analyzeContext gibi
                                                  // kaynaksız çağrılar ASLA tetiklemez
                 || !querySource.startsWith('repl_main_thread')
3. lastAssistant = messages.findLast(m => m.type === 'assistant'); yoksa null
4. gapMinutes = (Date.now() - new Date(lastAssistant.timestamp)) / 60_000
5. null döndür: !Number.isFinite(gapMinutes) || gapMinutes < 60
6. döndür { gapMinutes, config }
```

**Temizleme** (`maybeTimeBasedMicrocompact`, `microCompact.ts:446-530`):

```
1. trigger yoksa null
2. compactableIds = collectCompactableToolIds(messages)   // sıralı: assistant tool_use id'leri
3. keepRecent = max(1, config.keepRecent)                 // Taban 1: slice(-0) tüm diziyi
                                                          // döndürür (her şey kalır), 0 ise
                                                          // model sıfır bağlamla kalır — ikisi de saçma
4. keepSet  = son keepRecent id (sondan dilim)
   clearSet = compactableIds - keepSet
5. clearSet boşsa null
6. Her user mesajının tool_result bloklarından clearSet'teki id'leri
   'content' = TIME_BASED_MC_CLEARED_MESSAGE  ('[Old tool result content cleared]', satır 36)
   olarak DEĞİŞTİR; zaten temizlenmiş olanı tekrar sayma
   (block.content !== TIME_BASED_MC_CLEARED_MESSAGE koşulu)
7. tokensSaved = toplam kazanç (calculateToolResultTokens); 0 ise null
8. Yan etkiler:
   - resetMicrocompactState()  // cached-MC state'teki id'ler artık sunucuda yok;
                              // sonraki tur stale cache_edit denemesin diye sıfırla
   - notifyCacheDeletion(querySource)  // prompt-cache-break dedektörüne
                                       // "düşüş bizim, break değil" de
   - suppressCompactWarning()
9. döndür { messages: result }  // içerik değişti; cache READ düşer, kabul edilir
```

**Kenar durumlar:** son satırda `slice(-keepRecent)` için keepRecent=0 → `slice(-0)` tüm dizi → hiçbir şey silinmez; `Math.max(1, …)` bunu engeller. Timestamp parse edilemezse (NaN gap) tetiklenmez. Zaman-tabanlı MC çalıştıysa cached-MC state'i **mutlaka resetlenir** çünkü içerik değişti → sunucu cache'i bozuldu → cached MC'nin cache_edit varsayımları geçersiz (`microCompact.ts:511-517`).

### 1.6 Cached MC (cache_edits / cache_reference API kullanımı)

**Modül:** `cachedMicrocompact.ts` **leak'te yok**; kullanım sözleşmesi `microCompact.ts` üzerinden çıkarılır (fonksiyon isimleri ve log alanları):

- `isCachedMicrocompactEnabled()` — GrowthBook
- `isModelSupportedForCacheEditing(model)` — model kısıtı
- `registerToolResult(state, tool_use_id)` — ilk görülen compactable tool sonucunu kaydet
- `registerToolMessage(state, groupIds)` — user mesajı başına grup kaydı
- `getToolResultsToDelete(state)` — silme adayları (tetikleme eşiği: `config.triggerThreshold`; koruma: `config.keepRecent` — log'a yazılır, `microCompact.ts:354-355`)
- `createCacheEditsBlock(state, toolsToDelete)` → `{ type:'cache_edits', edits:[{type:'delete', cache_reference:<tool_use_id>}] }` (`claude.ts:3053-3055`)
- State: `toolOrder[]`, `registeredTools:Set`, `deletedRefs:Set`, `pinnedEdits[]` (`microCompact.ts:351`, `100-105`)

**İki geçişli kayıt** (`cachedMicrocompactPath`, `microCompact.ts:305-399`):
1. `collectCompactableToolIds` → compactable id seti
2. user mesajları: `tool_result` bloğu compactable ise ve henüz kayıtlı değilse `registerToolResult` + gruba ekle; sonra `registerToolMessage(state, groupIds)`

**Ana thread kısıtı:** forked ajanların (session_memory, prompt_suggestion) tool_result'ları global cachedMCState'e kaydedilmez — yoksa ana thread kendi konuşmasında olmayan tool'ları silmeye çalışır (`microCompact.ts:272-275`). `isMainThreadSource` **prefix eşleşmesi**: `!querySource || startsWith('repl_main_thread')` (`microCompact.ts:249-251`); output-style varyantlarını da kapsar.

**API katmanı enjeksiyonu** (`claude.ts`, `addCacheBreakpoints`):

```
1. cache_control marker'ı TEK mesaja: markerIndex = skipCacheWrite ? len-2 : len-1
   (skipCacheWrite = fire-and-forget fork: marker'ı sondan bir öncekine kaydır ki
   fork kendi tail'ini KVCC'ye yazmasın — mycro eviction notu, satır 3086)
2. useCachedMC ise:
   a. seenDeleteRefs = Set; her bloğu bununla dedupe et
   b. PINNED bloklar orijinal userMessageIndex'lerine yeniden eklenir (cache hit için
      her istekte aynı konumda durmalı) — satır 3127-3139
   c. YENİ cache_edits bloğu SON user mesajına eklenir ve pinCacheEdits(i, block) —
      satır 3141-3157
   d. Eklerken insertBlockAfterToolResults: son tool_result bloğundan sonraya splice;
      eklenen blok en sona denk gelirse ardına { type:'text', text:'.' } ekle
      (contentArray.ts:21-46)
3. enablePromptCaching: son cache_control marker'ından STRICTLY ÖNCEKI user
   mesajlarının her tool_result bloğuna cache_reference: tool_use_id ekle
   (API kuralı: cache_reference "before or on" son cache_control; strict "before"
   seçilmiş çünkü cache_edits splice'ı blok indexlerini kaydırır) — satır 3164-3212
   - Bloklar YENİ objelerle kopyalanır (kopyalanmazsa cache_editing desteklemeyen
     modellerin kullandığı mesajlara kontaminasyon olur)
```

**Geri besleme döngüsü** (`microCompact.ts:374-383`): `baselineCacheDeletedTokens` son assistant mesajının `usage.cache_deleted_input_tokens` değerinden alınır (API değeri kümülatiftir; delta = sonraki - baseline). Sınır mesajı (boundary message) API yanıtından **sonra** yazılır ki gerçek token tasarrufu kullanılsın.

**Diğer API'ler:** `consumePendingCacheEdits()` (API çağrısı başında tek seferlik tüketim, `claude.ts:1531`), `getPinnedCacheEdits()` (`claude.ts:1532`), `markToolsSentToAPIState()`, `resetMicrocompactState()`.

### 1.7 API tarafı context management (apiMicrocompact.ts)

Sunucu-uygulamalı strateji; istemci token eşikleriyle birebir:

```
DEFAULT_MAX_INPUT_TOKENS    = 180_000   // uyarı eşiği (trigger)
DEFAULT_TARGET_INPUT_TOKENS =  40_000   // "son 40k token gibi tut"
clear_at_least.value = triggerThreshold - keepTarget  // = 140_000

TOOLS_CLEARABLE_RESULTS = [shell*, Glob, Grep, FileRead, WebFetch, WebSearch]
TOOLS_CLEARABLE_USES    = [FileEdit, FileWrite, NotebookEdit]
```

**Strateji üretimi** (`getAPIContextManagement`, `apiMicrocompact.ts:64-153`):

```
1. hasThinking && !isRedactThinkingActive:
   push { type:'clear_thinking_20251015',
          keep: clearAllThinking ? {type:'thinking_turns', value:1} : 'all' }
   // clearAllThinking = >1h boşluk (cache miss); API şeması value>=1 ister,
   // edit atlanırsa model-policy default'u ("all") devreye girer, hiç temizlenmez
2. USER_TYPE !== 'ant' ise burada dur (tool temizleme ant-only)
3. USE_API_CLEAR_TOOL_RESULTS (env):
   push { type:'clear_tool_uses_20250919', trigger:{type:'input_tokens', value:180k},
          clear_at_least:{type:'input_tokens', value:140k},
          clear_tool_inputs: TOOLS_CLEARABLE_RESULTS }
4. USE_API_CLEAR_TOOL_USES (env):
   push { type:'clear_tool_uses_20250919', trigger:{type:'input_tokens', value:180k},
          clear_at_least:{type:'input_tokens', value:140k},
          exclude_tools: TOOLS_CLEARABLE_USES }   // bunlar DIŞINDAKİ tool_use'lar silinir
5. env override: API_MAX_INPUT_TOKENS, API_TARGET_INPUT_TOKENS
```

### 1.8 Sabitler tablosu (Claude Code)

| Sabit | Değer | Kaynak |
|-------|-------|--------|
| IMAGE_MAX_TOKEN_SIZE | 2000 | microCompact.ts:38 |
| TIME_BASED_MC_CLEARED_MESSAGE | `'[Old tool result content cleared]'` | microCompact.ts:36 |
| bytesPerToken (varsayılan) | 4 | tokenEstimation.ts:203 |
| bytesPerToken (json/jsonl/jsonc) | 2 | tokenEstimation.ts:208-214 |
| tahmin güvenlik payı | ×4/3, ceil | microCompact.ts:204 |
| gapThresholdMinutes | 60 (GB `tengu_slate_heron`) | timeBasedMCConfig.ts:30-34 |
| keepRecent (time-based) | 5, taban 1 | timeBasedMCConfig.ts:33, microCompact.ts:461 |
| cached MC feature flag | `CACHED_MICROCOMPACT` | microCompact.ts:276 |
| DEFAULT_MAX_INPUT_TOKENS | 180_000 | apiMicrocompact.ts:16 |
| DEFAULT_TARGET_INPUT_TOKENS | 40_000 | apiMicrocompact.ts:17 |

---

## 2. opencode compaction (prune + tail-turn select)

### 2.1 Bütçe formülleri (`overflow.ts` + `compaction.ts`)

```
COMPACTION_BUFFER = 20_000

usable(cfg, model, outputTokenMax):
  context = model.limit.context; context == 0 → 0
  reserved = cfg.compaction?.reserved
             ?? min(20_000, ProviderTransform.maxOutputTokens(model, outputTokenMax))
  model.limit.input varsa: max(0, limit.input - reserved)
  yoksa:              max(0, context - maxOutputTokens)

isOverflow(tokens, model, cfg):
  cfg.compaction?.auto === false → false; context == 0 → false
  count = tokens.total || input + output + cache.read + cache.write
  return count >= usable(...)
```

### 2.2 select() — tail-turn seçimi (`compaction.ts:223-269`)

**Girdi:** messages, cfg, model. **Çıktı:** `{ head, tail_start_id }`.

```
1. limit = cfg.compaction?.tail_turns
   limit !== undefined && limit <= 0 → { head: tüm mesajlar, tail_start_id: undefined }
                                        // compaction YOK; head tamamı
2. budget = preserveRecentBudget(cfg, model)
           = cfg.compaction?.preserve_recent_tokens
             ?? clamp(usable * 0.25, 2_000, 15_000)
3. all = turns(messages)
   turns(): her user mesajı (compaction part'ı olanlar hariç) bir Turn;
   turn.end = sonraki turn'ün start'ı (işaretçi), son turn end = mesaj sonu
4. all boşsa → head = tamamı
5. recent = limit === undefined ? all : all.slice(-limit)
6. total = 0; keep = undefined
   for i in reverse(recent):                     // SONDAN başla, tembel tahmin
     size = estimate(messages[turn.start..turn.end])
            // estimate = MessageV2.toModelMessagesEffect + Token.estimate(JSON)
     if total + size <= budget:
         total += size; keep = { start: turn.start, id: turn.id }; continue
     remaining = budget - total
     split = splitTurn(turn, budget: remaining, ...)
     if split: keep = split
     else if !keep: log("tail fallback", { budget, size, total })
     break
7. keep yoksa veya keep.start === 0 → { head: tamamı, tail_start_id: undefined }
8. → { head: messages[0..keep.start), tail_start_id: keep.id }
```

**Önemli ayrıntılar:**
- Tahmin **tembeldir**: yalnızca kuyruk adayı turn'ler tahmin edilir; maliyet tüm oturuma değil tutulan kuyruğa orantılı kalır (`compaction.ts:238-244` yorumu).
- `keep.start === 0` → hiçbir şey kesilmez (baştan kesmek anlamsız).
- `tail_start_id`, compaction part'ına yazılır; ertesi turda `compactionPart.tail_start_id !== selected.tail_start_id` ise güncellenir (`compaction.ts:461-466`) — yeni kuyruk işareti state'e işlenir.

### 2.3 splitTurn() — turn içi kesim (`compaction.ts:140-163`)

```
girdi: turn, budget (kalan), estimate
budget <= 0 → undefined
turn.end - turn.start <= 1 → undefined        // tek mesajlık turn kesilemez
for start in (turn.start+1 .. turn.end):
    size = estimate(messages[start..turn.end])
    size <= budget ise → return { start, id: messages[start].id }
return undefined
```

Kuyruk bütçesine sığmayan turn, **içinden** bölünür: kuyruğa girecek en eski sürekli dilim bulunur (assistant+tool mesajları). Başarısızsa ve hiç turn tutulmamışsa fallback: hiç compaction yok.

### 2.4 prune() — tool çıktısı budama (`compaction.ts:273-317`)

```
sabitler: PRUNE_MINIMUM = 20_000
          PRUNE_PROTECT = 40_000
          PRUNE_PROTECTED_TOOLS = ["skill"]
          TOOL_OUTPUT_MAX_CHARS = 2_000

1. cfg.compaction?.prune yoksa return
2. msgs = session.messages(); yoksa return
3. total=0; pruned=0; toPrune=[]; turns=0
   loop msgs SONDAN başa:
     msg.role == "user" → turns++
     turns < 2 → continue          // son 2 turn korunur (yeni iş dokunulmaz)
     assistant && msg.info.summary → break loop   // önceki compaction'ı GEÇME
     her part (sondan):
       tool değilse → continue
       status != "completed" → continue
       PRUNE_PROTECTED_TOOLS.includes(part.tool) → continue
       part.state.time.compacted (zaten budanmış) → break loop
       estimate = Token.estimate(part.state.output)
       total += estimate
       total <= PRUNE_PROTECT → continue   // ilk 40k token korunur
       pruned += estimate; toPrune.push(part)
4. pruned > PRUNE_MINIMUM ise:
     her part: state.time.compacted = Date.now(); session.updatePart(part)
```

**Mantık:** "Sondan geriye, 40k token değerinde tool çağrısı birikene kadar yürü; daha eski tamamlanmış tool çıktılarının çıktısını sil; ama toplam kazanç 20k token'ı geçmiyorsa diske hiç yazma (noise olmasın)". Budanan çıktı serialize'de `"[Old tool result content cleared]"` olarak görünür (`compaction.ts:77`).

### 2.5 process() — compaction akışı (`compaction.ts:319-557`)

```
1. parent user mesajı + compaction part'ı bulunmalı; değilse hata
2. overflow ise: parent'tan önceki SON non-compaction user mesajını bul,
   onu replay'e al, messages = onun öncesi; replay yoksa ya da geriye
   hiç non-compaction user kalmadıysa replay yok
3. model = "compaction" agent'ının modeli (varsa), yoksa kullanıcı mesajının modeli
4. history = compactionPart var VE son mesaj parent ise → messages[:-1], yoksa messages
5. prior = completedCompactions(history)
   // assistant mesajı: summary && finish && !error && parent'ı compaction part'lı
   // user mesajıysa → CompletedCompaction
   hidden = { prior'ların userIndex + assistantIndex }
   previousSummary = son prior'un summary metni
6. selected = select(history.filter(!hidden), cfg, model)
7. conversation = serialize(head) — format:
     user:      "[User]: <text>" + "[Attached <mime>: <filename>]"
     assistant: "[Assistant]: <text>" | "[Assistant reasoning]: <text>"
     tool:      "[Assistant tool call]: <tool>(<JSON input>)"
                + status completed: "[Tool result]: <output|attachmentlar>"
                  (truncate 2_000 karakter + "\n[truncated]")
                + status error: "[Tool error]: <error>"
8. prompt = plugin.compacting.prompt ?? [ buildPrompt({previousSummary,
            context:[conversation]}), ...plugin.context ].join("\n\n")
9. compaction assistant mesajı yarat (mode:"compaction", agent:"compaction",
   summary:true) ve processor'ı çalıştır
10. "compact" dönerse → ContextOverflowError, "stop"
11. "continue" && auto:
     replay varsa → replay mesajını YENİ user mesajı olarak kopyala
        (media partları "[Attached mime: name]" text'ine indirilir)
     replay yoksa && plugin autocontinue enabled:
        "Continue if you have next steps, or stop and ask for clarification..."
        + overflow ise medya açıklaması prefix'i; metadata.compaction_continue = true
```

### 2.6 Sabitler tablosu (opencode)

| Sabit | Değer | Kaynak |
|-------|-------|--------|
| PRUNE_MINIMUM | 20_000 | compaction.ts:28 |
| PRUNE_PROTECT | 40_000 | compaction.ts:29 |
| TOOL_OUTPUT_MAX_CHARS | 2_000 | compaction.ts:30 |
| PRUNE_PROTECTED_TOOLS | `["skill"]` | compaction.ts:31 |
| MIN_PRESERVE_RECENT_TOKENS | 2_000 | compaction.ts:32 |
| MAX_PRESERVE_RECENT_TOKENS | 15_000 | compaction.ts:33 |
| preserve_recent varsayılanı | clamp(usable × 0.25, 2k, 15k) | compaction.ts:115-120 |
| COMPACTION_BUFFER | 20_000 | overflow.ts:7 |

---

## 3. codex execpolicy DSL

### 3.1 Gramer (Starlark)

Dosyalar `Dialect::Extended` + f-string etkin (`parser.rs:59-60`) Starlark'tır. Üç builtin:

```
prefix_rule(pattern = [token...],            // zorunlu; token = string | [alt1, alt2, ...]
            decision = "allow"|"prompt"|"forbidden",   // opsiyonel, varsayılan "allow"
            match = [...],                    // opsiyonel pozitif örnekler
            not_match = [...],                // opsiyonel negatif örnekler
            justification = "...")            // opsiyonel, boş olmamalı
network_rule(host = "...", protocol = "http"|"https"|"socks5_tcp"|"socks5_udp",
             decision = "...", justification = "...")
host_executable(name = "bash", paths = ["/bin/bash", "/usr/bin/bash"])
```

**Örnek** (`examples/example.codexpolicy`): `pattern=["git","reset","--hard"], decision="forbidden", match=[["git","reset","--hard"]], not_match=[["git","reset","--keep"], "git reset --merge"]`. String örnekler `shlex::split` ile tokenlanır; boş örnek yasak.

**Pattern token kuralları** (`parser.rs:184-217`):
- string → `Single(s)`; liste → `Alts` (tek elemanlı liste `Single`'a daralır); boş liste yasak.
- `pattern` boş olamaz.
- İlk token'ın alternatifleri **her biri için ayrı bir PrefixRule** üretilir (`parser.rs:390-403`) — ilk token sabit tutulur çünkü policy ilk tokene göre indekslenir.

**`Decision` sıralaması** (`decision.rs:7-16`): `Allow < Prompt < Forbidden` (Ord türetilmiş; max birleştirme buna dayanır). Network kuralında `decision="deny"` → `Forbidden` (`parser.rs:253-258`).

**Network host normalizasyonu** (`rule.rs:156-212`): trim → `://` `/` `?` `#` içeriyorsa reddet → `[v6]` köşeli parantez çöz (port doğrula) → tek `:port` sıyır → sondaki `.`'ları trim, lowercase → boşsa reddet → `*` reddet (wildcard yasak) → whitespace reddet.

### 3.2 Parser akışı (`parser.rs:48-84`)

```
parse(policy_identifier, contents):
  ast = AstModule::parse(Extended + f-strings)
  globals = standard + policy_builtins
  Evaluator.extra = PolicyBuilder
  eval_module(ast)
  pending_example_validations[start_index..] doğrula:
      her validation için GEÇİCİ Policy kur (yalnız o kural grubu + host executables)
      not_match örnekleri: ilk eşleşen kural varsa → hata (ExampleDidMatch)
      match örnekleri:      hiçbir kural eşleşmiyorsa → hata (ExampleDidNotMatch)
      // doğrulama location bilgisiyle (dosya:satır:sütun) zenginleştirilir
build() → Policy { rules_by_program: MultiMap<program, RuleRef>,
                   network_rules, host_executables_by_name }
```

`host_executable` doğrulamaları: isim bare executable olmalı (tek komponent); her path mutlak olmalı ve basename'i `executable_lookup_key(name)` ile eşleşmeli; yinelenen path'ler elenir (`parser.rs:437-472`). Windows'ta lookup key `.exe/.cmd/.bat/.com` sonekini sıyırır ve lowercase yapar (`executable_name.rs:3-23`).

### 3.3 Eşleştirme (`policy.rs:268-334`)

```
matches_for_command_with_options(cmd, heuristics_fallback, options):
  1. matched = match_exact_rules(cmd)
       first = cmd[0]; rules_by_program.get_vec(first) üzerinde her rule.matches(cmd)
       (PrefixPattern: cmd.len() >= rest.len()+1 && cmd[0]==first &&
        her rest token PatternToken.matches(cmd[i]) → matched_prefix döner)
  2. boşsa && options.resolve_host_executables:
       match_host_executable_rules(cmd):
         cmd[0] mutlak path olmalı (AbsolutePathBuf)
         basename = executable_path_lookup_key(path)
         basename anahtarında kural var mı?
         host_executables_by_name[basename] varsa ve program path'lerden
           birine EŞİT değilse → hiç eşleşme yok (gated)
         kuralı basename_command = [basename, cmd[1..]] üzerinde dene
         eşleşirse resolved_program = orijinal path ile işaretle
  3. hâlâ boşsa && fallback varsa → [ HeuristicsRuleMatch { command, decision: fallback(cmd) } ]
  4. Evaluation.decision = matched_rules'ün DECISION'LARININ MAX'ı
     (biri Forbidden → Forbidden; Prompt, Allow'u ezer)
```

### 3.4 Fallback karar akışı (eşleşmemiş komutlar, `exec_policy.rs:727-820`)

`render_decision_for_unmatched_command(command, context)`:

```
1. dangerous_command_match = dangerous_command_match_for_origin(cmd, origin)
   (Generic: is_dangerous_command; Windows PowerShell: powershell_words_match)
2. is_known_safe = is_known_safe_command(cmd) (generic safelist)
3. Eğer is_known_safe && !used_complex_parsing
      && (approval_policy == UnlessTrusted || windows_managed_fs_restrictions_without_sandbox_backend)
      → Allow
4. dangerous varsa VEYA windows-managed-fs-sandbox'sız durumdaysa:
      approval Never → Forbidden; diğerleri → Prompt
5. approval matrisi:
   Never        → Allow (sandbox'a güven)
   UnlessTrusted→ Prompt (safelist zaten geçtiyse yukarıda dönmüştük)
   OnRequest / Granular:
     FS sandbox Unrestricted | External → Allow
     Restricted → sandbox_override isteniyorsa Prompt, değilse Allow
```

Reddetme nedenleri (prompt reddi): `PROMPT_CONFLICT_REASON` ("AskForApproval Never ama policy Prompt"), `REJECT_SANDBOX_APPROVAL_REASON`, `REJECT_RULES_APPROVAL_REASON` (`exec_policy.rs:47-51`).

### 3.5 BANNED prefix listesi (`exec_policy.rs:56-151`)

Tam liste — amendment önerisi için yasaklanmış prefix'ler (shell yorumlayıcıları ve tehlikeli taşıyıcılar):

```
/bin/bash, /bin/bash -c, /bin/bash -lc, /bin/sh, /bin/sh -c, /bin/sh -lc,
/bin/zsh, /bin/zsh -c, /bin/zsh -lc, Rscript,
bash, bash -c, bash -lc, bun, bun -e, bun run,
cmd, cmd /c, cmd /k, cmd.exe, cmd.exe /c, cmd.exe /k,
dash, dash -c, deno, deno eval, env, fish, fish -c, git,
julia, julia -e, ksh, ksh -c, lua, lua -e,
node, node -e, nodejs, nodejs -e, npm run, osascript,
perl, perl -e, php, php -r, pnpm run,
powershell, powershell -Command, -EncodedCommand, -File, -c,
powershell.exe (aynı 4 varyant), pwsh (Command/EncodedCommand/File/c/e/ec/f),
py, py -3, pypy, pypy3, python, python -, python -c,
python3, python3 -, python3 -c, pythonw, pyw, rm, ruby, ruby -e,
sh, sh -c, sh -lc, sudo, yarn run, zsh, zsh -c, zsh -lc
```

### 3.6 Amendment üretimi (`exec_policy.rs:894-975`)

```
try_derive_execpolicy_amendment_for_prefix_rule(prefix_rule, matched_rules, ...):
  prefix yok/boş → None
  BANNED listesinde TAM eşleşme → None
  matched_rules'te herhangi bir policy eşleşmesi varsa → None
  amendment = ExecPolicyAmendment::new(prefix)
  prefix_rule_would_approve_all_commands(...):
     policy'ye add_prefix_rule(prefix, Allow) ile klon ekle
     tüm komutların decision'ı Allow oluyorsa → amendment, yoksa None
```

### 3.7 Sabitler tablosu (execpolicy)

| Sabit | Değer | Kaynak |
|-------|-------|--------|
| Decision sıralaması | Allow < Prompt < Forbidden | decision.rs:7 |
| network protokol | http, https(https_connect\|http-connect), socks5_tcp, socks5_udp | rule.rs:117-146 |
| host_executable name | bare, tek komponent | parser.rs:234-251 |
| Windows exec sonekleri | .exe .cmd .bat .com | executable_name.rs:4 |
| kural dizini / dosya | `rules/` + `default.rules` | exec_policy.rs:53-55 |
| PROMPT_CONFLICT_REASON | `"approval required by policy, but AskForApproval is set to Never"` | exec_policy.rs:47 |

---

## 4. aider repo map (pagerank)

### 4.1 Tag üretimi (`repomap.py:279-364`)

**Girdi:** dosya yolu. **Çıktı:** `Tag(rel_fname, fname, line, name, kind)` akışı.

```
1. lang = filename_to_lang(fname); desteklenmiyorsa dur
2. query_scm = <lang>-tags.scm (tree-sitter-language-pack, yoksa tree-sitter-languages)
3. tree = parser.parse(code); captures = Query(language, query_scm)
4. her capture: tag "name.definition.*" → kind="def"; "name.reference.*" → kind="ref";
   diğerleri atlanır; line = node.start_point[0]
5. defs GÖRÜLDÜ ama hiç ref yoksa (ör. cpp tag dosyaları sadece def verir):
   pygments lexer ile Token.Name token'ları ref olarak geri doldur (line=-1)
```

**Tag cache** (`repomap.py:233-264`): `{fname: {mtime, data}}` — mtime aynıysa cache'ten döner; SQLite hatası → dizini sil, yeniden kur, olmazsa dict'e düş (`repomap.py:177-215`). Cache dizini `.aider.tags.cache.v{CACHE_VERSION}`, `CACHE_VERSION = 3` (tsl paketiyle 4) (`repomap.py:35-43`).

### 4.2 Kişiselleştirme (personalization) (`repomap.py:365-445`)

```
personalize = 100 / len(fnames)         // varsayılan pagerank kişileştirmesi 1/num_nodes
her dosya için (current_pers 0'dan başlar):
  chat_fnames'ta ise      → current_pers += personalize
  mentioned_fnames'ta ise → current_pers = max(current_pers, personalize)  // çift sayma önlenir
  mentioned_idents ile path bileşenleri/dir/basename(+uzantısız) kesişimi varsa
                           → current_pers += personalize   // her dosyada EN FAZLA 1 kez
  current_pers > 0 ise    → personalization[rel_fname] = current_pers
```

### 4.3 Ağırlık tablosu (çoklu çizge inşası) (`repomap.py:468-514`)

```
G = nx.MultiDiGraph()
1. Self-edge: her definer için, ident'in hiç ref'i yoksa → weight=0.1 (ruby gibi
   dil tuhaflıklarını telafi: def+ref aynı anda sayılmıyor)
2. Her ident (def ∩ ref) için:
   mul = 1.0
   ident in mentioned_idents                    → ×10
   (snake | kebab | camel) && len >= 8          → ×10
   ident.startswith("_")                        → ×0.1
   len(defines[ident]) > 5                      → ×0.1   // çok tanımlı = düşük değer
   her (referencer, num_refs) için:
     use_mul = mul
     referencer in chat_rel_fnames → ×50
     num_refs = sqrt(num_refs)                  // yüksek frekanslı düşük-değer
                                                // mention'ların dominasyonunu önler
     G.add_edge(referencer, definer, weight = use_mul * sqrt(num_refs))
```

### 4.4 Pagerank + rank dağıtımı (`repomap.py:519-573`)

```
ranked = nx.pagerank(G, weight="weight",
                     personalization=personalization, dangling=personalization)
         (ZeroDivisionError → kişileştirmesiz tekrar dene; yine olmazsa [])

ranked_definitions: her src node için:
  total_weight = Σ out_edges(src).weight
  her out edge (src→dst, ident):
    edge.rank = ranked[src] * weight / total_weight
    ranked_definitions[(dst, ident)] += edge.rank
sırala: (rank, (fname, ident)) tersine
chat_rel_fnames'taki dosyaların tag'ları ATLANIR (zaten konuşmada)
```

**Sıra dışı dosyalar:** pagerank top_rank'teki her fname (henüz listede yoksa) dosya olarak eklenir; kalan `other_fnames` sona eklenir (`repomap.py:560-573`). `filter_important_files` sonuçları (README vb.) her zaman listenin **başına** konur (`repomap.py:657-662`).

### 4.5 Binary-search token bütçesi (`repomap.py:666-706`)

```
num_tags = len(ranked_tags); lower=0; upper=num_tags; best_tree=None; best_tokens=0
middle = min(max_map_tokens // 25, num_tags)
while lower <= upper:
  tree = to_tree(ranked_tags[:middle], chat_rel_fnames)
  num_tokens = token_count(tree)
  pct_err = |num_tokens - max_map_tokens| / max_map_tokens;  ok_err = 0.15
  if (num_tokens <= max && num_tokens > best_tokens) || pct_err < ok_err:
      best = tree;  if pct_err < ok_err: break
  num_tokens < max ? lower = middle+1 : upper = middle-1
  middle = (lower+upper)//2
```

**token_count** (`repomap.py:89-101`): metin <200 karakter → model tokenizer'ı; yoksa `step = num_lines//100`, her step'te 1 satır örnekle → `est = sample_tokens / sample_len * len_text`.

**to_tree** (`repomap.py:748-784`): tag'ları dosyaya göre grupla; her dosya için `render_tree` (TreeContext, lois = tag satırları; cache anahtarı `(rel_fname, sorted(lois), mtime)`); çıktı satırları 100 karaktere kırpılır (minified js koruması).

**No-chat-files genişletmesi** (`repomap.py:120-132`): chat dosyası yoksa `target = min(max_map_tokens × map_mul_no_files(=8), max_context_window − 4096(padding))` ve `max_map_tokens = target` — tüm reponun daha geniş görünümü.

**Harita cache** (`repomap.py:586-627`): anahtar `(chat_fnames, other_fnames, max_map_tokens, [mentioned*])`; `refresh`: manual→son haritayı döndür; always→cache yok; files→cache kullan; auto→`map_processing_time > 1.0` ise cache kullan. RecursionError → repo map kalıcı kapatılır (`max_map_tokens=0`).

### 4.6 Sabitler tablosu (aider)

| Sabit | Değer | Kaynak |
|-------|-------|--------|
| map_tokens varsayılan | 1024 | repomap.py:49 |
| map_mul_no_files | 8 | repomap.py:56 |
| context padding | 4096 | repomap.py:123 |
| personalize tabanı | 100 / len(fnames) | repomap.py:383 |
| ident çarpanları | ×10 / ×10 / ×0.1 / ×0.1 | repomap.py:492-499 |
| chat dosyası çarpanı | ×50 | repomap.py:508-509 |
| frekans dönüşümü | sqrt(num_refs) | repomap.py:512 |
| self-edge | 0.1 | repomap.py:479 |
| ok_err | 0.15 | repomap.py:690 |
| ilk middle | max_map_tokens // 25 | repomap.py:676 |
| satır kırpma | 100 karakter | repomap.py:782 |
| token örnekleme | <200 char → tam; yoksa /100 satır | repomap.py:91-101 |
| CACHE_VERSION | 3 (tsl: 4) | repomap.py:35-37 |

---

## 5. oh-my-pi hashline

### 5.1 Dosya hash algoritması (`format.ts:108-121`)

```
normalizeFileHashText(text):  /[ \t\r]+(?=\n|$)/g → ""   // her satırın (ve son
                              // satırın) trailing [ \t\r]'i; CRLF ve display-
                              // trimmed satırlar tag'ı bozmaz
computeFileHash(text):
  low16 = Bun.hash.xxHash32(normalized, 0) & 0xffff
  return low16.toString(16).padStart(4, "0").toUpperCase()
```

- **4 hex karakter** (16-bit) — `HL_FILE_HASH_LENGTH = 4` (`format.ts:91`).
- Header formatı: `[<path>#<HASH>]` (`format.ts:133-135`).
- Tag tamamen **içerik-türevli**: aynı normalleştirilmiş metin aynı tag'i üretir; tag eşleşmesi = dosyanın okunduğu sürümle birebir aynı olduğunun kanıtı. 16-bit çakışmasında snapshot store'daki en son kayıtlı metin kullanılır (`patcher.ts:683-689`).
- Araç katmanı (MCP/plugin) satırları `N:hh` biçiminde sunar: `hh` = satır içeriğinden türetilmiş 2-hex satır imzası; satır-nitelikli anchor, aynı numaranın başka içerikteki bir satıra kaymasını engeller. Düzenleme tarafında geçerlilik, bölüm tag'i (dosya hash'i) üzerinden atomik doğrulanır.

### 5.2 Patch dili grameri (`format.ts:9-44`, `tokenizer.ts:336-467`)

```
[<path>#<HASH>]                    # bölüm başlığı (aynı dosyaya birden fazla bölüm yasak:
                                   # "Multiple hashline sections resolve to the same file")
  PUT 5.=9:                        # satır 5-9'u gövdeyle DEĞİŞTİR (":" gövde sözü verir)
  +yeni satır                      # literal payload satırı
  CUT 5.=9                         # 5-9'u sil, anonymous register'a yakala
  CUT 5.=9 @reg                    # adlı register'a yakala
  PUT <5: / PUT >5:                # 5. satırdan önceye / sonraya ekle
  PUT <1: / PUT >$:                # dosya başı / sonu (position-stable, drift'te bile güvenli)
  PUT 5*:                          # 5'te başlayan syntactic block'u değiştir (tree-sitter)
  PUT >5*:                         # block bittikten sonraya ekle
  REM                              # dosyayı sil (satır op'larıyla birleşemez)
  MV <dest>                        # dosyayı taşı/yeniden adlandır
  @reg                             # register referansı (isim: [A-Za-z0-9_-]+, max 64)
```

Ayrıca: `PUT 5-9` span formu ve `N:` tek satır formları; `PUT 5.=9` range ayracı `.=`; `<1` → `bof` (boş dosyada bile çalışır); `<N*` ≡ `<N` (block N'de başlar). Üst düzey snapshot satırları `N:TEXT` otomatik tek satırlık replacement'a çevrilir (yinelenen N → hata). apply_patch/unified-diff bulaşması (`*** Update File:`, `@@ -N,M +N,M @@`) açıkça reddedilir (`parser.ts:144-184`).

### 5.3 Parser/Executor (`parser.ts:207-788`)

Token akışı: `header | blank | payload-literal | raw | op-block | envelope-* | abort`. State: `pending {target, payloads, hadColon, deferredBlanks}`.

- **Range doğrulama** (`parser.ts:72-92`): uçlar pozitif safe integer; `end >= start`; span ≤ **100_000** (`MAX_EXPANDED_RANGE_LINES`).
- **Blanks:** gövde içi boş satırlar ertelenir (`deferredBlanks`), sonraki non-blank satır geldiğinde gövdeye katılır; sona doğru trailing boşluklar layout olarak düşer.
- **`-` satırlar** (`#resolveMinusRows`): hepsi markdown bullet şeklindeyse literal kalır; `+` satırlarla karışık diff bulaşmasıysa `-` satırları silinir; aksi halde reddedilir.
- **Bare prefix sıyırma** (`#stripBarePrefixesIfUniform`): gövdedeki TÜM bare satırlar `N:`/`N|` prefix'i taşıyorsa sıyır (read çıktısı yapıştırma imzası); karışıksa veya hepsi sayısal/quote literal ise (YAML `1: "x"`) dokunma.
- **Çakışan aralıklar** (`#normalizeOverlappingRanges`): birebir aynı kaynak satır setine sahip iki hunk → eskisi düşer (uyarı); kısmi çakışma → hata (before/after pair yazmaya çalışıyorsun, tek hunk kullan).
- **Op yayılımları:** replace → `before_anchor` insert'leri + per-line delete'ler; cut → cut edit + delete'ler; REM/MV dosya-op'u olarak ayrılır.

### 5.4 Apply (`apply.ts`)

- `validateLineBounds`: her anchor `1..fileLines.length` aralığında olmalı (dosya sürümü bağlaması bölüm hash'inde yapıldı).
- Hayalet son satır (`trailingPhantomLine`): newline ile biten dosyanın split sonucu `""` — insert için adreslenebilir, delete oraya düşerse yok sayılır (yoksa dosyanın son newline'ı yanlışlıkla silinir).
- Replacement grupları (`bucketAnchorEditsByLine`, `findReplacementGroup`): aynı satıra inen insert+delete'ler grup olarak ele alınır.
- **Sınır onarımı** (`buildGroupVariants`, `compareGroupVariant`): yanlış konumlandırılmış replacement sınırları, aday arama ile onarılır — kriterler: satır eşitliği, girinti, dar pure-closer şekli, tree-sitter `parsesCleanly` doğrulaması; aday sayısı `MAX_BOUNDARY_COMBOS = 512` ile sınırlanır.
- `repairReplacementIndentation`: `INDENT_TAB_WIDTH = 4`; payload kenarıyla hedef satır girintisi arasında otomatik girinti kaydırması (uyarı).
- `blockStart` landing correction: block sonrası eklenen gövde, block'un kapanış satırlarını aşacak derinlik iddia ediyorsa aşağı kaydırılır (uyarı).
- Çıktı: `{ text, firstChangedLine, warnings }` — saf fonksiyon.

### 5.5 Patcher — prepare/commit iki katmanı (`patcher.ts:243-575`)

```
apply(patch):                        // all-or-nothing
  1. tek bölüm → prepare+commit (hızlı yol)
  2. çok bölüm: HER BÖLÜMÜ ÖNCE prepare et (okuma, parse, tag doğrulama, in-memory
     apply) — herhangi biri başarısızsa HİÇBİR YAZMA YAPILMADI
  3. aynı canonical path'e çözen bölüm → hata
  4. noop bölüm → hata ("resulted in no changes")
  5. commit'leri sırayla yaz; yazma hatası → "Sections already written: ..." raporu
     (kısmi yazma raporlanır, rollback yok; sadece eksikleri yeniden iste)

prepare(section):
  1. parse (+ InvalidAbsoluteRangeError için tree-sitter block önerisi)
  2. fileHash yoksa → hata (missingSnapshotTagMessage)
  3. dosya yoksa → PATH RECOVERY: snapshot store'da aynı tag + aynı basename'e
     sahip TEK aday varsa (yazarın kendisi hariç) path'i ona yeniden bağla + uyarı
  4. preflightWrite (read-only hedef yazma kapısı)
  5. dosya hâlâ yoksa → "File not found: ... Use the write tool to create new files."
  6. BOM ayır, line ending tespit et, LF'ye normalize et
  7. #applyWithRecovery (aşağıda)

commit(prepared):
  REM → fs.delete + snapshot invalidate
  noop → snapshot kaydet, dön
  MV → snapshots.relocate + fs.move
  yazma → fs.writeText(persisted = BOM + restoreLineEndings(after))
  **Kritik:** snapshot, `after`'a değil, FS'in GERÇEKTEN YAZDIĞI metne
  (write.text) dayanır — ACP-bridge format-on-save metni değiştirdiyse sonraki
  okuma tag'i tutturmasın. `recorded != after` ise yalnızca O(1) drift UYARISI
  (diff değil — model görünen diff hunk'a odaklı kalır).
```

### 5.6 Stale-anchor reddi (`patcher.ts:673-758`)

```
#applyWithRecovery:
  expected = exists ? section.fileHash : undefined
  liveMatches = computeFileHash(normalized) == expected
  matchedSnapshot = liveMatches ? snapshots.byContent(path, normalized) : null

  1. Block edit'ler önce çözülür: baseText = (tag yok || liveMatches) ? normalized
     : snapshots.byHash(tag).text — tag'lı snapshot yoksa MismatchError
     (re-read; "16-bit tag collision" durumunda en son metin)
  2. tag yok veya canlı içerik tag'i tutturuyor:
     - enforceSeenLines → #assertSeenLines (bkz. 5.7)
     - applyEdits(normalized, resolved) → dön
  3. anchor-scoped edit YOKSA (sadece baş/son insert'leri): drift ölümcül değil —
     canlı içeriğe uygula + HEADTAIL_DRIFT_WARNING (position-stable op'lar)
  4. drift var + anchor-scoped edit var → Recovery.tryRecover (bkz. 5.8)
     başarısızsa → MismatchError
       { path, expectedFileHash, actualFileHash (Taze snapshot!),
         fileLines, anchorLines, hashRecognized }
       // model gerçek durumu GÖRMELİ: actual hash + satırlar + anchorlar hata
       // mesajına gömülür; retry tekrar aynı tag ile yapılırsa liveMatches
       // üzerinden düzelemez → re-read gerekir
```

### 5.7 Görülen-satır koruması (seen-lines) (`patcher.ts:56-66, 622-654`)

```
sabitler: SEEN_LINE_REVEAL_CAP = 40          // uyarıya gömülecek azami satır
          SEEN_LINE_REVEAL_MAX_COLUMNS = 512 // tek satır azami genişlik

#assertSeenLines(section, expected, matchedSnapshot):
  seen = matchedSnapshot.seenLines  (tag'i üreten read'in gösterdiği satırlar)
  unseen = section.collectAnchorLines() - seen
  unseen boş → geç
  her unseen satırın GERÇEK içeriğini uyarıya göm (kap 40 satır / 512 kolon;
  taşan satır "…" ile kesilir VE truncation bayrağı kalkar)
  truncated DEĞİLSE: gösterilen satırları seen'e EKLE (retry aynı tag ile
  doğrudan başarılı olur — hata mesajındaki içerik "gördüm" kanıtıdır)
  truncated İSE: HİÇBİR satır eklenmez (model ≤40 satırlık parçalı retry'larla
  guard'ı by-pass edemez, minified mega-satırı da uyarıya dökemez)
  throw unseenLinesMessage(...)
```

Guard yalnızca **drift'siz** yolda çalışır (anchor numaraları tag'lı içerikle 1:1 eşleşirken).

### 5.8 Recovery — drift'li dosyada anchor yeniden eşleme (`recovery.ts`)

```
tryRecover(path, currentText, fileHash, edits):
  1. snapshot = store.byHash(path, fileHash); yoksa → null (mismatch'e düş)
  2. uyarı seçimi: store.head(path) == snapshot
       ? RECOVERY_EXTERNAL_WARNING (dış değişiklik) 
       : RECOVERY_SESSION_CHAIN_WARNING (kendi zincirimiz)
  3. replayRemappedAnchorsOnCurrent:

remapEditsToCurrent(previous=snapshot.text, current):
  1. lineMap = diffLineRuns(previous, current) üzerinden:
       unchanged run'lar için previousLine → currentLine eşlemeleri
  2. validateRemappedAnchorContext:
     - her anchor'ın lineMap'te karşılığı olmalı
     - anchor satırı previous'te DUPLICATE değilse: komşu context'ten en az biri
       lineMap'te aynı offset'i korumalı (validateUniqueAnchorContext)
     - DUPLICATE ise: İKİ komşunun da offset'e uyması gerekir
       (validateDuplicateAnchorContext — iki olası eşlemeden birini doğrula)
  3. tüm edit'leri (delete/block/cut/paste/insert) yeni satırlara haritala
  4. TÜM offset'ler eşit olmalı (uniform shift); değilse → null
     (cut/paste span: her satır haritalanmalı, içi değişmişse → null)
  5. applyEdits(current, remapped); uygulanamazsa veya metin değişmediyse → null
  6. offset != 0 ise RECOVERY_LINE_REMAP_WARNING ekle
```

Recovery **fail-closed**: hedef değişmişse, silinmişse, bölünmüşse veya belirsizse (çift eşleşme) asla tahmin etmez — MismatchError + taze bağlam döner.

### 5.9 Block çözümleme (`block.ts`)

```
sabit: BLOCK_SUGGESTION_SCAN_LIMIT = 64

resolveBlockEdits(edits, text, path, resolver):
  her block edit (mode = replace|cut|insert_after|paste_after):
    span = resolver({path, text, line: anchor.line})   // tree-sitter
    span yoksa:
      insert_after/paste_after → PLAIN "after N" formuna DÜŞ (uyarı; kapanış
        satırıysa "closer" varyantı uyarısı) — patch asla blok yüzünden çökmez
      diğerleri: resolver yoksa BLOCK_RESOLVER_UNAVAILABLE; varsa
        findNextBlock/findEnclosingBlock (64 satır tarama) önerisiyle hata
    span.start == span.end → tek satırlık çözüm = yanlış anchor
      (case gövdesi + break arasına düşme) → hata (veya drop)
    yayılım:
      replace  → insert'ler span.start'ına + her satıra delete
      cut      → cut(span) + delete'ler
      insert_after → payload'lar span.end'ine (blockStart = span.start)
      paste_after  → span.end'ine paste (blockStart ile)
```

### 5.10 Sabitler tablosu (hashline)

| Sabit | Değer | Kaynak |
|-------|-------|--------|
| hash | xxHash32 & 0xffff, 4 hex büyük harf | format.ts:117-121 |
| hash normalizasyonu | trailing `[ \t\r]` her satırda | format.ts:108-110 |
| MAX_EXPANDED_RANGE_LINES | 100_000 | parser.ts:36 |
| REGISTER_NAME_MAX | 64 | tokenizer.ts:297 |
| SEEN_LINE_REVEAL_CAP | 40 | patcher.ts:56 |
| SEEN_LINE_REVEAL_MAX_COLUMNS | 512 | patcher.ts:66 |
| MAX_BOUNDARY_COMBOS | 512 | apply.ts:706 |
| INDENT_TAB_WIDTH | 4 | apply.ts:418 |
| BLOCK_SUGGESTION_SCAN_LIMIT | 64 | block.ts:24 |
| drift uyarıları | HEADTAIL_DRIFT_WARNING, RECOVERY_*, writeDriftWarning | messages.ts |
| section sayısı | aynı dosyaya birden çok bölüm yasak | patcher.ts:198-209 |
| noop | yazma yoksa hata | patcher.ts:274-277 |

---

## 6. pi cut-point

### 6.1 Token tahmini (`compaction.ts:252-311`)

```
ESTIMATED_IMAGE_CHARS = 4800
estimateTokens(message): chars → ceil(chars / 4)
  user/custom/toolResult: content string len; image bloğu → 4800
  assistant: text + thinking + toolCall(name + json(arguments))
  bashExecution: command.len + output.len
  branchSummary/compactionSummary: summary.len
estimateContextTokens(messages): SON geçerli assistant usage'ı bul
  (stopReason aborted/error değil, calculateContextTokens>0);
  varsa tokens = usage.totalTokens || (input+output+cacheRead+cacheWrite)
              + trailing (usage'den sonraki mesajların tahmini)
  yoksa tüm mesajların tahmini
shouldCompact(contextTokens, contextWindow, settings):
  !enabled → false
  contextTokens > contextWindow − reserveTokens
```

**Varsayılan ayarlar** (`compaction.ts:158-162`): `{ enabled: true, reserveTokens: 16384, keepRecentTokens: 20000 }`.

### 6.2 findValidCutPoints (`compaction.ts:312-344`)

```
girdi: entries[startIndex..endIndex) — geçerli kesim noktaları
her entry:
  type "message": role bashExecution|custom|branchSummary|compactionSummary|user|assistant
                  → kesim noktası (i dahil)
                  role toolResult → değil
  type thinking_level_change|model_change|active_tools_change|compaction|branch_summary|custom
                  → değil
  istisna: type "branch_summary" → kesim noktası (satır 341)
```

toolResult'lar ve kontrol entry'leri asla tek başına kesim noktası değildir.

### 6.3 findTurnStartIndex (`compaction.ts:347-361`)

```
girdi: entries, entryIndex, startIndex — entryIndex'ten GERİYE yürü
  branch_summary bulursa → o index
  message: role user | bashExecution → o index
yoksa → -1
```

Turn, kullanıcı-visible mesajla (user/bashExecution/branch_summary) başlar.

### 6.4 findCutPoint (`compaction.ts:374-422`)

```
girdi: entries, startIndex, endIndex, keepRecentTokens
1. cutPoints = findValidCutPoints(entries, start, end)
   boşsa → { firstKeptEntryIndex: start, turnStartIndex: -1, isSplitTurn: false }
2. accumulatedTokens = 0; cutIndex = cutPoints[0]
3. i = endIndex-1'den startIndex'e GERİYE:
     entry message değilse continue
     accumulatedTokens += estimateTokens(entry.message)
     accumulatedTokens >= keepRecentTokens ise:
       cutIndex = cutPoints'te >= i olan İLK kesim noktası
       break
4. while cutIndex > startIndex:
     prev = entries[cutIndex-1]
     prev compaction | message ise break      // kesim noktasından önce
     cutIndex--                               // anlamlı entry gelene kadar
                                              // (control entry'lerini atla)
5. cutEntry = entries[cutIndex]
   isUserMessage = cutEntry message && role == "user"
   turnStartIndex = isUserMessage ? -1 : findTurnStartIndex(entries, cutIndex, startIndex)
   return { firstKeptEntryIndex: cutIndex, turnStartIndex,
            isSplitTurn: !isUserMessage && turnStartIndex != -1 }
```

**Semantik:** Son mesajlardan geriye sayarak ~`keepRecentTokens` kadar token biriktir; bütçeyi aşan ilk mesajda, o mesajı içeren geçerli kesim noktasına kay; kesim bir user mesajına denk gelmiyorsa turn'ün başına çek (turn'ü BÖL) → turn'ün prefix'i ayrıca özetlenir, suffix'i korunur.

### 6.5 prepareCompaction / compact (`compaction.ts:616-848`)

```
prepareCompaction(pathEntries, settings):
  boş veya son entry compaction → undefined (tekrar compaction yok)
  prevCompactionIndex = sondan ilk compaction entry'si
  compactableEntries = önceki compaction'ın retainedTail'ini SANAL message
    entry'lerine çevir (id: "<compactionId>:retained:<i>") + ondan sonrası
    → kuyruk bir sonraki compaction'da da korunur (zincirleme)
  tokensBefore = estimateContextTokens(buildSessionContext(pathEntries).messages)
  cutPoint = findCutPoint(compactableEntries, 0, end, keepRecentTokens)
  historyEnd = isSplitTurn ? turnStartIndex : firstKeptEntryIndex
  messagesToSummarize = [0..historyEnd)   (compaction entry'leri atlanır)
  turnPrefixMessages = [turnStartIndex..firstKeptEntryIndex)  (sadece split'te)
  retainedTail = [firstKeptEntryIndex..end)
  fileOps = önceki compaction details (readFiles/modifiedFiles) + yeni mesajlardan
    çıkarılan file op'lar (split'te prefix de dahil)

compact(preparation):
  split turn değilse: generateSummaryWithUsage(messagesToSummarize, ...)
    maxTokens = min(floor(0.8 × reserveTokens), model.maxTokens)
    prompt = <conversation> + (previousSummary → <previous-summary>) + 
             SUMMARIZATION_PROMPT veya UPDATE_SUMMARIZATION_PROMPT
    (yapılandırılmış şablon: Goal / Constraints / Progress(Done,In Progress,Blocked) /
     Key Decisions / Next Steps / Critical Context — exact path/function/error korunur)
    + customInstructions → "Additional focus: ..."
  split turn ise: history özeti (0.8×reserve) AYRI, turn prefix özeti AYRI
    (maxTokens = min(floor(0.5 × reserveTokens), ...); TURN_PREFIX_SUMMARIZATION_PROMPT)
    summary = history + "\n\n---\n\n**Turn Context (split turn):**\n\n" + prefix
    usage = combineUsage
  summary += formatFileOperations(readFiles, modifiedFiles)
  sonuç: { summary, tokensBefore, usage, retainedTail, details:{readFiles, modifiedFiles} }
  // LLM çağrıları: cacheRetention="none", sessionId=uuidv7 (özetler cache yazamaz,
  // yeniden kullanılamaz) — completeSimpleWithRetries ile retry edilir
```

### 6.6 Sabitler tablosu (pi)

| Sabit | Değer | Kaynak |
|-------|-------|--------|
| DEFAULT settings | enabled:true, reserveTokens:16384, keepRecentTokens:20000 | compaction.ts:158-162 |
| ESTIMATED_IMAGE_CHARS | 4800 | compaction.ts:252 |
| char→token | ceil(chars/4) | compaction.ts:279,292,297,306 |
| özet maxTokens | min(floor(0.8×reserve), model.maxTokens) | compaction.ts:541-544 |
| turn prefix maxTokens | min(floor(0.5×reserve), model.maxTokens) | compaction.ts:805-808 |
| tetikleme | tokens > window − reserveTokens | compaction.ts:247-250 |

---

## 7. Karşılaştırma matrisi ve inşa notları

| Eksen | Claude MC | opencode | pi | hashline | aider |
|-------|-----------|----------|----|----------|-------|
| Token tahmini | len/4 (json 2), ×4/3 pay | model transform + Token.estimate | usage tabanlı + len/4 | — (byte hash) | örnekleme + model count |
| Koruma stratejisi | keepRecent (son N tool) | preserve_recent clamp(25% usable, 2k-15k) | keepRecentTokens sabit 20k | seen-lines + file hash | pagerank sıralı prefix |
| Tetikleme | gap 60dk / count eşiği / API 180k | overflow (≥ usable) | tokens > window − 16k | hash mismatch | explicit çağrı |
| Kesim birimi | tool_result bloğu | turn (user mesajı) | entry + turn bölme | satır aralığı | tag |
| Kayıp koruması | cache_edits ile prefix korunur | tail bütçeye bölünür (splitTurn) | split turn + retainedTail zinciri | recovery + mismatch + taze hash | chat dosyaları hariç |
| Başarısızlık modu | MC yok → autocompact | tail fallback: compaction yok | ok/err Result | MismatchError (re-read) | RecursionError → map kapat |

**İnşa önerileri (mekanizma seçimi):**
1. **Context yönetimi:** pi'nin `findCutPoint`+split-turn'ü ile opencode'un `select`/`splitTurn` bütçe mantığı birleştirilebilir; pi'nin `retainedTail` zinciri uzun oturumlarda opencode'un `tail_start_id` işaretinden daha eksiksizdir.
2. **Prompt-cache dostu temizleme:** Claude'un `cache_edits`/`cache_reference` deseni (cache_read azalması yerine prefix koruması) yalnızca cache destekleyen provider'larda; zaman-tabanlı MC (60dk eşik) her provider'da geçerli düşük-maliyetli varsayılan.
3. **Güvenlik:** execpolicy DSL'i (Starlark, örnek-doğrulamalı, max karar birleştirme) + BANNED prefix listesi ve `render_decision_for_unmatched_command` matrisi olduğu gibi taşınabilir; `prefix_rule_would_approve_all_commands` amendment güvencesi kritik.
4. **Repo map:** pagerank ağırlık tablosu (×50 chat, ×10 ident, sqrt frekans) ve %15 toleranslı binary search bütçesi doğrudan yeniden uygulanabilir.
5. **Edit güvenliği:** hashline'in 4-hex tam dosya hash'i + satır anchor'ları + seen-lines + fail-closed recovery (uniform offset + komşu doğrulama + duplicate kuralı) — stale-anchor'ı reddeden tek eksiksiz mekanizma; `N:hh` araç katmanı bu temel üzerinde satır imzaları ekler.
