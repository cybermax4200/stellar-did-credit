#!/bin/bash
# Strict mode: -e exits on error, -u treats unset variables as errors,
# -o pipefail propagates failures through pipelines. Together they ensure
# any unexpected failure stops the script immediately rather than silently
# producing a broken deployment.
set -euo pipefail

NETWORK=${NETWORK:-testnet}
SOURCE=${SOURCE:-deployer}
DEPLOYMENTS_FILE=${DEPLOYMENTS_FILE:-}
RESUME=false
FUND=false
FRIENDBOT_URL=${FRIENDBOT_URL:-https://friendbot.stellar.org}

# Admin key for wiring configurations. Defaults to deployer key if not explicitly set.
ADMIN=${ADMIN:-$SOURCE}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
  case "$1" in
    --resume)
      RESUME=true
      shift
      ;;
    --network)
      if [[ $# -lt 2 ]]; then
        echo "Error: --network requires a value" >&2
        exit 1
      fi
      NETWORK="$2"
      shift 2
      ;;
    --fund)
      FUND=true
      shift
      ;;
    --help|-h)
      echo "Usage: $0 [--resume] [--network <testnet|mainnet>] [--fund]" >&2
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      echo "Usage: $0 [--resume] [--network <testnet|mainnet>] [--fund]" >&2
      exit 1
      ;;
  esac
done

if [[ "$NETWORK" != "testnet" && "$NETWORK" != "mainnet" ]]; then
  echo "Error: network must be 'testnet' or 'mainnet'" >&2
  exit 1
fi

if [ -z "$DEPLOYMENTS_FILE" ]; then
  DEPLOYMENTS_FILE="deployments.${NETWORK}.json"
fi

if [[ "$NETWORK" == "mainnet" ]]; then
  echo "Mainnet deployment detected. Ensure the deployer account is funded and the admin key is held in secure offline storage." >&2
fi

# ---------------------------------------------------------------------------
# Horizon helpers
# ---------------------------------------------------------------------------

get_native_balance() {
  local account="$1"
  local horizon

  if [[ "$NETWORK" == "testnet" ]]; then
    horizon="https://horizon-testnet.stellar.org"
  else
    horizon="https://horizon.stellar.org"
  fi

  local response
  if ! response=$(curl -fsS --max-time 10 "$horizon/accounts/$account"); then
    echo "0"
    return
  fi

  local balance
  balance=$(printf '%s' "$response" | grep -o '"asset_type":"native"[^}]*"balance":"[^"]*"' | sed -E 's/.*"balance":"([^"]*)".*/\1/')
  if [[ -z "$balance" ]]; then
    balance=$(printf '%s' "$response" | grep -o '"balance":"[^"]*"' | head -n1 | sed -E 's/.*"balance":"([^"]*)".*/\1/')
  fi

  if [[ -z "$balance" ]]; then
    echo "0"
  else
    echo "$balance"
  fi
}

is_account_funded() {
  local balance
  balance=$(get_native_balance "$1")
  if [[ "$balance" == "0" || "$balance" == "0.0" || "$balance" == "0.0000000" || -z "$balance" ]]; then
    return 1
  fi
  return 0
}

fund_deployer() {
  if [[ "$NETWORK" != "testnet" ]]; then
    echo "Error: --fund is only supported on testnet." >&2
    exit 1
  fi

  if is_account_funded "$DEPLOYER_ADDRESS"; then
    echo "Deployer already funded: $(get_native_balance "$DEPLOYER_ADDRESS") XLM"
    return
  fi

  echo "Requesting funds from Friendbot ($FRIENDBOT_URL)..."

  local attempt=1
  local max_attempts=3
  while [[ $attempt -le $max_attempts ]]; do
    if curl -fsS --max-time 10 "$FRIENDBOT_URL/?addr=$DEPLOYER_ADDRESS" >/dev/null 2>&1; then
      break
    fi

    echo "Friendbot request failed (attempt $attempt/$max_attempts). Retrying..."
    attempt=$((attempt + 1))
    sleep 2
  done

  if [[ $attempt -gt $max_attempts ]]; then
    echo "Error: failed to fund deployer via Friendbot after $max_attempts attempts." >&2
    exit 1
  fi

  if ! is_account_funded "$DEPLOYER_ADDRESS"; then
    echo "Error: Friendbot call succeeded, but deployer balance is still zero." >&2
    exit 1
  fi

  echo "Deployer funded successfully: $(get_native_balance "$DEPLOYER_ADDRESS") XLM"
}

# ---------------------------------------------------------------------------
# Resume support
# ---------------------------------------------------------------------------
read_deployment_value() {
  local key="$1"
  local file="$2"

  if command -v jq >/dev/null 2>&1; then
    jq -r --arg key "$key" '
      def extract:
        if . == null then empty
        elif type == "string" then .
        elif type == "object" and has("address") then .address
        elif type == "object" and has("contract_id") then .contract_id
        elif type == "object" and has("id") then .id
        else empty end;

      ((.contracts[$key] // .[$key]) | extract) // empty
    ' "$file"
  elif command -v python3 >/dev/null 2>&1; then
    python3 - "$file" "$key" <<'PY'
import json
import sys
from typing import Any

file_path, key = sys.argv[1], sys.argv[2]

try:
    with open(file_path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
except FileNotFoundError:
    raise SystemExit(0)
except json.JSONDecodeError as exc:
    print(f"Error: failed to parse deployment metadata in {file_path}: {exc}", file=sys.stderr)
    raise SystemExit(1) from exc


def extract(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        for field in ("address", "contract_id", "id"):
            nested = value.get(field)
            if isinstance(nested, str) and nested:
                return nested
    return ""


if isinstance(data, dict):
    contracts = data.get("contracts")
    if isinstance(contracts, dict):
        if key in contracts:
            value = extract(contracts[key])
            if value:
                print(value)
                raise SystemExit(0)
    if key in data:
        value = extract(data[key])
        if value:
            print(value)
            raise SystemExit(0)
PY
  else
    echo "Error: jq or python3 is required to parse deployment metadata from $file" >&2
    exit 1
  fi
}

IDENTITY_ID=""
CREDIT_ID=""
REVOCATION_ID=""

if $RESUME && [ -f "$DEPLOYMENTS_FILE" ]; then
  echo "Resume mode: reading existing deployments from $DEPLOYMENTS_FILE ..."

  IDENTITY_ID=$(read_deployment_value "identity-oracle" "$DEPLOYMENTS_FILE")
  CREDIT_ID=$(read_deployment_value "credit-oracle" "$DEPLOYMENTS_FILE")
  REVOCATION_ID=$(read_deployment_value "revocation-registry" "$DEPLOYMENTS_FILE")

  echo "  identity-oracle:     ${IDENTITY_ID:-(missing)}"
  echo "  credit-oracle:       ${CREDIT_ID:-(missing)}"
  echo "  revocation-registry: ${REVOCATION_ID:-(missing)}"
elif $RESUME; then
  echo "Resume mode: no existing $DEPLOYMENTS_FILE found – proceeding with full deployment."
fi

# ---------------------------------------------------------------------------
# Validate deployer key
# ---------------------------------------------------------------------------
DEPLOYER_ADDRESS=$(stellar keys address "$SOURCE" 2>&1) || true
if [[ ! "$DEPLOYER_ADDRESS" =~ ^G[A-Z2-7]{54}$ ]]; then
  echo "Error: '$SOURCE' key not found. Run: stellar keys generate --global $SOURCE --network $NETWORK" >&2
  exit 1
fi
echo "Deployer address: $DEPLOYER_ADDRESS"

if [[ "$FUND" == "true" ]]; then
  fund_deployer
fi

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
echo "Building contracts..."
stellar contract build

# ---------------------------------------------------------------------------
# Deploy Pipeline
# ---------------------------------------------------------------------------
echo "Starting deployment pipeline..."

# 1. credit-oracle
if [ -n "$CREDIT_ID" ]; then
  echo "Skipping credit-oracle (already deployed: $CREDIT_ID)"
else
  echo "Deploying credit-oracle..."
  CREDIT_ID=$(stellar contract deploy \
    --wasm target/wasm32-unknown-unknown/release/credit_oracle.wasm \
    --source "$SOURCE" \
    --network "$NETWORK")
  echo "credit-oracle: $CREDIT_ID"
fi

# 2. identity-oracle
if [ -n "$IDENTITY_ID" ]; then
  echo "Skipping identity-oracle (already deployed: $IDENTITY_ID)"
else
  echo "Deploying identity-oracle..."
  IDENTITY_ID=$(stellar contract deploy \
    --wasm target/wasm32-unknown-unknown/release/identity_oracle.wasm \
    --source "$SOURCE" \
    --network "$NETWORK")
  echo "identity-oracle: $IDENTITY_ID"
fi

# 3. revocation-registry
if [ -n "$REVOCATION_ID" ]; then
  echo "Skipping revocation-registry (already deployed: $REVOCATION_ID)"
else
  echo "Deploying revocation-registry..."
  REVOCATION_ID=$(stellar contract deploy \
    --wasm target/wasm32-unknown-unknown/release/revocation_registry.wasm \
    --source "$SOURCE" \
    --network "$NETWORK")
  echo "revocation-registry: $REVOCATION_ID"
fi

# Atomic JSON output is written *before* wiring to guarantee IDs are saved if a later step fails.
echo "Saving intermediate deployment state to $DEPLOYMENTS_FILE..."
cat > "$DEPLOYMENTS_FILE" <<EOF
{
  "network": "$NETWORK",
  "deployed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "contracts": {
    "identity-oracle": "$IDENTITY_ID",
    "credit-oracle": "$CREDIT_ID",
    "revocation-registry": "$REVOCATION_ID"
  }
}
EOF

# ---------------------------------------------------------------------------
# Contract Initialization & Wiring
# ---------------------------------------------------------------------------
echo "Initializing contracts with admin: $ADMIN ..."
stellar contract invoke --id "$CREDIT_ID" --source "$SOURCE" --network "$NETWORK" -- initialize --admin "$ADMIN" 2>/dev/null || true
stellar contract invoke --id "$IDENTITY_ID" --source "$SOURCE" --network "$NETWORK" -- initialize --admin "$ADMIN" 2>/dev/null || true
stellar contract invoke --id "$REVOCATION_ID" --source "$SOURCE" --network "$NETWORK" -- initialize --admin "$ADMIN" 2>/dev/null || true

echo "4. Wiring revocation-registry to identity-oracle..."
CURRENT_REGISTRY=$(stellar contract invoke --id "$IDENTITY_ID" --network "$NETWORK" -- get_revocation_registry 2>/dev/null || echo "None")

if [[ "$CURRENT_REGISTRY" == *"$REVOCATION_ID"* ]]; then
  echo " -> Skipped: revocation-registry already set."
else
  echo " -> Calling set_revocation_registry..."
  if ! stellar contract invoke --id "$IDENTITY_ID" --network "$NETWORK" --source "$ADMIN" -- set_revocation_registry --registry_id "$REVOCATION_ID"; then
    echo "ERROR: Failed to execute set_revocation_registry on identity-oracle." >&2
    echo "Please verify admin key permissions or run scripts/verify-deployment.sh for diagnostics." >&2
    exit 1
  fi
  echo " -> Success."
fi

echo "5. Wiring identity-oracle to credit-oracle..."
CURRENT_ID_ORACLE=$(stellar contract invoke --id "$CREDIT_ID" --network "$NETWORK" -- get_identity_oracle 2>/dev/null || echo "None")

if [[ "$CURRENT_ID_ORACLE" == *"$IDENTITY_ID"* ]]; then
  echo " -> Skipped: identity-oracle already set."
else
  echo " -> Calling set_identity_oracle..."
  if ! stellar contract invoke --id "$CREDIT_ID" --network "$NETWORK" --source "$ADMIN" -- set_identity_oracle --identity_oracle_id "$IDENTITY_ID"; then
    echo "ERROR: Failed to execute set_identity_oracle on credit-oracle." >&2
    echo "Please verify admin key permissions or run scripts/verify-deployment.sh for diagnostics." >&2
    exit 1
  fi
  echo " -> Success."
fi

echo "6. Running final deployment verification..."
if [ -x "./scripts/verify-deployment.sh" ]; then
  if ! ./scripts/verify-deployment.sh; then
    echo "ERROR: Final wiring verification failed." >&2
    exit 1
  fi
else
  echo "Warning: ./scripts/verify-deployment.sh not found or not executable. Skipping verification."
fi

echo "Deployment and wiring completed successfully!"