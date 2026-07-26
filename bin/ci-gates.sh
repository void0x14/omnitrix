#!/usr/bin/env bash
# MASTER-PLAN invariant taramalari — her faz kapisinda calisir.
#
#   I2  omni-* -> xai-* tek yon; xai-* duzenlenmez
#   I4  sandbox yok
#   I5  model ismi/fiyati gomulu degil
#   I6  uretim yolunda unwrap/expect/panic! = 0
#
# Ihlal = kirmizi kapi. Cikis kodu 0 = tum kapilar yesil.
#
# Derleme kapilarini da calistirmak icin: bin/ci-gates.sh --build
# (cargo check + clippy; birkac dakika surer, o yuzden varsayilan degil)
set -uo pipefail

cd "$(dirname "$0")/.."

RUN_BUILD=0
[[ "${1:-}" == "--build" ]] && RUN_BUILD=1

# Yazilan crate'ler. clippy/check bunlarla sinirlidir: crates/common ve
# crates/codegen vendored'dir (I2), oradaki uyarilar duzeltilemez, dolayisiyla
# -D warnings deps'e yayilamaz. --no-deps tam olarak bunu saglar.
OMNI_CRATES=(
  omnitrix omni-tui omni-webui omni-scheduler omni-storage
  omni-provider omni-router omni-notify omni-backup omni-record
)

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
else
  vendor_diff=$(git diff --name-only "$VENDOR_BASE" -- crates/common crates/codegen)
  if [[ -n "$vendor_diff" ]]; then
    fail "I2a: xai-* vendored agac duzenlenmis ($VENDOR_BASE'e gore):"
    printf '        %s\n' $vendor_diff
  else
    pass "I2a: crates/common + crates/codegen diff = 0"
  fi
fi

# --- I2b: deklare edilen her xai-* bagimliligi fiilen kullaniliyor ----------
# B2'nin "17 deklare, 3 kullanim" tekrarini mekanik olarak imkansizlastirir.
unused_total=0
for manifest in crates/omni/*/Cargo.toml; do
  crate_dir=$(dirname "$manifest")
  crate_name=$(basename "$crate_dir")
  while read -r dep; do
    [[ -z "$dep" ]] && continue
    ident="${dep//-/_}"
    if ! grep -rqE "\b${ident}::" "$crate_dir/src" 2>/dev/null \
       && ! grep -rqE "\buse +${ident}\b" "$crate_dir/src" 2>/dev/null; then
      fail "I2b: $crate_name deklare ediyor ama kullanmiyor: $dep"
      unused_total=$((unused_total + 1))
    fi
  done < <(sed -n 's/^\(xai-[a-z0-9-]*\) *=.*/\1/p' "$manifest")
done
[[ $unused_total -eq 0 ]] && pass "I2b: kullanilmayan xai-* bagimliligi = 0"

# --- I4: sandbox yok --------------------------------------------------------
# K5: yeni kodda seccomp/bwrap/landlock/sandbox referansi yasak.
sandbox_hits=$(grep -rniE 'sandbox|seccomp|bwrap|landlock' \
  crates/omni migrations config prompts 2>/dev/null || true)
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

# --- I6: uretim yolunda unwrap/expect/panic! (RAPOR, kapi degil) ------------
# I6'nin zorlama noktasi Bolum 4'e gore 'clippy -D warnings'tir; clippy bunlari
# varsayilan olarak isaretlemez. Crate'ler YENIDEN YAZ fazlarina girdikce
# #![deny(clippy::unwrap_used)] ile crate basina zorlanacak. Buradaki sayim
# o borcun ne kadar eridigini gosterir; kapiyi kirmizi yapmaz.
i6_count=0
for crate in "${OMNI_CRATES[@]}"; do
  dir="crates/omni/$crate/src"
  [[ -d "$dir" ]] || continue
  n=$(awk '
    /^ *#\[cfg\(test\)\]/ { in_test = 1 }
    !in_test && /\.unwrap\(\)|\.expect\(|panic!\(/ { c++ }
    END { print c + 0 }
  ' "$dir"/*.rs 2>/dev/null || echo 0)
  [[ "$n" -gt 0 ]] && printf '        %-16s %s\n' "$crate" "$n"
  i6_count=$((i6_count + n))
done
printf 'I6 (rapor): uretim yolunda unwrap/expect/panic! toplam = %s\n' "$i6_count"

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
