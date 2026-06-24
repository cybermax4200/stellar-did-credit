#!/bin/bash
set -euo pipefail

NETWORK="${NETWORK:-testnet}"
SOURCE="deployer"

# Validate network selection
if [[ "$NETWORK" == "mainnet" ]] && [[ "${FORCE_MAINNET:-}" != "1" ]]; then
  echo "Error: Refusing to deploy to mainnet without --force-mainnet flag"
  exit 1
fi

# Warn if default network differs from target
DEFAULT_NETWORK=$(stellar keys list --network testnet 2>/dev/null | head -1 || echo "testnet")
if [[ "$DEFAULT_NETWORK" != "$NETWORK" ]]; then
  echo "Warning: Default network ($DEFAULT_NETWORK) differs from target network ($NETWORK)"
fi

echo "Building contracts..."
stellar contract build

echo "Deploying identity-oracle..."
IDENTITY_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/identity_oracle.wasm \
  --source $SOURCE \
  --network $NETWORK)
echo "identity-oracle: $IDENTITY_ID"

echo "identity-oracle: $IDENTITY_ID"

echo "Deploying credit-oracle..."
CREDIT_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/credit_oracle.wasm \
  --source $SOURCE \
  --network $NETWORK)
echo "credit-oracle: $CREDIT_ID"

echo "Deploying revocation-registry..."
REVOCATION_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/revocation_registry.wasm \
  --source $SOURCE \
  --network $NETWORK)
echo "revocation-registry: $REVOCATION_ID"

echo "Saving to deployments.testnet.json..."
TEMP_FILE=$(mktemp)
cat > "$TEMP_FILE" << EOF
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
mv "$TEMP_FILE" "deployments.${NETWORK}.json"

echo "Done. Contracts deployed to deployments.${NETWORK}.json"
