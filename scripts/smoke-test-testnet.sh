#!/usr/bin/env bash
# Smoke test for deployments.testnet.json
#
# Verifies that every contract address recorded in deployments.testnet.json
# is actually deployed and responsive on testnet, by invoking a cheap,
# read-only, argument-free function on each contract:
#
#   identity-oracle      -> get_revocation_registry
#   credit-oracle        -> get_scoring_weights
#   revocation-registry  -> get_batch_limit
#
# If every recorded address is still the "CXXXXXXX..." placeholder, the
# check is skipped (exit 0) since nothing has been deployed yet.
#
# Usage:
#   scripts/smoke-test-testnet.sh [path-to-deployments-file]
#
# Env:
#   NETWORK              Stellar network to invoke against (default: testnet)
#   INVOKE_TIMEOUT_SECS  Per-contract invoke timeout in seconds (default: 30)

set -euo pipefail

DEPLOYMENTS_FILE="${1:-deployments.testnet.json}"
NETWORK="${NETWORK:-testnet}"
INVOKE_TIMEOUT_SECS="${INVOKE_TIMEOUT_SECS:-30}"

# Contract key -> read-only, no-argument function used as a liveness probe.
CONTRACT_KEYS=("identity-oracle" "credit-oracle" "revocation-registry")
declare -A PROBE_FN=(
  ["identity-oracle"]="get_revocation_registry"
  ["credit-oracle"]="get_scoring_weights"
  ["revocation-registry"]="get_batch_limit"
)

# Placeholder addresses look like "CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".
PLACEHOLDER_RE='^CXXXXXXX'

if [ ! -f "$DEPLOYMENTS_FILE" ]; then
  echo "Error: $DEPLOYMENTS_FILE not found."
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "Error: jq is required to parse $DEPLOYMENTS_FILE."
  exit 1
fi

# Extract an address for a contract key, supporting both the plain-string
# form ("identity-oracle": "C...") and an object form
# ("identity-oracle": {"address": "C..."} or {"id": "C..."}).
extract_address() {
  local key="$1"
  jq -r --arg key "$key" '
    .contracts[$key] // .[$key]
    | if type == "string" then .
      elif type == "object" and has("address") then .address
      elif type == "object" and has("id") then .id
      else empty
      end
  ' "$DEPLOYMENTS_FILE"
}

declare -A ADDRESSES
any_real_address=0

for key in "${CONTRACT_KEYS[@]}"; do
  addr=$(extract_address "$key" || true)
  ADDRESSES["$key"]="$addr"

  if [ -z "$addr" ] || [ "$addr" == "null" ]; then
    echo "Notice: no address recorded for '$key' in $DEPLOYMENTS_FILE; skipping."
    continue
  fi

  if [[ "$addr" =~ $PLACEHOLDER_RE ]]; then
    echo "Notice: '$key' still has a placeholder address ($addr); skipping."
    continue
  fi

  any_real_address=1
done

if [ "$any_real_address" -eq 0 ]; then
  echo ""
  echo "All contract addresses in $DEPLOYMENTS_FILE are placeholders (or missing)."
  echo "Nothing has been deployed to testnet yet -- skipping smoke test."
  exit 0
fi

if ! command -v stellar >/dev/null 2>&1; then
  echo "Error: stellar CLI is required to run the smoke test."
  exit 1
fi

echo ""
echo "Running testnet smoke test against '$NETWORK' (timeout: ${INVOKE_TIMEOUT_SECS}s per contract)..."
echo ""

overall_status=0

for key in "${CONTRACT_KEYS[@]}"; do
  addr="${ADDRESSES[$key]}"
  fn="${PROBE_FN[$key]}"

  if [ -z "$addr" ] || [ "$addr" == "null" ] || [[ "$addr" =~ $PLACEHOLDER_RE ]]; then
    printf 'SKIP: %-22s (no real address recorded)\n' "$key"
    continue
  fi

  echo "Checking $key ($addr) via '$fn'..."
  if output=$(timeout "$INVOKE_TIMEOUT_SECS" stellar contract invoke \
      --id "$addr" \
      --network "$NETWORK" \
      -- "$fn" 2>&1); then
    printf 'PASS: %-22s responded to %s -> %s\n' "$key" "$fn" "$output"
  else
    status=$?
    if [ "$status" -eq 124 ]; then
      printf 'FAIL: %-22s timed out after %ss calling %s\n' "$key" "$INVOKE_TIMEOUT_SECS" "$fn"
    else
      printf 'FAIL: %-22s failed to respond to %s\n' "$key" "$fn"
      echo "$output"
    fi
    overall_status=1
  fi
  echo ""
done

if [ "$overall_status" -ne 0 ]; then
  echo "Smoke test FAILED: one or more testnet contracts did not respond as expected."
  echo "This may indicate a stale deployments.testnet.json, or a testnet/RPC outage."
  exit 1
fi

echo "Smoke test PASSED: all recorded testnet contracts are live and responsive."
