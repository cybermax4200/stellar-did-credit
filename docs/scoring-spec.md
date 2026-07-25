# Scoring Specification

The credit-oracle contract computes a score in the range **`MIN_SCORE` (300)–`MAX_SCORE` (850)**, matching the conventional credit score scale. The formula is deterministic, fully on-chain, and uses only data that has been explicitly submitted by trusted parties (feeders and lenders).

---

## Inputs

| Input                           | Source                        | Storage key                      |
| ------------------------------- | ----------------------------- | -------------------------------- |
| Active VC records               | identity-oracle via `get_vc_details` | cross-contract (when configured) |
| Issuer tier (`issuer_tier_bps`) | identity-oracle admin         | `IssuerTier(issuer)`             |
| Credential type weight          | credit-oracle admin           | `CredentialTypeWeight(type)`     |
| `vc_count` (legacy fallback)    | Feeder via `set_vc_count`     | `VcCount(subject)`               |
| `volume_30d`                    | Feeder via `update_tx_stats`  | `TxStats(subject).volume_30d`      |
| `avg_counterparties`            | Feeder via `update_tx_stats`  | `TxStats(subject).avg_counterparties` |
| `on_time_count` / `total_count` | Lender via `record_repayment` | `RepaymentRecord(subject)`         |

All inputs default to zero if never set. A subject with no history always scores exactly 300.

---

## Formula — step by step

### Step 1: Component scores (0–100 each)

**VC score** — rewards verified credentials with issuer-trust and type weighting (prototype). When identity-oracle is linked via `set_identity_oracle`, each active VC contributes weighted points; otherwise the legacy count formula applies.

**Weighted path** (identity-oracle configured):

```
credential_points(vc) = base_points × issuer_tier_bps × type_weight_bps ÷ 10_000
vc_score              = min( Σ credential_points(active_vc), 100 )
```

Defaults: `base_points = 20`, `issuer_tier_bps = 100` (1×), `type_weight_bps = 100` (1×, type `generic`).

Admin configuration:

- identity-oracle: `set_issuer_tier(admin, issuer, weight_bps)` — e.g. 200 = 2× issuer trust
- credit-oracle: `set_credential_type_weight(admin, credential_type, weight_bps)` — e.g. `kyc` at 150 = 1.5×

See [vc-weighting-design.md](vc-weighting-design.md) for recency decay (future) and worked tier/type examples.

**Legacy fallback** (no identity-oracle link, or feeder cache only):

```
vc_score = min(vc_count × 20, 100)
```

**Transaction score** — rewards on-chain transaction volume over the last 30 days.
One unit of score per 100,000,000 stroops (1 XLM), capped at 100 XLM:

```
tx_score = min(volume_30d ÷ 100_000_000, 100)   [integer division]
```

**Repayment score** — rewards on-time repayment rate as a percentage:

```
repay_score = 0                                   if total_count = 0
repay_score = (on_time_count × 10000 ÷ total_count) ÷ 100   [integer division]
```

This gives a value of 0–100 representing the repayment percentage (e.g. 80% on-time → 80).

**Counterparty bonus** — rewards transaction diversity with many distinct counterparties:

```
counterparty_bonus = 10   if avg_counterparties >= 10
counterparty_bonus = 0    otherwise
```

This bonus is added to the transaction component, weighted by `tx_weight`.

### Step 2: Weighted composite (0–100)

Default weights are **vc: 40, tx: 30, repayment: 30** (must sum to 100, configurable by admin):

```
composite = (vc_score × vc_weight + (tx_score + counterparty_bonus) × tx_weight + repay_score × repayment_weight) ÷ 100
```

### Step 3: Final score (300–850)

The composite is mapped onto the 300–850 range and clamped:

```
score = clamp(MIN_SCORE + composite × 550 ÷ 100, MIN_SCORE, MAX_SCORE)
```

The 550-point spread means a perfect composite of 100 yields `MIN_SCORE` + 550 = `MAX_SCORE`.

---

## Worked examples

All examples use default weights: vc=40, tx=30, repayment=30.

---

### Example 1: New user, no history → score 300

| Input              | Value     |
| ------------------ | --------- |
| vc_count           | 0         |
| volume_30d         | 0 stroops |
| avg_counterparties | 0         |
| on_time / total    | 0 / 0     |

**Calculation:**

```
vc_score    = min(0 × 20, 100)  = 0
tx_score    = min(0 ÷ 100_000_000, 100) = 0
repay_score = 0  (no repayment history)

composite = (0×40 + 0×30 + 0×30) ÷ 100 = 0

score = clamp(MIN_SCORE + 0×550÷100, MIN_SCORE, MAX_SCORE) = MIN_SCORE
```

**Result: `MIN_SCORE` (300)** — the floor. Every new address starts here.

---

### Example 2: Early stage -- 1 VC, 5 XLM, 70% repayment -> score 465

This is the "Early stage" profile from the README scoring table.
The trace below confirms the score using the exact integer arithmetic
executed by compute_score_pure in the contract.

| Input              | Value                        | Notes                             |
| ------------------ | ---------------------------- | --------------------------------- |
| vc_count           | 1                            |                                   |
| volume_30d         | 500,000,000 stroops (5 XLM)  |                                   |
| avg_counterparties | 0                            | no counterparty bonus             |
| on_time / total    | 7 / 10                       | 70% repayment as raw u32 counters |

> **Integer division note:** on_time_count and total_count are raw u32 counters,
> not a pre-computed percentage. '70% repayment' maps to any (on_time, total) pair
> whose integer division yields 70 -- e.g. (7,10), (14,20), (70,100).
> All give repay_score = 70. This example uses (7,10) as the minimal case.

**Calculation:**

```
vc_score    = min(1 x 20, 100) = min(20, 100) = 20

tx_score    = min(500_000_000 / 100_000_000, 100)   [integer division]
            = min(5, 100) = 5

repay_score = (7 x 10000 / 10) / 100               [integer division throughout]
            = 70000 / 10 / 100
            = 7000 / 100
            = 70

counterparty_bonus = 0  (avg_counterparties < 10)

composite = (20 x 40 + (5 + 0) x 30 + 70 x 30) / 100
          = (800 + 150 + 2100) / 100
          = 3050 / 100
          = 30   <- truncates: 3050 / 100 = 30.5, Rust u32 floors to 30

score = clamp(300 + 30 x 550 / 100, 300, 850)
      = clamp(300 + 16500 / 100, 300, 850)
      = clamp(300 + 165, 300, 850)
      = 465
```

**Result: 465** -- confirmed correct. The README table value is accurate.

The critical step is the composite truncation: 3050 / 100 = 30.5 in real arithmetic,
but Rust u32 integer division floors it to 30, producing a final score of 465 not 467.

---

### Example 3: 3 VCs, moderate volume, 80% repayment, diverse counterparties → score 630

| Input              | Value                          |
| ------------------ | ------------------------------ |
| vc_count           | 3                              |
| volume_30d         | 3,000,000,000 stroops (30 XLM) |
| avg_counterparties | 12 (diverse)                   |
| on_time / total    | 8 / 10                         |

**Calculation:**

```
vc_score    = min(3 × 20, 100) = min(60, 100) = 60
tx_score    = min(3_000_000_000 ÷ 100_000_000, 100) = min(30, 100) = 30
repay_score = (8 × 10000 ÷ 10) ÷ 100 = 8000 ÷ 100 = 80
counterparty_bonus = 10   (avg_counterparties >= 10)

composite = (60×40 + (30+10)×30 + 80×30) ÷ 100
          = (2400 + 1200 + 2400) ÷ 100
          = 6000 ÷ 100
          = 60

score = clamp(300 + 60×550÷100, 300, 850)
      = clamp(300 + 33000÷100, 300, 850)
      = clamp(300 + 330, 300, 850)
      = 630
```

**Result: 630** — the counterparty bonus adds 17 points compared to the same profile without diverse counterparties (the bonus contribution of 10 × 30 ÷ 100 = 3 points to composite, yielding 30 + 3 = 33 extra base points before the final mapping).

---

### Example 4: 5 VCs, high volume, 100% repayment, diverse counterparties → score 833

| Input              | Value                          |
| ------------------ | ------------------------------ |
| vc_count           | 5                              |
| volume_30d         | 8,000,000,000 stroops (80 XLM) |
| avg_counterparties | 15 (diverse)                   |
| on_time / total    | 20 / 20                        |

**Calculation:**

```
vc_score    = min(5 × 20, 100) = min(100, 100) = 100
tx_score    = min(8_000_000_000 ÷ 100_000_000, 100) = min(80, 100) = 80
repay_score = (20 × 10000 ÷ 20) ÷ 100 = 10000 ÷ 100 = 100
counterparty_bonus = 10   (avg_counterparties >= 10)

composite = (100×40 + (80+10)×30 + 100×30) ÷ 100
          = (4000 + 2700 + 3000) ÷ 100
          = 9700 ÷ 100
          = 97

score = clamp(300 + 97×550÷100, 300, 850)
      = clamp(300 + 53350÷100, 300, 850)
      = clamp(300 + 533, 300, 850)
      = 833
```

**Result: 833** — near the ceiling. Reaching 850 requires a perfect composite of 100, which needs ≥5 VCs, ≥100 XLM volume, 100% repayment rate, and diverse counterparties.

---

### Example 5: 5 VCs, 100 XLM volume, 100% repayment, no counterparty bonus → score 850 (MAX_SCORE)

| Input              | Value                              |
| ------------------ | ---------------------------------- |
| vc_count           | 5                                  |
| volume_30d         | 10,000,000,000 stroops (100 XLM)   |
| avg_counterparties | 0 (no bonus)                       |
| on_time / total    | 100 / 100                          |

**Calculation:**

```
vc_score    = min(5 × 20, 100) = min(100, 100) = 100
tx_score    = min(10_000_000_000 ÷ 100_000_000, 100) = min(100, 100) = 100
repay_score = (100 × 10000 ÷ 100) ÷ 100 = 10000 ÷ 100 = 100
counterparty_bonus = 0   (avg_counterparties < 10)

composite = (100×40 + (100+0)×30 + 100×30) ÷ 100
          = (4000 + 3000 + 3000) ÷ 100
          = 10000 ÷ 100
          = 100

score = clamp(300 + 100×550÷100, 300, 850)
      = clamp(300 + 550, 300, 850)
      = 850
```

**Result: `MAX_SCORE` (850)** — the ceiling. Each sub-score is at its maximum (100), producing a composite of exactly 100 and the highest achievable score. Verified by `test_exceptional_score_equals_850` in the credit-oracle test suite.

---

## Edge cases

### Stale score (`last_updated` more than `max_age_seconds` ago)

The contract does not enforce score freshness. `get_score` returns whatever was last computed, regardless of age. Consumers should call `is_stale(subject, max_age_seconds)` to check whether the stored score is outdated.

**Recommended `max_age_seconds` values:**

| Use case                         | Max age     | Seconds         | Rationale                                                  |
| -------------------------------- | ----------- | --------------- | ---------------------------------------------------------- |
| General lending (default)        | 30 days     | 2,592,000       | Balances freshness against compute cost; matches standard practice |
| High-frequency micro-lending     | 7 days      | 604,800         | Frequent small loans need more current data                |
| Collateralized lending           | 90 days     | 7,776,000       | Lower risk tolerance for staleness; collateral backs the loan |
| One-shot / recovery check        | 1 day       | 86,400          | Quick health check before a time-sensitive decision        |

When `is_stale` returns `true`, the caller should prompt the subject or feeder to call `compute_score` before relying on the score.

The feeder is responsible for keeping `TxStats` current. In the legacy cached path, the feeder also needs to keep `VcCount` current by calling `set_vc_count`; otherwise the score can drift from reality because the credit-oracle will continue using the last cached value. If the feeder stops updating, the score will not error — it will simply reflect stale inputs.

When an `IdentityOracleId` is configured, `compute_score` reads the active VC count directly from identity-oracle instead of relying on the cached `VcCount`. That cross-contract path removes revocation-related staleness because the score is recomputed from the live identity-oracle state rather than the feeder’s last submission.

### Open-call recomputation cooldown

`compute_score` remains authorization-free, but successful calls are rate-limited per subject by the configured `ComputeCooldownLedgers` value. The default is one ledger, so a subject can be recomputed again on the next ledger but not repeatedly in the same ledger.

The contract stores the last successful computation ledger under `LastComputed(Address)`. Admin/governance can update the interval with `update_compute_cooldown`; setting it to `0` disables the cooldown.

### All VCs revoked

If a subject's VCs are all revoked in identity-oracle, `is_verified` returns `false`.

- In the legacy cached path, the credit-oracle's `VcCount` cache is not automatically updated and still reflects whatever the feeder last submitted via `set_vc_count`. A high score with `is_verified = false` can therefore indicate that the feeder has not yet synced the revocation into the oracle cache.
- When an `IdentityOracleId` is configured, `compute_score` reads the active VC count directly from identity-oracle via `get_active_vc_count`. That cross-contract path eliminates revocation-related staleness, so the score reflects the current live VC state rather than the feeder's last cached submission.

**Implication:** a lender should always check `is_verified` on identity-oracle independently of the credit score, regardless of which VC-count path is in use.

### Feeder not updated (inputs never set)

If `set_vc_count` and `update_tx_stats` have never been called for a subject, both default to zero in the legacy cached path. The score will be driven entirely by repayment history (weight 30), with a maximum possible score of:

```
composite = (0×40 + 0×30 + 100×30) ÷ 100 = 30
score = 300 + 30×550÷100 = 300 + 165 = 465
```

A subject with perfect repayment history but no feeder data is capped at **465**. This is intentional — the protocol requires active data submission to unlock higher scores.

When the cross-contract path is configured, `VcCount` no longer depends on `set_vc_count` at all; it is fetched from identity-oracle during `compute_score`. In that mode, missing feeder submissions only affect the transaction-history portion of the score, while the VC component remains tied to the live identity-oracle state.

### Integer division truncation

All arithmetic uses integer (floor) division, matching Soroban's `u32`/`i128` semantics. This means:

- A repayment rate of 9/10 = 90% gives `repay_score = 90`, not 90.0
- A volume of 150,000,000 stroops gives `tx_score = 1`, not 1.5
- A composite of 57.3 gives `score = 300 + 57×550÷100 = 613`, not 614

Consumers should be aware that two subjects with slightly different inputs may receive the same score due to truncation.
