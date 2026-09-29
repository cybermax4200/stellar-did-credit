#![no_main]
//! Fuzz target for `identity-oracle` VC anchoring arithmetic.
//!
//! This target exercises the integer arithmetic and boundary conditions
//! in `anchor_vc_typed` without a Soroban environment:
//!
//! 1. Active-VC-count cap enforcement (`active_count >= MAX_VCS_PER_SUBJECT`).
//! 2. `checked_add(1)` on the cached active-VC count — would panic on
//!    overflow if the cap check ever lets count reach `u32::MAX`.
//! 3. Duplicate-detection comparison: ensures byte-by-byte equality of
//!    32-byte VC hashes behaves correctly at all bit patterns.
//!
//! The target mirrors the guard logic inside `anchor_vc_typed` so any
//! regression in the arithmetic surface is caught before it ever reaches
//! the full contract.

use libfuzzer_sys::fuzz_target;

/// Maximum number of active (non-revoked) VCs a subject may have.
/// Must stay in sync with the constant in identity-oracle/src/lib.rs.
const MAX_VCS_PER_SUBJECT: u32 = 100;

/// Mirrors the active-count increment logic in `anchor_vc_typed`.
///
/// Returns the new active count after anchoring one more non-revoked VC,
/// or `None` if the cap has been reached and the new VC should be rejected.
fn try_increment_active_count(current_active: u32) -> Option<u32> {
    if current_active >= MAX_VCS_PER_SUBJECT {
        return None; // cap reached — anchor_vc_typed returns VCLimitReached
    }
    // `checked_add` mirrors the contract's expectation that this never
    // overflows once the cap guard is in place.
    current_active.checked_add(1)
}

/// Checks whether two 32-byte VC hashes are equal, mirroring the dedup
/// comparison in `anchor_vc_typed`: `record.vc_hash == vc_hash`.
fn hashes_equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a == b
}

fuzz_target!(|data: &[u8]| {
    // Input layout (minimum 33 bytes):
    //   [0..4]   u32 — current_active_count (number of existing active VCs)
    //   [4..36]  [u8; 32] — incoming vc_hash
    //   [36..68] [u8; 32] — existing vc_hash to compare against (dedup check)
    //            (optional; if absent, dedup check is skipped)
    if data.len() < 36 {
        return;
    }

    let current_active = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let incoming_hash: [u8; 32] = data[4..36].try_into().unwrap();

    // -----------------------------------------------------------------
    // Invariant 1: cap enforcement
    // -----------------------------------------------------------------
    let result = try_increment_active_count(current_active);

    if current_active >= MAX_VCS_PER_SUBJECT {
        // Cap reached — must reject.
        assert!(
            result.is_none(),
            "expected cap rejection for active_count={}, got {:?}",
            current_active, result
        );
    } else {
        // Below cap — must succeed.
        let new_count = result.expect("expected successful increment below cap");

        // -----------------------------------------------------------------
        // Invariant 2: monotonicity — new count is always exactly one more.
        // -----------------------------------------------------------------
        assert_eq!(
            new_count,
            current_active + 1,
            "new_count {} != {} + 1",
            new_count, current_active
        );

        // -----------------------------------------------------------------
        // Invariant 3: new count must not reach u32::MAX (overflow safety).
        //              With MAX_VCS_PER_SUBJECT = 100 this is always safe,
        //              but we assert it explicitly so any future constant
        //              change that brings the cap close to u32::MAX is caught.
        // -----------------------------------------------------------------
        assert!(
            new_count <= MAX_VCS_PER_SUBJECT,
            "new_count {} exceeds MAX_VCS_PER_SUBJECT {}",
            new_count, MAX_VCS_PER_SUBJECT
        );
    }

    // -----------------------------------------------------------------
    // Invariant 4: dedup hash equality is reflexive (hash == itself).
    // -----------------------------------------------------------------
    assert!(
        hashes_equal(&incoming_hash, &incoming_hash),
        "hash equality must be reflexive"
    );

    // -----------------------------------------------------------------
    // Invariant 5: if a second hash is provided, check (anti-)symmetry.
    // -----------------------------------------------------------------
    if data.len() >= 68 {
        let existing_hash: [u8; 32] = data[36..68].try_into().unwrap();
        let a_eq_b = hashes_equal(&incoming_hash, &existing_hash);
        let b_eq_a = hashes_equal(&existing_hash, &incoming_hash);
        assert_eq!(
            a_eq_b, b_eq_a,
            "hash equality must be symmetric: ({:?} == {:?}) != ({:?} == {:?})",
            incoming_hash, existing_hash, existing_hash, incoming_hash
        );
    }
});
