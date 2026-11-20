//! # zk-circuit
//!
//! Groth16 circuit for `score > threshold` using arkworks-rs on BLS12-381.
//!
//! This crate implements the Phase 4 ZK proof layer circuit:
//!
//! 1. Encodes the `compute_score_pure` formula as R1CS constraints.
//! 2. Implements the `ScoreRecord` Pedersen vector commitment (binding all
//!    private inputs, including `avg_counterparties` — see ADR-001).
//! 3. Asserts `score > threshold` as a range proof within the circuit.
//! 4. Exposes `generate_proof` and `verify_proof`.
//! 5. Provides a depth-20 Poseidon Merkle membership gadget (Research Q1).
//!
//! The circuit uses **BLS12-381** (CAP-0059 available on Stellar) for the
//! pairing check.

pub mod commitment;
pub mod merkle;
pub mod range;
pub mod score;
pub mod score_circuit;

pub use score::{compute_score_pure, MAX_SCORE, MIN_SCORE};
pub use score_circuit::{ScoreCircuit, ScorePublicInputs, ScoreWitness, CIRCUIT_DOMAIN};

pub use merkle::{MerkleMembershipCircuit, MerklePublicInputs, MerkleWitness, MERKLE_DEPTH};

use ark_bls12_381::{Bls12_381, Fr};
use ark_groth16::{Groth16, Proof, ProvingKey, VerifyingKey};
use ark_relations::r1cs::SynthesisError;
use ark_snark::SNARK;
use ark_std::rand::{CryptoRng, RngCore};

/// A Groth16 proof over BLS12-381.
pub type ScoreProof = Proof<Bls12_381>;

/// Generate a Groth16 proof for the score > threshold statement.
///
/// # Arguments
/// * `witness` - The private witness (all score inputs).
/// * `public_inputs` - The public inputs (threshold + commitment).
/// * `pk` - The Groth16 proving key.
/// * `rng` - A CSPRNG.
///
/// # Returns
/// A Groth16 proof.
pub fn generate_proof<R: RngCore + CryptoRng>(
    witness: ScoreWitness,
    public_inputs: ScorePublicInputs,
    pk: &ProvingKey<Bls12_381>,
    rng: &mut R,
) -> Result<ScoreProof, SynthesisError> {
    let circuit = ScoreCircuit::new(Some(witness), public_inputs);
    Groth16::<Bls12_381>::prove(pk, circuit, rng)
}

/// Verify a Groth16 proof for the score > threshold statement.
///
/// # Arguments
/// * `proof` - The Groth16 proof.
/// * `public_inputs` - The public inputs (threshold + commitment).
/// * `vk` - The Groth16 verification key.
///
/// # Returns
/// `true` iff the proof is valid for the supplied public inputs.
pub fn verify_proof(
    proof: &ScoreProof,
    public_inputs: &ScorePublicInputs,
    vk: &VerifyingKey<Bls12_381>,
) -> Result<bool, SynthesisError> {
    // Build the public input vector in the same order the circuit declares them:
    let public_inputs_vec = vec![
        Fr::from(public_inputs.threshold),
        public_inputs.subject,
        public_inputs.credit_oracle_id,
        public_inputs.score_commitment,
        Fr::from(public_inputs.snapshot_ledger),
        public_inputs.domain_separator,
        Fr::from(public_inputs.current_timestamp),
        Fr::from(public_inputs.max_validity_window),
    ];

    Groth16::<Bls12_381>::verify(vk, &public_inputs_vec, proof)
}

/// Generate a fresh proving key and verification key for the circuit.
///
/// **Note:** This is for development/testing only. Production deployments must
/// use a trusted setup ceremony (see `docs/zk-proof-design.md` Step 3).
pub fn generate_test_keys<R: RngCore + CryptoRng>(
    rng: &mut R,
) -> Result<(ProvingKey<Bls12_381>, VerifyingKey<Bls12_381>), SynthesisError> {
    let circuit = ScoreCircuit::new(
        None,
        ScorePublicInputs {
            threshold: 0,
            subject: Fr::from(0u32),
            credit_oracle_id: Fr::from(0u32),
            score_commitment: Fr::from(0u32),
            snapshot_ledger: 0,
            domain_separator: Fr::from(0u32),
            current_timestamp: 0,
            max_validity_window: 0,
        },
    );
    Groth16::<Bls12_381>::circuit_specific_setup(circuit, rng)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commitment::PedersenCommitment;
    use ark_std::rand::rngs::StdRng;
    use ark_std::rand::SeedableRng;

    fn make_witness() -> ScoreWitness {
        ScoreWitness {
            score: 668,
            vc_count: 10,
            tx_volume_30d: 50_000_000_000,
            avg_counterparties: 10,
            repayment_rate: 5000,
            last_updated: 1_700_000_000,
            computed_at_ledger: 1234567,
            stale: false,
            score_timestamp: 1_700_000_000,
            vc_points: 50,
            on_time_count: 50,
            total_count: 100,
            total_repaid: 20_000_000_000,
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
            vc_score: 50,
            volume_score: 80,
            counterparty_bonus: 2,
            tx_score: 82,
            repayment_rate_score: 50,
            repayment_volume_score: 100,
            repay_score: 75,
            composite: 67,
            q_volume: 500,
            q_cp: 2,
            q_rv: 200,
            total_count_is_zero: false,
            total_count_inv: 0,
            blinding: Fr::from(42u32),
        }
    }

    fn make_public_inputs(w: &ScoreWitness, threshold: u32) -> ScorePublicInputs {
        let scheme = PedersenCommitment::new(CIRCUIT_DOMAIN);
        let fields = [
            Fr::from(w.score),
            Fr::from(w.vc_count),
            Fr::from(w.tx_volume_30d.max(0) as u64),
            Fr::from(w.avg_counterparties),
            Fr::from(w.repayment_rate),
            Fr::from(w.last_updated),
            Fr::from(w.computed_at_ledger),
            Fr::from(w.stale as u32),
        ];
        let commitment = scheme.commit(&fields, w.blinding);
        ScorePublicInputs {
            threshold,
            subject: Fr::from(1u32),          // dummy for test
            credit_oracle_id: Fr::from(2u32), // dummy for test
            score_commitment: commitment,
            snapshot_ledger: 12345,           // dummy for test
            domain_separator: Fr::from(3u32), // dummy for test
            current_timestamp: 1_700_000_100,
            max_validity_window: 3600,
        }
    }

    fn big_rng() -> StdRng {
        StdRng::from_seed([0u8; 32])
    }

    #[test]
    fn generate_and_verify_proof() {
        let mut rng = big_rng();
        let (pk, vk) = generate_test_keys(&mut rng).unwrap();

        let w = make_witness();
        let public_inputs = make_public_inputs(&w, 600);

        let proof = generate_proof(w, public_inputs.clone(), &pk, &mut rng).unwrap();
        let valid = verify_proof(&proof, &public_inputs, &vk).unwrap();
        assert!(valid, "proof should verify");
    }

    #[test]
    fn verify_wasm_proof_fixture() {
        use ark_serialize::CanonicalSerialize;
        use sha2::{Digest, Sha256};

        let mut rng = big_rng();
        let (pk, vk) = generate_test_keys(&mut rng).unwrap();
        let witness = make_witness();
        let mut inputs = make_public_inputs(&witness, 600);
        inputs.subject = Fr::from(0u32);
        inputs.credit_oracle_id = Fr::from(0u32);
        inputs.domain_separator = Fr::from(0u32);
        let proof = generate_proof(witness, inputs.clone(), &pk, &mut rng).unwrap();
        assert!(verify_proof(&proof, &inputs, &vk).unwrap());
        let mut bytes = Vec::new();
        proof.a.serialize_uncompressed(&mut bytes).unwrap();
        proof.b.serialize_uncompressed(&mut bytes).unwrap();
        proof.c.serialize_uncompressed(&mut bytes).unwrap();
        let expected = [0x77, 0xff, 0xa1, 0x11, 0x06, 0x6e, 0x81, 0x99, 0x39, 0x0c, 0x42, 0xc3, 0xdd, 0xfc, 0x58, 0xd8, 0x4e, 0x85, 0xaa, 0x86, 0xf2, 0x4d, 0xf6, 0xb5, 0xdf, 0xed, 0x18, 0xb0, 0x7b, 0xa5, 0x5e, 0xd5];
        assert_eq!(&Sha256::digest(&bytes)[..], &expected);
    }

    #[test]
    fn verify_rejects_wrong_threshold() {
        let mut rng = big_rng();
        let (pk, vk) = generate_test_keys(&mut rng).unwrap();

        let w = make_witness();
        let public_inputs = make_public_inputs(&w, 600);

        let proof = generate_proof(w, public_inputs.clone(), &pk, &mut rng).unwrap();

        // Verify with a different threshold -> should fail.
        let wrong_inputs = ScorePublicInputs {
            threshold: 700,
            subject: public_inputs.subject,
            credit_oracle_id: public_inputs.credit_oracle_id,
            score_commitment: public_inputs.score_commitment,
            snapshot_ledger: public_inputs.snapshot_ledger,
            domain_separator: public_inputs.domain_separator,
            current_timestamp: public_inputs.current_timestamp,
            max_validity_window: public_inputs.max_validity_window,
        };
        let valid = verify_proof(&proof, &wrong_inputs, &vk).unwrap();
        assert!(!valid, "proof should NOT verify with wrong threshold");
    }

    #[test]
    fn verify_rejects_tampered_current_timestamp() {
        let mut rng = big_rng();
        let (pk, vk) = generate_test_keys(&mut rng).unwrap();

        let w = make_witness();
        let public_inputs = make_public_inputs(&w, 600);

        let proof = generate_proof(w, public_inputs.clone(), &pk, &mut rng).unwrap();

        // Verify with altered current_timestamp -> should fail.
        let mut tampered_inputs = public_inputs.clone();
        tampered_inputs.current_timestamp += 10;
        let valid = verify_proof(&proof, &tampered_inputs, &vk).unwrap();
        assert!(
            !valid,
            "proof should NOT verify with tampered current_timestamp"
        );
    }

    #[test]
    fn verify_rejects_tampered_max_validity_window() {
        let mut rng = big_rng();
        let (pk, vk) = generate_test_keys(&mut rng).unwrap();

        let w = make_witness();
        let public_inputs = make_public_inputs(&w, 600);

        let proof = generate_proof(w, public_inputs.clone(), &pk, &mut rng).unwrap();

        // Verify with altered max_validity_window -> should fail.
        let mut tampered_inputs = public_inputs.clone();
        tampered_inputs.max_validity_window = 7200;
        let valid = verify_proof(&proof, &tampered_inputs, &vk).unwrap();
        assert!(
            !valid,
            "proof should NOT verify with tampered max_validity_window"
        );
    }
}
