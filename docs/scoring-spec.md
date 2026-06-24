# Scoring Specification

The credit-oracle contract computes a score in the range **300–850**, matching the conventional credit score scale. The formula is deterministic, fully on-chain, and uses only data that has been explicitly submitted by trusted parties (feeders and lenders).

---

## Inputs

| Input                           | Source                        | Storage key                   |
| ------------------------------- | ----------------------------- | ----------------------------- |
| `vc_count`                      | Feeder via `set_vc_count`     | `VcCount(subject)`            |
| `volume_30d`                    | Feeder via `update_tx_stats`  | `TxStats(subject).volume_30d` |
| `on_time_count` / `total_count` | Lender via `record_repayment` | `RepaymentRecord(subject)`    |

All inputs default to zero if never set. A subject with no history always scores exactly 300.

---

## Formula — step by step

### Step 1: Component scores (0–100 each)

**VC score** — rewards having verified credentials, capped at 5 VCs:

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

### Step 2: Weighted composite (0–100)

Default weights are **vc: 40, tx: 30, repayment: 30** (must sum to 100, configurable by admin):

```
composite = (vc_score × vc_weight + tx_score × tx_weight + repay_score × repayment_weight) ÷ 100
```

### Step 3: Final score (300–850)

The composite is mapped onto the 300–850 range and clamped:

```
score = clamp(300 + composite × 550 ÷ 100, 300, 850)
```

The 550-point spread means a perfect composite of 100 yields 300 + 550 = 850.

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

score = clamp(300 + 0×550÷100, 300, 850) = 300
```

**Result: 300** — the floor. Every new address starts here.

---

### Example 2: 3 VCs, moderate volume, 80% repayment → score ~613

| Input           | Value                          |
| --------------- | ------------------------------ |
| vc_count        | 3                              |
| volume_30d      | 3,000,000,000 stroops (30 XLM) |
| on_time / total | 8 / 10                         |

**Calculation:**

```
vc_score    = min(3 × 20, 100) = min(60, 100) = 60
tx_score    = min(3_000_000_000 ÷ 100_000_000, 100) = min(30, 100) = 30
repay_score = (8 × 10000 ÷ 10) ÷ 100 = 8000 ÷ 100 = 80

composite = (60×40 + 30×30 + 80×30) ÷ 100
          = (2400 + 900 + 2400) ÷ 100
          = 5700 ÷ 100
          = 57

score = clamp(300 + 57×550÷100, 300, 850)
      = clamp(300 + 31350÷100, 300, 850)
      = clamp(300 + 313, 300, 850)
      = 613
```

**Result: 613** — a solid mid-range score reflecting real but moderate credit activity.

---

### Example 3: 5 VCs, high volume, 100% repayment → score ~817

| Input           | Value                          |
| --------------- | ------------------------------ |
| vc_count        | 5                              |
| volume_30d      | 8,000,000,000 stroops (80 XLM) |
| on_time / total | 20 / 20                        |

**Calculation:**

```
vc_score    = min(5 × 20, 100) = min(100, 100) = 100
tx_score    = min(8_000_000_000 ÷ 100_000_000, 100) = min(80, 100) = 80
repay_score = (20 × 10000 ÷ 20) ÷ 100 = 10000 ÷ 100 = 100

composite = (100×40 + 80×30 + 100×30) ÷ 100
          = (4000 + 2400 + 3000) ÷ 100
          = 9400 ÷ 100
          = 94

score = clamp(300 + 94×550÷100, 300, 850)
      = clamp(300 + 51700÷100, 300, 850)
      = clamp(300 + 517, 300, 850)
      = 817
```

**Result: 817** — near the ceiling. Reaching 850 requires a perfect composite of 100, which needs ≥5 VCs, ≥100 XLM volume, and 100% repayment rate.

---

## Edge cases

### Stale score (`last_updated` more than 30 days ago)

The contract does not enforce score freshness. `get_score` returns whatever was last computed, regardless of age. The `last_updated` field in `ScoreRecord` is a ledger timestamp (Unix seconds) that consumers should check.

**Recommended consumer behaviour:** treat a score older than 30 days (2,592,000 seconds) as untrustworthy and prompt the subject or feeder to call `compute_score` again.

The feeder is responsible for keeping `TxStats` and `VcCount` current. If the feeder stops updating, the score will drift from reality but will not error — it will simply reflect stale inputs.

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

A subject with perfect repayment history but no feeder data is capped at **465**. This is intentional — the protocol requires active data submission to unlock higher scores.

### Integer division truncation

All arithmetic uses integer (floor) division, matching Soroban's `u32`/`i128` semantics. This means:

- A repayment rate of 9/10 = 90% gives `repay_score = 90`, not 90.0
- A volume of 150,000,000 stroops gives `tx_score = 1`, not 1.5
- A composite of 57.3 gives `score = 300 + 57×550÷100 = 613`, not 614

Consumers should be aware that two subjects with slightly different inputs may receive the same score due to truncation.
