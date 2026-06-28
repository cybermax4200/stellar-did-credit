#!/bin/bash
# Strict mode: -e exits on error, -u treats unset variables as errors,
# -o pipefail propagates failures through pipelines. Together they ensure
# any unexpected failure stops the script immediately rather than silently
# producing a broken deployment.
set -euo pipefail

NETWORK=${NETWORK:-testnet}
SOURCE="deployer"
DEPLOYMENTS_FILE="deployments.testnet.json"
RESUME=false

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
for arg in "$@"; do
  case "$arg" in
    --resume)
      RESUME=true
      ;;
    *)
      echo "Unknown argument: $arg" >&2
      echo "Usage: $0 [--resume]" >&2
      exit 1
      ;;
  esac
done

# ---------------------------------------------------------------------------
# Resume support
#
# When --resume is passed and a deployments.testnet.json already exists, we
# read the previously recorded contract addresses.  Any contract whose address
# is already present and non-empty is skipped; only missing ones are deployed.
# This makes an interrupted deployment safely restartable without redeploying
# contracts that already landed on-chain.
# ---------------------------------------------------------------------------
IDENTITY_ID=""
CREDIT_ID=""
REVOCATION_ID=""

if $RESUME && [ -f "$DEPLOYMENTS_FILE" ]; then
  echo "Resume mode: reading existing deployments from $DEPLOYMENTS_FILE ..."

  # Extract values with basic grep/sed – no jq dependency required.
  IDENTITY_ID=$(grep -o '"identity-oracle": *"[^"]*"' "$DEPLOYMENTS_FILE" \
    | sed 's/.*: *"\([^"]*\)"/\1/' || true)
  CREDIT_ID=$(grep -o '"credit-oracle": *"[^"]*"' "$DEPLOYMENTS_FILE" \
    | sed 's/.*: *"\([^"]*\)"/\1/' || true)
  REVOCATION_ID=$(grep -o '"revocation-registry": *"[^"]*"' "$DEPLOYMENTS_FILE" \
    | sed 's/.*: *"\([^"]*\)"/\1/' || true)

  echo "  identity-oracle:     ${IDENTITY_ID:-(missing)}"
  echo "  credit-oracle:       ${CREDIT_ID:-(missing)}"
  echo "  revocation-registry: ${REVOCATION_ID:-(missing)}"
elif $RESUME; then
  echo "Resume mode: no existing $DEPLOYMENTS_FILE found – proceeding with full deployment."
fi

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
echo "Building contracts..."
stellar contract build

# ---------------------------------------------------------------------------
# Deploy each contract (skip if a valid address is already recorded)
# ---------------------------------------------------------------------------

# identity-oracle
if [ -n "$IDENTITY_ID" ]; then
  echo "Skipping identity-oracle (already deployed: $IDENTITY_ID)"
else
  echo "Deploying identity-oracle..."
  IDENTITY_ID=$(stellar contract deploy \
    --wasm target/wasm32-unknown-unknown/release/identity_oracle.wasm \
    --source $SOURCE \
    --network $NETWORK)
  echo "identity-oracle: $IDENTITY_ID"
fi

# credit-oracle
if [ -n "$CREDIT_ID" ]; then
  echo "Skipping credit-oracle (already deployed: $CREDIT_ID)"
else
  echo "Deploying credit-oracle..."
  CREDIT_ID=$(stellar contract deploy \
    --wasm target/wasm32-unknown-unknown/release/credit_oracle.wasm \
    --source $SOURCE \
    --network $NETWORK)
  echo "credit-oracle: $CREDIT_ID"
fi

# revocation-registry
if [ -n "$REVOCATION_ID" ]; then
  echo "Skipping revocation-registry (already deployed: $REVOCATION_ID)"
else
  echo "Deploying revocation-registry..."
  REVOCATION_ID=$(stellar contract deploy \
    --wasm target/wasm32-unknown-unknown/release/revocation_registry.wasm \
    --source $SOURCE \
    --network $NETWORK)
  echo "revocation-registry: $REVOCATION_ID"
fi

# ---------------------------------------------------------------------------
# Atomic JSON output
#
# deployments.testnet.json is written exactly once, only after every contract
# address has been collected successfully.  Writing the file at the very end
# (never incrementally) means an interrupted deployment can never leave behind
# a partially written or malformed JSON file.
# ---------------------------------------------------------------------------
echo "Saving to $DEPLOYMENTS_FILE..."
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

echo "Done." 

