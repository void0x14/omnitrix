# No-Telemetry — Omnitrix'te xAI Telemetrisini Yok Etme

**Tarih:** 2026-07-29
**Yetki:** Kullanıcı isteği (Elon Musk/xAI'ya veri gitmesin)

## Amaç

rossnoah/grok-build-no-telemetry fork'unun yaptığı gibi, omnitrix içindeki
vendored `xai-*` ağacında SpaceXAI ürün telemetrisini tamamen devre dışı
bırakmak. Env/config/remote hiçbir ayar tekrar açamaz.

## Kapsam

Altı telemetri kanalı yok edilecek:

| # | Kanal | Mekanizma |
|---|---|---|
| 1 | Product analytics | Mixpanel + product event HTTP POST'lar (xai-grok-telemetry -> xai-mixpanel) |
| 2 | Mixpanel network I/O | xai-mixpanel crate'inin tüm HTTP çağrıları (belt-and-braces) |
| 3 | Sentry | Hata/çökme raporlama |
| 4 | OTLP export | OpenTelemetry span export -> cli-chat-proxy |
| 5 | Trace upload | Session trace / GCS artifact upload (heap profil, auth diagnostic) |
| 6 | Feedback | /feedback POST -> cli-chat-proxy |

## Değişiklikler

### A — 6 patch'in birebir uygulanması (crates/codegen/xai-*)

Her patch hedef dosyayı ve yapılacak değişikliği belirtir.

**0001 — Product analytics devre dışı (`xai-grok-shell/src/agent/config.rs`, `xai-grok-telemetry/src/client.rs`, `xai-grok-telemetry/src/config.rs`)**
- `Config::is_telemetry_enabled()` -> `false`
- `Config::resolve_telemetry_mode()` -> `TelemetryMode::Disabled` (env/feature/config gereksiz)
- `track()` -> boş gövde (hiçbir HTTP POST yok)
- `sync_profile()` -> boş gövde (Mixpanel engage yok)
- `init()` -> `*guard = None` (telemetry client kurma)
- `init_if_needed()` -> boş gövde
- `TelemetryConfig::default()` -> tüm events/mixpanel alanları `None`/`false`
- `apply_env_overrides()` -> Mixpanel/events env'leri yok sayılır

**0002 — Mixpanel network I/O'nun kısırlaştırılması (`xai-mixpanel/src/lib.rs`)**
- `Mixpanel::track()` -> boş gövde (base64 encode + POST api.mixpanel.com/track yok)
- `Mixpanel::engage()` -> boş gövde (POST api.mixpanel.com/engage yok)

**0003 — Sentry devre dışı (`xai-grok-telemetry/src/sentry.rs`, `xai-grok-shell/src/agent/config.rs`)**
- `init()` -> `SentryClientOptions::default()` (DSN'siz, hiçbir şey raporlanmaz)
- `flush_on_shutdown()` -> boş gövde
- `is_error_reporting_disabled_sync()` -> `true` (env/feature yok sayılır)

**0004 — OTLP export devre dışı (`xai-grok-telemetry/src/config.rs`, `xai-grok-telemetry/src/instrumentation.rs`, `xai-grok-telemetry/src/otel_layer/mod.rs`, `xai-grok-shell/src/agent/config.rs`, `xai-grok-shell/src/auth/credential_provider.rs`)**
- `TelemetryConfig::default()` -> `otel_enabled: Some(false)`, `otel_log_user_prompts: Some(false)`, `otel_log_tool_details: Some(false)`
- `InstrumentationMode` -> default `Disabled`, env `Server` yine Disabled
- `build_tracer_provider()` -> `config.exporter.enabled = false` (provider hep disabled)
- `is_telemetry_explicitly_disabled_sync()` -> `true`
- `build_default_otel_layer_config()` -> `enabled: false && ...` (otomatik disabled)
- `credential_provider.rs` -> OTLP export pin disabled

**0005 — Trace upload devre dışı (`xai-grok-shell/src/agent/config.rs`, `xai-grok-telemetry/src/config.rs`)**
- `Config::is_trace_upload_enabled()` -> `false`
- `Config::resolve_trace_upload()` -> `Resolved::new(false, Default)`
- `TelemetryConfig::trace_upload` default -> `Some(false)`
- `apply_env_overrides()` -> trace_upload env yok sayılır

**0006 — Feedback devre dışı (`xai-grok-shell/src/agent/config.rs`, `xai-grok-shell/src/extensions/feedback.rs`)**
- `Config::is_feedback_enabled()` -> `false`
- `Config::resolve_feedback()` -> `Resolved::new(false, Default)`
- Feedback hata mesajı -> "Feedback is permanently disabled in this no-telemetry fork"

### B — CI kapı düzenlemesi

`bin/ci-gates.sh`'te I2a kontrolü:
- Varsayılan VENDOR_BASE çözümlemesi DEĞİŞMEZ (upstream/main veya "Synced from monorepo" commit)
- I2a diff taramasında **kalıcı muafiyet listesi** (`grep -vE`) kullanılır — patch'lenmiş
  telemetri dosyaları diff'ten çıkarılır, **diğer tüm xai-* dosyaları** hala denetlenir:
  - `crates/codegen/xai-grok-telemetry/`
  - `crates/codegen/xai-mixpanel/`
  - `crates/codegen/xai-grok-shell/src/agent/config.rs`
  - `crates/codegen/xai-grok-shell/src/auth/credential_provider.rs`
  - `crates/codegen/xai-grok-shell/src/extensions/feedback.rs`
- Muafiyet **kalıcı ve koşulsuz** (env değişkeni yok, her zaman aktif)
  Çünkü patch'ler kalıcı — env ile açıp kapatılacak bir şey değil.
- Herhangi bir başka xai-* dosyası değişirse I2a hala kırmızı olur

### C — No-telemetry faz kapısı (I8)

Yeni bir faz kapısı (`bin/ci-gates.sh`'e I8 olarak eklenir) şu 6 fonksiyonun
literal `false`/`true` döndürdüğünü grepl'eyerek assert eder (test dosyaları
`#[cfg(test)]` blokları atlanır, yalnızca üretim yolu taranır):

1. `Config::is_telemetry_enabled()` -> `false`
2. `is_session_metrics_enabled()` -> `false`
3. `Config::is_trace_upload_enabled()` -> `false`
4. `Config::is_feedback_enabled()` -> `false`
5. `is_error_reporting_disabled_sync()` -> `true`
6. `is_telemetry_explicitly_disabled_sync()` -> `true`

Birisi değişirse kırmızı kapi → telemetri tekrar açılmış demektir.

### D — Kaynak patch'lerin saklanması

Fork'un `patches/` dizini omnitrix'te:
- `third_party/no-telemetry-patches/0001-disable-product-analytics.patch` (vb.)
Her patch imzalı SHA256 referansı ve uyumlu xai-* versiyon notu içerir.

Opsiyonel: `bin/rebase-no-telemetry.sh` — telemetri patch'lerini yeni upstream
base'e karşı yeniden uygulamak için.

### E — Doğrulama

- `cargo check --workspace --all-targets`
- `cargo test --workspace`
- `cargo run -p omnitrix -- --version` (minimum version gate çalışmalı)
- `OMNI_NO_TELEMETRY_PATCHES=1 bin/ci-gates.sh` (I2a muafiyetli)
- `bin/ci-gates.sh --build` (I8 no-telemetry gate yeşil mi)
- Tüm 6 fonksiyon grep testi

## Kararlar

- **Patch'leri doğrudan xai-* ağacına uygula (overlay değil).** Fork'un yaptığı
  da bu: çalışma ağacındaki dosyalar fiziksel olarak değişir. Patch'ler
  üçüncü_tarafta referans olarak saklanır.
- **I2a'ya yazılı muafiyet kullan.** VENDOR_BASE'i değiştirmek yerine belirli
  dosyaları diff dışı bırakalım. Gerisi I2a diffleri yakalamaya devam eder.
- **I8 no-telemetry gate ekle.** Gerçek koruma burada — kodun anlamsal
  bütünlüğünü (telemetri kapalı mı?) mekanik olarak denetler.
- **Mimari invariant bozulur ancak kabul edilir:** Kullanıcı bilerek ve isteyerek
  bu sınırı aşmayı kabul etmiştir. I2a muafiyeti + I8 gate bu kararı kapatır.

## Gelecek rebase

Upstream xAI monorepo'dan yeni bir senkronizasyon geldiğinde:
1. Yeni xai-* ağacı patchesiz olur
2. `scripts/rebase-no-telemetry.sh` eski patch'leri yeni base'e uygular
3. Hunk hataları varsa elle düzeltilir
4. I8 gate testleri hedef fonksiyonların hala false döndürdüğünü doğrular
