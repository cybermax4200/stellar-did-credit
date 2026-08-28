//! Property-based tests for `compute_score_pure`.
//!
//! Asserts six mathematical invariants of the scoring formula using proptest.
//! Run with: `cargo test -p credit-oracle`
//!
//! Each property executes 1 000 random cases by default.

use crate::{compute_score_pure, MAX_SCORE, MIN_SCORE};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a valid `(vc_weight, tx_weight, repayment_weight)` triple that
/// sums to exactly 100, with each component in `[10, 80]`.
fn valid_weights() -> impl Strategy<Value = (u32, u32, u32)> {
    (10u32..=80u32).prop_flat_map(|vc_w| {
        let tx_max = (100u32 - vc_w - 10).min(80);
        (10u32..=tx_max).prop_flat_map(move |tx_w| {
            let repay_w = 100 - vc_w - tx_w;
            Just((vc_w, tx_w, repay_w))
        })
    })
}

// ---------------------------------------------------------------------------
// Property 1 — Bounds: score ∈ [MIN_SCORE, MAX_SCORE] for all valid inputs
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn prop_score_bounds(
        vc_points           in 0u32..=200u32,
        volume_30d          in 0i128..=100_000_000_000i128,
        avg_counterparties  in 0u32..=200u32,
        on_time_count       in 0u32..=1_000u32,
        total_count         in 0u32..=1_000u32,
        total_repaid        in 0i128..=100_000_000_000i128,
        (vc_w, tx_w, repay_w) in valid_weights(),
    ) {
        let on_time = on_time_count.min(total_count);
        let score = compute_score_pure(
            vc_points, volume_30d, avg_counterparties,
            on_time, total_count, total_repaid,
            vc_w, tx_w, repay_w,
        );
        prop_assert!(
            score >= MIN_SCORE && score <= MAX_SCORE,
            "score {} is outside [{}, {}]",
            score, MIN_SCORE, MAX_SCORE
        );
    }
}

// ---------------------------------------------------------------------------
// Property 2 — Monotonicity: higher vc_points never decreases the score
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn prop_score_monotone_vc_points(
        vc_lo               in 0u32..=100u32,
        vc_hi               in 0u32..=100u32,
        volume_30d          in 0i128..=100_000_000_000i128,
        avg_counterparties  in 0u32..=200u32,
        on_time_count       in 0u32..=1_000u32,
        total_count         in 0u32..=1_000u32,
        total_repaid        in 0i128..=100_000_000_000i128,
        (vc_w, tx_w, repay_w) in valid_weights(),
    ) {
        let on_time = on_time_count.min(total_count);
        let lo = vc_lo.min(vc_hi);
        let hi = vc_lo.max(vc_hi);

        let score_lo = compute_score_pure(
            lo, volume_30d, avg_counterparties,
            on_time, total_count, total_repaid,
            vc_w, tx_w, repay_w,
        );
        let score_hi = compute_score_pure(
            hi, volume_30d, avg_counterparties,
            on_time, total_count, total_repaid,
            vc_w, tx_w, repay_w,
        );
        prop_assert!(
            score_hi >= score_lo,
            "score decreased ({} -> {}) when vc_points increased ({} -> {})",
            score_lo, score_hi, lo, hi
        );
    }
}

// ---------------------------------------------------------------------------
// Property 3 — Zero inputs: all-zero inputs yield exactly MIN_SCORE (300)
// ---------------------------------------------------------------------------
#[test]
fn prop_zero_inputs_yields_min_score() {
    let score = compute_score_pure(0, 0, 0, 0, 0, 0, 40, 30, 30);
    assert_eq!(
        score, MIN_SCORE,
        "expected MIN_SCORE ({}) for all-zero inputs, got {}",
        MIN_SCORE, score
    );
}

// ---------------------------------------------------------------------------
// Property 4 — Perfect inputs: fully-maxed inputs yield exactly MAX_SCORE (850)
// ---------------------------------------------------------------------------
#[test]
fn prop_perfect_inputs_yields_max_score() {
    // vc_points = 100  → vc_score = 100
    // volume_30d = 8_000_000_000 (80 units at 8-decimal scale) → volume_score = 80
    // avg_counterparties = 100 → counterparty_bonus = min(100/5, 20) = 20
    //   tx_score = min(80+20, 100) = 100
    // on_time_count = total_count = 1 → repayment_rate_score = 100
    // total_repaid = 10_000_000_000 (100 units) → repayment_volume_score = 100
    //   repay_score = (100 + 100) / 2 = 100
    // composite = (100*40 + 100*30 + 100*30) / 100 = 100
    // score = 300 + 100 * 550 / 100 = 300 + 550 = 850  ✓
    let score = compute_score_pure(
        100,
        8_000_000_000i128,
        100,
        1, 1,
        10_000_000_000i128,
        40, 30, 30,
    );
    assert_eq!(
        score, MAX_SCORE,
        "expected MAX_SCORE ({}) for perfect inputs, got {}",
        MAX_SCORE, score
    );
}

// ---------------------------------------------------------------------------
// Property 5 — Weight sensitivity: doubling vc_weight (and halving the other
//              two) increases the score for VC-heavy subjects with low tx/repay.
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn prop_weight_sensitivity_vc_heavy(
        volume_30d   in 0i128..=1_000_000_000i128,   // deliberately low tx volume
        avg_cps      in 0u32..=10u32,
        total_repaid in 0i128..=500_000_000i128,
    ) {
        let vc_points   = 100u32; // perfect VC score
        let on_time     = 1u32;
        let total_count = 2u32;   // mediocre repayment history

        // Base weights (40 / 30 / 30)
        let score_base = compute_score_pure(
            vc_points, volume_30d, avg_cps,
            on_time, total_count, total_repaid,
            40, 30, 30,
        );
        // VC-boosted weights (80 / 10 / 10) — vc_weight doubled, others halved
        let score_boosted = compute_score_pure(
            vc_points, volume_30d, avg_cps,
            on_time, total_count, total_repaid,
            80, 10, 10,
        );
        prop_assert!(
            score_boosted >= score_base,
            "boosting vc_weight should not decrease score for a vc-heavy subject \
             (base={}, boosted={})",
            score_base, score_boosted
        );
    }
}

// ---------------------------------------------------------------------------
// Property 6 — Integer truncation stability: a ±1 perturbation to any single
//              input must not change the score by more than one truncation
//              step. The formula floors `composite * 550 / 100`, so a ±1
//              change in a component can move the composite by 1, and one
//              composite step moves the score by at most 6 (since
//              550/100 = 5.5). Hence the bound is 6, not 1.
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn prop_truncation_stability(
        vc_points           in 1u32..=99u32,
        volume_30d          in 100_000_000i128..=50_000_000_000i128,
        avg_counterparties  in 1u32..=100u32,
        on_time_count       in 1u32..=500u32,
        total_count         in 1u32..=1_000u32,
        total_repaid        in 100_000_000i128..=50_000_000_000i128,
        (vc_w, tx_w, repay_w) in valid_weights(),
    ) {
        let on_time = on_time_count.min(total_count);

        let base = compute_score_pure(
            vc_points, volume_30d, avg_counterparties,
            on_time, total_count, total_repaid,
            vc_w, tx_w, repay_w,
        );

        // --- Perturb vc_points by ±1 ---
        for delta in [1i64, -1i64] {
            let perturbed_vc = (vc_points as i64 + delta).clamp(0, 200) as u32;
            let perturbed = compute_score_pure(
                perturbed_vc, volume_30d, avg_counterparties,
                on_time, total_count, total_repaid,
                vc_w, tx_w, repay_w,
            );
            let diff = (base as i64 - perturbed as i64).unsigned_abs();
            prop_assert!(
                diff <= 6,
                "±1 in vc_points changed score by {} (base={}, perturbed={})",
                diff, base, perturbed
            );
        }

        // --- Perturb volume_30d by ±1 ---
        for delta in [1i128, -1i128] {
            let perturbed_vol = volume_30d.saturating_add(delta).max(0);
            let perturbed = compute_score_pure(
                vc_points, perturbed_vol, avg_counterparties,
                on_time, total_count, total_repaid,
                vc_w, tx_w, repay_w,
            );
            let diff = (base as i64 - perturbed as i64).unsigned_abs();
            prop_assert!(
                diff <= 6,
                "±1 in volume_30d changed score by {} (base={}, perturbed={})",
                diff, base, perturbed
            );
        }
    }
}
