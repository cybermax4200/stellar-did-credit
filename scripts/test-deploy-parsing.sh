#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

cat > "$workdir/deployments.json" <<'EOF'
{
  "network": "testnet",
  "contracts": {
    "identity-oracle": { "address": "CIDENTITY123" },
    "credit-oracle": { "address": "CCREDIT123" },
    "revocation-registry": { "address": "CREVOC123" }
  }
}
EOF

mockbin="$workdir/bin"
mkdir -p "$mockbin"

export PATH="$mockbin:$(echo "$PATH" | tr ':' '\n' | grep -v '^/mnt/c/' | tr '\n' ':')"
hash -r 2>/dev/null || true

cat > "$mockbin/stellar" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "keys" && "$2" == "address" ]]; then
  echo "GBRPYHIL2CI3FNQ4BXLFMNDLFJUNPU2HY3ZMFSHONUCEOASW7QC7OX2"
  exit 0
fi
if [[ "$1" == "contract" && "$2" == "build" ]]; then
  exit 0
fi
if [[ "$1" == "contract" && "$2" == "deploy" ]]; then
  echo "CDEPLOYED123"
  exit 0
fi
if [[ "$1" == "contract" && "$2" == "invoke" ]]; then
  if [[ "${MOCK_INITIALIZE_FAIL:-0}" == "1" ]]; then
    for arg in "$@"; do
      if [[ "$arg" == "initialize" ]]; then
        echo "HostError: Error(Contract, #1) - AlreadyInitialized" >&2
        exit 1
      fi
    done
  fi
  exit 0
fi
printf 'unexpected stellar invocation: %s\n' "$*" >&2
exit 1
MOCK
chmod +x "$mockbin/stellar"
sed -i 's/\r$//' "$mockbin/stellar"
hash -r 2>/dev/null || true

export DEPLOYMENTS_FILE="$workdir/deployments.json"

output=$(cd "$repo_root" && bash scripts/deploy.sh --resume --network testnet 2>&1)

if ! grep -q "Skipping identity-oracle (already deployed: CIDENTITY123)" <<<"$output"; then
  echo "Expected resume mode to reuse the nested identity address" >&2
  echo "$output" >&2
  exit 1
fi

if ! grep -q "Skipping credit-oracle (already deployed: CCREDIT123)" <<<"$output"; then
  echo "Expected resume mode to reuse the nested credit address" >&2
  echo "$output" >&2
  exit 1
fi

if ! grep -q "Skipping revocation-registry (already deployed: CREVOC123)" <<<"$output"; then
  echo "Expected resume mode to reuse the nested revocation address" >&2
  echo "$output" >&2
  exit 1
fi

echo "deploy resume parsing test passed"

# ---------------------------------------------------------------------------
# Test --initialize flag execution and phase logging
# ---------------------------------------------------------------------------
init_file="$workdir/deployments-init.json"
rm -f "$init_file"
export DEPLOYMENTS_FILE="$init_file"

init_output=$(cd "$repo_root" && bash scripts/deploy.sh --network testnet --initialize 2>&1)

for phase in "✓ Deploying..." "✓ Initializing..." "✓ Configuring..." "✓ Verifying..." "✓ Deployment complete"; do
  if ! grep -q "$phase" <<<"$init_output"; then
    echo "Expected output to contain phase: $phase" >&2
    echo "$init_output" >&2
    exit 1
  fi
done

if ! grep -q '"initialized": true' "$init_file"; then
  echo "Expected deployments JSON to contain 'initialized': true" >&2
  cat "$init_file" >&2
  exit 1
fi

echo "deploy --initialize flag test passed"

# ---------------------------------------------------------------------------
# Test idempotency handling when contracts are already initialized
# ---------------------------------------------------------------------------
idemp_file="$workdir/deployments-idemp.json"
rm -f "$idemp_file"
export DEPLOYMENTS_FILE="$idemp_file"
export MOCK_INITIALIZE_FAIL=1

idemp_output=$(cd "$repo_root" && bash scripts/deploy.sh --network testnet --initialize 2>&1)

if ! grep -q "already initialized, skipping" <<<"$idemp_output"; then
  echo "Expected idempotency warning for already initialized contract" >&2
  echo "$idemp_output" >&2
  exit 1
fi

echo "deploy idempotency test passed"
