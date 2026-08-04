# No-Telemetry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** rossnoah/grok-build-no-telemetry fork'undaki 6 telemetri patch'ini omnitrix'in vendored xai-* ağacına uygula, CI kapılarını güncelle, her şeyin hala derlenip çalıştığını doğrula.

**Architecture:** 6 patch sırayla `crates/codegen/xai-*` dosyalarına uygulanır. Her patch bir telemetri kanalını (Product analytics, Mixpanel, Sentry, OTLP, Trace upload, Feedback) kalıcı olarak devre dışı bırakır. Sonra `bin/ci-gates.sh`'te I2a muafiyet + I8 no-telemetry kapısı eklenir. En son build + test ile doğrulanır.

**Tech Stack:** Rust, bash (ci-gates.sh), git patch

---

### Task 1: Patch kaynaklarını third_party/ altına indir

**Files:**
- Create: `third_party/no-telemetry-patches/0001-disable-product-analytics.patch`
- Create: `third_party/no-telemetry-patches/0002-neuter-mixpanel-crate.patch`
- Create: `third_party/no-telemetry-patches/0003-disable-sentry.patch`
- Create: `third_party/no-telemetry-patches/0004-disable-otlp-export.patch`
- Create: `third_party/no-telemetry-patches/0005-disable-trace-upload.patch`
- Create: `third_party/no-telemetry-patches/0006-disable-feedback.patch`

- [ ] **Step 1: Download 6 patch file from GitHub raw**

Run:
```bash
BASE="https://raw.githubusercontent.com/rossnoah/grok-build-no-telemetry/main/patches"
for f in 0001-disable-product-analytics 0002-neuter-mixpanel-crate \
         0003-disable-sentry 0004-disable-otlp-export \
         0005-disable-trace-upload 0006-disable-feedback; do
  curl -fsSL "$BASE/$f.patch" -o "third_party/no-telemetry-patches/$f.patch"
done
```

Expected: 6 dosya `third_party/no-telemetry-patches/` altında oluşur.

- [ ] **Step 2: Patch'leri uygula (git apply)**

Her patch'i `crates/codegen` altındaki ilgili dosyalara uygula:

```bash
cd /home/void0x14/Documents/omnitrix
git apply --ignore-whitespace third_party/no-telemetry-patches/0001-disable-product-analytics.patch
git apply --ignore-whitespace third_party/no-telemetry-patches/0002-neuter-mixpanel-crate.patch
git apply --ignore-whitespace third_party/no-telemetry-patches/0003-disable-sentry.patch
git apply --ignore-whitespace third_party/no-telemetry-patches/0004-disable-otlp-export.patch
git apply --ignore-whitespace third_party/no-telemetry-patches/0005-disable-trace-upload.patch
git apply --ignore-whitespace third_party/no-telemetry-patches/0006-disable-feedback.patch
```

If any `git apply` fails due to hunk mismatch, the patch must be applied manually per the patch content. Use `--reject` flag to see exact failures.

- [ ] **Step 3: Doğrulama — değişen dosyaları kontrol et**

```bash
git diff --stat
```
Expected: `crates/codegen/xai-grok-telemetry/src/client.rs`, `config.rs`, `sentry.rs`, `instrumentation.rs`, `otel_layer/mod.rs`, `xai-mixpanel/src/lib.rs`, `xai-grok-shell/src/agent/config.rs`, `xai-grok-shell/src/auth/credential_provider.rs`, `xai-grok-shell/src/extensions/feedback.rs` değişmiş olmalı.

- [ ] **Step 4: Commit**

```bash
git add third_party/no-telemetry-patches/
git add crates/codegen/
git commit -m "feat: apply no-telemetry patches (product analytics, mixpanel, sentry, otlp, trace upload, feedback)"
```

---

### Task 2: CI kapılarını güncelle (I2a muafiyet + I8 no-telemetry gate)

**Files:**
- Modify: `bin/ci-gates.sh`

- [ ] **Step 1: I2a diff taramasına muafiyet listesi ekle**

`bin/ci-gates.sh` dosyasında I2a bloğunu bul (satır ~90-111). Diff hesaplama satırının hemen öncesine muafiyet filter'ı ekle:

Değişiklik:
```bash
  if ! vendor_diff=$(git diff --name-only "$VENDOR_BASE" -- crates/common crates/codegen); then
```
→
```bash
  if ! vendor_diff=$(git diff --name-only "$VENDOR_BASE" -- crates/common crates/codegen \
    | grep -vE '^(crates/codegen/xai-grok-telemetry/|crates/codegen/xai-mixpanel/|crates/codegen/xai-grok-shell/src/(agent/config|auth/credential_provider|extensions/feedback)\.rs)$'); then
```

Bu, patch'lenmiş telemetri dosyalarını I2a diff'inden çıkarır. Diğer xai-* dosyaları hala denetlenir.

- [ ] **Step 2: I8 no-telemetry faz kapısını ekle**

`bin/ci-gates.sh` sonunda, I6 rapor bloğundan sonra (satır ~264'ten önce) I8 bloğunu ekle:

```bash
# --- I8: no-telemetry gate — telemetri fonksiyonları kapalı mı? -----------
# I8 = I2a muafiyetinin tamamlayıcısı: anlamsal denetim.
# Telemetriyi tekrar açabilecek bir değişiklik yapıldıysa kırmızı.
i8_fail=0
i8_pass() { :; }
i8_fail() { i8_fail=1; printf '%s[FAIL]%s %s\n' "$RED" "$RESET" "$1"; }

# 6 anahtar fonksiyonun üretim yolundaki dönüş değerini kontrol et
# Sadece src/ taranır (test blokları hariç)

# 1. Config::is_telemetry_enabled() -> false
if grep -rnE 'is_telemetry_enabled.*->.*true' crates/codegen/xai-grok-telemetry/src/ \
   crates/codegen/xai-grok-shell/src/agent/config.rs 2>/dev/null \
   | grep -v '/tests/\|#\[cfg(test)\]'; then
  i8_fail "I8: is_telemetry_enabled true dönüyor (telemetri açılabilir)"
fi

# 2. is_session_metrics_enabled() -> false
if grep -rnE 'is_session_metrics_enabled.*->.*true' crates/codegen/xai-grok-telemetry/src/ \
   2>/dev/null | grep -v '/tests/\|#\[cfg(test)\]'; then
  i8_fail "I8: is_session_metrics_enabled true dönüyor"
fi

# 3. Config::is_trace_upload_enabled() -> false
if grep -rnE 'is_trace_upload_enabled.*->.*true' crates/codegen/xai-grok-shell/src/agent/config.rs \
   2>/dev/null | grep -v '/tests/\|#\[cfg(test)\]'; then
  i8_fail "I8: is_trace_upload_enabled true dönüyor"
fi

# 4. Config::is_feedback_enabled() -> false
if grep -rnE 'is_feedback_enabled.*->.*true' crates/codegen/xai-grok-shell/src/agent/config.rs \
   2>/dev/null | grep -v '/tests/\|#\[cfg(test)\]'; then
  i8_fail "I8: is_feedback_enabled true dönüyor"
fi

# 5. is_error_reporting_disabled_sync() -> true (disabled = true, so it's good if false)
if grep -rnE 'is_error_reporting_disabled_sync.*->.*false' crates/codegen/xai-grok-shell/src/agent/config.rs \
   2>/dev/null | grep -v '/tests/\|#\[cfg(test)\]'; then
  i8_fail "I8: is_error_reporting_disabled_sync false dönüyor (error reporting açılabilir)"
fi

# 6. is_telemetry_explicitly_disabled_sync() -> true (disabled = true)
if grep -rnE 'is_telemetry_explicitly_disabled_sync.*->.*false' crates/codegen/xai-grok-shell/src/agent/config.rs \
   2>/dev/null | grep -v '/tests/\|#\[cfg(test)\]'; then
  i8_fail "I8: is_telemetry_explicitly_disabled_sync false dönüyor (OTLP açılabilir)"
fi

if [[ $i8_fail -eq 0 ]]; then
  pass "I8: tüm telemetri fonksiyonları kapalı (no-telemetry gate yeşil)"
fi
```

- [ ] **Step 3: FAILED değişkenine I8 sonucunu yansıt**

`bin/ci-gates.sh` sonunda, tüm kapı denetimlerinden sonra (`FAILED=0` başlangıcı zaten var), I8 sonucu da `$FAILED`'e yansıtılmalı. Ekle:

```bash
# (Mevcut I1/I2a/I2b/I4/I5/I7/IP bloklarından sonra, I8'den hemen sonra)
if [[ $i8_fail -ne 0 ]]; then
  FAILED=1
fi
```

- [ ] **Step 4: Commit**

```bash
git add bin/ci-gates.sh
git commit -m "feat(ci): add I2a telemetry-path muafiyet + I8 no-telemetry gate"
```

---

### Task 3: Build + test doğrulaması

**Files:** (none)

- [ ] **Step 1: cargo check**

```bash
cargo check --workspace --all-targets 2>&1
```
Expected: success (exit 0)

- [ ] **Step 2: cargo test**

```bash
cargo test --workspace 2>&1
```
Expected: all tests passing

- [ ] **Step 3: omnitrix --version**

```bash
cargo run -p omnitrix -- --version
```
Expected: version string

- [ ] **Step 4: ci-gates**

```bash
bash bin/ci-gates.sh 2>&1
```
Expected: I2a yeşil, I8 yeşil, tüm invariantlar geçiyor

- [ ] **Step 5: NO_TELEMETRY audit — tüm değişiklikler dosya bazında doğrulandı mı?**

```bash
# track() boş gövde mi?
grep -A5 'pub async fn track(' crates/codegen/xai-grok-telemetry/src/client.rs | head -10
# track() boş gövde mi? (xai-mixpanel)
grep -A5 'pub async fn track(' crates/codegen/xai-mixpanel/src/lib.rs | head -10
# Sentry init no-op mu?
grep -A10 'pub fn init(config: Config)' crates/codegen/xai-grok-telemetry/src/sentry.rs | head -15
# is_telemetry_enabled false mu?
grep -A3 'fn is_telemetry_enabled' crates/codegen/xai-grok-shell/src/agent/config.rs | head -10
```

Tümü beklenen şekilde boş/no-op olmalı.

---

### Task 4: SOURCE_REV güncelle

**Files:**
- Modify: `SOURCE_REV`

- [ ] **Step 1: Mevcut HEAD SHA'sını SOURCE_REV'e yaz**

```bash
git rev-parse HEAD > SOURCE_REV
```

Bu, vendored xai-* ağacının patch'lenmiş halini temsil eden commit SHA'sını kaydeder.

- [ ] **Step 2: Commit**

```bash
git add SOURCE_REV
git commit -m "chore: update SOURCE_REV after no-telemetry patches"
```

MERHABA: Uzaktaki x-org/grok-build main'den farklıyız — artık telemetrisiz kendi branchedüzimizdeyiz, SOURCE_REV bunu yansıtır.
