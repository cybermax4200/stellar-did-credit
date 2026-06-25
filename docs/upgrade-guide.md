# Contract Upgrade Guide

All three protocol contracts (`identity-oracle`, `credit-oracle`, `revocation-registry`) support in-place WASM hash upgrades via `env.deployer().update_current_contract_wasm()`. This preserves the contract address and all stored state — no migration required.

---

## How it works

Soroban allows a deployed contract to replace its own WASM bytecode while keeping its contract ID and all ledger storage intact. The `upgrade` function accepts the SHA-256 hash of the new WASM, verifies the caller is the stored admin, then calls `update_current_contract_wasm`.

**Function signature (identical across all three contracts):**

```rust
pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>)
```

---

## Step-by-step upgrade process

### 1. Build the new WASM

```bash
cargo build --release --target wasm32-unknown-unknown \
  -p <identity-oracle|credit-oracle|revocation-registry>
```

The compiled WASM is output to:

```
target/wasm32-unknown-unknown/release/<contract_name>.wasm
```

### 2. Upload the new WASM to the network

Upload the WASM blob to Stellar — this registers it and returns its hash:

```bash
stellar contract upload \
  --network testnet \
  --source deployer \
  --wasm target/wasm32-unknown-unknown/release/<contract_name>.wasm
```

The command prints the 32-byte WASM hash. Copy it.

### 3. Call `upgrade` on the deployed contract

```bash
stellar contract invoke \
  --network testnet \
  --source <admin-key-name> \
  --id <CONTRACT_ADDRESS> \
  -- upgrade \
  --admin <ADMIN_ADDRESS> \
  --new_wasm_hash <WASM_HASH_FROM_STEP_2>
```

Replace `<CONTRACT_ADDRESS>`, `<ADMIN_ADDRESS>`, and `<WASM_HASH_FROM_STEP_2>` with actual values from `deployments.testnet.json` and step 2.

### 4. Verify the upgrade

Invoke any read function on the contract to confirm it responds correctly after the upgrade:

```bash
stellar contract invoke \
  --network testnet \
  --source deployer \
  --id <CONTRACT_ADDRESS> \
  -- get_scoring_weights   # credit-oracle example
```

---

## Security notes

- Only the address stored as `Admin` at initialization time can call `upgrade`.
- `admin.require_auth()` is enforced — the transaction must be signed by the admin keypair.
- There is no timelock. For production deployments, consider routing `upgrade` through a multisig or governance contract before calling it directly.
- Always test upgrades on testnet before applying to mainnet contracts.
