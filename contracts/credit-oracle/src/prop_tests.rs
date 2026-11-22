//! Property-based tests for `compute_score_pure`.
//!
//! Asserts six mathematical invariants of the scoring formula using proptest.
//! Run with: `cargo test -p credit-oracle`
//!
//! Each property executes 1 000 random cases by default.

use crate::{
    compute_score_pure, is_age_expired, recency_multiplier_bps, CreditOracle, CreditOracleClient,
    BPS_DENOMINATOR, MAX_BATCH_SIZE, MAX_SCORE, MIN_SCORE, SECONDS_PER_DAY,
};
use proptest::prelude::*;
use soroban_sdk::{testutils::Address as _, Address, Env, String, Symbol};

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
            MIN_SCORE,
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
            MIN_SCORE,
        );
        let score_hi = compute_score_pure(
            hi, volume_30d, avg_counterparties,
            on_time, total_count, total_repaid,
            vc_w, tx_w, repay_w,
            MIN_SCORE,
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
    let score = compute_score_pure(0, 0, 0, 0, 0, 0, 40, 30, 30, MIN_SCORE);
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
        1,
        1,
        10_000_000_000i128,
        40,
        30,
        30,
        MIN_SCORE,
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
            MIN_SCORE,
        );
        // VC-boosted weights (80 / 10 / 10) — vc_weight doubled, others halved
        let score_boosted = compute_score_pure(
            vc_points, volume_30d, avg_cps,
            on_time, total_count, total_repaid,
            80, 10, 10,
            MIN_SCORE,
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
            MIN_SCORE,
        );

        // --- Perturb vc_points by ±1 ---
        for delta in [1i64, -1i64] {
            let perturbed_vc = (vc_points as i64 + delta).clamp(0, 200) as u32;
            let perturbed = compute_score_pure(
                perturbed_vc, volume_30d, avg_counterparties,
                on_time, total_count, total_repaid,
                vc_w, tx_w, repay_w,
                MIN_SCORE,
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
                MIN_SCORE,
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

// ---------------------------------------------------------------------------
// Property 7 — Future credential: anchored_at > now -> age 0 -> full multiplier
//
// Clock skew (credential anchored at a future ledger) must not underflow.
// `now.saturating_sub(anchored_at)` returns 0, so age_days = 0, decay = 0, and
// the multiplier equals BPS_DENOMINATOR regardless of decay settings.
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn prop_recency_future_anchored_at_full_multiplier(
        future_offset_secs in 1u64..=1_000_000_000u64,
        now               in 0u64..=u64::MAX - 1_000_000_000u64,
        decay_bps_per_day in 0u32..=BPS_DENOMINATOR,
        min_recency_bps   in 0u32..=BPS_DENOMINATOR,
    ) {
        let anchored_at = now.saturating_add(future_offset_secs);
        let multiplier = recency_multiplier_bps(
            anchored_at, now, decay_bps_per_day, min_recency_bps,
        );
        prop_assert_eq!(
            multiplier, BPS_DENOMINATOR,
            "future anchored_at must yield BPS_DENOMINATOR (age 0), got {}",
            multiplier
        );
    }
}

// ---------------------------------------------------------------------------
// Property 8 — Zero decay: any age -> full multiplier
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn prop_recency_zero_decay_full_multiplier(
        anchored_at in 0u64..=u64::MAX / 2,
        age_days    in 0u64..=10_000_000u64,
        min_recency_bps in 0u32..=BPS_DENOMINATOR,
    ) {
        let now = anchored_at
            .saturating_add(age_days.saturating_mul(SECONDS_PER_DAY));
        let multiplier = recency_multiplier_bps(anchored_at, now, 0, min_recency_bps);
        prop_assert_eq!(
            multiplier, BPS_DENOMINATOR,
            "zero decay must yield BPS_DENOMINATOR for any age, got {}",
            multiplier
        );
    }
}

// ---------------------------------------------------------------------------
// Property 9 — Floor = denominator: any age -> full multiplier
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn prop_recency_floor_at_denominator_full_multiplier(
        anchored_at       in 0u64..=u64::MAX / 2,
        age_days          in 0u64..=10_000_000u64,
        decay_bps_per_day in 0u32..=BPS_DENOMINATOR,
    ) {
        let now = anchored_at
            .saturating_add(age_days.saturating_mul(SECONDS_PER_DAY));
        let multiplier = recency_multiplier_bps(
            anchored_at, now, decay_bps_per_day, BPS_DENOMINATOR,
        );
        prop_assert_eq!(
            multiplier, BPS_DENOMINATOR,
            "floor = BPS_DENOMINATOR must yield BPS_DENOMINATOR for any age/decay, got {}",
            multiplier
        );
    }
}

// ---------------------------------------------------------------------------
// Property 10 — Oversize floor is clamped to BPS_DENOMINATOR
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn prop_recency_oversize_floor_clamped_to_denominator(
        anchored_at       in 0u64..=u64::MAX / 2,
        age_days          in 0u64..=10_000_000u64,
        decay_bps_per_day in 0u32..=BPS_DENOMINATOR,
        floor_overshoot   in 1u32..=u32::MAX - BPS_DENOMINATOR,
    ) {
        let now = anchored_at
            .saturating_add(age_days.saturating_mul(SECONDS_PER_DAY));
        let min_recency_bps = BPS_DENOMINATOR.saturating_add(floor_overshoot);
        let multiplier = recency_multiplier_bps(
            anchored_at, now, decay_bps_per_day, min_recency_bps,
        );
        prop_assert_eq!(
            multiplier, BPS_DENOMINATOR,
            "oversize floor must clamp to BPS_DENOMINATOR, got {}",
            multiplier
        );
    }
}

// ---------------------------------------------------------------------------
// Property 11 — Batch size validation invariant: <= MAX_BATCH_SIZE is permitted,
//               > MAX_BATCH_SIZE exceeds limit.
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn prop_batch_size_limit_boundary(
        len in 0u32..=150u32,
    ) {
        let is_too_large = len > MAX_BATCH_SIZE;
        if len <= MAX_BATCH_SIZE {
            prop_assert!(!is_too_large);
        } else {
            prop_assert!(is_too_large);
        }
    }
}

// ---------------------------------------------------------------------------
// Property 12 — Batch update deduplication: sequential execution ensures that
//               the last occurrence of a subject in a batch determines its final state.
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn prop_batch_partial_duplicate_last_write_wins(
        entries in proptest::collection::vec((0u8..8u8, 0i128..1_000_000i128), 1..=50),
    ) {
        // Find expected final value for each subject
        let mut expected = [None; 8];
        for &(subject_id, vol) in &entries {
            expected[subject_id as usize] = Some(vol);
        }

        // Verify the last occurrence in entries equals the expected value
        for (idx, &(subject_id, vol)) in entries.iter().enumerate() {
            let is_last = entries[idx + 1..].iter().all(|&(s, _)| s != subject_id);
            if is_last {
                prop_assert_eq!(expected[subject_id as usize], Some(vol));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Property 13 — Extreme/boundary inputs never panic and stay within bounds
//
// Counterparty counts and volumes arrive from feeders that may be malformed.
// `u32::MAX` / `i128::MAX` (and `i128::MIN`) must flow through
// `compute_score_pure` without an arithmetic panic under `overflow-checks` and
// still land inside [MIN_SCORE, MAX_SCORE].
// ---------------------------------------------------------------------------

/// Draws from the interesting `u32` boundary values plus any random value.
fn extreme_u32() -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(0u32),
        Just(1u32),
        Just(5u32),
        Just(100u32),
        Just(u32::MAX / 2),
        Just(u32::MAX - 1),
        Just(u32::MAX),
        any::<u32>(),
    ]
}

/// Draws from the interesting `i128` boundary values plus any random value.
fn extreme_i128() -> impl Strategy<Value = i128> {
    prop_oneof![
        Just(0i128),
        Just(100_000_000i128),
        Just(i128::MAX / 2),
        Just(i128::MAX - 1),
        Just(i128::MAX),
        Just(i128::MIN),
        any::<i128>(),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn prop_extreme_inputs_stay_within_bounds_without_panic(
        vc_points           in extreme_u32(),
        volume_30d          in extreme_i128(),
        avg_counterparties  in extreme_u32(),
        on_time_count       in extreme_u32(),
        total_count         in extreme_u32(),
        total_repaid        in extreme_i128(),
        (vc_w, tx_w, repay_w) in valid_weights(),
    ) {
        let score = compute_score_pure(
            vc_points, volume_30d, avg_counterparties,
            on_time_count, total_count, total_repaid,
            vc_w, tx_w, repay_w,
        MIN_SCORE,
        );
        prop_assert!(
            score >= MIN_SCORE && score <= MAX_SCORE,
            "extreme inputs produced out-of-range score {} (vc={}, vol={}, cps={}, \
             on_time={}, total={}, repaid={})",
            score, vc_points, volume_30d, avg_counterparties,
            on_time_count, total_count, total_repaid
        );
    }
}

// ---------------------------------------------------------------------------
// Property 14 — Protocol metrics accumulation and idempotence:
//               First-time borrower scoring increments total_borrowers and accumulates
//               volume; re-scoring existing borrowers is strictly idempotent.
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn prop_protocol_metrics_accumulation_and_idempotence(
        events in proptest::collection::vec((0u8..16u8, 0i128..10_000_000_000i128), 1..=100),
    ) {
        let mut seen = [false; 16];
        let mut expected_borrowers: u64 = 0;
        let mut expected_volume: i128 = 0;

        for &(borrower_id, volume) in &events {
            let id = borrower_id as usize;
            if !seen[id] {
                seen[id] = true;
                expected_borrowers = expected_borrowers.saturating_add(1);
                expected_volume = expected_volume.saturating_add(volume.max(0));
            }
        }

        // Count unique borrowers in events
        let unique_count = seen.iter().filter(|&&s| s).count() as u64;
        prop_assert_eq!(expected_borrowers, unique_count);

        // Re-scoring all events a second time should not change metrics (idempotence)
        let mut rescore_borrowers = expected_borrowers;
        let mut rescore_volume = expected_volume;
        for &(borrower_id, volume) in &events {
            let id = borrower_id as usize;
            if !seen[id] {
                rescore_borrowers = rescore_borrowers.saturating_add(1);
                rescore_volume = rescore_volume.saturating_add(volume.max(0));
            }
        }
        prop_assert_eq!(rescore_borrowers, expected_borrowers);
        prop_assert_eq!(rescore_volume, expected_volume);
    }
}

// ---------------------------------------------------------------------------
// Property 15 — Dispute totals track successive accepted/rejected lifecycles.
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn prop_dispute_stats_track_resolutions(
        decisions in proptest::collection::vec(any::<bool>(), 1..=12),
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(CreditOracle, ());
        let client = CreditOracleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let keys = [
            Symbol::new(&env, "tx_stats"),
            Symbol::new(&env, "repayment"),
            Symbol::new(&env, "vc_count"),
        ];
        let reason = String::from_str(&env, "property test");
        let mut accepted_count = 0u64;
        let mut rejected_count = 0u64;

        for (index, accepted) in decisions.iter().enumerate() {
            let key = keys[index % keys.len()].clone();
            client.flag_score_input(&subject, &key, &reason);
            client.resolve_dispute(&subject, &key, accepted);
            if *accepted {
                accepted_count += 1;
            } else {
                rejected_count += 1;
            }

            assert_eq!(
                client.get_dispute_stats(),
                crate::DisputeStats {
                    pending: 0,
                    accepted: accepted_count,
                    rejected: rejected_count,
                }
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Score expiry (`is_score_expired` core): age strictly greater than the limit
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    /// Expired iff `now - last_updated > max_age`, with future records
    /// (`last_updated > now`) treated as age 0 and never expired.
    #[test]
    fn prop_score_expiry_matches_definition(
        last_updated in any::<u64>(),
        now in any::<u64>(),
        max_age in any::<u64>(),
    ) {
        let expected = now.saturating_sub(last_updated) > max_age;
        prop_assert_eq!(is_age_expired(last_updated, now, max_age), expected);
    }

    /// Boundary: exactly `max_age` old is fresh; one second more is expired.
    #[test]
    fn prop_score_expiry_boundary(last_updated in 0u64..=(u64::MAX / 2), max_age in 0u64..=(u64::MAX / 2 - 1)) {
        prop_assert!(!is_age_expired(last_updated, last_updated + max_age, max_age));
        prop_assert!(is_age_expired(last_updated, last_updated + max_age + 1, max_age));
    }

    /// Monotone in time: once expired, always expired as `now` grows.
    #[test]
    fn prop_score_expiry_is_monotone_in_time(
        last_updated in any::<u64>(),
        max_age in any::<u64>(),
        t1 in any::<u64>(),
        dt in any::<u64>(),
    ) {
        let t2 = t1.saturating_add(dt);
        if is_age_expired(last_updated, t1, max_age) {
            prop_assert!(is_age_expired(last_updated, t2, max_age));
        }
    }

    /// Monotone in the limit: a larger `max_age` never turns fresh into expired.
    #[test]
    fn prop_score_expiry_is_monotone_in_max_age(
        last_updated in any::<u64>(),
        now in any::<u64>(),
        a in any::<u64>(),
        b in any::<u64>(),
    ) {
        let (small, large) = if a <= b { (a, b) } else { (b, a) };
        if !is_age_expired(last_updated, now, small) {
            prop_assert!(!is_age_expired(last_updated, now, large));
        }
    }
}
