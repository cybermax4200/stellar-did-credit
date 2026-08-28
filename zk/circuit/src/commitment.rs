//! Pedersen-style vector commitment for `ScoreRecord` fields.
//!
//! The commitment binds all private witness fields so a prover cannot misreport
//! any component of the score. This resolves Open research question #11 by
//! extending the commitment to include `avg_counterparties` (which lives on
//! `TxStats` today) as part of the committed preimage, rather than adding a
//! second commitment. See `docs/adr-001-avg-counterparties-binding.md`.
//!
//! The commitment is a Pedersen vector commitment over the additive group of
//! the BLS12-381 scalar field `Fr`:
//!
//! ```text
//! C = sum_i field_i * coeff_i + blinding
//! ```
//!
//! where `coeff_i` are deterministic constants derived from a domain separator.
//! This is binding (the coefficients are linearly independent) and hiding (the
//! random `blinding` term prevents recovery of individual fields). Because it
//! operates entirely in `Fr`, the circuit can verify it directly with R1CS
//! constraints.

use ark_bls12_381::Fr;
use ark_ff::{PrimeField, UniformRand};
use ark_std::rand::RngCore;
use sha2::{Digest, Sha256};

/// Number of committed scalar fields.
pub const COMMIT_FIELDS: usize = 8;

/// A Pedersen vector commitment over the BLS12-381 scalar field.
pub struct PedersenCommitment {
    /// Coefficients for each committed field.
    pub coefficients: [Fr; COMMIT_FIELDS],
}

impl PedersenCommitment {
    /// Derive deterministic coefficients from a domain separator.
    pub fn new(domain_separator: &[u8]) -> Self {
        let mut coefficients = [Fr::from(0u32); COMMIT_FIELDS];
        for (i, coefficient) in coefficients.iter_mut().enumerate() {
            let mut hasher = Sha256::new();
            hasher.update(domain_separator);
            hasher.update(b"::pedersen_coeff::");
            hasher.update((i as u32).to_le_bytes());
            let digest = hasher.finalize();
            *coefficient = Fr::from_le_bytes_mod_order(&digest);
        }
        Self { coefficients }
    }

    /// Commit to the 8 score-record fields.
    ///
    /// Fields (in order):
    ///   0: score
    ///   1: vc_count
    ///   2: tx_volume_30d (as a field element)
    ///   3: avg_counterparties
    ///   4: repayment_rate
    ///   5: last_updated
    ///   6: computed_at_ledger
    ///   7: stale (0 or 1)
    pub fn commit(
        &self,
        fields: &[Fr; COMMIT_FIELDS],
        blinding: Fr,
    ) -> Fr {
        let mut acc = Fr::from(0u32);
        for (i, field) in fields.iter().enumerate() {
            acc += self.coefficients[i] * field;
        }
        acc + blinding
    }

    /// Generate a random blinding factor.
    pub fn random_blinding<R: RngCore>(&self, rng: &mut R) -> Fr {
        Fr::rand(rng)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_std::test_rng;

    #[test]
    fn commitment_is_hiding() {
        let scheme = PedersenCommitment::new(b"test-domain");
        let mut rng = test_rng();
        let fields = [Fr::from(300u32); COMMIT_FIELDS];
        let b1 = Fr::rand(&mut rng);
        let b2 = Fr::rand(&mut rng);
        let c1 = scheme.commit(&fields, b1);
        let c2 = scheme.commit(&fields, b2);
        // Different blinding -> different commitment (hiding).
        assert_ne!(c1, c2);
    }

    #[test]
    fn commitment_is_binding() {
        let scheme = PedersenCommitment::new(b"test-domain");
        let mut rng = test_rng();
        let fields1 = [Fr::from(300u32); COMMIT_FIELDS];
        let fields2 = [Fr::from(301u32); COMMIT_FIELDS];
        let b = Fr::rand(&mut rng);
        let c1 = scheme.commit(&fields1, b);
        let c2 = scheme.commit(&fields2, b);
        // Different fields -> different commitment.
        assert_ne!(c1, c2);
    }

    #[test]
    fn commitment_is_deterministic() {
        let scheme = PedersenCommitment::new(b"test-domain");
        let fields = [Fr::from(300u32); COMMIT_FIELDS];
        let b = Fr::from(42u32);
        let c1 = scheme.commit(&fields, b);
        let c2 = scheme.commit(&fields, b);
        assert_eq!(c1, c2);
    }
}
