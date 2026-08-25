# Scoring Specification

The credit-oracle contract computes a score in the range **`MIN_SCORE` (300)–`MAX_SCORE` (850)**, matching the conventional credit score scale. The formula is deterministic, fully on-chain, and uses only data that has been explicitly submitted by trusted parties (feeders and lenders).

---

## Inputs

| Input                           | Source                        | Storage key                           |
| ------------------------------- | ----------------------------- | ------------------------------------- |
| `vc_count`                      | Feeder via `set_vc_count`     | `VcCount(subject)`                    |
| `volume_30d`                    | Feeder via `update_tx_stats`  | `TxStats(subject).volume_30d`         |
| `avg_counterparties`            | Feeder via `update_tx_stats`  | `TxStats(subject).avg_counterparties` |
| `on_time_count` / `total_count` | Lender via `record_repayment` | `RepaymentRecord(subject)`            |
| `total_repaid`                  | Lender via `record_repayment` | `RepaymentRecord(subject)`            |

All inputs default to zero if never set. A subject with no history always scores exactly 300.

---

## Formula — step by step

### Step 1: Component scores (0–100 each)

**VC score** — rewards having verified credentials, capped at 5 VCs:

```
vc_score = min(vc_count × 20, 100)
```

**Transaction score** — rewards on-chain transaction volume over the last 30 days and network diversity (unique counterparties). It has two sub-components:

- **Volume sub-score**: 1 point per 100,000,000 stroops (1 XLM), capped at 80.
- **Counterparty bonus**: 1 point per 5 unique counterparties (`avg_counterparties`), capped at 20.

```
volume_score      = min(volume_30d ÷ 100_000_000, 80)    [integer division]
counterparty_bonus = min(avg_counterparties ÷ 5, 20)      [integer division]
tx_score          = min(volume_score + counterparty_bonus, 100)
```

> **Important:** The counterparty bonus is a sub-component of `tx_score`. It is multiplied by `tx_weight` in the composite calculation (Step 2). To prevent degenerate scoring (such as setting `tx_weight` to 0 which would silently eliminate the counterparty bonus), each weight component must satisfy `MIN_COMPONENT_WEIGHT` = 10 (10%). Proposals or weight changes setting any component weight below 10 are rejected with `InvalidWeights`.

**Repayment score** — rewards both on-time repayment rate and repayment volume.
The volume sub-score gives 1 point per 100,000,000 stroops (1 XLM), capped at
100, so repayment size can influence scoring without making the repayment
component unbounded:

```
repayment_rate_score   = 0                                                if total_count = 0
repayment_rate_score   = (on_time_count × 10000 ÷ total_count) ÷ 100      [integer division]
repayment_volume_score = min(total_repaid ÷ 100_000_000, 100)             [integer division]
repay_score            = (repayment_rate_score + repayment_volume_score) ÷ 2
```

This gives a value of 0–100 where repayment timing and repayment amount each
contribute half of the repayment component. Negative repayment amounts are
clamped to zero for scoring.

### Step 2: Weighted composite (0–100)

Default weights are **vc: 40, tx: 30, repayment: 30** (must sum to 100, each component ≥ `MIN_COMPONENT_WEIGHT` = 10, configurable via governance proposals):

```
composite = (vc_score × vc_weight + tx_score × tx_weight + repay_score × repayment_weight) ÷ 100
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

| Input           | Value     |
| --------------- | --------- |
| vc_count        | 0         |
| volume_30d      | 0 stroops |
| on_time / total | 0 / 0     |

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

### Example 2: 3 VCs, moderate volume, 80% repayment → score ~569

| Input           | Value                          |
| --------------- | ------------------------------ |
| vc_count        | 3                              |
| volume_30d      | 3,000,000,000 stroops (30 XLM) |
| on_time / total | 8 / 10                         |
| total_repaid    | 3,000,000,000 stroops (30 XLM) |

**Calculation:**

```
vc_score    = min(3 × 20, 100) = min(60, 100) = 60
tx_score    = min(3_000_000_000 ÷ 100_000_000, 100) = min(30, 100) = 30
repayment_rate_score   = (8 × 10000 ÷ 10) ÷ 100 = 8000 ÷ 100 = 80
repayment_volume_score = min(3_000_000_000 ÷ 100_000_000, 100) = 30
repay_score            = (80 + 30) ÷ 2 = 55

composite = (60×40 + 30×30 + 55×30) ÷ 100
          = (2400 + 900 + 1650) ÷ 100
          = 4950 ÷ 100
          = 49

score = clamp(300 + 49×550÷100, 300, 850)
      = clamp(300 + 26950÷100, 300, 850)
      = clamp(300 + 269, 300, 850)
      = 569
```

**Result: 569** — a mid-range score reflecting real but moderate credit activity and repayment volume.

---

### Example 3: 5 VCs, high volume, 100% repayment → score ~817

| Input           | Value                            |
| --------------- | -------------------------------- |
| vc_count        | 5                                |
| volume_30d      | 8,000,000,000 stroops (80 XLM)   |
| on_time / total | 20 / 20                          |
| total_repaid    | 10,000,000,000 stroops (100 XLM) |

**Calculation:**

```
vc_score    = min(5 × 20, 100) = min(100, 100) = 100
tx_score    = min(8_000_000_000 ÷ 100_000_000, 100) = min(80, 100) = 80
repayment_rate_score   = (20 × 10000 ÷ 20) ÷ 100 = 10000 ÷ 100 = 100
repayment_volume_score = min(10_000_000_000 ÷ 100_000_000, 100) = 100
repay_score            = (100 + 100) ÷ 2 = 100

composite = (100×40 + 80×30 + 100×30) ÷ 100
          = (4000 + 2400 + 3000) ÷ 100
          = 9400 ÷ 100
          = 94

score = clamp(300 + 94×550÷100, 300, 850)
      = clamp(300 + 51700÷100, 300, 850)
      = clamp(300 + 517, 300, 850)
      = 817
```

**Result: 817** — near the ceiling. Reaching 850 requires a perfect composite of 100, which needs ≥5 VCs, enough transaction volume and counterparty diversity to reach a `tx_score` of 100, 100% repayment rate, and ≥100 XLM total repayment volume.

---

## Edge cases

### Stale score (`last_updated` more than 30 days ago, or `computed_at_ledger` far behind)

The contract now includes a `stale` flag in `ScoreRecord` that is computed at read time by `get_score`.
A score is considered stale when the difference between the current ledger sequence and `computed_at_ledger`
exceeds `STALE_LEDGER_AGE` (86,400 ledgers, approximately 30 days at 5-second ledgers).

The `ScoreRecord` includes three fields for freshness assessment:

- **`last_updated`** — a ledger timestamp (Unix seconds) that consumers can compare against wall-clock time.
- **`computed_at_ledger`** — the ledger sequence number when the score was last computed. Compare this against the current ledger sequence for a deterministic, clock-independent freshness check.
- **`stale`** — a boolean flag computed at read time by `get_score`. `true` means the score is older than `STALE_LEDGER_AGE` ledgers and should not be trusted for lending decisions without recomputation.

Consumers should treat a score with `stale = true` as untrustworthy and prompt the subject or feeder to call `compute_score` again.
Lenders should check `stale` before making lending decisions, and may also apply domain-specific policies (e.g., reject scores where `stale = true` or where `last_updated` exceeds a custom threshold).

### All VCs revoked

If a subject's VCs are all revoked in identity-oracle, `is_verified` returns `false`. However, the credit-oracle's `VcCount` cache is not automatically updated — it reflects whatever the feeder last submitted via `set_vc_count`.

**Implication:** a lender should always check `is_verified` on identity-oracle independently of the credit score. A high score with `is_verified = false` indicates the feeder has not yet synced the revocation.

In the future cross-contract version, `compute_score` will call `get_active_vc_count` directly, eliminating this lag.

### Feeder not updated (inputs never set)

If `set_vc_count` and `update_tx_stats` have never been called for a subject, both default to zero. The score will be driven entirely by repayment history (weight 30), with a maximum possible score of:

```
composite = (0×40 + 0×30 + 100×30) ÷ 100 = 30
score = 300 + 30×550÷100 = 300 + 165 = 465
```

A subject with perfect repayment history and capped repayment volume but no feeder data is capped at **465**. This is intentional — the protocol requires active data submission to unlock higher scores.

### Integer division truncation

All arithmetic uses integer (floor) division, matching Soroban's `u32`/`i128` semantics. This means:

- A repayment rate of 9/10 = 90% gives `repayment_rate_score = 90`, not 90.0
- A volume of 150,000,000 stroops gives `tx_score = 1`, not 1.5
- A total repayment volume of 150,000,000 stroops gives `repayment_volume_score = 1`, not 1.5
- A composite of 49.5 gives `score = 300 + 49×550÷100 = 569`, not 572

Consumers should be aware that two subjects with slightly different inputs may receive the same score due to truncation.
