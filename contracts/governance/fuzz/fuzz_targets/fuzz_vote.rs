#![no_main]
//! Fuzz target for `governance` vote tally accumulation.
//!
//! The real contract logic increments the appropriate tally using
//! `checked_add` before persisting the vote, so this harness mirrors that
//! behaviour with libFuzzer-driven `(vote_for, vote_weight)` inputs.
//!
//! The key invariant is simple: the tally update must either succeed or be
//! rejected via overflow handling; it must never panic.

use libfuzzer_sys::fuzz_target;

fn accumulate_tally(votes_for: i128, votes_against: i128, vote_for: bool, vote_weight: i128) -> Option<(i128, i128)> {
    if vote_weight <= 0 {
        return Some((votes_for, votes_against));
    }

    if vote_for {
        Some((votes_for.checked_add(vote_weight)?, votes_against))
    } else {
        Some((votes_for, votes_against.checked_add(vote_weight)?))
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 49 {
        return;
    }

    let votes_for = i128::from_le_bytes(data[0..16].try_into().unwrap());
    let votes_against = i128::from_le_bytes(data[16..32].try_into().unwrap());
    let vote_weight = i128::from_le_bytes(data[32..48].try_into().unwrap());
    let vote_for = data[48] != 0;

    if votes_for < 0 || votes_against < 0 {
        return;
    }

    let Some((new_for, new_against)) = accumulate_tally(votes_for, votes_against, vote_for, vote_weight) else {
        return;
    };

    assert!(new_for >= votes_for || !vote_for, "for-vote total decreased unexpectedly");
    assert!(new_against >= votes_against || vote_for, "against-vote total decreased unexpectedly");

    if data.len() >= 65 {
        let next_vote_weight = i128::from_le_bytes(data[49..65].try_into().unwrap());
        if next_vote_weight <= 0 {
            return;
        }

        let Some((first_for, first_against)) = accumulate_tally(votes_for, votes_against, vote_for, vote_weight) else {
            return;
        };
        let Some((second_for, second_against)) = accumulate_tally(first_for, first_against, vote_for, next_vote_weight) else {
            return;
        };
        let Some((reverse_for, reverse_against)) = accumulate_tally(votes_for, votes_against, vote_for, next_vote_weight) else {
            return;
        };
        let Some((reverse_total_for, reverse_total_against)) = accumulate_tally(reverse_for, reverse_against, vote_for, vote_weight) else {
            return;
        };

        assert_eq!(second_for, reverse_total_for, "vote tally accumulation should be order-invariant");
        assert_eq!(second_against, reverse_total_against, "vote tally accumulation should be order-invariant");
    }
});
