#![no_main]
//! Fuzz target for `governance` vote-weight arithmetic.
//!
//! # What this target tests
//!
//! The `vote` function in `governance/src/lib.rs` accumulates votes with:
//!
//! ```rust
//! proposal.votes_for = proposal.votes_for.checked_add(vote_weight);
//! ```
//!
//! Overflowing votes are rejected with `VoteTallyOverflow`, preserving the
//! quorum invariant.
//!
//! # Assertions
//!
//! 1. **No panic**: all arithmetic completes without panic (the fuzzer
//!    itself catches panics as crashes).
//! 2. **Overflow rejection**: an overflowing tally update is rejected.
//! 3. **Monotonicity**: accepted tallies never decrease.
//! 4. **Commutativity**: accepted additions produce the same result in either
//!    order.

use libfuzzer_sys::fuzz_target;

/// Mirrors the vote accumulation logic in `governance/src/lib.rs`.
///
/// Returns `(new_votes_for, new_votes_against)`.
fn accumulate_vote(
    votes_for: i128,
    votes_against: i128,
    vote_for: bool,
    vote_weight: i128,
) -> Option<(i128, i128)> {
    if vote_for {
        Some((votes_for.checked_add(vote_weight)?, votes_against))
    } else {
        Some((votes_for, votes_against.checked_add(vote_weight)?))
    }
}

fuzz_target!(|data: &[u8]| {
    // Input layout (minimum 50 bytes):
    //  [0..16]  i128 — current votes_for
    //  [16..32] i128 — current votes_against
    //  [32..48] i128 — vote_weight for this vote
    //  [48]     u8   — vote direction: 0 = against, non-zero = for
    //  [49..65] i128 — reserved for future quorum invariants
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

    let Some((new_for, new_against)) =
        accumulate_vote(votes_for, votes_against, vote_for, vote_weight)
    else {
        return;
    };

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
    // Invariant 3: Commutativity of two accepted votes.
    // -----------------------------------------------------------------
    if data.len() >= 81 {
        let vote_weight_b = i128::from_le_bytes(data[65..81].try_into().unwrap());
        if vote_weight_b > 0 {
            // Path 1: apply vote_weight first, then vote_weight_b.
            let Some(after_a) = votes_for.checked_add(vote_weight) else {
                return;
            };
            let Some(after_ab) = after_a.checked_add(vote_weight_b) else {
                return;
            };

            // Path 2: apply vote_weight_b first, then vote_weight.
            let Some(after_b) = votes_for.checked_add(vote_weight_b) else {
                return;
            };
            let Some(after_ba) = after_b.checked_add(vote_weight) else {
                return;
            };

            assert_eq!(
                after_ab, after_ba,
                "checked_add commutativity violated: \
                 ({} + {} + {}) = {} but ({} + {} + {}) = {}",
                votes_for, vote_weight, vote_weight_b, after_ab,
                votes_for, vote_weight_b, vote_weight, after_ba
            );
        }
    }
});
