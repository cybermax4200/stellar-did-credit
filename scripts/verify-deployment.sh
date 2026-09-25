#!/bin/bash
set -euo pipefail

NETWORK="${NETWORK:-mainnet}"

# Look for deployments file
DEPLOYMENTS_FILE="deployments.${NETWORK}.json"

if [ ! -f "$DEPLOYMENTS_FILE" ]; then
    echo "Error: $DEPLOYMENTS_FILE not found."
    exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "Error: jq is required to parse $DEPLOYMENTS_FILE."
    exit 1
fi

IDENTITY_ORACLE_ID=$(jq -r '.contracts["identity-oracle"] // .["identity-oracle"] | if type == "string" then . elif type == "object" and has("id") then .id elif type == "object" and has("address") then .address else empty end' "$DEPLOYMENTS_FILE")
CREDIT_ORACLE_ID=$(jq -r '.contracts["credit-oracle"] // .["credit-oracle"] | if type == "string" then . elif type == "object" and has("id") then .id elif type == "object" and has("address") then .address else empty end' "$DEPLOYMENTS_FILE")
REVOCATION_REGISTRY_ID=$(jq -r '.contracts["revocation-registry"] // .["revocation-registry"] | if type == "string" then . elif type == "object" and has("id") then .id elif type == "object" and has("address") then .address else empty end' "$DEPLOYMENTS_FILE")
GOVERNANCE_ID=$(jq -r '.contracts["governance"] // .["governance"] | if type == "string" then . elif type == "object" and has("id") then .id elif type == "object" and has("address") then .address else empty end' "$DEPLOYMENTS_FILE")

if [ -z "$IDENTITY_ORACLE_ID" ] || [ "$IDENTITY_ORACLE_ID" == "null" ]; then
    echo "Error: identity-oracle ID not found in $DEPLOYMENTS_FILE."
    exit 1
fi

echo "Verifying identity-oracle configuration for ID: $IDENTITY_ORACLE_ID"
REGISTRY_ADDR=$(stellar contract invoke \
  --id "$IDENTITY_ORACLE_ID" \
  --network "$NETWORK" \
  -- get_revocation_registry 2>/dev/null || echo "error")

if [ "$REGISTRY_ADDR" == "error" ]; then
    echo "Error: failed to invoke get_revocation_registry on identity-oracle."
    exit 1
fi

echo "Revocation registry configured as: $REGISTRY_ADDR"
if [ "$REGISTRY_ADDR" == "null" ] || [ -z "$REGISTRY_ADDR" ]; then
    echo "Warning: revocation-registry is not linked to identity-oracle!"
    exit 1
fi

if [ -n "$CREDIT_ORACLE_ID" ] && [ "$CREDIT_ORACLE_ID" != "null" ]; then
    echo "Verifying credit-oracle configuration for ID: $CREDIT_ORACLE_ID"
    IDENTITY_ADDR=$(stellar contract invoke \
      --id "$CREDIT_ORACLE_ID" \
      --network "$NETWORK" \
      -- get_identity_oracle 2>/dev/null || echo "error")
    
    if [ "$IDENTITY_ADDR" == "error" ]; then
        echo "Error: failed to invoke get_identity_oracle on credit-oracle."
        exit 1
    fi
    
    echo "Identity oracle configured as: $IDENTITY_ADDR"
    if [ "$IDENTITY_ADDR" == "null" ] || [ -z "$IDENTITY_ADDR" ]; then
        echo "Warning: identity-oracle is not linked to credit-oracle!"
        exit 1
    fi
    
    # Verify governance is the credit-oracle admin
    CREDIT_ORACLE_ADMIN=$(stellar contract invoke \
      --id "$CREDIT_ORACLE_ID" \
      --network "$NETWORK" \
      -- get_admin 2>/dev/null || echo "error")
    
    if [ "$CREDIT_ORACLE_ADMIN" == "error" ]; then
        echo "Error: failed to invoke get_admin on credit-oracle."
        exit 1
    fi
    
    echo "Credit-oracle admin: $CREDIT_ORACLE_ADMIN"
    
    if [ -n "$GOVERNANCE_ID" ] && [ "$GOVERNANCE_ID" != "null" ]; then
        echo "Governance contract: $GOVERNANCE_ID"
        if [ "$CREDIT_ORACLE_ADMIN" == "$GOVERNANCE_ID" ]; then
            echo "PASS: Governance is correctly set as credit-oracle admin"
        else
            echo "FAIL: Credit-oracle admin ($CREDIT_ORACLE_ADMIN) does not match governance contract ($GOVERNANCE_ID)"
            echo "Error: Governance votes will have no effect - admin mismatch detected"
            exit 1
        fi
    else
        echo "Warning: governance contract ID not found in $DEPLOYMENTS_FILE - cannot verify admin relationship"
    fi
fi

echo "Deployment configuration verified successfully!"
