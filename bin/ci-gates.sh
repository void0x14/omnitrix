#!/usr/bin/env bash
# MASTER-PLAN invariant taramalari — her faz kapisinda calisir.
#
#   I2  omni-* -> xai-* tek yon; xai-* duzenlenmez
#   I4  sandbox yok
#   I5  model ismi/fiyati gomulu degil
#
# Ihlal = kirmizi kapi. Cikis kodu 0 = tum kapilar yesil.
set -uo pipefail

cd "$(dirname "$0")/.."

RED=$'\033[31m'; GREEN=$'\033[32m'; RESET=$'\033[0m'
FAILED=0

fail() { printf '%s[FAIL]%s %s\n' "$RED" "$RESET" "$1"; FAILED=1; }
pass() { printf '%s[ OK ]%s %s\n' "$GREEN" "$RESET" "$1"; }

# --- I2a: vendored agac (crates/common + crates/codegen) degistirilmedi -----
# Referans: monorepo'dan son senkron commit'i (SOURCE_REV ile ayni nokta).
VENDOR_BASE="${OMNI_VENDOR_BASE:-$(git log --format=%H -n1 --grep='Synced from monorepo')}"
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

exit $FAILED
