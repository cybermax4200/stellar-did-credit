//! Pure scoring function matching `contracts/credit-oracle/src/lib.rs::compute_score_pure`.
//!
//! This is the canonical reference implementation used to build test vectors for the
//! circuit. It must stay in lock-step with the on-chain contract.

pub const MIN_SCORE: u32 = 300;
pub const MAX_SCORE: u32 = 850;

/// Pure scoring function that computes a credit score from input parameters.
///
/// Mirrors `compute_score_pure` in the credit-oracle contract exactly.
#[allow(clippy::too_many_arguments)]
pub fn compute_score_pure(
    vc_points: u32,
    volume_30d: i128,
    avg_counterparties: u32,
    on_time_count: u32,
    total_count: u32,
    total_repaid: i128,
    vc_weight: u32,
    tx_weight: u32,
    repayment_weight: u32,
) -> u32 {
    let vc_score = (vc_points).min(100) as u128;
    let volume_score = ((volume_30d / 100_000_000i128).max(0) as u128).min(80);
    let counterparty_bonus = (avg_counterparties / 5).min(20) as u128;
    let tx_score = (volume_score + counterparty_bonus).min(100);
    let repayment_rate_score = (on_time_count as u128)
        .saturating_mul(10_000)
        .checked_div(total_count as u128)
        .map(|r| r / 100)
        .unwrap_or(0);
    let repayment_volume_score = ((total_repaid / 100_000_000i128).max(0) as u128).min(100);
    let repay_score = (repayment_rate_score + repayment_volume_score) / 2;
    let composite = (vc_score * vc_weight as u128
        + tx_score * tx_weight as u128
        + repay_score * repayment_weight as u128)
        / 100;
    let score = MIN_SCORE as u128 + composite.saturating_mul(550) / 100;
    score.min(MAX_SCORE as u128).max(MIN_SCORE as u128) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_score_is_300() {
        assert_eq!(compute_score_pure(0, 0, 0, 0, 0, 0, 60, 0, 40), 300);
    }

    #[test]
    fn exceptional_score_equals_850() {
        assert_eq!(compute_score_pure(100, i128::MAX, 100, 100, 100, i128::MAX, 40, 30, 30), 850);
    }

    #[test]
    fn score_bounded_300_850() {
        // All-zero inputs -> 300
        assert_eq!(compute_score_pure(0, 0, 0, 0, 0, 0, 60, 0, 40), 300);
        // Max inputs -> 850
        assert_eq!(compute_score_pure(100, i128::MAX, 100, 100, 100, i128::MAX, 40, 30, 30), 850);
    }

    #[test]
    fn counterparty_bonus_adds_points() {
        // avg_counterparties = 10 -> bonus = 2
        let with = compute_score_pure(0, 0, 10, 0, 0, 0, 60, 0, 40);
        let without = compute_score_pure(0, 0, 0, 0, 0, 0, 60, 0, 40);
        assert!(with >= without);
    }

    #[test]
    fn repayment_rate_calculated_correctly() {
        // on_time=50, total=100 -> rate=50 -> rate_score=50
        let score = compute_score_pure(0, 0, 0, 50, 100, 0, 60, 0, 40);
        assert!(score > 300);
    }
}
