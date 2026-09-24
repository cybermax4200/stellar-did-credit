# Scoring Specification & Freshness Enforcement

## Scoring Formula

The `credit-oracle` computes a credit score (300–850) for any subject from three
weighted components: verified credentials (identity strength), on-chain
transaction activity (breadth of financial behavior), and repayment history
(reliability). The exact formula below mirrors `compute_score_pure` in
`contracts/credit-oracle/src/lib.rs`.

### Components

All arithmetic uses **integer division with truncation**, matching the contract,
which operates on stroop (1 XLM = 100,000,000 stroops).

```
vc_points            = vc_count × 20                          # 20 points per verified VC
vc_score             = min(vc_points, 100)

volume_score         = clamp(volume_30d_stroops ÷ 100_000_000, 0, 80)   # 1 point per XLM, cap 80
counterparty_bonus   = min(avg_counterparties ÷ 5, 20)                  # 1 point per 5 counterparties, cap 20
tx_score             = min(volume_score + counterparty_bonus, 100)

repayment_rate_score   = (on_time_count × 10000 ÷ total_count) ÷ 100   # 0–100; 0 when total_count = 0
repayment_volume_score = clamp(total_repaid_stroops ÷ 100_000_000, 0, 100)  # 1 point per XLM, cap 100
repay_score            = (repayment_rate_score + repayment_volume_score) ÷ 2
```

### Composite and final score

```
composite   = (vc_score × vc_weight
             + tx_score × tx_weight
             + repay_score × repayment_weight) ÷ 100

final_score = clamp(300 + composite × 550 ÷ 100, 300, 850)
```

Default weights (governed on-chain via the governance contract, see
[docs/governance.md](governance.md)):

| Weight       | Default |
| ------------ | ------- |
| `vc_weight`  | 40      |
| `tx_weight`  | 30      |
| `repayment_weight` | 30 |

### Edge cases

- **No history** (`total_count = 0`): `repayment_rate_score = 0` (division by
  zero short-circuits to 0), so a subject with no repayment record earns nothing
  from the repayment component.
- **Negative volumes** are clamped to 0 via the `max(…, 0)` floors.
- **Max-out**: `vc_score` caps at 100, `tx_score` caps at 100 (volume at 80 plus
  up to 20 counterparty bonus), and `repay_score` caps at 100. To reach 850 a
  subject needs 100+ counterparties on top of maxed VCs, volume, and repayment.

## Worked examples

**Example scores** (all arithmetic uses integer division, matching the contract):

| Profile     | VCs | 30d Volume | Total repaid | Counterparties | Repayment rate | Score |
| ----------- | --- | ---------- | ------------ | -------------- | -------------- | ----- |
| New user    | 0   | 0 XLM      | 0 XLM        | 0              | —              | 300   |
| Early stage | 1   | 5 XLM      | 5 XLM        | 0              | 70%            | 410   |
| Established | 2   | 20 XLM     | 20 XLM       | 0              | 85%            | 503   |
| Strong      | 3   | 50 XLM     | 50 XLM       | 5              | 95%            | 630   |
| Exceptional | ≥5  | 100+ XLM   | 100+ XLM     | 100+           | 100%           | 850   |

### New user (no history) → 300

Inputs: `vc_count = 0`, no volume, no repayment record.

```
vc_score = 0, tx_score = 0, repayment_rate_score = 0, repayment_volume_score = 0
repay_score = 0
composite   = (0 + 0 + 0) ÷ 100 = 0
final_score = 300 + 0 = 300
```

### Established profile → 503

Inputs: 2 VCs, 20 XLM volume (2,000,000,000 stroops), 20 XLM total repaid,
0 counterparties, 85% on-time repayment (17 on-time out of 20).

```
vc_points            = 2 × 20 = 40                          → vc_score = 40
volume_score         = 2,000,000,000 ÷ 100,000,000 = 20     → clamp(20, 0, 80) = 20
counterparty_bonus   = min(0 ÷ 5, 20) = 0
tx_score             = min(20 + 0, 100) = 20
repayment_rate_score = (17 × 10000) ÷ 20 ÷ 100 = 8500 ÷ 100 = 85
repayment_volume     = 2,000,000,000 ÷ 100,000,000 = 20     → clamp(20, 0, 100) = 20
repay_score          = (85 + 20) ÷ 2 = 52
composite            = (40×40 + 20×30 + 52×30) ÷ 100 = 3760 ÷ 100 = 37
final_score          = 300 + 37×550 ÷ 100 = 300 + 203 = 503
```

Cross-checked by the unit test `scoring_examples`:

```
cargo test -p credit-oracle -- scoring_examples
```

### Exceptional profile → 850

Inputs: ≥5 VCs (vc_points capped at 100), 100+ XLM volume, 100+ XLM total repaid,
100+ counterparties, 100% on-time repayment.

```
vc_score             = min(100, 100) = 100
volume_score         = min(100, 80) = 80
counterparty_bonus   = min(100 ÷ 5, 20) = 20
tx_score             = min(80 + 20, 100) = 100
repayment_rate_score = (20 × 10000) ÷ 20 ÷ 100 = 100
repayment_volume     = 100
repay_score          = (100 + 100) ÷ 2 = 100
composite            = (100×40 + 100×30 + 100×30) ÷ 100 = 100
final_score          = 300 + 100×550 ÷ 100 = 850
```

## Freshness and State Synchronization

To prevent lenders from evaluating scores derived from outdated credential
states, the `credit-oracle` tracks state updates via the `identity-oracle`
contract.

- **Last Identity State Change Tracking**: Whenever a subject's verifiable
  credentials are anchored or revoked, the `identity-oracle` records the current
  ledger sequence via `get_last_state_change_ledger(subject)`.
- **Dynamic Staleness Check**: When `get_score(subject)` is called on the
  `credit-oracle`, it performs a cross-contract lookup to compare
  `ScoreRecord.computed_at_ledger` against the subject's latest identity state
  change ledger. If `computed_at_ledger < last_state_change`, the returned
  `ScoreRecord` sets `stale: true`.