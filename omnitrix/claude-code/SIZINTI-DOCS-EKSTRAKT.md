# Claude Code Sızıntı Dokümanları — Analiz Ekstraktı

> Kaynak: `/home/void0x14/Documents/claude-code-leaks/claude-code-source-code/docs/en/`
> Taban: Claude Code **v2.1.88** decompile edilmiş kaynak kodu analizi.
> 5 doküman: 01-telemetry-and-privacy, 02-hidden-features-and-codenames, 03-undercover-mode, 04-remote-control-and-killswitches, 05-future-roadmap

---

## 1. HIDDEN FEATURES (Gizli Özellikler)

### Model Kod Adları (animal codenames)

| Kod Adı | Rol | Kanıt |
|---------|-----|-------|
| **Tengu** (天狗) | Telemetry/ürün prefix'i | 250+ analytics event ve feature flag `tengu_*` prefix'i kullanıyor |
| **Capybara** | Sonnet serisi modeli, v8'de | `capybara-v2-fast[1m]`, v8 davranış sorunları için prompt patch'leri |
| **Fennec** (耳廓狐) | Opus 4.6'nın öncülü | `fennec-latest` → `opus` migrasyonu |
| **Numbat** (袋食蚁兽) | Sıradaki model lansmanı | "Remove this section when we launch numbat" (prompts.ts:402) |

> Kaynak: `02-hidden-features-and-codenames.md`

### Kapybara v8 Davranış Sorunları (dış kullanıcıya yansıyan)

1. Stop-sequence yanlış tetiklenme (~%10, `<functions>` prompt sonunda) — `messages.ts:2141`
2. Boş `tool_result` → sıfır çıktı — `toolResultStorage.ts:281`
3. Aşırı yorumlama — anti-comment prompt patch'leri gerekli — `prompts.ts:204`
4. Yüksek yanlış-iddia oranı: v8 %29-30 FC vs v4 %16.7 — `prompts.ts:237`
5. Yetersiz doğrulama — "thoroughness counterweight" gerekli — `prompts.ts:210`

> Kaynak: `02-hidden-features-and-codenames.md`

### Feature Flag Adlandırma Konvansiyonu

Tüm flag'ler `tengu_` + **rastgele kelime çifti** (sıfat/malzeme + doğa/nesne) — amaç flag adından işlevin anlaşılmasını engellemek:

| Flag | Amaç |
|------|------|
| `tengu_onyx_plover` | Auto Dream (arka plan bellek konsolidasyonu) |
| `tengu_coral_fern` | memdir özelliği |
| `tengu_moth_copse` / `tengu_slate_thimble` | memdir switch'leri |
| `tengu_herring_clock` | Team memory |
| `tengu_passport_quail` | Path özelliği |
| `tengu_sedge_lantern` | Away Summary |
| `tengu_frond_boric` | Analytics killswitch |
| `tengu_amber_quartz_disabled` | Voice mode killswitch |
| `tengu_amber_flint` | Agent teams |
| `tengu_hive_evidence` | Verification agent |

> Kaynak: `02-hidden-features-and-codenames.md`

### Gizli Komutlar

| Komut | Durum | Açıklama |
|-------|-------|----------|
| `/btw` | Aktif | Araya girmeden yan soru sorma |
| `/stickers` | Aktif | Sticker siparişi (tarayıcı açar) |
| `/thinkback` | Aktif | 2025 Yıl Özeti |
| `/effort` | Aktif | Model effort seviyesi |
| `/good-claude` | Stub | Gizli placeholder |
| `/bughunter` | Stub | Gizli placeholder |

> Kaynak: `02-hidden-features-and-codenames.md`

### Ant-Only (İç Kullanıcı) Ayrıcalıkları

- **Prompt farkları**: iç kullanıcıya "daha fazla açıkla", dışa "ekstra kısa ol"; içe özel sayısal anchor'lar ("tools arası ≤25 kelime"), içe özel doğrulama agent'ı, anti-over-commenting patch'leri — `prompts.ts`
- **Tool farkları**: `REPLTool`, `SuggestBackgroundPRTool`, `TungstenTool` (performans paneli), `VerifyPlanExecutionTool`, agent nesting (agent → agent)
- **Model override**: `tengu_ant_model_override` flag'i ile default model/effort/system prompt ekleme/aliases — `antModels.ts`

> Kaynak: `02-hidden-features-and-codenames.md`, `04-remote-control-and-killswitches.md`

---

## 2. KILLSWITCH MEKANİZMALARI

| Mekanizma | Kaynak Dosya | Etki |
|-----------|-------------|------|
| Bypass permissions killswitch (Statsig gate) | `permissions/bypassPermissionsKillswitch.ts` | İzin bypass'ı uzaktan kapatılabilir |
| Auto mode circuit breaker | `permissions/autoModeState.ts` | Auto moda yeniden giriş engellenir |
| Fast mode killswitch | `fastMode.ts` → `GET /api/claude_code_penguin_mode` | Kullanıcı için **kalıcı** fast mode kapatma |
| Analytics sink killswitch | `analytics/sinkKillswitch.ts:4` → `tengu_frond_boric` | Tüm analytics çıkışı uzaktan durdurulur |
| Agent teams gate | `agentSwarmsEnabled.ts` → `tengu_amber_flint` | Env var + GrowthBook gate ikisi de gerekli |
| Voice mode killswitch | `voice/voiceModeEnabled.ts:21` → `tengu_amber_quartz_disabled` | Acil kapatma |
| Model override | `model/antModels.ts:32-33` → `tengu_ant_model_override` | Ant kullanıcıların modeli uzaktan değiştirilir |
| Fast mode flag'leri | `tengu_penguins_off`, `tengu_marble_sandcastle` | Fast mode kullanılabilirliği |

### Remote Managed Settings (Uzaktan Yönetilen Ayarlar)

- Her uygun oturum `GET /api/claude_code/settings` çeker — `remoteManagedSettings/index.ts:105-107`
- **Saatte bir** poll, hata durumunda 5 retry — `index.ts:52-54`
- OAuth kullanıcıları için yalnızca Enterprise/C4E ve Team aboneleri uygun
- **Accept-or-Die**: "tehlikeli" değişiklikte kullanıcı reddederse `gracefulShutdownSync(1)` ile **uygulama zorla kapatılır** — `securityCheck.tsx:67-73`
- Sunucu erişilemezse diske cache'lenmiş ayarlar kullanılır ve uygulanmış ayarlar kalıcı kalır — `index.ts:433-436`

> Kaynak: `04-remote-control-and-killswitches.md`

---

## 3. UNDERCOVER MODE

**Tanım**: Anthropic çalışanlarının (sadece `USER_TYPE === 'ant'`) public/open-source repo'lara katkı yaparken AI izini tamamen gizleyen güvenlik sistemi. Model, katkılarını insan geliştirici gibi sunmaya zorlanır.

### Aktivasyon Mantığı — `undercover.ts:28-37`

```typescript
if (process.env.USER_TYPE === 'ant') {
  if (isEnvTruthy(process.env.CLAUDE_CODE_UNDERCOVER)) return true
  return getRepoClassCached() !== 'internal'  // Auto: internal allowlist dışı her repo
}
```

- **Varsayılan AÇIK**: internal allowlist dışındaki tüm repolarda otomatik aktif
- **Kapatılamaz**: "There is NO force-OFF. This guards against model codename leaks"
- Dış build'lerde bundler tarafından dead-code-eliminated; asla çalışmaz

### Modele Verilen Talimat — `undercover.ts:39-69`

Commit mesajı / PR başlığı / PR body'sinden YASAKLANANLAR:
- Model kod adları (Capybara, Tengu vb.)
- Yayınlanmamış sürüm numaraları (opus-4-7, sonnet-4-8)
- İç repo/proje adları, internal tooling, Slack kanalları, short linkler (go/cc)
- **"Claude Code" ifadesi ve AI olduğuna dair her ipucu**
- Co-Authored-By satırları ve tüm attribution

İYİ: "Fix race condition in file watcher initialization"
KÖTÜ: "1-shotted by claude-opus-4-6", "Generated with Claude Code"

### Attribution Gizleme Altyapısı

- `maskModelCodename()` — `capybara-v2-fast` → `cap*****-v2-fast` — `model/model.ts:386-392`
- Attribution fallback: "Claude Opus 4.6" — `attribution.ts:70-72`

> Kaynak: `03-undercover-mode.md`

---

## 4. TELEMETRİ ANALİZİ

### İki Katmanlı Pipeline

| Katman | Endpoint | Detay |
|--------|----------|-------|
| **1P (First-party)** | `https://api.anthropic.com/api/event_logging/batch` | OpenTelemetry + Protocol Buffers; 200 event/batch, 10 sn'de bir flush; quadratic backoff, 8 retry, disk persist (`~/.claude/telemetry/`) |
| **3P (Datadog)** | `https://http-intake.logs.us5.datadoghq.com/api/v2/logs` | Yalnızca 64 ön-onaylı event tipi; token `pubbbf48e6d78dae54bceaa4acf463299bf` |

> Kaynak: `01-telemetry-and-privacy.md` (kaynak: `analytics/firstPartyEventLoggingExporter.ts`, `analytics/datadog.ts`)

### Toplanan Veriler (`metadata.ts:417-496`)

- **Ortam parmak izi**: platform, arch, nodeVersion, terminal tipi, package manager'lar, CI/CD + GitHub Actions metadata, WSL versiyonu, Linux distro, kernel, VCS tipi
- **Süreç metrikleri**: uptime, rss, heapTotal/heapUsed, CPU %, memory arrays
- **Kullanıcı takibi**: model, session ID, user ID, device ID, account UUID, organization UUID, abonelik tier'i (max/pro/enterprise/team), **repo remote URL hash'i (SHA256, ilk 16 karakter)**, agent tipi, team adı, parent session ID
- **Dosya uzantısı takibi**: `rm, mv, cp, touch, mkdir, chmod, chown, cat, head, tail, sort, stat, diff, wc, grep, rg, sed` komutlarındaki dosya argümanlarının **uzantıları** ayrıca loglanıyor (`metadata.ts:340-412`)

### Tool Input Loglama

- Varsayılan kırpma: string 512 char (128 + ellipsis gösterim), JSON 4096 char, array max 20 item, nested 2 seviye — `metadata.ts:236-241`
- **`OTEL_LOG_TOOL_DETAILS=1`** → **tam tool input'ları loglanır** (backdoor) — `metadata.ts:86-88`

### Opt-Out Sorunu

- 1P logging **kapatılamıyor**: `is1PEventLoggingEnabled()` yalnızca test ortamı / Bedrock-Vertex (3P cloud) / global opt-out ile false döner — `firstPartyEventLogger.ts:141-144`
- **Kullanıcıya açık hiçbir ayar yok**
- GrowthBook A/B grupları rızasız atanır; `id, sessionId, deviceID, platform, organizationUUID, subscriptionType` gönderilir — `growthbook.ts`

### Telemetri Çıkarımları (omnitrix dersi)

- Kullanıcı repo URL hash'i gönderiliyor → rakibin hangi projelerde kullanıldığı korporasyon bazında eşleştirilebiliyor
- `tengu_frond_boric` killswitch ile analytics'i uzaktan kapatabilmeleri, veri akışının tek taraflı kontrolü olduğunu gösteriyor

---

## 5. ROADMAP (Rakibin Yönü)

### 1. Yeni Modeller
- **Numbat** (袋食蚁兽): sıradaki model; lansmanda `prompts.ts:402`'deki output-efficiency bölümü revize edilecek → daha iyi native output control bekleniyor
- **Opus 4.7**, **Sonnet 4.8** geliştiriliyor (undercover.ts:49'da yasaklı sürüm numaraları olarak geçiyor)
- Kod adı zinciri: Fennec → Opus 4.6 → Numbat? ; Capybara → Sonnet v8
- 20+ `@[MODEL LAUNCH]` checklist marker'ı: default model, family ID, knowledge cutoff, pricing, context window, thinking support, migration script'leri

### 2. KAIROS — Otonom Agent Modu (en büyük unreleased özellik)
- `<tick>` heartbeat prompt'larıyla "hayatta tutulan" otonom çalışma; "Bias toward action — read files, make changes, commit without asking"
- **Terminal focus'a göre otonomi**: unfocused (kullanıcı yok) → tam otonom; focused → işbirlikçi
- Tool'lar: `SleepTool` (pacing), `SendUserFileTool`, `PushNotificationTool`, `SubscribePRTool` (GitHub PR webhook'ları), `BriefTool` (proaktif durum)
- Flag'ler: KAIROS, PROACTIVE, KAIROS_PUSH_NOTIFICATION, KAIROS_GITHUB_WEBHOOKS, KAIROS_BRIEF

### 3. Voice Mode
- Push-to-talk tam implement edilmiş, `VOICE_MODE` flag'i arkasında
- `voice_stream` WebSocket, mTLS, conversation_engine modelleri; OAuth-only (API key/Bedrock/Vertex yok)

### 4. Unreleased Tool'lar

| Tool | Flag | Açıklama |
|------|------|----------|
| WebBrowserTool | `WEB_BROWSER_TOOL` | Dahili tarayıcı otomasyonu (codename: bagel) |
| TerminalCaptureTool | `TERMINAL_PANEL` | Terminal panel yakalama/izleme |
| WorkflowTool | `WORKFLOW_SCRIPTS` | Önceden tanımlı workflow script'leri |
| MonitorTool | `MONITOR_TOOL` | Sistem/süreç izleme |
| SnipTool | `HISTORY_SNIP` | Konuşma geçmişi kırpma |
| ListPeersTool | `UDS_INBOX` | Unix domain socket peer keşfi |
| RemoteTriggerTool | `AGENT_TRIGGERS_REMOTE` | Uzaktan agent tetikleme |
| VerifyPlanExecutionTool | VERIFY_PLAN env | Plan doğrulama |
| OverflowTestTool | `OVERFLOW_TEST_TOOL` | Context overflow testi |

### 5. Coordinator Mode — `COORDINATOR_MODE`
Multi-agent koordinasyon: paylaşılan state ve mesajlaşma ile birden çok agent koordineli görev.

### 6. Buddy System (sanal evcil hayvanlar, tam implement, lansman yok)
- 18 tür (capybara dahil — tür adı model canary'siyle çakışıyor, `String.fromCharCode()` ile gizleniyor), 5 nadirlik tier'i (%60→%1), 7 şapka, 5 istatistik (DEBUGGING, PATIENCE, CHAOS, WISDOM, SNARK), %1 shiny, kullanıcı ID hash'inden deterministik üretim — `src/buddy/`

### 7. Dream Task — `tengu_onyx_plover`
İdlerken arka planda bellek konsolidasyonu yapan auto-dreaming subagent.

> Kaynak: `05-future-roadmap.md`

---

## 6. OMNITRIX İÇİN DERSLER (Özet)

1. **Kod adı + flag gizleme konvansiyonu**: `tengu_` + anlamsız kelime çifti flag adlandırması, canary string listesi (`excluded-strings.txt`), build çıktısı taraması — omnitrix gizli özelliklerini aynı şekilde koruyabilir; tersine, rakibin flag'lerini bu pattern'den tanıyabiliriz.
2. **Uzaktan kontrol mimarisi**: saatlik settings poll + accept-or-die dialog + GrowthBook flag killswitch'leri — omnitrix'in kendi güncelleme/killswitch altyapısı için referans; kullanıcı rızası konusunda farklılaşma fırsatı (Claude Code kullanıcıyı zorla kapatıyor).
3. **Undercover mode**: AI attribution'ı tamamen gizleme sistemi — bu, dış repo katkılarında şeffaflık kozu. Omnitrix açık kaynak katkılarında AI işaretlemesini **varsayılan yaparak** etik fark yaratabilir (rakip bunu gizliyor).
4. **Telemetri**: repo URL hash'i + tool input backdoor'u (`OTEL_LOG_TOOL_DETAILS`) + kapatılamayan 1P logging — omnitrix: varsayılan kapalı / opt-in telemetri, mükemmel bir satış/pazarlama farkı. Ayrıca hangi verinin "uzak analize" gittiğini bilmek, gizlilik vaadini ürünleştirmeyi sağlar.
5. **Rakibin yönü = otonom agent**: KAIROS (tick-heartbeat, terminal focus'a göre otonomi, PR webhook aboneliği, push notification) + Coordinator Mode + Dream Task — omnitrix roadmap'i bu üç ayağı önceliklendirmeli; WebBrowserTool (bagel) zaten kodda hazır.
6. **Model yönetim olgunluğu**: 20+ `@[MODEL LAUNCH]` checklist'i, migration script'leri (fennec→opus), kod adı maskeleme — model değişimini süreçleştirmek için şablon.
7. **Capybara v8 dersleri**: yüksek false-claim oranı (%29-30) ve stop-sequence tetikleme sorunları — omnitrix prompt/pipeline'ında doğrulama agent'ı (tengu_hive_evidence benzeri) ve anti-false-claim katmanı şart.
8. **Buddy System**: gamification (pet + rarity + hat + shiny) kullanıcı bağlılığı aracı olarak ciddiye alınıyor — omnitrix UI/UX katmanında kullanılabilir.

---

## Kaynak Dosya Eşlemesi

| Doküman | Konu |
|---------|------|
| `docs/en/01-telemetry-and-privacy.md` | Telemetri & gizlilik |
| `docs/en/02-hidden-features-and-codenames.md` | Hidden features & kod adları |
| `docs/en/03-undercover-mode.md` | Undercover mode |
| `docs/en/04-remote-control-and-killswitches.md` | Uzaktan kontrol & killswitch'ler |
| `docs/en/05-future-roadmap.md` | Gelecek roadmap |
