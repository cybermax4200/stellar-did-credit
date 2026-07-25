# Epoch Model

**Audience:** Contributors — developers extending or maintaining the protocol contracts.

This document explains how Soroban ledger-based epoch (TTL) management works across the three protocol contracts, what actions bump the TTL, what gets invalidated when TTL expires, and related epoch-like mechanisms (compute cooldown, weight timelock). Understanding this model is essential for writing correct contract code, operating feeders, and diagnosing liveness issues.

---

## Table of contents

- [1. Soroban ledger time](#1-soroban-ledger-time)
- [2. Storage TTL model](#2-storage-ttl-model)
- [3. What bumps the TTL](#3-what-bumps-the-ttl)
- [4. What TTL expiry invalidates](#4-what-ttl-expiry-invalidates)
- [5. Related epoch concepts](#5-related-epoch-concepts)
- [6. Worked examples](#6-worked-examples)
- [7. Maintaining liveness](#7-maintaining-liveness)
- [8. Summary table](#8-summary-table)
- [9. Further reading](#9-further-reading)

---

## 1. Soroban ledger time

Soroban contracts do not have access to wall-clock time in the traditional sense. Time is measured in discrete units:

| Unit | Approximate duration | Used for |
|------|---------------------|----------|
| **Ledger** | ~5 seconds on Stellar mainnet/testnet | TTL counts, compute cooldown, weight timelock |
| **Ledger timestamp** | Unix seconds (set by validators) | Score staleness checks (`is_stale`) |

A ledger is the fundamental unit of progress in the Stellar network. Every ~5 seconds a new ledger is produced, and contracts execute within the context of a single ledger sequence number and a `ledger.timestamp()`.

All three contracts define two constants that control TTL management:

```rust
const INSTANCE_BUMP_THRESHOLD: u32 = 5000;   // ~7 hours  (5000 × 5s)
const INSTANCE_BUMP_AMOUNT: u32 = 500_000;   // ~30 days (500_000 × 5s)
```

---

## 2. Storage TTL model

Soroban provides two storage tiers, each with independent TTL (time-to-live) semantics.

### 2.1 Instance storage

**Key characteristics:**

- Stores contract-global singleton state: `Admin`, `Config` (weights), `PendingWeights`, `IdentityOracleId`, `ComputeCooldownLedgers`, etc.
- Has a **single TTL** for the entire instance storage entry.
- When this TTL reaches zero, the contract is **archived** — all data is lost and the contract can never be called again.
- The TTL is measured in ledgers from the last `extend_ttl` call.

**Which keys live here (per contract):**

| Contract | Instance storage keys |
|----------|-----------------------|
| identity-oracle | `Admin`, `PendingAdmin`, `RevocationRegistryId` |
| credit-oracle | `Admin`, `PendingAdmin`, `Config` (weights), `PendingWeights`, `PendingWeightsEffectiveLedger`, `ComputeCooldownLedgers`, `IdentityOracleId` |
| revocation-registry | `Admin`, `PendingAdmin` |

### 2.2 Persistent storage

**Key characteristics:**

- Stores per-address and per-hash data: `DIDDocument`, `VCAnchors`, `TxStats`, `Score`, `TrustedFeeder`, `TrustedIssuer`, `Status(vc_hash)`, etc.
- Each **individual key** has its own TTL.
- When a persistent key's TTL expires, only that specific key's data is lost.
- Persistent keys are **not** extended automatically — the contract must explicitly call `extend_ttl` on each persistent key when it is read or written, or the entries may expire independently.

> **Current state:** None of the three contracts explicitly extend TTL on persistent storage keys after initial writes. This is a known limitation (tracked in cross-contract VC count roadmap items). In practice, persistent entries created by admin actions (feeder/lender/issuer registrations) are likely to be touched again before TTL expiry. Entries created by non-admin actions (VC anchors, scores) may expire if the subject is inactive for an extended period.

### 2.3 TTL refresh mechanism

The Soroban SDK provides:

```rust
env.storage().instance().extend_ttl(threshold: u32, amount: u32);
env.storage().persistent().extend_ttl(key: &DataKey, threshold: u32, amount: u32);
```

The contract checks the remaining TTL. If it is below `threshold`, the TTL is extended by `amount` ledgers. If it is above `threshold`, the call is a no-op. This means frequent calls do not perpetually extend — only calls when the TTL is running low.

---

## 3. What bumps the TTL

Not every function call extends the instance TTL. The design intentionally separates "admin operations that manage the contract" from "user operations that use the contract."

### 3.1 Functions that extend instance TTL

These functions call `env.storage().instance().extend_ttl(...)` after successful authentication. Any of these, when called at least once every ~7 days (5000 ledgers), keeps the contract alive.

| Contract | Function | Rationale |
|----------|----------|-----------|
| identity-oracle | `initialize` | First-time setup; future admin calls refresh |
| identity-oracle | `register_issuer` | Admin manages issuer whitelist |
| identity-oracle | `deregister_issuer` | Admin manages issuer whitelist |
| identity-oracle | `set_revocation_registry` | Admin configures cross-contract link |
| identity-oracle | `propose_new_admin` | Admin manages ownership |
| identity-oracle | `accept_admin` | Admin transfer step |
| identity-oracle | `upgrade` | Admin upgrades contract WASM |
| credit-oracle | `initialize` | First-time setup |
| credit-oracle | `register_feeder` | Admin manages feeder whitelist |
| credit-oracle | `deregister_feeder` | Admin manages feeder whitelist |
| credit-oracle | `register_lender` | Admin manages lender whitelist |
| credit-oracle | `deregister_lender` | Admin manages lender whitelist |
| credit-oracle | `propose_weights` | Admin/governor initiates weight change |
| credit-oracle | `propose_new_admin` | Admin manages ownership |
| credit-oracle | `accept_admin` | Admin transfer step |
| credit-oracle | `upgrade` | Admin upgrades contract WASM |
| revocation-registry | `initialize` | First-time setup |
| revocation-registry | `propose_new_admin` | Admin manages ownership |
| revocation-registry | `accept_admin` | Admin transfer step |
| revocation-registry | `upgrade` | Admin upgrades contract WASM |

### 3.2 Functions that do NOT extend instance TTL

These functions intentionally do not call `extend_ttl`. This is not an oversight — it is a deliberate design choice to keep gas costs low for the most frequent operations and to ensure that liveness is driven by admin/management activity, not by user activity.

| Contract | Function | Why it does not extend |
|----------|----------|----------------------|
| identity-oracle | `anchor_did` | User operation; touches only persistent storage |
| identity-oracle | `anchor_vc` | Issuer operation; touches only persistent storage |
| identity-oracle | `mark_vc_revoked` | Issuer operation; touches only persistent storage |
| identity-oracle | `is_verified` | Read-only query |
| identity-oracle | `get_active_vc_count` | Read-only query |
| identity-oracle | `verify_vc` | Read-only query |
| identity-oracle | `list_issuers` | Read-only query |
| credit-oracle | `update_tx_stats` | Feeder operation; touches only persistent storage |
| credit-oracle | `record_repayment` | Lender operation; touches only persistent storage |
| credit-oracle | `set_vc_count` | Feeder operation (deprecated); touches only persistent storage |
| credit-oracle | `compute_score` | Open-call, no auth; touches only persistent storage (see ADR-001) |
| credit-oracle | `get_score` | Read-only query |
| credit-oracle | `is_stale` | Read-only query |
| credit-oracle | `get_scoring_weights` | Read-only query |
| credit-oracle | `get_pending_weights` | Read-only query |
| credit-oracle | `get_compute_cooldown` | Read-only query |
| credit-oracle | `apply_weights` | Permissionless, but reads/writes instance storage — see [§3.3](#33-apply_weights-edge-case) |
| revocation-registry | `revoke` | Issuer operation; touches only persistent storage |
| revocation-registry | `batch_revoke` | Issuer operation; touches only persistent storage |
| revocation-registry | `is_revoked` | Read-only query |
| revocation-registry | `get_revocation_record` | Read-only query |

### 3.3 `apply_weights` edge case

`apply_weights` reads and writes instance storage (`PendingWeights`, `PendingWeightsEffectiveLedger`, `Config`) but does **not** call `extend_ttl`. This is because `apply_weights` is permissionless — any address may call it once the timelock expires. If it also extended the TTL, a user could repeatedly call it to keep the contract alive without admin involvement, potentially working around an admin's intention to let the contract archive.

However, the implication is: if a weight proposal is made and no admin-gated function is called for ~30 days between the proposal and the timelock expiry, `apply_weights` will panic because instance storage is already archived. The weight proposal is effectively stranded. See [§7](#7-maintaining-liveness) for how to avoid this.

---

## 4. What TTL expiry invalidates

### 4.1 Instance storage expiry → full contract archive

This is the most severe outcome. When instance storage TTL reaches zero:

1. **The contract is archived.** All calls to any function will fail with a storage error.
2. **All data is lost.** Instance storage keys (`Admin`, `Config`, etc.) and **all persistent storage keys** are lost. This includes every score, every VC anchor, every issuer registration.
3. **The contract cannot be recovered.** Even with the admin key, there is no way to un-archive a contract. You must deploy a new instance, re-initialize, and re-register all participants.

**This must never happen in production.** See [§7](#7-maintaining-liveness) for mitigation.

### 4.2 Persistent storage key expiry → per-key data loss

When an individual persistent storage key expires:

- That specific datum is lost (e.g., a single subject's `ScoreRecord` or `TxStats`).
- The contract continues to function for all other keys.
- The next read of the key returns the default value (e.g., `None` for `get_score`, `0` for `get_active_vc_count` on an uninitialized subject).

This is less catastrophic but still problematic: a subject's score silently reverts to 300 if their `ScoreRecord` persistent entry expires, even if the inputs are still valid.

### 4.3 Score staleness (independent of TTL)

Score staleness is a separate concept from storage TTL. The `is_stale(subject, max_age_seconds)` function on credit-oracle checks:

```rust
now.saturating_sub(r.last_updated) > max_age_seconds
```

This compares the current ledger timestamp against `ScoreRecord.last_updated`. A score can be:

- **Stale but stored:** TTL is fine, the data exists, but it was computed long ago. Consumer should prompt `compute_score`.
- **Fresh but expired:** The `ScoreRecord` persistent key TTL has expired, so `get_score` returns `None`. `is_stale` would also return `true` (no record exists), but the underlying issue is storage TTL, not score freshness.

| State | `get_score` returns | `is_stale` returns | Likely cause |
|-------|--------------------|--------------------|--------------|
| Normal | `Some(record)` | `false` | Everything healthy |
| Stale | `Some(record)` | `true` | Score not recomputed recently |
| Expired persistent key | `None` | `true` | Persistent TTL not extended |
| Archived contract | Error | Error | Instance TTL expired |

---

## 5. Related epoch concepts

Beyond storage TTL, the protocol has several ledger-based mechanisms that act like epochs for specific subsystems.

### 5.1 Score compute cooldown

**Storage key:** `ComputeCooldownLedgers` (instance) + `LastComputed(Address)` (persistent)

**Default value:** 1 ledger

**Purpose:** Prevents repeated `compute_score` calls from grinding the same subject's `last_updated` timestamp within the same ledger. This avoids timestamp-based gaming where a caller could repeatedly recompute a score with slightly different inputs and pick the highest result.

**How it works:**

```rust
let current_ledger = env.ledger().sequence();
let cooldown: u32 = storage.get(&DataKey::ComputeCooldownLedgers);

if cooldown > 0 {
    if let Some(last_ledger) = last_computed {
        if current_ledger < last_ledger + cooldown {
            return Err(ComputeCooldownActive);
        }
    }
}
// ... compute score ...
storage.set(&DataKey::LastComputed(subject), &current_ledger);
```

**Admin control:** The admin can change the cooldown via `update_compute_cooldown`. Setting it to `0` disables cooldown entirely.

### 5.2 Weight proposal timelock

**Storage key:** `PendingWeightsEffectiveLedger` (instance)

**Constant:** `TIMELOCK_LEDGERS = 17_280` (~24 hours)

**Purpose:** All weight changes go through a mandatory timelock. Once `propose_weights` is called, the weights cannot be applied until the current ledger sequence exceeds `effective_ledger = proposal_ledger + TIMELOCK_LEDGERS`.

**How it works:**

```rust
// In propose_weights:
let effective_ledger = env.ledger().sequence() + TIMELOCK_LEDGERS;
storage.set(&DataKey::PendingWeights, &weights);
storage.set(&DataKey::PendingWeightsEffectiveLedger, &effective_ledger);

// In apply_weights:
if env.ledger().sequence() < effective_ledger {
    panic!("timelock not expired");
}
```

**Governance bypass:** The admin can bypass the timelock via `update_weights`, which also clears any pending proposal.

### 5.3 Score staleness threshold

The `is_stale` function on credit-oracle accepts a `max_age_seconds` parameter chosen by the consumer. This is not a contract-enforced epoch but rather a consumer-defined policy boundary. See [docs/scoring-spec.md](scoring-spec.md) for recommended thresholds by use case.

---

## 6. Worked examples

### Example 1: Feeder goes offline → contract archived

**Setup:** credit-oracle contract has been live for 6 months. The admin calls `register_feeder` every few days, which extends instance TTL.

**Scenario:** The admin loses their key (or goes on vacation). No admin-gated function is called for 35 days (500,000+ ledgers). Countless `compute_score` and `update_tx_stats` calls are made by users and feeders, but **none of these extend instance TTL**.

**Result:** Instance storage TTL hits zero. The contract is archived. All scores, all user data, all feeder registrations — everything is lost.

**Prevention:** See [§7](#7-maintaining-liveness).

### Example 2: Compute cooldown prevents same-ledger gaming

**Setup:** Default cooldown of 1 ledger. Subject has `vc_count = 2` and `tx_volume_30d = 0`.

**Sequence:**
1. Ledger 1000: Someone calls `compute_score(subject)`. Score = 430. `LastComputed(subject) = 1000`.
2. Ledger 1000 (same ledger): Someone tries again. **Rejected** — `1000 < 1000 + 1` → `ComputeCooldownActive`.
3. Ledger 1001: Someone calls `compute_score(subject)`. Allowed — `1001 >= 1000 + 1`.

This ensures at most one `compute_score` per subject per ledger.

### Example 3: Weight proposal timelock in action

**Setup:** Default weights (40/30/30). Admin proposes new weights (50/25/25) at ledger 50000.

**Sequence:**
1. Ledger 50000: `propose_weights` called. `PendingWeights = {50, 25, 25}`, `EffectiveLedger = 50000 + 17280 = 67280`.
2. Ledger 50000–67279: `apply_weights` **fails** — timelock not yet expired.
3. Ledger 67280: `apply_weights` succeeds. New weights take effect.

If the admin changes their mind before the timelock expires, they can call `update_weights` directly (bypassing the timelock) or `propose_weights` again (overwriting the pending proposal).

### Example 4: `apply_weights` vs instance TTL expiry

**Setup:** Admin proposes new weights at ledger 50000. Then no admin activity for 28 days (483,840 ledgers). The timelock of 17,280 ledgers expires at ledger 67,280, but instance TTL is now critically low.

**Scenario A — TTL still valid:** The admin (or any caller) calls `apply_weights` at ledger 68,000 (before TTL expiry). It succeeds.

**Scenario B — TTL expired:** No one calls any function until ledger 550,000. Instance TTL has expired. `apply_weights` panics with a storage read error because `PendingWeightsEffectiveLedger` no longer exists.

**Moral:** Even a timelocked proposal can become stranded if the contract's instance storage expires before `apply_weights` is called. The admin must ensure at least one admin-gated function is called every ~30 days.

---

## 7. Maintaining liveness

### 7.1 Who can extend TTL

Only admin-gated functions extend instance TTL (see [§3.1](#31-functions-that-extend-instance-ttl)). This means **only the admin** can prevent contract archival. No amount of user activity (scores, VC anchors, revocations) will keep the contract alive.

### 7.2 Monitoring recommendation

Operators should set up monitoring that alerts if no admin-gated transaction has been submitted for any of the three contracts in the last **25 days** (below the 30-day TTL to provide a safety buffer).

A simple health check (e.g., calling `get_scoring_weights` on credit-oracle) can confirm the contract is still alive, but it does **not** extend TTL. The admin must actively call an admin-gated function.

### 7.3 Automated liveness (cron job)

If the admin key is available in a secure signing environment (e.g., a hardware wallet connected to an offline signing server), a cron job can periodically submit a no-effect admin transaction. For example:

- Call `register_feeder` with an already-registered feeder address (idempotent — it just resets the `TrustedFeeder` flag and extends TTL).
- Call `deregister_feeder` + `register_feeder` for the same feeder (two transactions, but extends TTL twice).

However, **be extremely careful with automated admin signing.** An automated admin key is a security risk. Prefer human-driven monitoring with a documented emergency runbook.

### 7.4 Multi-admin support

If a governance contract is deployed in Phase 5, the governance address can also call `propose_weights` (which extends TTL). This distributes liveness responsibility across multiple parties. Until then, only the single admin address can extend TTL.

---

## 8. Summary table

| Mechanism | Scope | Duration | Who extends/resets | Data lost on expiry |
|-----------|-------|----------|--------------------|---------------------|
| Instance storage TTL | Per contract | ~30 days (500K ledgers) | Admin only (via admin-gated calls) | **Everything** — contract archived |
| Persistent storage TTL | Per storage key | Default Soroban value | Not explicitly extended in current code | Single key/value pair |
| Compute cooldown | Per subject per contract | Configurable (default 1 ledger) | N/A — resets each time `compute_score` succeeds | N/A — not a data loss mechanism |
| Weight timelock | Per weight proposal | ~24 hours (17,280 ledgers) | Admin can bypass via `update_weights` | `PendingWeights` if expired before `apply_weights` |
| Score staleness | Per subject | Consumer-defined (seconds) | Fresh `compute_score` call | N/A — `ScoreRecord` still in storage |

---

## 9. Further reading

- [Architecture Overview](architecture.md) — contains the original TTL management section with the pattern used across all contracts
- [Scoring Specification](scoring-spec.md) — defines score staleness, compute cooldown edge cases, and recommended `max_age_seconds` values
- [Upgrade Guide](upgrade-guide.md) — how contract upgrades preserve storage state (TTL is **not** reset on upgrade)
- [Event Indexing Guide](event-indexing.md) — how feeders subscribe to events that may signal when TTL maintenance is needed
- [Soroban documentation — storage](https://developers.stellar.org/docs/smart-contracts/concepts/data-storage)
- [Soroban documentation — TTL and archival](https://developers.stellar.org/docs/smart-contracts/concepts/lifecycle)

