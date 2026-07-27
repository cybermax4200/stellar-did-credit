# Storage Migration Strategy

This document outlines the strategy for managing contract storage layout transitions between versions within the Stellar DID Credit protocol.

## Why it matters

Soroban contracts use key-value storage (Instance, Persistent, Temporary) where complex types are serialized into bytes on-chain. When a contract's WASM byte-code is upgraded in-place:
1. The address and storage entries are preserved.
2. The code is replaced.

If the new code expects a different structure layout for a storage entry (e.g., adding/removing fields or changing enum variants), trying to deserialize the old byte representation with the new type definition will fail with a serialization/deserialization panic, rendering that record unreadable.

---

## Strategy Components

To ensure robust upgrades, we implement a three-layered strategy:

### 1. Storage Versioning
Contracts maintain a global schema version in instance storage under `DataKey::StorageVersion`.
* **New deployments**: Initialized directly to the latest version (V2).
* **Existing deployments**: Implicitly assumed to be Version 1 if the version key is missing.

### 2. Dual-Layout Deserialization
For any modified types, the contract defines both the old structure layout (e.g., `RepaymentRecordV1`) and the new layout (`RepaymentRecord`).
During operations (`record_repayment`, `compute_score`), the contract checks `StorageVersion`:
* If `StorageVersion < 2`, it deserializes using the V1 layout, then converts/updates in-memory.
* If `StorageVersion >= 2`, it deserializes directly using the V2 layout.

This prevents deserialization panics for existing users who have not yet been migrated.

### 3. One-Time Migration Orchestration
A `migrate` function is added to the contracts:
* Gated by version checks: can only run if `StorageVersion < 2`.
* Gated by admin authorization: only the admin can call the migration.
* Takes a batch of subjects, reads their old structures, transforms them to the new layout (adding fields like `total_repaid` with defaults of 0), and writes them back.
* Sets `StorageVersion` to 2 upon successful completion.

---

## Example: `RepaymentRecord` Migration (V1 to V2)

### V1 Layout:
```rust
pub struct RepaymentRecordV1 {
    pub on_time_count: u32,
    pub total_count: u32,
}
```

### V2 Layout (adds `total_repaid`):
```rust
pub struct RepaymentRecord {
    pub on_time_count: u32,
    pub total_count: u32,
    pub total_repaid: i128,
}
```

### Migration Logic:
```rust
pub fn migrate(env: Env, subjects: Vec<Address>) -> Result<(), CreditOracleError> {
    require_admin(&env);
    let version = env.storage().instance().get(&DataKey::StorageVersion).unwrap_or(1);
    if version >= 2 {
        return Ok(());
    }
    for subject in subjects.iter() {
        let key = DataKey::RepaymentRecord(subject.clone());
        if let Some(old_record) = env.storage().persistent().get::<_, RepaymentRecordV1>(&key) {
            let new_record = RepaymentRecord {
                on_time_count: old_record.on_time_count,
                total_count: old_record.total_count,
                total_repaid: 0,
            };
            env.storage().persistent().set(&key, &new_record);
        }
    }
    env.storage().instance().set(&DataKey::StorageVersion, &2u32);
    Ok(())
}
```

This ensures that the transition is atomic, backward-compatible, and zero-downtime.
