# Reentrancy Analysis — Cross-Contract Call Graph

Closes #272.

## Audited call graph

```
Governance.execute()
  └─► CreditOracle.propose_weights()          [one-way, no callback]

Governance.accept_oracle_admin()
  └─► CreditOracle.accept_admin()             [one-way, no callback]

CreditOracle.compute_score()
  └─► IdentityOracle.get_active_vc_count()    [one-way read, no callback]

IdentityOracle.set_revocation_registry()
  └─► RevocationRegistry.set_identity_oracle() [one-way setup, no callback]

IdentityOracle.is_record_revoked()            [called from anchor_vc, is_verified, verify_vc, get_vc_details]
  └─► RevocationRegistry.is_revoked()         [read-only, no callback]

RevocationRegistry.revoke()                   ← GUARDED
  └─► IdentityOracle.mark_vc_revoked()        [outbound write]
        └─► is_record_revoked()
              └─► RevocationRegistry.is_revoked()  [read-only callback — circular]
```

## Why reentrancy is currently non-exploitable

`RevocationRegistry.is_revoked()` is a pure read: it reads one persistent
storage entry and returns a `bool`. It does not write state, emit events, or
make further outbound calls. Therefore the circular path

```
revoke → mark_vc_revoked → is_revoked
```

cannot corrupt state today.

## Why the guard is still justified

1. **Upgrade risk.** Either contract can be upgraded in-place via
   `deployer().update_current_contract_wasm()`. A future version of
   `is_revoked` or `mark_vc_revoked` could write state or call back into
   `revoke`, turning the currently-safe circular path into a real reentrancy
   vector. The guard costs one instance-storage read on the hot path and
   eliminates the entire class of future risk.

2. **Circular dependency is confirmed.** The call graph above shows a real
   A → B → A cycle. Even if today's B leg is read-only, the structural
   precondition for reentrancy is present.

3. **Minimal overhead.** `enter_guard` / `exit_guard` each perform a single
   instance-storage operation. Instance storage is the cheapest Soroban
   storage tier. The guard adds no persistent storage entries and leaves no
   stale state after a successful or failed call.

## Guard implementation

`RevocationKey::ReentrancyLock` is an instance-storage boolean that is:

- **set** by `enter_guard` immediately before `invoke_contract`
- **cleared** by `exit_guard` immediately after `invoke_contract` returns
- **absent** at all other times (Soroban `remove` on a missing key is a no-op)

If `enter_guard` finds the key already present it returns
`RevocationRegistryError::ReentrancyDetected` (error code 7), which causes
the entire transaction to revert. Because Soroban transactions are atomic,
the lock is also rolled back, leaving no stale state.

Only `revoke()` is guarded. `batch_revoke()` does not call identity-oracle
and therefore does not need a guard.

## Trust assumptions

| Pair | Trust level | Rationale |
|---|---|---|
| Governance → CreditOracle | Governance is the oracle admin | Governance contract address is stored in CreditOracle at init time |
| CreditOracle → IdentityOracle | Read-only query | Admin-configured; IdentityOracle cannot call back into CreditOracle |
| IdentityOracle → RevocationRegistry | Read-only query | Admin-configured; `is_revoked` is a pure read |
| RevocationRegistry → IdentityOracle | Write callback | Admin-configured via `set_identity_oracle`; guarded by `ReentrancyLock` |

## Circular dependency analysis

Only one circular dependency exists in the current codebase:

```
RevocationRegistry ↔ IdentityOracle
```

No other pair of contracts calls each other. Governance, CreditOracle, and
IdentityOracle form a directed acyclic graph when the RevocationRegistry
callback is excluded.

## Upgrade considerations

- If `RevocationRegistry.revoke()` is upgraded to make additional outbound
  calls, each new `invoke_contract` site must be evaluated for reentrancy and
  wrapped with `enter_guard` / `exit_guard` if a callback path back into
  `revoke` is possible.
- If `IdentityOracle.mark_vc_revoked()` is upgraded to call back into
  `RevocationRegistry` with a state-changing function, the guard will
  correctly block the reentrant call and revert the transaction.
- The `ReentrancyLock` key is part of the public `RevocationKey` enum and
  must not be repurposed or removed in future upgrades.
