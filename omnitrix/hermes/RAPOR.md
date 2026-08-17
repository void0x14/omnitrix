# Hermes — Feature Ekstraksiyon Raporu

Kaynak: `NousResearch/hermes-agent` (2026-08-14 tarihinde `git clone --depth 1` ile
`/home/void0x14/Documents/omnitrix/omnitrix/hermes/hermes-agent/` altına indirildi).

Not: Görevdeki "Yunan tanrısı Hermes mesajlaşma protokolü" adayı ile karışma riski
yoktu — bulunan tek kodlama ajanı adayı bu repodur (Hermes Protocol, `hermes-agent-org/hermes`
adresindeki aynı projenin farklı bir org hesabına taşınmış hâli + `Lumio-Research/hermes-agent-rs`
adlı v0.1 Rust portu da aynı ailenin parçasıdır).

## 1. Kimlik (dil, mimari, lisans)

| Alan | Değer |
|---|---|
| Repo | https://github.com/NousResearch/hermes-agent |
| Geliştirici | Nous Research (Hermes 3 model laboratuvarı) |
| Lisans | MIT (LICENSE dosyası doğrulandı) |
| Dil | Python 3.11+ (ana) + TypeScript (TUI/Desktop/web) |
| Yıldız | 230.556, 45.698 fork, son push 2026-08-14 (aktif) |
| Yüzeyler | Terminal TUI (Textual tabanlı), Hermes Desktop (Electron, `apps/desktop/`), web, 20+ mesajlaşma platformu (gateway) |
| Model desteği | 200+ (Nous Portal, OpenRouter, OpenAI, Anthropic, yerel llama.cpp/vLLM) — provider-agnostik |
| Boyut | Repo ~222 MB (depth 1) |

Mimari: tek Python agent core (`agent/` = 134 modül) + `tools/` (60+ tool) +
`gateway/` (platform adaptörleri) + `hermes/` (state/CLI) + `apps/desktop/` (Electron).
Ajan döngüsü `agent/conversation_loop.py` + `agent/turn_finalizer.py` üzerinden akar.

## 2. EN İYİ feature'lar (ÇAL listesi)

### A. Persistent memory — nasıl çalışıyor (kod doğrulamalı)

1. **İki depo, tek tool** (`tools/memory_tool.py`, 1248 satır):
   - `MEMORY.md` → ajanın kişisel notları (ortam gerçekleri, proje kuralları, araç tuhafllıkları)
   - `USER.md` → kullanıcı profili (tercihler, iletişim stili, beklentiler)
   - Tek `memory` tool'u, `add|replace|remove` aksiyonları; `§` ayraçlı entry'ler;
     karakter limitleri model-bağımsız (memory: 2200, user: 1375 char)
2. **Frozen-snapshot pattern'i** — memory, session başında system prompt'a donmuş
   anlık görüntü olarak enjekte edilir. Session içi yazımlar diske ANINDA yazılır
   (durable, fcntl/msvcrt file-lock + atomik write) ama system prompt'u değiştirmez —
   böylece tüm session boyunca prompt prefix cache'i korunur. Yeni snapshot bir
   sonraki session'da gelir. (Bu, omnitrix'e aktarılacak en değerli pattern.)
3. **Nudge mekanizması** — `memory.nudge_interval` (varsayılan 10) her N kullanıcı
   turunda ajana "kaydedilecek bir şey var mı?" diye hatırlatır; `memory` tool'u
   kullanıldığında sayaç sıfırlanır (`agent/turn_context.py:643`,
   `agent/tool_executor.py:606`). Ayrıca session reset öncesi "flush" kaydı.
4. **Güvenlik**: memory içeriği system prompt'a girmeden önce injection/exfil
   pattern taraması (`tools/threat_patterns.py`, "strict" scope), dışarıdan edit
   sonrası round-trip doğrulaması (drift guard — sessiz veri kaybını engeller).
5. **Üç katman + eklenti**: session memory + persistent (SQLite + FTS5 full-text,
   `hermes_state_search.py`, ~10ms arama) + kullanıcı modeli (Honcho plugini,
   profile-scoped izolasyon). Harici provider'lar `memory.provider` config'iyle
   takılabilir (`agent/memory_manager.py`).

### B. Self-improving (kapalı öğrenme döngüsü) — nasıl çalışıyor

1. **Skill nudge** — `skills.creation_nudge_interval` (varsayılan 15) her N
   tool-çağrı iterasyonunda ajana "bu görevden ders çıkar" hatırlatır
   (`agent/turn_finalizer.py:732`, `agent/codex_runtime.py:884`).
2. **Arka plan review fork'u** — turn bittikten SONRA, kullanıcıya cevap
   verildikten sonra ayrı bir `AIAgent` spawn edilir (`agent/background_review.py`,
   1144 satır, ~30K token/event): memory review + skill review prompt'larıyla
   konuşmayı tarar. SKILL_REVIEW_PROMPT'i class-level skill'ler ister, düzeltme
   sinyallerini (tarz, workflow, bugfix) skill'lere gömer, tercihen hâlihazırda
   yüklenmiş skill'i patch eder. Cron/arka plan ajanlarında devre dışı (fayda yok).
3. **Skill lifecycle** (`tools/skill_manager_tool.py`, 1810 satır): `skill_manage`
   tool'u create/edit/patch/delete; frontmatter doğrulama, boyut limiti (≤15KB),
   güvenlik taraması, lint bulguları. Skill'ler `~/.hermes/skills/<category>/<name>/SKILL.md`
   markdown dosyaları — agentskills.io standardı, dışarıdan paylaşılabilir.
4. **Curator** (`agent/curator.py`, 2019 satır): periyodik arka plan LLM review'i —
   skill'leri consolidate eder, eskiyenleri archive/prune eder, silinen skill'leri
   "absorbed into" ile sınıflandırır, rapor üretir. Guard'ları var: kullanılmış
   skill'ler silinmez, backup + human-review varsayılanı.
5. **Learning graph** (`agent/learning_graph.py`): Desktop'ta "görünür öğrenme" —
   skill ↔ memory bağlantı grafiği (lexical overlap ile), kullanıcıya zaman içinde
   ne öğrendiğini gösterir.

### C. Diğer dikkat çekenler

- **Terminal backends**: local, Docker, SSH, Daytona, Modal, Singularity — VPS'te
  veya serverless'ta çalışır; "hermes" laptop'a bağlı değil.
- **Programmatic tool calling**: Python script'ten tool RPC (delegate), sıfır
  context-cost pipeline'lar.
- **Cron scheduler**: doğal dilde planlanmış görevler, platforma teslimat.
- **GEPA/DSPy self-evolution** (ayrı repo `hermes-agent-self-evolution`): skill
  metinlerini evrimleştirir, ICLR 2026 Oral, ~$2-10/optimizasyon.

## 3. Dezavantajlar (LOBOTOMİ listesi)

- **Ağır bağımlılık**: Python + uv + 60+ tool + Electron desktop; repo 222MB,
  kurulum wizard'ı ağır — omnitrix'e bütün olarak taşınması pratik değil.
- **v0.x hızla değişiyor**: API/dokümantasyon hareketli hedef; stabil kontrat yok
  (2026-07'de v0.18.2, günde birden fazla commit).
- **Token maliyeti**: her background review ayrı bir agent spawn ediyor (~30K
  token/event); nudge interval'ları küçükse sürekli review maliyeti birikir.
- **Memory dar**: 2200/1375 char limitleri — derin proje bilgisi için yetersiz;
  "bounded curated memory" felsefesi gereği gerçek bilgi çoğunlukla skill'lere
  itiliyor.
- **Learning latency**: öğrenme değeri haftalarca kullanım ister; tek seferlik
  görevlerde fayda yok (raporlar "3 hafta değerlendir" diyor).
- **Curator riski**: otomatik konsolidasyon/archive yanlış silme yapabilir —
  drift guard ve backup'lar var ama karmaşık bir sistem, bakım yükü var.
- **Bir gateway'de 20+ platform = saldırı yüzeyi**: CVE'siz olduğu iddia ediliyor
  ama platform adaptör sayısı güvenlik yüzeyini artırıyor.

## 4. omnitrix için öneri

1. **Frozen-snapshot memory'yi kopyala** (en yüksek değer/çaba oranı): system
   prompt'a enjekte edilen sabit snapshot + disk'e anında yazım + `§`-ayraçlı
   char-limitli entry'ler + nudge sayacı (her N turn). Bu pattern omnitrix'in
   mevcut memory'sinden çok daha ucuz ve cache-dostu.
2. **Nudge-based learning loop'u uygula**: `creation_nudge_interval` + turn sonrası
   arka plan review fork'u (düşük maliyetli model kullanarak). Ama review'ı ayrı
   agent yerine omnitrix'in kendi tool'larına boğazla — Hermes'in ~30K token/event
   fork maliyetini taşımak gerekmez.
3. **Skill = markdown + frontmatter** standardını kabul et (agentskills.io): skill'ler
   dosya sistemiyle paylaşılabilir olmalı, DB'ye gömülmemeli.
4. **Alma**: `tools/memory_tool.py`, `agent/turn_finalizer.py` (nudge bölümü),
   `agent/background_review.py` (prompt'lar), `agent/curator.py` (konsolidasyon
   fikirleri) dosyalarını referans olarak oku; bütün repo'yu entegre etme.
5. **Yapma**: gateway'li 20+ platform yüzeyini, Electron desktop'ı ve 60+ tool'u
   kopyalama — omnitrix'in kapsamı terminal + çekirdek özellikler olmalı.
