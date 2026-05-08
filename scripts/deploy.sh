#!/bin/bash
set -e

NETWORK="testnet"
SOURCE="deployer"

echo "Building contracts..."
stellar contract build

echo "Deploying identity-oracle..."
IDENTITY_ID=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/identity_oracle.wasm \
  --source $SOURCE \
  --network $NETWORK)
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
cat > deployments.testnet.json << EOF
{
  "network": "testnet",
  "deployed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "contracts": {
    "identity-oracle": "$IDENTITY_ID",
    "credit-oracle": "$CREDIT_ID",
    "revocation-registry": "$REVOCATION_ID"
  }
}
EOF

echo "Done."
