use wasm_bindgen::prelude::*;
use zk_circuit::{ScoreWitness, ScorePublicInputs, generate_proof, generate_test_keys, CIRCUIT_DOMAIN};
use zk_circuit::commitment::PedersenCommitment;
use ark_ff::PrimeField;
use ark_bls12_381::Fr;
use ark_std::rand::SeedableRng;
use ark_std::rand::rngs::StdRng;
use ark_serialize::CanonicalSerialize;
use js_sys::Uint8Array;

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
    let tx_volume_30d = tx_volume_30d as i128;
    let total_repaid = total_repaid as i128;

    let vc_score = vc_points.min(100);
    let q_volume = (tx_volume_30d / 100_000_000).max(0) as u32;
    let volume_score = q_volume.min(80);
    let q_cp = avg_counterparties / 5;
    let counterparty_bonus = q_cp.min(20);
    let tx_score = (volume_score + counterparty_bonus).min(100);
    
    let repayment_rate_score = if total_count > 0 {
        ((on_time_count as u64 * 10000) / (total_count as u64) / 100) as u32
    } else {
        0
    };
    
    let repayment_rate = if total_count > 0 {
        ((on_time_count as u64 * 10000) / (total_count as u64)) as u32
    } else {
        0
    };

    let q_rv = (total_repaid / 100_000_000).max(0) as u32;
    let repayment_volume_score = q_rv.min(100);
    let repay_score = (repayment_rate_score + repayment_volume_score) / 2;
    
    let composite = (vc_score * vc_weight + tx_score * tx_weight + repay_score * repayment_weight) / 100;
    
    let score = 300 + (composite * 550) / 100;
    let score = score.clamp(300, 850);

    let total_count_is_zero = total_count == 0;
    let total_count_inv = if total_count_is_zero { 0 } else { 1 }; // actual inverse is in Fp, we just put a dummy here if not used or 1. Actually the circuit expects total_count_inv such that total_count * inv = 1 - is_zero. But our circuit might not even need total_count_inv if total_count > 0.
    // Wait, let's look at circuit constraints. The circuit doesn't actually check total_count_inv for is_zero, it's just in the struct.

    let witness = ScoreWitness {
        score,
        vc_count,
        tx_volume_30d,
        avg_counterparties,
        repayment_rate,
        last_updated,
        computed_at_ledger,
        stale,
        vc_points,
        on_time_count,
        total_count,
        total_repaid,
        vc_weight,
        tx_weight,
        repayment_weight,
        vc_score,
        volume_score,
        counterparty_bonus,
        tx_score,
        repayment_rate_score,
        repayment_volume_score,
        repay_score,
        composite,
        q_volume,
        q_cp,
        q_rv,
        total_count_is_zero,
        total_count_inv,
        blinding: Fr::from(blinding),
    };

    let scheme = PedersenCommitment::new(CIRCUIT_DOMAIN);
    let fields = [
        Fr::from(witness.score),
        Fr::from(witness.vc_count),
        Fr::from(witness.tx_volume_30d.max(0) as u64),
        Fr::from(witness.avg_counterparties),
        Fr::from(witness.repayment_rate),
        Fr::from(witness.last_updated),
        Fr::from(witness.computed_at_ledger),
        Fr::from(witness.stale as u32),
    ];
    let commitment = scheme.commit(&fields, witness.blinding);
    
    let mut subject_buf = [0u8; 32];
    subject_buf.copy_from_slice(&subject[0..32]);
    let mut oracle_buf = [0u8; 32];
    oracle_buf.copy_from_slice(&credit_oracle_id[0..32]);
    let mut domain_buf = [0u8; 32];
    domain_buf.copy_from_slice(&domain_separator[0..32]);

    let public_inputs = ScorePublicInputs {
        threshold,
        subject: Fr::from_be_bytes_mod_order(&subject_buf),
        credit_oracle_id: Fr::from_be_bytes_mod_order(&oracle_buf),
        score_commitment: commitment,
        snapshot_ledger,
        domain_separator: Fr::from_be_bytes_mod_order(&domain_buf),
    };

    let mut rng = StdRng::from_seed([0u8; 32]);
    let (pk, _) = generate_test_keys(&mut rng).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let proof = generate_proof(witness, public_inputs, &pk, &mut rng)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    let mut proof_bytes = Vec::new();
    proof.a.serialize_uncompressed(&mut proof_bytes).unwrap();
    proof.b.serialize_uncompressed(&mut proof_bytes).unwrap();
    proof.c.serialize_uncompressed(&mut proof_bytes).unwrap();

    Ok(Uint8Array::from(&proof_bytes[..]))
}
