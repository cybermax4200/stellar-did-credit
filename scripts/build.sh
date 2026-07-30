#!/bin/bash
# Build all Soroban contracts for wasm32-unknown-unknown (release) and report
# binary sizes.  Prints bytes + KB for each contract and warns when a binary
# approaches or exceeds the configurable threshold.
#
# Environment variables:
#   WASM_SIZE_WARN_BYTES  – threshold in bytes that triggers a WARNING line.
#                           Defaults to 512 KB (524288 bytes), which is the
#                           current Stellar protocol deployment limit.
#                           Override to 0 to disable warnings entirely.
set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
# Stellar's WASM deployment limit as of Protocol 21 is 512 KB.
# Adjust this variable if the limit changes in a future protocol version.
WASM_SIZE_WARN_BYTES=${WASM_SIZE_WARN_BYTES:-$((512 * 1024))}

WASM_DIR="target/wasm32-unknown-unknown/release"

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
echo "Building contracts (wasm32-unknown-unknown, release)..."
cargo build --target wasm32-unknown-unknown --release --workspace
echo ""

# ---------------------------------------------------------------------------
# Size report
# ---------------------------------------------------------------------------
echo "WASM binary sizes"
echo "-----------------"
printf "%-40s %12s %10s %s\n" "Contract" "Bytes" "KB" "Status"
printf "%-40s %12s %10s %s\n" "--------" "-----" "--" "------"

WARNED=0

for wasm in "$WASM_DIR"/*.wasm; do
  [ -f "$wasm" ] || continue

  NAME=$(basename "$wasm")

  # stat is available on both Linux (GNU) and macOS (BSD) with different flags;
  # fall back to wc -c which works everywhere.
  if stat --version >/dev/null 2>&1; then
    # GNU stat (Linux / CI)
    SIZE=$(stat -c%s "$wasm")
  else
    # BSD stat (macOS)
    SIZE=$(stat -f%z "$wasm")
  fi

  KB=$(awk "BEGIN { printf \"%.2f\", $SIZE / 1024 }")

  if [ "$WASM_SIZE_WARN_BYTES" -gt 0 ] && [ "$SIZE" -gt "$WASM_SIZE_WARN_BYTES" ]; then
    STATUS="⚠ WARNING: exceeds ${WASM_SIZE_WARN_BYTES}-byte threshold"
    WARNED=1
  else
    STATUS="OK"
  fi

  printf "%-40s %12d %10s %s\n" "$NAME" "$SIZE" "${KB} KB" "$STATUS"
done

echo ""

if [ "$WARNED" -eq 1 ]; then
  echo "WARNING: One or more contracts exceed the configured size threshold" \
       "(WASM_SIZE_WARN_BYTES=${WASM_SIZE_WARN_BYTES})."
  echo "Stellar deployment limit reference: https://developers.stellar.org/docs/soroban/deployment"
else
  echo "All contracts are within the size threshold."
fi
