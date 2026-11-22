//! WASM bindings for the score proof system.
//!
//! The scoring/witness derivation and the Poseidon commitment live in
//! `zk-circuit`; this crate only adapts them to JS types. All logic is in
//! plain Rust functions (`*_inner`) so it is unit-testable natively.

use ark_bls12_381::Fr;
use ark_ff::PrimeField;
use ark_serialize::CanonicalSerialize;
use ark_std::rand::rngs::StdRng;
use ark_std::rand::SeedableRng;
use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;
use zk_circuit::commitment::commitment_to_bytes_be;
use zk_circuit::{
    generate_proof, generate_test_keys, generate_tier_proof, generate_tier_test_keys,
    threshold_statement_holds, tier_for_score, tier_statement_holds, ScoreInputs,
    ScorePublicInputs, ScoreWitness, TierPublicInputs, TIER_COUNT,
};

/// Maximum score age accepted by the freshness constraint (30 days, as before).
const MAX_VALIDITY_WINDOW_SECS: u64 = 86_400 * 30;

/// Raw (flat) arguments shared by every entry point.
struct RawArgs {
    inputs: ScoreInputs,
    blinding: u32,
}

#[allow(clippy::too_many_arguments)]
fn raw_args(
    vc_points: u32,
    tx_volume_30d: f64,
    avg_counterparties: u32,
    on_time_count: u32,
    total_count: u32,
    total_repaid: f64,
    vc_weight: u32,
    tx_weight: u32,
    repayment_weight: u32,
    vc_count: u32,
    last_updated: u64,
    computed_at_ledger: u32,
    stale: bool,
    blinding: u32,
) -> RawArgs {
    RawArgs {
        inputs: ScoreInputs {
            vc_points,
            tx_volume_30d: tx_volume_30d as i128,
            avg_counterparties,
            on_time_count,
            total_count,
            total_repaid: total_repaid as i128,
            vc_weight,
            tx_weight,
            repayment_weight,
            vc_count,
            last_updated,
            computed_at_ledger,
            stale,
            // The score is treated as recorded at `last_updated`; the verifier
            // time bound below is the same instant (age 0).
            score_timestamp: last_updated,
        },
        blinding,
    }
}

fn witness(args: &RawArgs) -> Result<ScoreWitness, String> {
    ScoreWitness::from_inputs(&args.inputs, Fr::from(args.blinding)).map_err(|e| e.to_string())
}

fn field_from_32(name: &str, bytes: &[u8]) -> Result<Fr, String> {
    if bytes.len() != 32 {
        return Err(format!(
            "{name} must be exactly 32 bytes, got {}",
            bytes.len()
        ));
    }
    Ok(Fr::from_be_bytes_mod_order(bytes))
}

fn proof_to_bytes(proof: &zk_circuit::ScoreProof) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for r in [
        proof.a.serialize_uncompressed(&mut out),
        proof.b.serialize_uncompressed(&mut out),
        proof.c.serialize_uncompressed(&mut out),
    ] {
        r.map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn commitment_inner(args: &RawArgs) -> Result<[u8; 32], String> {
    Ok(commitment_to_bytes_be(witness(args)?.commitment()))
}

#[allow(clippy::too_many_arguments)]
fn threshold_proof_inner(
    args: &RawArgs,
    threshold: u32,
    subject: &[u8],
    credit_oracle_id: &[u8],
    snapshot_ledger: u32,
    domain_separator: &[u8],
) -> Result<Vec<u8>, String> {
    let w = witness(args)?;
    let public_inputs = ScorePublicInputs {
        threshold,
        subject: field_from_32("subject", subject)?,
        credit_oracle_id: field_from_32("credit_oracle_id", credit_oracle_id)?,
        score_commitment: w.commitment(),
        snapshot_ledger,
        domain_separator: field_from_32("domain_separator", domain_separator)?,
        current_timestamp: args.inputs.last_updated,
        max_validity_window: MAX_VALIDITY_WINDOW_SECS,
    };
    if !threshold_statement_holds(&w, &public_inputs) {
        return Err(if w.score <= threshold {
            format!(
                "score {} is not greater than threshold {threshold}",
                w.score
            )
        } else {
            "witness does not satisfy the score circuit".to_string()
        });
    }
    let mut rng = StdRng::from_seed([0u8; 32]);
    let (pk, _) = generate_test_keys(&mut rng).map_err(|e| e.to_string())?;
    let proof = generate_proof(w, public_inputs, &pk, &mut rng).map_err(|e| e.to_string())?;
    proof_to_bytes(&proof)
}

#[allow(clippy::too_many_arguments)]
fn tier_proof_inner(
    args: &RawArgs,
    tier_level: u32,
    subject: &[u8],
    credit_oracle_id: &[u8],
    snapshot_ledger: u32,
    domain_separator: &[u8],
) -> Result<Vec<u8>, String> {
    if tier_level == 0 || tier_level > TIER_COUNT {
        return Err(format!(
            "tier_level must be in 1..={TIER_COUNT}, got {tier_level}"
        ));
    }
    let w = witness(args)?;
    let public_inputs = TierPublicInputs {
        tier_level,
        subject: field_from_32("subject", subject)?,
        credit_oracle_id: field_from_32("credit_oracle_id", credit_oracle_id)?,
        score_commitment: w.commitment(),
        snapshot_ledger,
        domain_separator: field_from_32("domain_separator", domain_separator)?,
        current_timestamp: args.inputs.last_updated,
        max_validity_window: MAX_VALIDITY_WINDOW_SECS,
    };
    if !tier_statement_holds(&w, &public_inputs) {
        return Err(format!(
            "score {} is in tier {}, not tier {tier_level}",
            w.score,
            tier_for_score(w.score)
        ));
    }
    let mut rng = StdRng::from_seed([0u8; 32]);
    let (pk, _) = generate_tier_test_keys(&mut rng).map_err(|e| e.to_string())?;
    let proof = generate_tier_proof(w, public_inputs, &pk, &mut rng).map_err(|e| e.to_string())?;
    proof_to_bytes(&proof)
}

fn js_err(e: String) -> JsValue {
    JsValue::from_str(&e)
}

/// Generate a Groth16 proof that the subject's score is `> threshold`.
/// Returns `A || B || C` (arkworks uncompressed encoding).
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn generate_score_proof(
    vc_points: u32,
    tx_volume_30d: f64,
    avg_counterparties: u32,
    on_time_count: u32,
    total_count: u32,
    total_repaid: f64,
    vc_weight: u32,
    tx_weight: u32,
    repayment_weight: u32,
    vc_count: u32,
    last_updated: u64,
    computed_at_ledger: u32,
    stale: bool,
    blinding: u32,
    threshold: u32,
    subject: &[u8],
    credit_oracle_id: &[u8],
    snapshot_ledger: u32,
    domain_separator: &[u8],
) -> Result<Uint8Array, JsValue> {
    let args = raw_args(
        vc_points,
        tx_volume_30d,
        avg_counterparties,
        on_time_count,
        total_count,
        total_repaid,
        vc_weight,
        tx_weight,
        repayment_weight,
        vc_count,
        last_updated,
        computed_at_ledger,
        stale,
        blinding,
    );
    let bytes = threshold_proof_inner(
        &args,
        threshold,
        subject,
        credit_oracle_id,
        snapshot_ledger,
        domain_separator,
    )
    .map_err(js_err)?;
    Ok(Uint8Array::from(&bytes[..]))
}

/// Generate a Groth16 proof that the subject's score lies in tier
/// `tier_level` (1 = Bronze >= 600, 2 = Silver >= 700, 3 = Gold >= 800).
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn generate_tier_score_proof(
    vc_points: u32,
    tx_volume_30d: f64,
    avg_counterparties: u32,
    on_time_count: u32,
    total_count: u32,
    total_repaid: f64,
    vc_weight: u32,
    tx_weight: u32,
    repayment_weight: u32,
    vc_count: u32,
    last_updated: u64,
    computed_at_ledger: u32,
    stale: bool,
    blinding: u32,
    tier_level: u32,
    subject: &[u8],
    credit_oracle_id: &[u8],
    snapshot_ledger: u32,
    domain_separator: &[u8],
) -> Result<Uint8Array, JsValue> {
    let args = raw_args(
        vc_points,
        tx_volume_30d,
        avg_counterparties,
        on_time_count,
        total_count,
        total_repaid,
        vc_weight,
        tx_weight,
        repayment_weight,
        vc_count,
        last_updated,
        computed_at_ledger,
        stale,
        blinding,
    );
    let bytes = tier_proof_inner(
        &args,
        tier_level,
        subject,
        credit_oracle_id,
        snapshot_ledger,
        domain_separator,
    )
    .map_err(js_err)?;
    Ok(Uint8Array::from(&bytes[..]))
}

/// The Poseidon `score_commitment` (32 bytes, big-endian) for the given
/// inputs. This is the exact value the circuit checks, so SDKs must use it
/// rather than computing their own hash.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn compute_score_commitment(
    vc_points: u32,
    tx_volume_30d: f64,
    avg_counterparties: u32,
    on_time_count: u32,
    total_count: u32,
    total_repaid: f64,
    vc_weight: u32,
    tx_weight: u32,
    repayment_weight: u32,
    vc_count: u32,
    last_updated: u64,
    computed_at_ledger: u32,
    stale: bool,
    blinding: u32,
) -> Result<Uint8Array, JsValue> {
    let args = raw_args(
        vc_points,
        tx_volume_30d,
        avg_counterparties,
        on_time_count,
        total_count,
        total_repaid,
        vc_weight,
        tx_weight,
        repayment_weight,
        vc_count,
        last_updated,
        computed_at_ledger,
        stale,
        blinding,
    );
    let bytes = commitment_inner(&args).map_err(js_err)?;
    Ok(Uint8Array::from(&bytes[..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> RawArgs {
        raw_args(
            50,
            50_000_000_000.0,
            10,
            50,
            100,
            20_000_000_000.0,
            40,
            30,
            30,
            10,
            1_700_000_000,
            1_234_567,
            false,
            42,
        )
    }

    #[test]
    fn commitment_matches_circuit_witness() {
        let a = args();
        let w = witness(&a).unwrap();
        assert_eq!(w.score, 668);
        assert_eq!(
            commitment_inner(&a).unwrap(),
            commitment_to_bytes_be(w.commitment())
        );
    }

    #[test]
    fn commitment_depends_on_blinding() {
        let mut a = args();
        let c1 = commitment_inner(&a).unwrap();
        a.blinding = 43;
        assert_ne!(c1, commitment_inner(&a).unwrap());
    }

    #[test]
    fn short_byte_inputs_error_instead_of_panicking() {
        let a = args();
        let ok = [1u8; 32];
        assert!(threshold_proof_inner(&a, 600, &[1u8; 31], &ok, 1, &ok).is_err());
        assert!(threshold_proof_inner(&a, 600, &ok, &[], 1, &ok).is_err());
        assert!(tier_proof_inner(&a, 1, &ok, &ok, 1, &[0u8; 33]).is_err());
    }

    #[test]
    fn invalid_tier_levels_and_weights_are_errors() {
        let ok = [1u8; 32];
        for t in [0u32, 4, u32::MAX] {
            assert!(tier_proof_inner(&args(), t, &ok, &ok, 1, &ok).is_err());
        }
        let mut bad = args();
        bad.inputs.vc_weight = 90; // 90 + 30 + 30 != 100
        assert!(commitment_inner(&bad).is_err());
    }

    #[test]
    fn unsatisfiable_statements_are_errors_not_bogus_proofs() {
        let ok = [1u8; 32];
        // Score 668: tier 1 only; threshold must be < 668.
        let e = tier_proof_inner(&args(), 3, &ok, &ok, 1, &ok).unwrap_err();
        assert!(e.contains("tier 1") && e.contains("tier 3"), "{e}");
        assert!(tier_proof_inner(&args(), 2, &ok, &ok, 1, &ok).is_err());
        let e = threshold_proof_inner(&args(), 700, &ok, &ok, 1, &ok).unwrap_err();
        assert!(e.contains("668") && e.contains("700"), "{e}");
        assert!(threshold_proof_inner(&args(), 668, &ok, &ok, 1, &ok).is_err());
    }

    /// The proof the WASM entry points emit must verify under exactly the
    /// public inputs they construct (timestamps included), for both circuits,
    /// and must be rejected under any other claim.
    #[test]
    fn emitted_proofs_verify_under_the_public_inputs_they_commit_to() {
        use ark_serialize::CanonicalDeserialize;
        use zk_circuit::{verify_proof, verify_tier_proof, ScoreProof};

        fn parse(bytes: &[u8]) -> ScoreProof {
            let mut r = bytes;
            let a = ark_bls12_381::G1Affine::deserialize_uncompressed(&mut r).unwrap();
            let b = ark_bls12_381::G2Affine::deserialize_uncompressed(&mut r).unwrap();
            let c = ark_bls12_381::G1Affine::deserialize_uncompressed(&mut r).unwrap();
            assert!(r.is_empty());
            ScoreProof { a, b, c }
        }

        let a = args();
        let w = witness(&a).unwrap();
        let zero = [0u8; 32];
        let mut rng = StdRng::from_seed([0u8; 32]);

        let threshold_public = ScorePublicInputs {
            threshold: 600,
            subject: Fr::from(0u32),
            credit_oracle_id: Fr::from(0u32),
            score_commitment: w.commitment(),
            snapshot_ledger: 12345,
            domain_separator: Fr::from(0u32),
            current_timestamp: a.inputs.last_updated,
            max_validity_window: MAX_VALIDITY_WINDOW_SECS,
        };
        let (_, vk) = generate_test_keys(&mut rng).unwrap();
        let proof = parse(&threshold_proof_inner(&a, 600, &zero, &zero, 12345, &zero).unwrap());
        assert!(verify_proof(&proof, &threshold_public, &vk).unwrap());
        let mut other = threshold_public.clone();
        other.threshold = 601;
        assert!(!verify_proof(&proof, &other, &vk).unwrap());

        let tier_public = TierPublicInputs {
            tier_level: 1,
            subject: Fr::from(0u32),
            credit_oracle_id: Fr::from(0u32),
            score_commitment: w.commitment(),
            snapshot_ledger: 12345,
            domain_separator: Fr::from(0u32),
            current_timestamp: a.inputs.last_updated,
            max_validity_window: MAX_VALIDITY_WINDOW_SECS,
        };
        let mut rng = StdRng::from_seed([0u8; 32]);
        let (_, tier_vk) = generate_tier_test_keys(&mut rng).unwrap();
        let proof = parse(&tier_proof_inner(&a, 1, &zero, &zero, 12345, &zero).unwrap());
        assert!(verify_tier_proof(&proof, &tier_public, &tier_vk).unwrap());
        let mut other = tier_public.clone();
        other.tier_level = 2;
        assert!(!verify_tier_proof(&proof, &other, &tier_vk).unwrap());
    }

    #[test]
    fn threshold_and_tier_proofs_serialize_to_expected_length() {
        let ok = [1u8; 32];
        // A (96) || B (192) || C (96) uncompressed.
        assert_eq!(
            threshold_proof_inner(&args(), 600, &ok, &ok, 7, &ok)
                .unwrap()
                .len(),
            384
        );
        assert_eq!(
            tier_proof_inner(&args(), 1, &ok, &ok, 7, &ok)
                .unwrap()
                .len(),
            384
        );
    }
}
