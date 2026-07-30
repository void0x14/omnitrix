#!/usr/bin/env bash
# MASTER-PLAN invariant taramalari — her faz kapisinda calisir.
#
#   I1  her faz kapisi = komut + metrik + esik (kapi test dosyalari mevcut)
#   I2  omni-* -> xai-* tek yon; xai-* duzenlenmez
#   I4  sandbox yok
#   I5  model ismi/fiyati gomulu degil
#   I6  uretim yolunda unwrap/expect/panic! = 0
#   I7  her yan etkili islem once WAL niyet kaydi (write_journal semasi)
#   IP  Path::canonicalize yasak; dunce::canonicalize kullanilir
#
# Ihlal = kirmizi kapi. Cikis kodu 0 = tum kapilar yesil.
#
# Derleme kapilarini da calistirmak icin: bin/ci-gates.sh --build
# (cargo check + clippy; birkac dakika surer, o yuzden varsayilan degil)
#
# CI'nin clippy paket listesini uretmek icin: bin/ci-gates.sh --list-crates
# (tek kaynak burasi; .github/workflows/ci.yml bu ciktiyi tuketir)
set -uo pipefail

cd "$(dirname "$0")/.."

RUN_BUILD=0
LIST_CRATES=0
for arg in "$@"; do
  case "$arg" in
    --build)       RUN_BUILD=1 ;;
    --list-crates) LIST_CRATES=1 ;;
    -h|--help)
      sed -n '2,18p' "$0"
      exit 0
      ;;
    *)
      printf 'bilinmeyen arguman: %s (--build | --list-crates | --help)\n' "$arg" >&2
      exit 2
      ;;
  esac
done

# Yazilan crate'ler. clippy/check bunlarla sinirlidir: crates/common ve
# crates/codegen vendored'dir (I2), oradaki uyarilar duzeltilemez, dolayisiyla
# -D warnings deps'e yayilamaz. --no-deps tam olarak bunu saglar.
#
# Liste crates/omni/*/Cargo.toml'dan turetilir: elle tutulan bir kopya yeni faz
# crate'leri eklendiginde sessizce eskiyor ve clippy kapisi onlari atliyordu.
# Paket adi manifestten okunur (dizin adi != paket adi olabilir).
OMNI_CRATES=()
while read -r pkg; do
  [[ -n "$pkg" ]] && OMNI_CRATES+=("$pkg")
done < <(
  for m in crates/omni/*/Cargo.toml; do
    [[ -f "$m" ]] || continue
    # Yalniz [package] bolumundeki name; [[bin]]/[[test]] hedef adlari degil.
    awk '
      /^\[package\]/ { in_pkg = 1; next }
      /^\[/          { in_pkg = 0 }
      in_pkg && match($0, /^name *= *"[^"]*"/) {
        line = $0
        sub(/^name *= *"/, "", line)
        sub(/".*$/, "", line)
        print line
        exit
      }
    ' "$m"
  done | sort
)

if [[ ${#OMNI_CRATES[@]} -eq 0 ]]; then
  printf 'ci-gates: crates/omni altinda paket bulunamadi\n' >&2
  exit 2
fi

if [[ $LIST_CRATES -eq 1 ]]; then
  printf '%s\n' "${OMNI_CRATES[@]}"
  exit 0
fi

RED=$'\033[31m'; GREEN=$'\033[32m'; RESET=$'\033[0m'
FAILED=0

fail() { printf '%s[FAIL]%s %s\n' "$RED" "$RESET" "$1"; FAILED=1; }
pass() { printf '%s[ OK ]%s %s\n' "$GREEN" "$RESET" "$1"; }

# --- I2a: vendored agac (crates/common + crates/codegen) degistirilmedi -----
# Referans: upstream/main. MASTER-PLAN 1.1'e gore xai-* agaci monorepo'dan
# senkronlanan vendored koddur; omnitrix onu tuketir, fork etmez.
# Yedek referans son senkron commit'i; --grep SADECE konu satirina bakar,
# yoksa "Synced from monorepo" ifadesini govdesinde gecen kendi commit'lerimiz
# baz olarak secilir ve kapi sahte yesile doner.
default_vendor_base() {
  git rev-parse --verify --quiet upstream/main && return
  git log --format='%H %s' -n50 \
    | awk '$0 ~ / Synced from monorepo$/ { print $1; exit }'
}
VENDOR_BASE="${OMNI_VENDOR_BASE:-$(default_vendor_base)}"
if [[ -z "$VENDOR_BASE" ]]; then
  fail "I2a: vendored baz commit bulunamadi (OMNI_VENDOR_BASE ver)"
elif ! git rev-parse --verify --quiet "${VENDOR_BASE}^{commit}" >/dev/null; then
  # Cozulemeyen baz (yanlis OMNI_VENDOR_BASE ya da sig kopya): 'git diff' bos
  # cikti + hata verir, kapi sahte yesile donerdi. Acikca kirmizi olsun.
  fail "I2a: baz commit cozulemedi: $VENDOR_BASE (fetch-depth 0 ve gecerli SHA gerekir)"
else
  if ! vendor_diff=$(git diff --name-only "$VENDOR_BASE" -- crates/common crates/codegen \
    | grep -vE '^(crates/codegen/xai-grok-telemetry/|crates/codegen/xai-mixpanel/|crates/codegen/xai-grok-shell/src/(agent/config|auth/credential_provider|extensions/feedback)\.rs)'); then
    fail "I2a: vendored diff hesaplanamadi ($VENDOR_BASE)"
  elif [[ -n "$vendor_diff" ]]; then
    fail "I2a: xai-* vendored agac duzenlenmis ($VENDOR_BASE'e gore):"
    printf '        %s\n' $vendor_diff
  else
    pass "I2a: crates/common + crates/codegen diff = 0"
  fi
fi

# --- I2b: deklare edilen her xai-* bagimliligi fiilen kullaniliyor ----------
# B2'nin "17 deklare, 3 kullanim" tekrarini mekanik olarak imkansizlastirir.
#
# Tarama koku yalnizca src/ degildir: dev-dependency'ler crate-yerel
# tests|benches|examples icinde, omni-tests'in bagimliliklari ise manifestte
# `path = "../../../tests/*.rs"` ile baglanan kok tests/ dosyalarinda kullanilir.
# Sadece src/'ye bakmak bunlari "kullanilmiyor" diye yanlis kirmiziya cevirdi.
crate_scan_roots() {
  local dir="$1" manifest="$2" p
  for p in src build.rs tests benches examples; do
    [[ -e "$dir/$p" ]] && printf '%s\n' "$dir/$p"
  done
  # Manifestte bildirilen hedefler ([[bin]]/[[test]]/[[bench]]/[[example]]).
  # Satir basindaki `path = "..."` sadece hedef tablolarinda gecer; inline
  # bagimlilik tablolari (`foo = { path = ".." }`) bu kalibi eslemez.
  while read -r p; do
    [[ -n "$p" && -e "$dir/$p" ]] && printf '%s\n' "$dir/$p"
  done < <(sed -n 's/^ *path *= *"\([^"]*\)".*/\1/p' "$manifest")
}

unused_total=0
for manifest in crates/omni/*/Cargo.toml; do
  crate_dir=$(dirname "$manifest")
  crate_name=$(basename "$crate_dir")

  roots=()
  while read -r root; do
    [[ -n "$root" ]] && roots+=("$root")
  done < <(crate_scan_roots "$crate_dir" "$manifest")
  # Kaynagi olmayan crate: her deklarasyon tanimi geregi kullanilmiyor demektir.
  [[ ${#roots[@]} -eq 0 ]] && roots=(/dev/null)

  while read -r dep; do
    [[ -z "$dep" ]] && continue
    ident="${dep//-/_}"
    if ! grep -rqE "\b${ident}::" "${roots[@]}" 2>/dev/null \
       && ! grep -rqE "\buse +${ident}\b" "${roots[@]}" 2>/dev/null; then
      fail "I2b: $crate_name deklare ediyor ama kullanmiyor: $dep"
      unused_total=$((unused_total + 1))
    fi
  done < <(sed -n 's/^\(xai-[a-z0-9-]*\) *=.*/\1/p' "$manifest")
done
[[ $unused_total -eq 0 ]] && pass "I2b: kullanilmayan xai-* bagimliligi = 0"

# --- I4: sandbox yok --------------------------------------------------------
# K5: yeni kodda seccomp/bwrap/landlock/sandbox referansi yasak.
# tests/ + benches/ de tarama kapsamindadir (Faz 3-10 kapi testleri oraya yazilir);
# benches/results uretilen olcum JSON'udur, kaynak degildir.
sandbox_hits=$(grep -rniE --exclude-dir=results --exclude-dir=target \
  'sandbox|seccomp|bwrap|landlock' \
  crates/omni migrations config prompts tests benches 2>/dev/null || true)
if [[ -n "$sandbox_hits" ]]; then
  fail "I4: sandbox referansi bulundu:"
  printf '        %s\n' "$sandbox_hits"
else
  pass "I4: sandbox/seccomp/bwrap/landlock referansi = 0"
fi

# --- I5: model ismi/fiyati gomulu degil ------------------------------------
# AS7: rol -> model cozumu calisma zamaninda katalogdan yapilir.
# config/models.toml katalog override'inin ta kendisidir, taramadan muaf.
model_hits=$(grep -rniE \
  '"(gpt|o[134]|claude|sonnet|opus|haiku|grok|deepseek|gemini|llama|mistral|qwen|kimi|glm)[-_.][0-9a-z][0-9a-z._-]*"' \
  crates/omni 2>/dev/null || true)
if [[ -n "$model_hits" ]]; then
  fail "I5: gomulu model ismi bulundu:"
  printf '        %s\n' "$model_hits"
else
  pass "I5: literal model-adi = 0"
fi

# --- I7: her yan etkili islem once WAL niyet kaydi --------------------------
# 3.2 / Bolum 14: zorlama noktasi `write_journal` semasidir. Sema migrations'tan
# kalkarsa crash-only yol sessizce niyetsiz yazmaya doner; kapi kirmizi olur.
if grep -rqiE 'create +table[^;]*write_journal' migrations 2>/dev/null; then
  pass "I7: write_journal semasi migrations'ta tanimli"
else
  fail "I7: write_journal semasi migrations'ta bulunamadi (WAL niyet kaydi kapisi)"
fi

# --- IP: Path::canonicalize yasak, dunce::canonicalize kullanilir -----------
# clippy.toml `disallowed-methods` ile bunu zaten yasaklar; burada ayrica
# taranir cunki clippy yalnizca derlenen hedefleri gorur, kapi ise tum agaci.
# Yorum satirlarindaki "Path::canonicalize" gecisleri cagri degildir: kalip
# sadece cagri parantezini eslesin diye `canonicalize(` uzerinden kurulur.
canon_hits=$(grep -rnE --include='*.rs' 'canonicalize\(' \
  crates/omni tests 2>/dev/null | grep -v 'dunce::canonicalize(' || true)
if [[ -n "$canon_hits" ]]; then
  fail "IP: dunce disi canonicalize cagrisi bulundu:"
  printf '        %s\n' "$canon_hits"
else
  pass "IP: dunce disi canonicalize cagrisi = 0"
fi

# --- I1: faz kapisi test dosyalari mevcut ----------------------------------
# Bolum 4 klasor yapisi + Bolum 20 kapi tablosu: her faz kapisinin bir kosulur
# karsiligi olmali. Dosyanin VARLIGI kapidir; kok tests/ sanal workspace'te
# hicbir pakete ait olmadigi icin ayrica bir crate'in [[test]] hedefi olarak
# BAGLI olmasi gerekir — bagli degilse hic kosmaz, bu yuzden raporlanir.
declare -A GATE_TESTS=(
  [crash_recovery]="3.2 crash-only (Faz 1)"
  [diff_visibility]="5.2 diff gorunurlugu (Faz 1)"
  [ui_parity]="K7 iki yuz esitligi (Faz 2)"
  [multiagent_fanout]="K1 fan-out (Faz 3)"
  [provider_fallback]="6.5 saglayici fallback (Faz 4)"
  [grounding_redteam]="3.4 grounding (Faz 4)"
  [tool_allowlist_redteam]="K3 tool broker (Faz 5)"
)
missing_gate_tests=0
unbound_gate_tests=()
for t in $(printf '%s\n' "${!GATE_TESTS[@]}" | sort); do
  if [[ ! -f "tests/$t.rs" ]]; then
    fail "I1: kapi testi yok: tests/$t.rs — ${GATE_TESTS[$t]}"
    missing_gate_tests=$((missing_gate_tests + 1))
  elif ! grep -rqF "tests/$t.rs" crates/omni/*/Cargo.toml 2>/dev/null; then
    unbound_gate_tests+=("$t")
  fi
done
[[ $missing_gate_tests -eq 0 ]] && pass "I1: kapi testi dosyalari mevcut (${#GATE_TESTS[@]}/${#GATE_TESTS[@]})"
if [[ ${#unbound_gate_tests[@]} -gt 0 ]]; then
  printf 'I1 (rapor): hicbir crate'"'"'in [[test]] hedefi degil, kosmuyor: %s\n' \
    "${unbound_gate_tests[*]}"
fi

# --- I6: uretim yolunda unwrap/expect/panic! (RAPOR, kapi degil) ------------
# I6'nin zorlama noktasi Bolum 4'e gore 'clippy -D warnings'tir; clippy bunlari
# varsayilan olarak isaretlemez. Crate'ler YENIDEN YAZ fazlarina girdikce
# #![deny(clippy::unwrap_used)] ile crate basina zorlanacak. Buradaki sayim
# o borcun ne kadar eridigini gosterir; kapiyi kirmizi yapmaz.
#
# Sayim kaba ama dosya-basina dogrudur: `in_test` her yeni dosyada sifirlanir
# (FNR==1), aksi halde ilk `#[cfg(test)]`den sonraki TUM dosyalar sayilmiyordu.
# Alt moduller icin src/ agaci ozyinelemeli taranir.
i6_count=0
for crate in "${OMNI_CRATES[@]}"; do
  # omni-tests bastan sona test kosum takimidir (publish=false, kok tests/
  # hedeflerini barindirir); I6 uretim yolu icindir, testlerde serbest.
  [[ "$crate" == "omni-tests" ]] && continue
  dir="crates/omni/$crate/src"
  [[ -d "$dir" ]] || continue
  # -exec ... + toplu cagirir; her cagri bir sayi basar, ikinci awk toplar.
  n=$(find "$dir" -name '*.rs' -type f -exec awk '
    FNR == 1 { in_test = 0 }
    /^ *#\[cfg\(test\)\]/ { in_test = 1 }
    !in_test && /\.unwrap\(\)|\.expect\(|panic!\(/ { c++ }
    END { print c + 0 }
  ' {} + 2>/dev/null | awk '{ s += $1 } END { print s + 0 }')
  [[ -z "$n" ]] && n=0
  [[ "$n" -gt 0 ]] && printf '        %-16s %s\n' "$crate" "$n"
  i6_count=$((i6_count + n))
done
printf 'I6 (rapor): uretim yolunda unwrap/expect/panic! toplam = %s\n' "$i6_count"

# --- I8: no-telemetry gate — telemetri fonksiyonlari kapali mi? -----------
i8_fail=0

# 1. Config::is_telemetry_enabled() -> false
if grep -rnE 'is_telemetry_enabled.*->.*true' \
  crates/codegen/xai-grok-telemetry/src/ \
  crates/codegen/xai-grok-shell/src/agent/config.rs 2>/dev/null \
  | grep -v '/tests/\|#\[cfg(test)\]' | grep -q .; then
  fail "I8: is_telemetry_enabled true donuyor (telemetri acilabilir)"
  i8_fail=1
fi

# 2. is_session_metrics_enabled() -> false (xai-grok-telemetry/src/)
if grep -rnE 'is_session_metrics_enabled.*->.*true' \
  crates/codegen/xai-grok-telemetry/src/ 2>/dev/null \
  | grep -v '/tests/\|#\[cfg(test)\]' | grep -q .; then
  fail "I8: is_session_metrics_enabled true donuyor"
  i8_fail=1
fi

# 3. Config::is_trace_upload_enabled() -> false
if grep -rnE 'is_trace_upload_enabled.*->.*true' \
  crates/codegen/xai-grok-shell/src/agent/config.rs 2>/dev/null \
  | grep -v '/tests/\|#\[cfg(test)\]' | grep -q .; then
  fail "I8: is_trace_upload_enabled true donuyor"
  i8_fail=1
fi

# 4. Config::is_feedback_enabled() -> false
if grep -rnE 'is_feedback_enabled.*->.*true' \
  crates/codegen/xai-grok-shell/src/agent/config.rs 2>/dev/null \
  | grep -v '/tests/\|#\[cfg(test)\]' | grep -q .; then
  fail "I8: is_feedback_enabled true donuyor"
  i8_fail=1
fi

# 5. is_error_reporting_disabled_sync() -> true (disabled = true, false is bad)
if grep -rnE 'is_error_reporting_disabled_sync.*->.*false' \
  crates/codegen/xai-grok-shell/src/agent/config.rs 2>/dev/null \
  | grep -v '/tests/\|#\[cfg(test)\]' | grep -q .; then
  fail "I8: is_error_reporting_disabled_sync false donuyor (error reporting acilabilir)"
  i8_fail=1
fi

# 6. is_telemetry_explicitly_disabled_sync() -> true (disabled = true)
if grep -rnE 'is_telemetry_explicitly_disabled_sync.*->.*false' \
  crates/codegen/xai-grok-shell/src/agent/config.rs 2>/dev/null \
  | grep -v '/tests/\|#\[cfg(test)\]' | grep -q .; then
  fail "I8: is_telemetry_explicitly_disabled_sync false donuyor (OTLP acilabilir)"
  i8_fail=1
fi

if [[ $i8_fail -eq 0 ]]; then
  pass "I8: tum telemetri fonksiyonlari kapali (no-telemetry gate yesil)"
fi

if [[ $i8_fail -ne 0 ]]; then
  FAILED=1
fi

# --- Derleme kapilari (opsiyonel, --build) ---------------------------------
if [[ $RUN_BUILD -eq 1 ]]; then
  pkg_args=()
  for crate in "${OMNI_CRATES[@]}"; do pkg_args+=(-p "$crate"); done

  if cargo check --workspace --all-targets >/dev/null 2>&1; then
    pass "derleme: cargo check --workspace --all-targets yesil"
  else
    fail "derleme: cargo check --workspace --all-targets kirmizi"
  fi

  if cargo clippy "${pkg_args[@]}" --all-targets --no-deps -- -D warnings >/dev/null 2>&1; then
    pass "lint: clippy -D warnings temiz (omni-* crate'leri)"
  else
    fail "lint: clippy -D warnings kirmizi (omni-* crate'leri)"
  fi

  if cargo run -q -p omnitrix -- --version >/dev/null 2>&1; then
    pass "bin: omnitrix --version calisiyor"
  else
    fail "bin: omnitrix --version calismiyor"
  fi
fi

exit $FAILED
