//! Groth16 R1CS circuit for `score > threshold`.
//!
//! The circuit re-implements `compute_score_pure` from the credit-oracle
//! contract as R1CS constraints, commits all private inputs with a Pedersen
//! vector commitment, and asserts `score > threshold` via a range proof.

use ark_bls12_381::Fr;
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::prelude::*;
use ark_r1cs_std::uint32::UInt32;
use ark_r1cs_std::uint64::UInt64;
use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};

use crate::commitment::PedersenCommitment;
use crate::range::{boolean_to_fp, enforce_u32_gt_fp, uint32_to_fp, uint64_to_fp};

/// Domain separator for the circuit (binds to scoring-spec revision).
pub const CIRCUIT_DOMAIN: &[u8] = b"stellar-did-credit::score-gt-threshold::v1";

/// Public inputs for the circuit.
#[derive(Clone, Debug)]
pub struct ScorePublicInputs {
    /// The threshold the prover claims `score > threshold`.
    pub threshold: u32,
    /// Commitment to the private ScoreRecord fields (an `Fr` element).
    pub score_commitment: Fr,
}

/// Private witness for the circuit.
#[derive(Clone, Debug)]
pub struct ScoreWitness {
    // ScoreRecord fields
    pub score: u32,
    pub vc_count: u32,
    pub tx_volume_30d: i128,
    pub avg_counterparties: u32,
    pub repayment_rate: u32,
    pub last_updated: u64,
    pub computed_at_ledger: u32,
    pub stale: bool,
    // Raw scoring inputs
    pub vc_points: u32,
    pub on_time_count: u32,
    pub total_count: u32,
    pub total_repaid: i128,
    // Active weights
    pub vc_weight: u32,
    pub tx_weight: u32,
    pub repayment_weight: u32,
    // Intermediate values (computed by the circuit, but provided for clarity)
    pub vc_score: u32,
    pub volume_score: u32,
    pub counterparty_bonus: u32,
    pub tx_score: u32,
    pub repayment_rate_score: u32,
    pub repayment_volume_score: u32,
    pub repay_score: u32,
    pub composite: u32,
    // Integer-division quotients (witness-provided, constrained by the circuit)
    pub q_volume: u32,
    pub q_cp: u32,
    pub q_rv: u32,
    // total_count == 0 handling
    pub total_count_is_zero: bool,
    pub total_count_inv: u32,
    // Commitment blinding
    pub blinding: Fr,
}

impl Default for ScoreWitness {
    fn default() -> Self {
        Self {
            score: 300,
            vc_count: 0,
            tx_volume_30d: 0,
            avg_counterparties: 0,
            repayment_rate: 0,
            last_updated: 0,
            computed_at_ledger: 0,
            stale: false,
            vc_points: 0,
            on_time_count: 0,
            total_count: 0,
            total_repaid: 0,
            vc_weight: 60,
            tx_weight: 0,
            repayment_weight: 40,
            vc_score: 0,
            volume_score: 0,
            counterparty_bonus: 0,
            tx_score: 0,
            repayment_rate_score: 0,
            repayment_volume_score: 0,
            repay_score: 0,
            composite: 0,
            q_volume: 0,
            q_cp: 0,
            q_rv: 0,
            total_count_is_zero: false,
            total_count_inv: 0,
            blinding: Fr::from(0u32),
        }
    }
}

/// The score > threshold circuit.
pub struct ScoreCircuit {
    pub witness: Option<ScoreWitness>,
    pub public_inputs: ScorePublicInputs,
}

impl ScoreCircuit {
    pub fn new(witness: Option<ScoreWitness>, public_inputs: ScorePublicInputs) -> Self {
        Self {
            witness,
            public_inputs,
        }
    }
}

impl ConstraintSynthesizer<Fr> for ScoreCircuit {
    fn generate_constraints(
        self,
        cs: ConstraintSystemRef<Fr>,
    ) -> Result<(), SynthesisError> {
        let w = self.witness.as_ref();

        // ---- Public inputs ----
        let threshold = FpVar::new_input(cs.clone(), || {
            Ok(Fr::from(self.public_inputs.threshold))
        })?;
        let commitment = FpVar::new_input(cs.clone(), || {
            Ok(self.public_inputs.score_commitment)
        })?;

        // ---- Private witness ----
        let score = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.score).unwrap_or(0)))?;
        let vc_count = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.vc_count).unwrap_or(0)))?;
        let tx_volume_30d = UInt64::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.tx_volume_30d.max(0) as u64).unwrap_or(0))
        })?;
        let avg_counterparties = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.avg_counterparties).unwrap_or(0))
        })?;
        let repayment_rate = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.repayment_rate).unwrap_or(0))
        })?;
        let last_updated = UInt64::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.last_updated).unwrap_or(0))
        })?;
        let computed_at_ledger = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.computed_at_ledger).unwrap_or(0))
        })?;
        let stale = Boolean::new_witness(cs.clone(), || Ok(w.map(|w| w.stale).unwrap_or(false)))?;

        // Raw scoring inputs
        let vc_points = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.vc_points).unwrap_or(0))
        })?;
        let on_time_count = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.on_time_count).unwrap_or(0))
        })?;
        let total_count = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.total_count).unwrap_or(0))
        })?;
        let total_repaid = UInt64::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.total_repaid.max(0) as u64).unwrap_or(0))
        })?;
        let vc_weight = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.vc_weight).unwrap_or(0))
        })?;
        let tx_weight = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.tx_weight).unwrap_or(0))
        })?;
        let repayment_weight = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.repayment_weight).unwrap_or(0))
        })?;

        // Intermediate values
        let vc_score = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.vc_score).unwrap_or(0))
        })?;
        let volume_score = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.volume_score).unwrap_or(0))
        })?;
        let counterparty_bonus = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.counterparty_bonus).unwrap_or(0))
        })?;
        let tx_score = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.tx_score).unwrap_or(0))
        })?;
        let repayment_rate_score = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.repayment_rate_score).unwrap_or(0))
        })?;
        let repayment_volume_score = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.repayment_volume_score).unwrap_or(0))
        })?;
        let repay_score = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.repay_score).unwrap_or(0))
        })?;
        let composite = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.composite).unwrap_or(0))
        })?;

        // Integer-division quotients
        let q_volume = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.q_volume).unwrap_or(0))
        })?;
        let q_cp = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.q_cp).unwrap_or(0))
        })?;
        let q_rv = UInt32::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.q_rv).unwrap_or(0))
        })?;

        let blinding = FpVar::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.blinding).unwrap_or(Fr::from(0u32)))
        })?;

        // ---- Constraint 1: score > threshold ----
        enforce_u32_gt_fp(&score, &threshold)?;

        // ---- Constraint 2: vc_score == min(vc_points, 100) ----
        // vc_score <= 100 (range check, 7 bits since 100 < 2^7 = 128)
        let vc_score_bits = vc_score.to_bits_le();
        for b in vc_score_bits.iter().skip(7) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // vc_score <= vc_points
        // (vc_score - vc_points) * (vc_score - 100) == 0
        let vc_score_fp = uint32_to_fp(&vc_score);
        let vc_points_fp = uint32_to_fp(&vc_points);
        let hundred = FpVar::constant(Fr::from(100u32));
        let diff1 = vc_score_fp.clone() - vc_points_fp;
        let diff2 = vc_score_fp.clone() - hundred;
        let product = diff1 * diff2;
        product.enforce_equal(&FpVar::constant(Fr::from(0u32)))?;

        // ---- Constraint 3: volume_score == min(tx_volume_30d / 100_000_000, 80) ----
        // q_volume = tx_volume_30d / 100_000_000
        // tx_volume_30d = q_volume * 100_000_000 + r, 0 <= r < 100_000_000
        let volume_divisor = FpVar::constant(Fr::from(100_000_000u64));
        let tx_volume_fp = uint64_to_fp(&tx_volume_30d);
        let q_volume_fp = uint32_to_fp(&q_volume);
        let min_volume = q_volume_fp.clone() * volume_divisor;
        let vol_diff = tx_volume_fp.clone() - min_volume;
        // vol_diff < 100_000_000 (27 bits since 2^27 = 134,217,728 > 100,000,000)
        let vol_diff_bits = vol_diff.to_bits_le()?;
        for b in vol_diff_bits.iter().skip(27) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // volume_score == min(q_volume, 80)
        // volume_score <= 80 (range check, 7 bits)
        let volume_score_bits = volume_score.to_bits_le();
        for b in volume_score_bits.iter().skip(7) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // (volume_score - q_volume) * (volume_score - 80) == 0
        let volume_score_fp = uint32_to_fp(&volume_score);
        let eighty = FpVar::constant(Fr::from(80u32));
        let d1 = volume_score_fp.clone() - q_volume_fp;
        let d2 = volume_score_fp.clone() - eighty;
        let prod = d1 * d2;
        prod.enforce_equal(&FpVar::constant(Fr::from(0u32)))?;

        // ---- Constraint 4: counterparty_bonus == min(avg_counterparties / 5, 20) ----
        // q_cp = avg_counterparties / 5
        // avg_counterparties = q_cp * 5 + r, 0 <= r < 5
        let cp_divisor = FpVar::constant(Fr::from(5u32));
        let avg_cp_fp = uint32_to_fp(&avg_counterparties);
        let q_cp_fp = uint32_to_fp(&q_cp);
        let min_cp = q_cp_fp.clone() * cp_divisor;
        let cp_diff = avg_cp_fp.clone() - min_cp;
        // cp_diff < 5 (3 bits since 2^3 = 8 > 5)
        let cp_diff_bits = cp_diff.to_bits_le()?;
        for b in cp_diff_bits.iter().skip(3) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // counterparty_bonus == min(q_cp, 20)
        // counterparty_bonus <= 20 (range check, 5 bits since 20 < 2^5 = 32)
        let cp_bonus_bits = counterparty_bonus.to_bits_le();
        for b in cp_bonus_bits.iter().skip(5) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // (counterparty_bonus - q_cp) * (counterparty_bonus - 20) == 0
        let cp_bonus_fp = uint32_to_fp(&counterparty_bonus);
        let twenty = FpVar::constant(Fr::from(20u32));
        let d1 = cp_bonus_fp.clone() - q_cp_fp;
        let d2 = cp_bonus_fp.clone() - twenty;
        let prod = d1 * d2;
        prod.enforce_equal(&FpVar::constant(Fr::from(0u32)))?;

        // ---- Constraint 5: tx_score == min(volume_score + counterparty_bonus, 100) ----
        // tx_score <= 100 (range check, 7 bits)
        let tx_score_bits = tx_score.to_bits_le();
        for b in tx_score_bits.iter().skip(7) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // (tx_score - (volume_score + counterparty_bonus)) * (tx_score - 100) == 0
        let tx_score_fp = uint32_to_fp(&tx_score);
        let sum = volume_score_fp + cp_bonus_fp;
        let hundred2 = FpVar::constant(Fr::from(100u32));
        let d1 = tx_score_fp.clone() - sum;
        let d2 = tx_score_fp.clone() - hundred2;
        let prod = d1 * d2;
        prod.enforce_equal(&FpVar::constant(Fr::from(0u32)))?;

        // ---- Constraint 6: repayment_rate_score == floor(on_time_count * 100 / total_count) ----
        // repayment_rate_score = (on_time_count * 10000 / total_count) / 100
        //                       = floor(on_time_count * 100 / total_count)
        // Enforce: on_time_count * 100 = repayment_rate_score * total_count + r, 0 <= r < total_count
        // repayment_rate_score <= 100 (range check, 7 bits)
        let rr_score_bits = repayment_rate_score.to_bits_le();
        for b in rr_score_bits.iter().skip(7) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        let on_time_fp = uint32_to_fp(&on_time_count);
        let total_fp = uint32_to_fp(&total_count);
        let hundred3 = FpVar::constant(Fr::from(100u32));
        let rr_score_fp = uint32_to_fp(&repayment_rate_score);
        let on_time_100 = on_time_fp * hundred3;
        let rr_times_total = rr_score_fp.clone() * total_fp.clone();
        let rr_diff = on_time_100 - rr_times_total;
        // rr_diff >= 0 and rr_diff < total_count
        // Range-check rr_diff to 32 bits.
        let rr_diff_bits = rr_diff.to_bits_le()?;
        for b in rr_diff_bits.iter().skip(32) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // Enforce rr_diff < total_count: total_count - rr_diff in [1, 2^32)
        let total_minus_rr = total_fp - rr_diff.clone();
        let tmr_bits = total_minus_rr.to_bits_le()?;
        for b in tmr_bits.iter().skip(32) {
            b.enforce_equal(&Boolean::constant(false))?;
        }

        // ---- Constraint 7: repayment_volume_score == min(total_repaid / 100_000_000, 100) ----
        // q_rv = total_repaid / 100_000_000
        // total_repaid = q_rv * 100_000_000 + r, 0 <= r < 100_000_000
        let total_repaid_fp = uint64_to_fp(&total_repaid);
        let q_rv_fp = uint32_to_fp(&q_rv);
        let rv_divisor = FpVar::constant(Fr::from(100_000_000u64));
        let rv_min = q_rv_fp.clone() * rv_divisor;
        let rv_diff = total_repaid_fp - rv_min;
        // rv_diff < 100_000_000 (27 bits)
        let rv_diff_bits = rv_diff.to_bits_le()?;
        for b in rv_diff_bits.iter().skip(27) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // repayment_volume_score == min(q_rv, 100)
        // repayment_volume_score <= 100 (range check, 7 bits)
        let rv_score_bits = repayment_volume_score.to_bits_le();
        for b in rv_score_bits.iter().skip(7) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // (repayment_volume_score - q_rv) * (repayment_volume_score - 100) == 0
        let rv_score_fp = uint32_to_fp(&repayment_volume_score);
        let hundred6 = FpVar::constant(Fr::from(100u32));
        let d1 = rv_score_fp.clone() - q_rv_fp;
        let d2 = rv_score_fp.clone() - hundred6;
        let prod = d1 * d2;
        prod.enforce_equal(&FpVar::constant(Fr::from(0u32)))?;

        // ---- Constraint 8: repay_score == (repayment_rate_score + repayment_volume_score) / 2 ----
        let repay_score_fp = uint32_to_fp(&repay_score);
        let rr_plus_rv = rr_score_fp + rv_score_fp;
        let repay_doubled = repay_score_fp.clone() * FpVar::constant(Fr::from(2u32));
        repay_doubled.enforce_equal(&rr_plus_rv)?;

        // ---- Constraint 9: composite == (vc_score*vc_w + tx_score*tx_w + repay_score*repay_w) / 100 ----
        let vc_weight_fp = uint32_to_fp(&vc_weight);
        let tx_weight_fp = uint32_to_fp(&tx_weight);
        let repay_weight_fp = uint32_to_fp(&repayment_weight);
        let composite_fp = uint32_to_fp(&composite);
        let hundred4 = FpVar::constant(Fr::from(100u32));
        let vc_term = vc_score_fp * vc_weight_fp;
        let tx_term = tx_score_fp * tx_weight_fp;
        let repay_term = repay_score_fp * repay_weight_fp;
        let numerator = vc_term + tx_term + repay_term;
        // composite == numerator / 100  =>  numerator == composite * 100 + r, 0 <= r < 100
        let composite_100 = composite_fp.clone() * hundred4;
        let comp_diff = numerator - composite_100;
        // comp_diff < 100 (7 bits)
        let comp_diff_bits = comp_diff.to_bits_le()?;
        for b in comp_diff_bits.iter().skip(7) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // composite <= 100 (range check, 7 bits)
        let composite_bits = composite.to_bits_le();
        for b in composite_bits.iter().skip(7) {
            b.enforce_equal(&Boolean::constant(false))?;
        }

        // ---- Constraint 10: score == 300 + composite * 550 / 100 ----
        // composite * 550 = (score - 300) * 100 + r, 0 <= r < 100
        let score_fp = uint32_to_fp(&score);
        let min_score = FpVar::constant(Fr::from(300u32));
        let five_fifty = FpVar::constant(Fr::from(550u32));
        let hundred5 = FpVar::constant(Fr::from(100u32));
        let composite_550 = composite_fp * five_fifty;
        let score_minus_300 = score_fp.clone() - min_score;
        let score_100 = score_minus_300 * hundred5;
        let score_diff = composite_550 - score_100;
        // score_diff < 100 (7 bits)
        let score_diff_bits = score_diff.to_bits_le()?;
        for b in score_diff_bits.iter().skip(7) {
            b.enforce_equal(&Boolean::constant(false))?;
        }
        // score in [300, 850] (range check, 10 bits since 850 < 2^10 = 1024)
        let score_bits = score.to_bits_le();
        for b in score_bits.iter().skip(10) {
            b.enforce_equal(&Boolean::constant(false))?;
        }

        // ---- Constraint 11: Pedersen commitment ----
        // C = sum(field_i * coeff_i) + blinding
        // Check C == public commitment.
        let stale_fp = boolean_to_fp(&stale);
        let fields = [
            score_fp.clone(),
            uint32_to_fp(&vc_count),
            tx_volume_fp.clone(),
            avg_cp_fp.clone(),
            uint32_to_fp(&repayment_rate),
            uint64_to_fp(&last_updated),
            uint32_to_fp(&computed_at_ledger),
            stale_fp,
        ];
        let scheme = PedersenCommitment::new(CIRCUIT_DOMAIN);
        let mut acc = FpVar::constant(Fr::from(0u32));
        for (i, field) in fields.iter().enumerate() {
            let coeff = FpVar::constant(scheme.coefficients[i]);
            acc += field * coeff;
        }
        acc += blinding;
        acc.enforce_equal(&commitment)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_relations::r1cs::{ConstraintSystem, SynthesisMode};

    #[test]
    fn circuit_satisfied_for_valid_witness() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        cs.set_mode(SynthesisMode::Prove {
            construct_matrices: true,
        });

        let w = ScoreWitness {
            score: 668,
            vc_count: 10,
            tx_volume_30d: 50_000_000_000,
            avg_counterparties: 10,
            repayment_rate: 5000,
            last_updated: 1_700_000_000,
            computed_at_ledger: 1234567,
            stale: false,
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
        };

        // Compute the commitment.
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

        let circuit = ScoreCircuit::new(
            Some(w),
            ScorePublicInputs {
                threshold: 600,
                score_commitment: commitment,
            },
        );
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap());
    }
}
