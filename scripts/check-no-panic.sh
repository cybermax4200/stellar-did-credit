#!/usr/bin/env bash
# -----------------------------------------------------------------------
# check-no-panic.sh
#
# Fails if bare panic!() exists in contracts/*/src/*.rs outside #[test]
# blocks and #[cfg(test)] modules.  soroban_sdk::panic_with_error! is
# allowed (it's a structured error, not a bare panic).
#
# Excludes:
#   - contracts/tests/ (entire test crate)
#   - files matching *test* in their name
#
# Usage:  bash scripts/check-no-panic.sh
# Exit 0  — all clean
# Exit 1  — bare panic! found in contract source
# -----------------------------------------------------------------------
set -euo pipefail

FAILED=0

echo "Scanning contracts/*/src/*.rs for bare panic!() calls..."
echo ""

# Collect files first, then iterate (avoids subshell variable issues)
FILES=()
while IFS= read -r f; do
  FILES+=("$f")
done < <(find contracts/*/src -name '*.rs' \
  -not -path '*/tests/*' \
  -not -name '*test*' \
  2>/dev/null | sort)

for f in "${FILES[@]}"; do
  # Use awk to skip test code:
  #   - #[cfg(test)] blocks
  #   - mod tests { ... }
  #   - #[test] functions
  # Allow:
  #   - panic_with_error! (structured Soroban error)
  #   - comments containing the word "panic"
  MATCHES=$(awk '
    /^[[:space:]]*#\[cfg\(test\)\]/  { in_test = 1; next }
    /^[[:space:]]*mod tests/         { in_test = 1; next }
    /^[[:space:]]*#\[test\]/         { in_test = 1; next }
    in_test && /^[[:space:]]*}/      { in_test = 0; next }
    in_test { next }
    /panic_with_error!/ { next }
    /\/\/.*panic/ { next }
    /panic!/ { printf "%d:%s\n", NR, $0 }
  ' "$f")

  if [ -n "$MATCHES" ]; then
    echo "FAIL: $f"
    echo "$MATCHES"
    echo ""
    FAILED=1
  fi
done

if [ "$FAILED" -eq 1 ]; then
  echo "ERROR: Found bare panic!() in contract source files."
  echo "Use soroban_sdk::panic_with_error! or return a Result with a typed error."
  echo "See: CONTRIBUTING.md -> 'No panic! in contract logic'"
  exit 1
fi

echo "All contract source files are free of bare panic!() calls."
