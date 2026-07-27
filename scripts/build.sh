#!/bin/bash
set -euo pipefail

THRESHOLD_KB=${WASM_SIZE_LIMIT_KB:-100}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --threshold)
      if [[ $# -lt 2 ]]; then
        echo "Error: --threshold requires a value (KB)" >&2
        exit 1
      fi
      THRESHOLD_KB="$2"
      shift 2
      ;;
    --help|-h)
      echo "Usage: $0 [--threshold <KB>]" >&2
      echo "" >&2
      echo "Builds all Soroban contracts and reports WASM binary sizes." >&2
      echo "" >&2
      echo "Options:" >&2
      echo "  --threshold  Warning threshold in KB (default: 100, or WASM_SIZE_LIMIT_KB env var)" >&2
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      echo "Usage: $0 [--threshold <KB>]" >&2
      exit 1
      ;;
  esac
done

THRESHOLD_BYTES=$((THRESHOLD_KB * 1024))
TARGET_DIR="target/wasm32-unknown-unknown/release"

echo "Building contracts (release, wasm32-unknown-unknown)..."
cargo build --release --target wasm32-unknown-unknown

echo ""
echo "================================================================="
echo "  WASM Binary Size Report"
echo "================================================================="

HAS_WARNING=false
ANY_FOUND=false

if [ ! -d "$TARGET_DIR" ]; then
  echo "Error: build output directory not found: $TARGET_DIR" >&2
  exit 1
fi

for wasm in "$TARGET_DIR"/*.wasm; do
  [ -f "$wasm" ] || continue
  ANY_FOUND=true

  filename=$(basename "$wasm")

  if [[ "$(uname)" == "Darwin" ]]; then
    bytes=$(stat -f%z "$wasm" 2>/dev/null)
  else
    bytes=$(stat -c%s "$wasm" 2>/dev/null)
  fi

  if [[ -z "$bytes" ]]; then
    bytes=$(wc -c < "$wasm" | tr -d ' ')
  fi

  kb=$(awk "BEGIN {printf \"%.2f\", $bytes / 1024}")

  if [ "$bytes" -gt "$THRESHOLD_BYTES" ]; then
    printf "  WARNING  %-35s %10d bytes  %8s KB  (exceeds %d KB threshold)\n" "$filename" "$bytes" "$kb" "$THRESHOLD_KB"
    HAS_WARNING=true
  else
    printf "  OK       %-35s %10d bytes  %8s KB\n" "$filename" "$bytes" "$kb"
  fi
done

if [ "$ANY_FOUND" = false ]; then
  echo "  No WASM files found in $TARGET_DIR"
fi

echo "-----------------------------------------------------------------"
echo "Threshold: ${THRESHOLD_KB} KB per contract"
echo "Result: All contracts are within the size threshold."
if [ "$HAS_WARNING" = true ]; then
  echo "Result: Some contracts exceed the size threshold."
fi
echo "================================================================="
