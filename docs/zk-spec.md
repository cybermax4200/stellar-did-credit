# ZK Circuit Specification

## 1. Overview
Circuit for proving score > threshold without revealing exact score. Groth16 on BLS12-381, verified on-chain via Soroban.

## 2. Public Inputs
| Name | Type | Description |
|------|------|-------------|
| threshold | u32 | Minimum score to prove |
| subject | Address | Stellar account bound to proof |
| credit_oracle_id | Address | Source contract |
| score_commitment | BytesN<32> | Poseidon commitment to ScoreRecord (see §4a) |
| snapshot_ledger | u32 | Ledger sequence at computation |
| domain_separator | BytesN<32> | Protocol version binding |
| current_timestamp | u64 | Verifier current timestamp (seconds) |
| max_validity_window | u64 | Maximum allowed score age in seconds |

**Scope note — reconciling with issue #177's literal wording:** the issue text lists "subject DID" and "VC hashes" as example inputs. This spec diverges from both, deliberately:

- **`subject: Address`, not a DID.** Per `did-spec.md`, `did:stellar:<network>:<account_address>` is derived directly and deterministically from the Stellar Address — the DID *is* the Address plus a network prefix, nothing else. Using the native `Address` type avoids encoding a DID string inside the circuit for no added binding strength; the two are 1:1 equivalent identifiers. Flagging this explicitly rather than assuming it's obvious, since a reviewer following the issue text literally could reasonably ask where the DID went.
- **`vc_count`, not VC hashes, in the private witness (§3).** `identity-oracle` does anchor individual VC hashes (`anchor_vc` → `VCRecord`), but `credit-oracle`'s `ScoreRecord` only ever stores the resolved `vc_count` — the scoring formula never sees individual hashes. Committing to `vc_count` alone means the circuit proves the score formula was applied correctly to *whatever count credit-oracle cached*, but does **not** prove those VCs are real, currently anchored, and non-revoked at proof time — it trusts `credit-oracle`'s cross-contract read of `identity-oracle` at `compute_score()` time. This matches the Non-goals already stated in `zk-proof-design.md` ("proving properties of individual VCs... only the aggregated score path") but is called out here explicitly since it's a direct divergence from the issue's literal wording, not an oversight. A stronger v2 design — the circuit including a Merkle-inclusion proof of individual VC hashes against `identity-oracle` state — is tracked as Open research question 5 in `zk-proof-design.md`. **This is a scope decision for the reviewer to confirm, not something implementation should proceed on unconfirmed.**

## 3. Private Witness
| Name | Type | Description |
|------|------|-------------|
| score | u32 | Actual credit score (300-850) |
| vc_count | u32 | Active VC count |
| tx_volume_30d | i128 | 30-day transaction volume (stroops) |
| avg_counterparties | u32 | Average distinct counterparties, 30d (see §4 audit note) |
| repayment_rate | u32 | Basis points (0-10000) |
| last_updated | u64 | Ledger timestamp |
| score_timestamp | u64 | Score generation timestamp |
| vc_weight | u32 | Active VC weight |
| tx_weight | u32 | Active TX weight |
| repay_weight | u32 | Active repayment weight |
| blinding | field element | Random blinding factor |

Note: `tx_volume_30d` is typed `i128` on-chain (`TxStats.volume_30d` / `ScoreRecord.tx_volume_30d`); the circuit range-checks it as non-negative and splits it into limbs for field arithmetic (see zk-proof-design.md, "Field encoding notes").

## 4. Constraints
```
score > threshold
score >= 300 AND score <= 850
vc_score = min(vc_count * 20, 100)
tx_score = min(tx_volume_30d / 100_000_000, 100)
repay_score = repayment_rate / 100
counterparty_bonus = 10 if avg_counterparties >= 10 else 0
composite = (vc_score * vc_weight + (tx_score + counterparty_bonus) * tx_weight + repay_score * repay_weight) / 100
score = clamp(300 + (composite * 550) / 100, 300, 850)
vc_weight + tx_weight + repay_weight == 100
score_commitment == PoseidonCommit(score, vc_count, tx_volume_30d, avg_counterparties, repayment_rate, last_updated, computed_at_ledger, stale, blinding)   -- see §4a
score_timestamp <= current_timestamp
current_timestamp - score_timestamp <= max_validity_window
```

**Audit note — counterparty_bonus binding (fixed in this revision):** an earlier draft left `counterparty_bonus` as a free witness value with nothing tying it to on-chain data. `vc_score`, `tx_score`, and `repay_score` are each fully determined by fields already on the committed `ScoreRecord` (`vc_count`, `tx_volume_30d`, `repayment_rate`), so a prover can't lie about those without breaking the commitment opening. `avg_counterparties`, however, is not a field on `ScoreRecord` — it lives only on `TxStats` in `credit-oracle`'s storage — so nothing constrained `counterparty_bonus`. A dishonest prover could set `avg_counterparties = 10` (or higher) unconditionally and inflate the proven score by up to 3 composite points with no way for the verifier to catch it.

Fix applied above: `avg_counterparties` is now part of both the witness and the `score_commitment` preimage. This means `score_commitment` must cover the full set of `compute_score_pure` inputs, not just the subset `get_score` currently returns. **Open implementation question:** either (a) extend `ScoreRecord` to expose `avg_counterparties` (a contract change requiring a deprecation path per `CONTRIBUTING.md`), or (b) have the prover source `avg_counterparties` from `TxStats` directly and commit to it separately alongside the `ScoreRecord` commitment, with the verifier requiring both. Option (b) avoids a migration and is the recommended default for Phase 4 v1, but needs reviewer sign-off before implementation — tracked as Open research question 11 in `docs/zk-proof-design.md`.

## 4a. Commitment (Poseidon)

`score_commitment` is a Poseidon hash over the BLS12-381 scalar field `Fr`
(issue #1185). It replaces the earlier Pedersen-style linear combination and
the SDK's off-chain SHA-256, so there is now **one** commitment function used
by the circuit, the WASM helper (`compute_score_commitment`) and the SDK.

```
state0 = [ DOMAIN_TAG, score, vc_count, tx_volume_30d, avg_counterparties,
           repayment_rate, last_updated, computed_at_ledger, stale, blinding ]
C      = Poseidon-permutation(state0)[1]
```

| Parameter | Value |
|-----------|-------|
| Field | BLS12-381 scalar field `Fr` (255-bit) |
| State width `t` | 10 (rate 9, capacity 1) |
| S-box | `x^5` (`gcd(5, r-1) = 1`) |
| Full rounds `R_F` | 8 (4 + 4) |
| Partial rounds `R_P` | 60 |
| Round constants / MDS | Grain LFSR generation (Poseidon reference procedure) via `ark_crypto_primitives::sponge::poseidon::find_poseidon_ark_and_mds(255, 9, 8, 60, 0)` |
| Domain tag (capacity element) | ASCII `sdc::score-commit::v2` read as a big-endian integer |
| Output | state element 1, encoded as 32-byte big-endian |
| Mode | fixed length: all 9 inputs fit the rate, exactly one permutation |

All nine inputs are single field elements (every field is `<= 2^64` or a
boolean), so no packing is needed. `R_P = 60` is rounded up from 59, which a
re-implementation of the paper's round-number inequalities gives for
`t = 10`, `alpha = 5`, 128-bit security (with the usual +2 `R_F` / +7.5% `R_P`
margin); it matches the published table row for `t = 10`. Before mainnet,
re-check these numbers against the reference `calc_round_numbers.py`.

**Cost.** Each S-box costs 3 R1CS constraints; the MDS layer and round constants
are linear and free: `3 * (8 * 10 + 60) = 420` at most; the implementation
uses 417. A SHA-256 commitment is on the order of 25,000. The in-circuit
gadget is `commitment::poseidon_commit_gadget`.

**Cross-checks (all in `zk/circuit/src/commitment.rs` tests).** The native
permutation equals arkworks' own `PoseidonSponge` output for the same
parameters; the gadget equals the native hash bit-for-bit on random inputs;
known-answer vectors pin the round constants, the MDS matrix, the permutation
and three commitments; the gadget stays under 600 constraints.

## 4b. Tier-band statement

A second circuit, `ScoreTierCircuit`, proves membership in a score tier rather
than a single threshold (issue #1184). It reuses the same committed score
record and score-formula constraints, so one `score_commitment` serves both
statements, and its public inputs have the same eight-element layout
(including `current_timestamp` and `max_validity_window`) with `tier_level` in
place of `threshold`.

| `tier_level` | Name | Band (inclusive lower, exclusive upper) |
|--------------|------|-----------------------------------------|
| 1 | Bronze | `600 <= score < 700` |
| 2 | Silver | `700 <= score < 800` |
| 3 | Gold | `800 <= score <= 850` (upper bound 851) |

In-circuit: `(t-1)(t-2)(t-3) = 0`; one-hot selectors (Lagrange basis on
`{1,2,3}`) pick the band bounds; then `score >= lower` and `score < upper` are
range-checked. A score in the Gold band can therefore only be proven as
`tier_level = 3`. The verifier learns the tier and nothing finer.

The two circuits have different verification keys, and `score-range-verifier`
exposes `verify_score_tier` / `verify_tier_and_consume`, which return the
verified tier (`1..=3`, or `0` when not verified). Scores below 600 have no tier.

## 5. Gate Count
Approximately 2,400 gates (32-bit comparisons including the `avg_counterparties >= 10` threshold check, integer divisions, weighted sum, Poseidon commitment of 8 scalars + blinding (417 constraints, §4a), 7 range checks). Revised upward slightly from the original 2,200 estimate to account for the added `avg_counterparties` binding — see §4. Treat as an order-of-magnitude estimate until a reference circuit is implemented (roadmap step 1 in zk-proof-design.md).

## 6. Soroban Verifier
env.crypto().groth16_verify(&VERIFICATION_KEY, &public_inputs, &proof) with public inputs: threshold, subject, credit_oracle_id, score_commitment, snapshot_ledger, domain_separator, current_timestamp, max_validity_window.

Tier proofs use the same layout with `tier_level` first and a separate embedded VK:
`verify_score_tier(proof, ScoreTierPublicInputs) -> u32` and
`verify_tier_and_consume(consumer, proof, ScoreTierPublicInputs, nonce) -> Result<u32, _>`.

The embedded VKs are development keys (fixed-seed setup), regenerated with
`cargo run -p zk-circuit --bin generate_vk -- threshold` and `... -- tier`.
They must be replaced by trusted-setup output before mainnet.

## 7. Proof System
Groth16 on BLS12-381 via CAP-0059. WASM prover via arkworks-rs or snarkjs. Proof size ~192 bytes.

## 8. Prover Flow
Subject calls compute_score() -> reads ScoreRecord + Weights from chain -> generates witness with random blinding -> runs Groth16 prover in browser WASM -> submits proof to verifier contract.

In browsers the prover (1-2 s) must not run on the UI thread. `@stellar-did-credit/zk-wasm/worker` wraps it in a Web Worker: `generateScoreProofAsync`, `generateTierProofAsync` and `computeScoreCommitmentAsync` return Promises, with typed requests, input validation, timeout and abort support. See `packages/zk-wasm/examples/zk-worker-demo`.

## 9. Versioning
Circuit version 2 (contract `CIRCUIT_VERSION = 2`): Poseidon commitment (§4a) and the tier circuit (§4b). Domain strings: `stellar-did-credit::score-gt-threshold::v2` (threshold), `stellar-did-credit::score-tier-band::v2` (tier). Version 1 used a Pedersen-style commitment and is not compatible: proofs, VKs and commitments must all be regenerated.

## 10. References
CAP-0059: BLS12-381 host functions, docs/scoring-spec.md, docs/zk-proof-design.md