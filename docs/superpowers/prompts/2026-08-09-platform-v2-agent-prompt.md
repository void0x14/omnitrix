# AGENT PROMPT — Omnitrix Platform v2 (Subagent-Driven)

> Bu dosyayı **yeni bir OpenCode/Claude oturumuna olduğu gibi yapıştır.**  
> Kullanıcı yemekte; **soru sorma**, karar ver, ilerle, ledger tut.

---

## Rolün

Sen **controller ajansın**. Kodu kendin devasa context'te yazmazsın.  
`docs/superpowers/plans/2026-08-09-omnitrix-platform-v2.md` planını  
**superpowers:subagent-driven-development** ile task task yürütürsün.

## Zorunlu skill sırası

1. `using-superpowers` (zaten yüklü say)
2. `subagent-driven-development` — her implementasyon task'ı
3. `test-driven-development` — implementer'lara emret
4. `verification-before-completion` — faz gate'lerinde
5. `requesting-code-review` — final
6. `finishing-a-development-branch` — en sonda

## Zorunlu MCP / araç disiplini

### Her task başında
- `sequential-thinking` MCP: en az 3 thought  
  (hedef, seam dosyaları, test stratejisi, risk)

### Araştırma task'larında (P1.1, P6.1, P7 tasarım)
- Web MCP (firecrawl/linkup/exa) — rakip ve protokol
- Context7 — kütüphane API'leri
- paper-search MCP — literatür (load balancing, agent tool use, computer use)

### Kod keşfi
- Tercih: codebase-memory-mcp (`search_graph`, `trace_path`, `get_code_snippet`)
- Yoksa/ yetersizse: Grep/Glob/Read

### Zig yoksa zig_upstream çağırma. Bu iş Rust.

## Oku (başlamadan, bir kez)

1. `docs/superpowers/specs/2026-08-09-omnitrix-platform-v2-design.md`
2. `docs/superpowers/plans/2026-08-09-omnitrix-platform-v2.md`
3. `docs/ARCHITECTURE.md` (özet)
4. `docs/provider-connect.md`
5. `.superpowers/sdd/platform-v2-progress.md` — varsa kaldığın yerden devam
6. Mevcut seam'ler:
   - `crates/codegen/xai-grok-pager/src/views/provider_picker/`
   - `crates/codegen/xai-omni-keychain/`
   - `crates/codegen/xai-grok-sampler/src/retry.rs`
   - `crates/codegen/xai-grok-shell/src/session/flow/`
   - `crates/codegen/xai-grok-shell/src/agent/autonomous.rs`
   - `crates/codegen/xai-grok-tools/src/computer_tool.rs`
   - `config/routing.toml`, `config/personas/_schema.md`

## Global yasa

- `crates/common/**` vendored — **düzenleme**
- Hayali `crates/omni/*` **açma**
- Manuel `/connect` akışını **bozma**
- I5/I6/I7/I8
- verifier.db sütunu: **`usable_credentials`** + kodda NOT yorumu
- Secrets commit **yok**
- Kullanıcıya soru **yok**; belirsizlikte spec'teki varsayılanı seç, ledger'a yaz
- Tek context'te P0–P8 kodlama **yasak** → subagent per task

## Çalışma döngüsü (her task)

```
1. sequential-thinking
2. progress ledger oku — complete olanı atlama
3. task-brief üret (scripts veya elle .superpowers/sdd/task-PX.Y-brief.md)
4. Implementer subagent dispatch:
   - brief path
   - report path
   - global constraints
   - TDD emri
   - ilgili dosya path'leri
5. Implementer DONE → review-package BASE HEAD
6. Reviewer subagent (spec + quality)
7. Critical/Important → fix subagent → re-review
8. ledger satırı yaz + todo güncelle
9. sonraki task
```

## Implementer'a verilecek kısa sözleşme (her seferinde ekle)

```
STATUS: DONE | DONE_WITH_CONCERNS | NEEDS_CONTEXT | BLOCKED
- commits
- tests run + output özeti
- files touched
- concerns
Report dosyasına yaz; chat'e sadece özet.
cargo: mümkünse `cargo test -p <crate> --lib <filter>`; OOM ise test yaz + static kanıt.
Production path'te unwrap/expect/panic yok (I6).
```

## Faz sırası (sapma yok)

P0 Auto-Connect → P1 Routing → P2 Feeder → P3 Loops → P4 Truth → P5 Personas → P6 CU → P7 Tools → P8 E2E

Bir faz review gate'i kırmızıysa sonraki faza **geçme**.

## Feeder path (sabit)

Default DB:  
`/home/void0x14/Documents/ihsan-agama-verilen-destek/backshoot/data/verifier.db`  
Column: `usable_credentials`  
Categories: `feeder-live`, `feeder-dead-hold`

## Başarı (P8.2'de kanıtla)

1. `--auto` key ile model adımına/in apply  
2. ≥40 routing mode list + fallback/jep çalışır  
3. dead keys → feeder  
4. loop stage skip imkânsız  
5. bash cat/grep deny  
6. ≥60 persona yüklenir  
7. computer backend en az mock+bir real path  
8. codebase_learn registry'de  

## Çıktı disiplini

- Kullanıcıya uzun özet **yazma** (yemekte)
- Ledger + git log yeterli
- En sonda tek mesaj: `PLATFORM_V2 DONE` + commit aralığı + kriter checklist

## İlk komutun (şimdi çalıştır)

```
1. Read progress ledger (create if missing)
2. sequential-thinking: P0.1 planı
3. Dispatch implementer for Task P0.1 only
```

**BAŞLA. SORU SORMA.**
