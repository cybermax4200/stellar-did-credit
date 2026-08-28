# ADR-001: Binding `avg_counterparties` in the ZK commitment

**Status:** Accepted
**Date:** 2025
**Deciders:** Phase 4 contributors
**Related:** `docs/zk-proof-design.md` Open research question #11

## Context

The scoring formula's `counterparty_bonus` term depends on `avg_counterparties`,
which lives on `TxStats` (not on `ScoreRecord`). The `ScoreRecord` returned by
`get_score` does not expose `avg_counterparties`. Without binding it, a prover
could set `avg_counterparties >= 10` unconditionally and claim a
`counterparty_bonus` the on-chain data doesn't support, inflating the proven
score by up to 3 composite points undetected.

## Decision

**Extend the Pedersen commitment preimage to include `avg_counterparties`** as
one of the committed fields, rather than adding a separate `TxStats`
commitment.

The commitment preimage is:

```
C = score·G0 + vc_count·G1 + tx_volume_30d·G2 + avg_counterparties·G3
  + repayment_rate·G4 + last_updated·G5 + computed_at_ledger·G6
  + stale·G7 + blinding·H
```

## Rationale

- **Single commitment to manage.** A separate `TxStats` commitment would require
  the verifier to check two commitments and the prover to manage two blinding
  factors, doubling the surface for misbinding and increasing proof complexity.
- **No contract migration required for the circuit.** The prover reads
  `avg_counterparties` from `TxStats` (already on-chain) and includes it in the
  witness. The circuit binds it to the commitment. The verifier contract only
  needs to check the single commitment against the public input.
- **Soundness restored.** The `counterparty_bonus` term is now pinned to a
  committed value, closing the inflation vector described in the soundness note.

## Consequences

- The prover SDK must read `avg_counterparties` from `TxStats` (via
  `get_tx_stats`) in addition to `ScoreRecord`.
- The circuit's commitment has 8 committed fields (was 7 without
  `avg_counterparties`).
- A future contract change could add `avg_counterparties` to `ScoreRecord` for
  convenience, but it is **not required** for Phase 4 v1.

## Alternatives considered

1. **Extend `ScoreRecord` with `avg_counterparties`** — requires a contract
   migration and deprecation path; deferred to a later phase.
2. **Separate `TxStats` commitment** — adds a second commitment to manage and
   verify; rejected for Phase 4 v1.
