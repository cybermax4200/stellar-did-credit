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

mkdir -p "$workdir/bin"
cat > "$workdir/bin/stellar" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "keys" && "$2" == "address" ]]; then
  echo "GTESTDEPLOYERAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
  exit 0
fi
if [[ "$1" == "contract" && "$2" == "build" ]]; then
  exit 0
fi
if [[ "$1" == "contract" && "$2" == "deploy" ]]; then
  echo "CDEPLOYED123"
  exit 0
fi
printf 'unexpected stellar invocation: %s\n' "$*" >&2
exit 1
EOF
chmod +x "$workdir/bin/stellar"

export PATH="$workdir/bin:$PATH"
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
