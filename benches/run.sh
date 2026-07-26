#!/usr/bin/env bash
# benches/run.sh — Faz kapisi olcum kosucusu (MASTER-PLAN Bolum 4).
#
# Uc kapiyi olcer ve esige vurur:
#   cold-start (8.2)  exec -> ilk TUI frame   < 100ms sicak / < 400ms soguk
#   shutdown   (8.1)  SIGINT -> exit          < 50ms (bos + yuk altinda)
#   RSS               yerlesik bellek         < esik (kB)
#
# Cikis kodu 0 = tum kapilar yesil, 1 = en az bir esik asildi, 2 = olcum hatasi.
#
# ONEMLI: olcum DAIMA release profiliyle yapilir. Debug ikilisi cold-start
# esigini anlamsizlastirir (sembol + optimizasyon farki).
#
# Kullanim:
#   benches/run.sh                 # release build + olcum + benches/results/latest.json
#   benches/run.sh --runs 50       # ek bayraklar dogrudan omni-bench'e gecer
#   OMNI_BENCH_PROFILE=release-dist benches/run.sh
#   sudo -E benches/run.sh --drop-caches   # gercek soguk-onbellek serisi

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${OMNI_BENCH_PROFILE:-release}"
OUT_DIR="${OMNI_BENCH_OUT:-$ROOT/benches/results}"
THRESHOLDS="${OMNI_BENCH_THRESHOLDS:-$ROOT/benches/thresholds.json}"

mkdir -p "$OUT_DIR"

echo "[omni-bench] profil=$PROFILE"
cargo build --profile "$PROFILE" -p omnitrix -p omni-bench --manifest-path "$ROOT/Cargo.toml"

# cargo 'dev'/'release' profillerini target/debug ve target/release altina koyar;
# ozel profiller kendi adlariyla gelir.
case "$PROFILE" in
  dev)     BIN_DIR="$ROOT/target/debug" ;;
  release) BIN_DIR="$ROOT/target/release" ;;
  *)       BIN_DIR="$ROOT/target/$PROFILE" ;;
esac

OMNITRIX_BIN="$BIN_DIR/omnitrix"
BENCH_BIN="$BIN_DIR/omni-bench"

for f in "$OMNITRIX_BIN" "$BENCH_BIN"; do
  if [[ ! -x "$f" ]]; then
    echo "[omni-bench] ikili yok: $f" >&2
    exit 2
  fi
done

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
JSON_OUT="$OUT_DIR/$STAMP.json"

set +e
"$BENCH_BIN" \
  --bin "$OMNITRIX_BIN" \
  --config-dir "$ROOT/config" \
  --thresholds "$THRESHOLDS" \
  --json "$JSON_OUT" \
  "$@"
STATUS=$?
set -e

ln -sf "$(basename "$JSON_OUT")" "$OUT_DIR/latest.json"
echo "[omni-bench] JSON: $JSON_OUT (-> $OUT_DIR/latest.json)"
exit "$STATUS"
