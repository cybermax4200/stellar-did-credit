#![no_main]
//! Fuzz target for `governance` vote-weight arithmetic.
//!
//! # What this target tests
//!
//! The `vote` function in `governance/src/lib.rs` accumulates votes with:
//!
//! ```rust
//! proposal.votes_for = proposal.votes_for.saturating_add(vote_weight);
//! ```
//!
//! `saturating_add` on `i128` silently caps at `i128::MAX` instead of
//! panicking. This is a correctness issue: if a voter with weight near
//! `i128::MAX / 2` votes twice, `votes_for` saturates at `i128::MAX`.
//! The execute path checks:
//!
//! ```rust
//! if proposal.votes_for + proposal.votes_against < proposal.quorum_required
//! ```
//!
//! A saturated `votes_for == i128::MAX` will always be greater than any
//! finite `quorum_required`, effectively bypassing the quorum check for any
//! proposal where a single heavy voter dominates.
//!
//! # Assertions
//!
//! 1. **No panic**: all arithmetic completes without panic (the fuzzer
//!    itself catches panics as crashes).
//! 2. **Saturation detection**: if `votes_for` saturates to `i128::MAX`,
//!    the target asserts the bug is present so CI surfaces it as a failure,
//!    documenting the exact input that triggers it.
//! 3. **Quorum bypass**: if saturated `votes_for >= quorum_required` while
//!    `original votes_for < quorum_required`, the quorum check has been
//!    silently bypassed — assert this as a failure.
//! 4. **Monotonicity**: `votes_for` after accumulation must be ≥ before.
//! 5. **Commutativity**: accumulating (a then b) produces the same result
//!    as (b then a) under saturating addition.

use libfuzzer_sys::fuzz_target;

/// Mirrors the vote accumulation logic in `governance/src/lib.rs`.
///
/// Returns `(new_votes_for, new_votes_against)`.
fn accumulate_vote(
    votes_for: i128,
    votes_against: i128,
    vote_for: bool,
    vote_weight: i128,
) -> (i128, i128) {
    if vote_for {
        (votes_for.saturating_add(vote_weight), votes_against)
    } else {
        (votes_for, votes_against.saturating_add(vote_weight))
    }
}

/// Returns `true` when the saturating-addition result differs from the
/// mathematically exact sum, i.e. saturation actually occurred.
fn saturated(before: i128, addend: i128) -> bool {
    before.checked_add(addend).is_none()
}

fuzz_target!(|data: &[u8]| {
    // Input layout (minimum 50 bytes):
    //  [0..16]  i128 — current votes_for
    //  [16..32] i128 — current votes_against
    //  [32..48] i128 — vote_weight for this vote
    //  [48]     u8   — vote direction: 0 = against, non-zero = for
    //  [49..65] i128 — quorum_required (optional; used for bypass check)
    if data.len() < 49 {
        return;
    }

    let votes_for = i128::from_le_bytes(data[0..16].try_into().unwrap());
    let votes_against = i128::from_le_bytes(data[16..32].try_into().unwrap());
    let vote_weight = i128::from_le_bytes(data[32..48].try_into().unwrap());
    let vote_for = data[48] != 0;

    // The contract rejects non-positive vote_weight; mirror that guard.
    if vote_weight <= 0 {
        return;
    }
    // Negative accumulator states are not valid in the contract.
    if votes_for < 0 || votes_against < 0 {
        return;
    }

    let (new_for, new_against) = accumulate_vote(votes_for, votes_against, vote_for, vote_weight);

    // -----------------------------------------------------------------
    // Invariant 1: Monotonicity — accumulated totals never decrease.
    // -----------------------------------------------------------------
    assert!(
        new_for >= votes_for,
        "votes_for decreased: {} -> {} (weight={}, vote_for={})",
        votes_for, new_for, vote_weight, vote_for
    );
    assert!(
        new_against >= votes_against,
        "votes_against decreased: {} -> {} (weight={}, vote_for={})",
        votes_against, new_against, vote_weight, vote_for
    );

    // -----------------------------------------------------------------
    // Invariant 2: Saturation detection.
    //
    // When saturation occurs the accumulated total is NOT equal to the
    // mathematically exact sum, meaning vote tallies have been silently
    // corrupted. Assert this as a failure so the fuzzer surfaces the
    // exact triggering input (votes_for, vote_weight) in CI.
    //
    // NOTE: This assertion is intentionally FAILING when saturation is
    // triggered — that is the bug we want the fuzzer to find and report.
    // Once issue #528 is resolved (the contract validates that no single
    // voter's weight can exceed i128::MAX - current_total), this assert
    // should never fire.
    // -----------------------------------------------------------------
    if vote_for && saturated(votes_for, vote_weight) {
        assert!(
            new_for < i128::MAX,
            "SATURATION BUG: votes_for saturated to i128::MAX \
             (votes_for={}, vote_weight={}). \
             A voter with near-max weight can bypass the quorum check \
             by making votes_for == i128::MAX, which is always >= any \
             finite quorum_required. See issue #528.",
            votes_for, vote_weight
        );
    }
    if !vote_for && saturated(votes_against, vote_weight) {
        assert!(
            new_against < i128::MAX,
            "SATURATION BUG: votes_against saturated to i128::MAX \
             (votes_against={}, vote_weight={}). \
             See issue #528.",
            votes_against, vote_weight
        );
    }

    // -----------------------------------------------------------------
    // Invariant 3: Quorum-bypass detection.
    //
    // If the quorum was NOT met before the vote but IS met after only due
    // to saturation, the quorum check has been silently bypassed.
    // -----------------------------------------------------------------
    if data.len() >= 65 {
        let quorum_required = i128::from_le_bytes(data[49..65].try_into().unwrap());
        if quorum_required > 0 {
            let total_before = votes_for.saturating_add(votes_against);
            let total_after = new_for.saturating_add(new_against);
            let quorum_before = total_before >= quorum_required;
            let quorum_after = total_after >= quorum_required;

            // If saturation is the *only* reason quorum is now met, flag it.
            let became_quorate_via_saturation = !quorum_before
                && quorum_after
                && (saturated(votes_for, vote_weight) || saturated(votes_against, vote_weight));

            assert!(
                !became_quorate_via_saturation,
                "QUORUM BYPASS: saturation caused quorum to be met when it should not be \
                 (votes_for={}, vote_weight={}, quorum_required={}, \
                 total_before={}, total_after={}). See issue #528.",
                votes_for, vote_weight, quorum_required, total_before, total_after
            );
        }
    }

    // -----------------------------------------------------------------
    // Invariant 4: Commutativity of two sequential votes (a then b) ==
    //              (b then a) under saturating addition.
    // -----------------------------------------------------------------
    if data.len() >= 81 {
        let vote_weight_b = i128::from_le_bytes(data[65..81].try_into().unwrap());
        if vote_weight_b > 0 {
            // Path 1: apply vote_weight first, then vote_weight_b.
            let after_a = votes_for.saturating_add(vote_weight);
            let after_ab = after_a.saturating_add(vote_weight_b);

            // Path 2: apply vote_weight_b first, then vote_weight.
            let after_b = votes_for.saturating_add(vote_weight_b);
            let after_ba = after_b.saturating_add(vote_weight);

            assert_eq!(
                after_ab, after_ba,
                "saturating_add commutativity violated: \
                 ({} + {} + {}) = {} but ({} + {} + {}) = {}",
                votes_for, vote_weight, vote_weight_b, after_ab,
                votes_for, vote_weight_b, vote_weight, after_ba
            );
        }
    }
});
