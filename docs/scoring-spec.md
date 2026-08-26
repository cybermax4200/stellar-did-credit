# Scoring Specification & Freshness Enforcement

## Freshness and State Synchronization
To prevent lenders from evaluating scores derived from outdated credential states, the `credit-oracle` tracks state updates via the `identity-oracle` contract.

- **Last Identity State Change Tracking**: Whenever a subject's verifiable credentials are anchored or revoked, the `identity-oracle` records the current ledger sequence via `get_last_state_change_ledger(subject)`.
- **Dynamic Staleness Check**: When `get_score(subject)` is called on the `credit-oracle`, it performs a cross-contract lookup to compare `ScoreRecord.computed_at_ledger` against the subject's latest identity state change ledger. If `computed_at_ledger < last_state_change`, the returned `ScoreRecord` sets `stale: true`.
