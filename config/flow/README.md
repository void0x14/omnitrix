# Flow Governor — Akış Şablonları ve Kurallar

Bu dizin, deterministik akış boru hattı denetleyicisinin (Flow Governor)
yapılandırma katmanıdır. Çalışma zamanı yükleme post-MVP'dir; şu an akış
şablonları ve kurallar `xai-grok-shell/src/session/flow/` içinde **gömülü
varsayılanlar** olarak yaşar (`definition::default_flows`, `classifier::default_rules`).
`flows.toml.example` ve `rules.toml.example` bu gömülü varsayılanların insan
okur TOML yansımasıdır — ileride config yükleyici bu dosyaları okur ve
varsayılanların üzerine biner.

## Çekirdek ilkeler (kullanıcı direktifi — ASLA ihlal etme)

1. **Akış AI insiyatifinde DEĞİLDİR.** Akış seçimi (universal/commit/direct),
   aşama sırası, aşama kapıları, kanıt doğrulaması ve ihlal düzeltmesi tamamen
   durum makinelerine ve sisteme aittir. AI yalnızca mevcut aşamanın araçlarıyla
   çalışır ve aşamayı `flow_checkpoint` aracıyla kapatır.
2. **Kilitli aşama araçları modelin tool listesinde HİÇ YOKTUR.** Model yasak
   aracı "üretemez"; üretirse mevcut `NonExistingTool` yolu + governor
   düzeltmesi devreye girer.
3. **Erken bitirme reddedilir.** Aşama kanıtı tamamlanmadıkça stop gate
   `KeepWorking{direktif}` döner (aşama başına en fazla 3 düzeltme —
   `MAX_REDIRECTS_PER_STAGE`; sonra deterministik kilit açma).
4. **Kararları AI verir, sonuçları sistem yönetir.** Örnek: stack seçimini AI
   yapar (AI kararı), ama stack doğrulama/çürütme döngüsü ve süre kararı
   (MVP vs TAM) sistemindir.
5. **Düzeltmeler görünmezdir.** Kullanıcı yalnızca aşama geçişlerini ve
   tamamlanmayı görür; ihlaller `flow_events.jsonl` kaydına düşer (7/24 denetim).
6. **Basit görevler ağır akışa zorlanmaz.** `direct` akışı: tek aşama, tüm
   araçlar; yine de stop gate denetimi vardır.

## Akışlar

### universal (12 aşama — `OTONOM ARACIM` belgesi satır 111-114)
```
analyze → research → digest → stack_select → stack_verify → duration
→ plan → decompose → parallel_query → execute → verify → notify
```
- `duration`: sistem kararı (MVP vs TAM) — `FlowDurationSystem`.
- `parallel_query`: sistem pas geçer (bağımlılık analizi — post-MVP).
- `notify`: sistem bildirimi (olay kaydı + opsiyonel webhook, post-MVP).

### commit (4 aşama — kullanıcı örneği)
```
status → stage → commit → commit_verify
```
Bash kuralları: `--force/--hard/reset/rebase/push/cherry-pick/merge/clean/
reflog delete/filter-branch/gc` REDDEDİLİR. Status/verify aşamalarında yalnızca
salt-okunur git komutları.

### direct (1 aşama — "very basic" görevler)
```
do
```

## Aşama → araç grupları

| Grup | Tool'lar (registry id) |
|---|---|
| Read | read_file, read_file_concise, list_dir, grep, hashline_read, codex_read_file, opencode_read, search |
| Search | search_tool, grep, list_dir |
| Write | search_replace, apply_patch, opencode_edit, opencode_write, hashline_edit, image_gen, image_edit |
| Bash | bash, opencode_bash |
| Web | web_search, web_fetch |
| Research | grok_research |
| Computer | grok_computer |
| Plan | enter_plan_mode, exit_plan_mode |
| Task | task, task_output, wait_tasks, workflow, scheduler_create, scheduler_delete, scheduler_list |
| Meta (HER AŞAMADA AÇIK) | ask_user_question, todo_write, update_goal, use_tool, skill, memory_search, memory_get |
| All | execute aşaması (universal), do (direct) |

`meta` grubu asla kilitlenmez (kullanıcı etkileşimi + todo/ask hayati).

## Kayıtlar

- `session_dir/flow_events.jsonl` — tüm akış olayları (aşama geçişleri,
  checkpoint redleri, ihlaller, tamamlanma) + `events.jsonl` içinde
  `flow.*` etiketli satırlar + unified_log.
- Kayıt formatı JSONL: `{"seq":1,"artifact":"problem_list","ok":true,"detail":"..."}`
