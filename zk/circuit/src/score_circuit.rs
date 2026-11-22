//! Groth16 R1CS circuit for `score > threshold`.
//!
//! The circuit re-implements `compute_score_pure` from the credit-oracle
//! contract as R1CS constraints, commits all private inputs with a Pedersen
//! vector commitment, and asserts `score > threshold` via a range proof.

use ark_bls12_381::Fr;
use ark_ff::Field;
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::prelude::*;
use ark_r1cs_std::uint32::UInt32;
use ark_r1cs_std::uint64::UInt64;
use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};

use crate::commitment::{poseidon_commit, poseidon_commit_gadget, COMMIT_FIELDS, COMMIT_INPUTS};
use crate::range::{
    boolean_to_fp, enforce_u32_gt_fp, enforce_u32_lt_fp, uint32_to_fp, uint64_to_fp,
};

/// Domain separator for the circuit (binds to scoring-spec revision).
///
/// `v2`: the score commitment moved from a Pedersen-style linear combination to
/// a Poseidon hash (issue #1185), which is a breaking circuit change.
pub const CIRCUIT_DOMAIN: &[u8] = b"stellar-did-credit::score-gt-threshold::v2";

/// Public inputs for the circuit.
#[derive(Clone, Debug)]
pub struct ScorePublicInputs {
    /// The threshold the prover claims `score > threshold`.
    pub threshold: u32,
    /// Subject bound to the proof (SHA-256 of XDR).
    pub subject: Fr,
    /// Credit oracle ID (SHA-256 of XDR).
    pub credit_oracle_id: Fr,
    /// Commitment to the private ScoreRecord fields.
    pub score_commitment: Fr,
    /// Snapshot ledger.
    pub snapshot_ledger: u32,
    /// Domain separator.
    pub domain_separator: Fr,
    /// Current timestamp in seconds (verifier time).
    pub current_timestamp: u64,
    /// Maximum validity window in seconds.
    pub max_validity_window: u64,
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
    /// Timestamp when the score was recorded (seconds).
    pub score_timestamp: u64,
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
            score_timestamp: 0,
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

/// Raw (pre-derivation) inputs to the scoring formula plus the committed
/// `ScoreRecord` metadata. [`ScoreWitness::from_inputs`] derives every
/// intermediate value the circuit expects.
#[derive(Clone, Debug, Default)]
pub struct ScoreInputs {
    pub vc_points: u32,
    pub tx_volume_30d: i128,
    pub avg_counterparties: u32,
    pub on_time_count: u32,
    pub total_count: u32,
    pub total_repaid: i128,
    pub vc_weight: u32,
    pub tx_weight: u32,
    pub repayment_weight: u32,
    pub vc_count: u32,
    pub last_updated: u64,
    pub computed_at_ledger: u32,
    pub stale: bool,
    /// Timestamp when the score was recorded (seconds); checked by the
    /// freshness constraint.
    pub score_timestamp: u64,
}

/// Why a [`ScoreInputs`] value cannot be turned into a witness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WitnessError {
    /// `vc_weight + tx_weight + repayment_weight != 100`.
    WeightsDoNotSumTo100,
    /// A quotient does not fit the circuit's 32-bit witness slot.
    ValueOutOfRange,
}

impl core::fmt::Display for WitnessError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WitnessError::WeightsDoNotSumTo100 => {
                write!(f, "vc_weight + tx_weight + repayment_weight must equal 100")
            }
            WitnessError::ValueOutOfRange => write!(f, "scoring input out of range"),
        }
    }
}

impl std::error::Error for WitnessError {}

impl ScoreWitness {
    /// Derive the full witness (score, intermediates, division quotients)
    /// from raw scoring inputs, mirroring `compute_score_pure` exactly.
    pub fn from_inputs(inputs: &ScoreInputs, blinding: Fr) -> Result<Self, WitnessError> {
        if inputs.vc_weight as u64 + inputs.tx_weight as u64 + inputs.repayment_weight as u64 != 100
        {
            return Err(WitnessError::WeightsDoNotSumTo100);
        }
        let to_u32 = |v: u64| u32::try_from(v).map_err(|_| WitnessError::ValueOutOfRange);
        let q = |v: i128, div: i128| -> Result<u32, WitnessError> {
            u32::try_from((v / div).max(0)).map_err(|_| WitnessError::ValueOutOfRange)
        };
        let vc_score = inputs.vc_points.min(100);
        let q_volume = q(inputs.tx_volume_30d, 100_000_000)?;
        let volume_score = q_volume.min(80);
        let q_cp = inputs.avg_counterparties / 5;
        let counterparty_bonus = q_cp.min(20);
        let tx_score = (volume_score + counterparty_bonus).min(100);

        let (repayment_rate_score, repayment_rate) = if inputs.total_count > 0 {
            let scaled = inputs.on_time_count as u64 * 10_000 / inputs.total_count as u64;
            (to_u32(scaled / 100)?, to_u32(scaled)?)
        } else {
            (0, 0)
        };
        let q_rv = q(inputs.total_repaid, 100_000_000)?;
        let repayment_volume_score = q_rv.min(100);
        let repay_score =
            to_u32((repayment_rate_score as u64 + repayment_volume_score as u64) / 2)?;

        let composite = to_u32(
            (vc_score as u64 * inputs.vc_weight as u64
                + tx_score as u64 * inputs.tx_weight as u64
                + repay_score as u64 * inputs.repayment_weight as u64)
                / 100,
        )?;
        let score = (300 + composite as u64 * 550 / 100).clamp(300, 850) as u32;

        Ok(Self {
            score,
            vc_count: inputs.vc_count,
            tx_volume_30d: inputs.tx_volume_30d,
            avg_counterparties: inputs.avg_counterparties,
            repayment_rate,
            last_updated: inputs.last_updated,
            computed_at_ledger: inputs.computed_at_ledger,
            stale: inputs.stale,
            score_timestamp: inputs.score_timestamp,
            vc_points: inputs.vc_points,
            on_time_count: inputs.on_time_count,
            total_count: inputs.total_count,
            total_repaid: inputs.total_repaid,
            vc_weight: inputs.vc_weight,
            tx_weight: inputs.tx_weight,
            repayment_weight: inputs.repayment_weight,
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
            total_count_is_zero: inputs.total_count == 0,
            total_count_inv: if inputs.total_count == 0 { 0 } else { 1 },
            blinding,
        })
    }
}

impl ScoreWitness {
    /// The 8 committed `ScoreRecord` fields as field elements, in commitment
    /// order (see `commitment::poseidon_commit`).
    pub fn commitment_fields(&self) -> [Fr; COMMIT_FIELDS] {
        [
            Fr::from(self.score),
            Fr::from(self.vc_count),
            Fr::from(self.tx_volume_30d.max(0) as u64),
            Fr::from(self.avg_counterparties),
            Fr::from(self.repayment_rate),
            Fr::from(self.last_updated),
            Fr::from(self.computed_at_ledger),
            Fr::from(self.stale as u32),
        ]
    }

    /// The Poseidon commitment to this witness's `ScoreRecord` fields.
    pub fn commitment(&self) -> Fr {
        poseidon_commit(&self.commitment_fields(), self.blinding)
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

/// Public-input values shared by every statement over the score record.
struct PublicCommon {
    subject: Fr,
    credit_oracle_id: Fr,
    score_commitment: Fr,
    snapshot_ledger: u32,
    domain_separator: Fr,
    current_timestamp: u64,
    max_validity_window: u64,
}

/// Allocated public-input variables plus the plain timestamp values the
/// freshness constraint needs to compute its witness.
struct PublicVars {
    /// Statement-specific input: `threshold` or `tier_level`.
    slot0: FpVar<Fr>,
    commitment: FpVar<Fr>,
    current_timestamp: FpVar<Fr>,
    max_validity_window: FpVar<Fr>,
    current_timestamp_val: u64,
    max_validity_window_val: u64,
}

/// Allocate the eight public inputs in the order shared by every statement
/// over the score record: `[slot0, subject, credit_oracle_id,
/// score_commitment, snapshot_ledger, domain_separator, current_timestamp,
/// max_validity_window]`.
///
/// `slot0` is the statement-specific input: the `threshold` for
/// [`ScoreCircuit`], the `tier_level` for [`ScoreTierCircuit`]. Keeping the
/// shape identical means the on-chain verifier ABI is the same for both.
fn alloc_public_inputs(
    cs: ConstraintSystemRef<Fr>,
    slot0: Fr,
    c: &PublicCommon,
) -> Result<PublicVars, SynthesisError> {
    let slot0 = FpVar::new_input(cs.clone(), || Ok(slot0))?;
    let _subject = FpVar::new_input(cs.clone(), || Ok(c.subject))?;
    let _credit_oracle_id = FpVar::new_input(cs.clone(), || Ok(c.credit_oracle_id))?;
    let commitment = FpVar::new_input(cs.clone(), || Ok(c.score_commitment))?;
    let _snapshot_ledger = FpVar::new_input(cs.clone(), || Ok(Fr::from(c.snapshot_ledger)))?;
    let _domain_separator = FpVar::new_input(cs.clone(), || Ok(c.domain_separator))?;
    let current_timestamp = FpVar::new_input(cs.clone(), || Ok(Fr::from(c.current_timestamp)))?;
    let max_validity_window = FpVar::new_input(cs, || Ok(Fr::from(c.max_validity_window)))?;
    Ok(PublicVars {
        slot0,
        commitment,
        current_timestamp,
        max_validity_window,
        current_timestamp_val: c.current_timestamp,
        max_validity_window_val: c.max_validity_window,
    })
}

/// Allocate the private witness, re-derive the score from the raw inputs
/// (`compute_score_pure`) and bind it to the public `commitment`.
///
/// Returns the (constrained) `score` so each statement can add its own
/// predicate over it.
fn enforce_score_record(
    cs: ConstraintSystemRef<Fr>,
    w: Option<&ScoreWitness>,
    pv: &PublicVars,
) -> Result<UInt32<Fr>, SynthesisError> {
    let commitment = &pv.commitment;
    // ---- Private witness ----
    let score = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.score).unwrap_or(0)))?;
    let vc_count = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.vc_count).unwrap_or(0)))?;
    let tx_volume_30d = UInt64::new_witness(cs.clone(), || {
        Ok(w.map(|w| w.tx_volume_30d.max(0) as u64).unwrap_or(0))
    })?;
    let avg_counterparties = UInt32::new_witness(cs.clone(), || {
        Ok(w.map(|w| w.avg_counterparties).unwrap_or(0))
    })?;
    let repayment_rate =
        UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.repayment_rate).unwrap_or(0)))?;
    let last_updated =
        UInt64::new_witness(cs.clone(), || Ok(w.map(|w| w.last_updated).unwrap_or(0)))?;
    let computed_at_ledger = UInt32::new_witness(cs.clone(), || {
        Ok(w.map(|w| w.computed_at_ledger).unwrap_or(0))
    })?;
    let stale = Boolean::new_witness(cs.clone(), || Ok(w.map(|w| w.stale).unwrap_or(false)))?;
    let score_timestamp =
        UInt64::new_witness(cs.clone(), || Ok(w.map(|w| w.score_timestamp).unwrap_or(0)))?;

    // Raw scoring inputs
    let vc_points = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.vc_points).unwrap_or(0)))?;
    let on_time_count =
        UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.on_time_count).unwrap_or(0)))?;
    let total_count =
        UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.total_count).unwrap_or(0)))?;
    let total_repaid = UInt64::new_witness(cs.clone(), || {
        Ok(w.map(|w| w.total_repaid.max(0) as u64).unwrap_or(0))
    })?;
    let vc_weight = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.vc_weight).unwrap_or(0)))?;
    let tx_weight = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.tx_weight).unwrap_or(0)))?;
    let repayment_weight = UInt32::new_witness(cs.clone(), || {
        Ok(w.map(|w| w.repayment_weight).unwrap_or(0))
    })?;

    // Intermediate values
    let vc_score = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.vc_score).unwrap_or(0)))?;
    let volume_score =
        UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.volume_score).unwrap_or(0)))?;
    let counterparty_bonus = UInt32::new_witness(cs.clone(), || {
        Ok(w.map(|w| w.counterparty_bonus).unwrap_or(0))
    })?;
    let tx_score = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.tx_score).unwrap_or(0)))?;
    let repayment_rate_score = UInt32::new_witness(cs.clone(), || {
        Ok(w.map(|w| w.repayment_rate_score).unwrap_or(0))
    })?;
    let repayment_volume_score = UInt32::new_witness(cs.clone(), || {
        Ok(w.map(|w| w.repayment_volume_score).unwrap_or(0))
    })?;
    let repay_score =
        UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.repay_score).unwrap_or(0)))?;
    let composite = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.composite).unwrap_or(0)))?;

    // Integer-division quotients
    let q_volume = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.q_volume).unwrap_or(0)))?;
    let q_cp = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.q_cp).unwrap_or(0)))?;
    let q_rv = UInt32::new_witness(cs.clone(), || Ok(w.map(|w| w.q_rv).unwrap_or(0)))?;

    let blinding = FpVar::new_witness(cs.clone(), || {
        Ok(w.map(|w| w.blinding).unwrap_or(Fr::from(0u32)))
    })?;

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

    // ---- Constraint 11: Poseidon commitment ----
    // C = Poseidon(domain_tag, fields..., blinding)[1]
    // Check C == public commitment.
    let stale_fp = boolean_to_fp(&stale);
    let inputs: [FpVar<Fr>; COMMIT_INPUTS] = [
        score_fp.clone(),
        uint32_to_fp(&vc_count),
        tx_volume_fp.clone(),
        avg_cp_fp.clone(),
        uint32_to_fp(&repayment_rate),
        uint64_to_fp(&last_updated),
        uint32_to_fp(&computed_at_ledger),
        stale_fp,
        blinding,
    ];
    let computed = poseidon_commit_gadget(&inputs)?;
    computed.enforce_equal(commitment)?;

    // ---- Constraint 12: Timestamp freshness ----
    // 1. score_timestamp <= current_timestamp
    // 2. current_timestamp - score_timestamp <= max_validity_window
    let score_ts_val = w.map(|w| w.score_timestamp).unwrap_or(0);
    let diff_curr_val = pv.current_timestamp_val.saturating_sub(score_ts_val);
    let diff_curr = UInt64::new_witness(cs.clone(), || Ok(diff_curr_val))?;
    let diff_curr_fp = uint64_to_fp(&diff_curr);
    let score_ts_fp = uint64_to_fp(&score_timestamp);
    (diff_curr_fp.clone() + score_ts_fp).enforce_equal(&pv.current_timestamp)?;

    let diff_win_val = pv.max_validity_window_val.saturating_sub(diff_curr_val);
    let diff_window = UInt64::new_witness(cs.clone(), || Ok(diff_win_val))?;
    let diff_win_fp = uint64_to_fp(&diff_window);
    (diff_win_fp + diff_curr_fp).enforce_equal(&pv.max_validity_window)?;

    Ok(score)
}

impl ConstraintSynthesizer<Fr> for ScoreCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        let p = &self.public_inputs;
        let pv = alloc_public_inputs(
            cs.clone(),
            Fr::from(p.threshold),
            &PublicCommon {
                subject: p.subject,
                credit_oracle_id: p.credit_oracle_id,
                score_commitment: p.score_commitment,
                snapshot_ledger: p.snapshot_ledger,
                domain_separator: p.domain_separator,
                current_timestamp: p.current_timestamp,
                max_validity_window: p.max_validity_window,
            },
        )?;
        let score = enforce_score_record(cs, self.witness.as_ref(), &pv)?;

        // ---- score > threshold ----
        enforce_u32_gt_fp(&score, &pv.slot0)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tier-band statement (issue #1184)
// ---------------------------------------------------------------------------

/// Domain separator for the tier-band circuit.
pub const TIER_CIRCUIT_DOMAIN: &[u8] = b"stellar-did-credit::score-tier-band::v2";

/// Lower bounds (inclusive) of the tiers, indexed by `tier_level - 1`:
/// Bronze `>= 600`, Silver `>= 700`, Gold `>= 800`.
pub const TIER_THRESHOLDS: [u32; 3] = [600, 700, 800];

/// Number of tiers (valid `tier_level` values are `1..=TIER_COUNT`).
pub const TIER_COUNT: u32 = 3;

/// Exclusive upper bound of tier `tier_level` (1-based). The top tier is
/// bounded by the maximum score, `MAX_SCORE + 1`.
pub const fn tier_upper_bound(tier_level: u32) -> u32 {
    match tier_level {
        1 => TIER_THRESHOLDS[1],
        2 => TIER_THRESHOLDS[2],
        _ => crate::score::MAX_SCORE + 1,
    }
}

/// The tier a score falls in: `0` below the lowest threshold (no tier), else
/// `1..=3`. Mirrors the in-circuit bands exactly.
pub const fn tier_for_score(score: u32) -> u32 {
    if score >= TIER_THRESHOLDS[2] {
        3
    } else if score >= TIER_THRESHOLDS[1] {
        2
    } else if score >= TIER_THRESHOLDS[0] {
        1
    } else {
        0
    }
}

/// Public inputs for the tier-band circuit. Same shape as
/// [`ScorePublicInputs`] with `tier_level` in place of `threshold`.
#[derive(Clone, Debug)]
pub struct TierPublicInputs {
    /// The tier the prover claims membership in (`1..=3`).
    pub tier_level: u32,
    /// Subject bound to the proof (SHA-256 of XDR).
    pub subject: Fr,
    /// Credit oracle ID (SHA-256 of XDR).
    pub credit_oracle_id: Fr,
    /// Commitment to the private ScoreRecord fields.
    pub score_commitment: Fr,
    /// Snapshot ledger.
    pub snapshot_ledger: u32,
    /// Domain separator.
    pub domain_separator: Fr,
    /// Current timestamp in seconds (verifier time).
    pub current_timestamp: u64,
    /// Maximum validity window in seconds.
    pub max_validity_window: u64,
}

/// The tier-band circuit: proves `TIER_THRESHOLDS[t-1] <= score < upper(t)` for
/// the public `tier_level = t`, over the same committed score record as
/// [`ScoreCircuit`]. The verifier learns the tier and nothing finer.
pub struct ScoreTierCircuit {
    pub witness: Option<ScoreWitness>,
    pub public_inputs: TierPublicInputs,
}

impl ScoreTierCircuit {
    pub fn new(witness: Option<ScoreWitness>, public_inputs: TierPublicInputs) -> Self {
        Self {
            witness,
            public_inputs,
        }
    }
}

impl ConstraintSynthesizer<Fr> for ScoreTierCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        let p = &self.public_inputs;
        let pv = alloc_public_inputs(
            cs.clone(),
            Fr::from(p.tier_level),
            &PublicCommon {
                subject: p.subject,
                credit_oracle_id: p.credit_oracle_id,
                score_commitment: p.score_commitment,
                snapshot_ledger: p.snapshot_ledger,
                domain_separator: p.domain_separator,
                current_timestamp: p.current_timestamp,
                max_validity_window: p.max_validity_window,
            },
        )?;
        let score = enforce_score_record(cs, self.witness.as_ref(), &pv)?;
        let tier = pv.slot0.clone();

        // ---- tier_level in {1, 2, 3} ----
        let one = FpVar::<Fr>::constant(Fr::from(1u32));
        let two = FpVar::<Fr>::constant(Fr::from(2u32));
        let three = FpVar::<Fr>::constant(Fr::from(3u32));
        let t_m1 = &tier - &one;
        let t_m2 = &tier - &two;
        let t_m3 = &tier - &three;
        let p23 = &t_m2 * &t_m3; // (t-2)(t-3)
        let p13 = &t_m1 * &t_m3; // (t-1)(t-3)
        let p12 = &t_m1 * &t_m2; // (t-1)(t-2)
        (&p12 * &t_m3).enforce_equal(&FpVar::constant(Fr::from(0u32)))?;

        // ---- one-hot tier selectors (Lagrange basis on {1, 2, 3}) ----
        //   is1 = (t-2)(t-3) / 2,  is2 = -(t-1)(t-3),  is3 = (t-1)(t-2) / 2
        let half = Fr::from(2u32).inverse().expect("2 is invertible");
        let is1 = &p23 * FpVar::constant(half);
        let is2 = &p13 * FpVar::constant(-Fr::from(1u32));
        let is3 = &p12 * FpVar::constant(half);

        // ---- band bounds selected by tier ----
        let lower = &is1 * FpVar::constant(Fr::from(TIER_THRESHOLDS[0]))
            + &is2 * FpVar::constant(Fr::from(TIER_THRESHOLDS[1]))
            + &is3 * FpVar::constant(Fr::from(TIER_THRESHOLDS[2]));
        let upper = &is1 * FpVar::constant(Fr::from(tier_upper_bound(1)))
            + &is2 * FpVar::constant(Fr::from(tier_upper_bound(2)))
            + &is3 * FpVar::constant(Fr::from(tier_upper_bound(3)));

        // score >= lower  <=>  score > lower - 1
        enforce_u32_gt_fp(&score, &(lower - &one))?;
        // score < upper
        enforce_u32_lt_fp(&score, &upper)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;
    use ark_relations::r1cs::{ConstraintSystem, SynthesisMode};

    /// A hand-written witness (independent of `ScoreWitness::from_inputs`).
    fn make_test_witness() -> ScoreWitness {
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

    fn make_test_public_inputs(
        w: &ScoreWitness,
        current_ts: u64,
        max_window: u64,
    ) -> ScorePublicInputs {
        ScorePublicInputs {
            threshold: 600,
            subject: Fr::from(1u32),
            credit_oracle_id: Fr::from(2u32),
            score_commitment: w.commitment(),
            snapshot_ledger: 12345,
            domain_separator: Fr::from(3u32),
            current_timestamp: current_ts,
            max_validity_window: max_window,
        }
    }

    fn valid_circuit() -> ScoreCircuit {
        let w = make_test_witness();
        let public_inputs = make_test_public_inputs(&w, 1_700_000_100, 3600);
        ScoreCircuit::new(Some(w), public_inputs)
    }

    #[test]
    fn circuit_satisfied_for_valid_witness() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        cs.set_mode(SynthesisMode::Prove {
            construct_matrices: true,
        });
        valid_circuit().generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap());
    }

    /// The derived witness equals the hand-written one field for field that the
    /// circuit constrains, so `from_inputs` cannot drift from the circuit.
    #[test]
    fn derived_witness_matches_hand_written_witness() {
        let by_hand = make_test_witness();
        let mut derived = reference_witness();
        assert_eq!(derived.commitment(), by_hand.commitment());
        derived.total_count_inv = by_hand.total_count_inv;
        assert!(satisfied(ScoreCircuit::new(
            Some(derived.clone()),
            make_test_public_inputs(&derived, 1_700_000_100, 3600)
        )));
        assert_eq!(derived.score, by_hand.score);
        assert_eq!(derived.composite, by_hand.composite);
    }

    #[test]
    fn test_timestamp_freshness_exact_window_boundary() {
        let w = make_test_witness();
        let p = make_test_public_inputs(&w, 1_700_003_600, 3600);
        assert!(
            satisfied(ScoreCircuit::new(Some(w), p)),
            "score exactly at max_validity_window must be satisfied"
        );
    }

    #[test]
    fn test_timestamp_freshness_zero_age() {
        let w = make_test_witness();
        let p = make_test_public_inputs(&w, 1_700_000_000, 3600);
        assert!(
            satisfied(ScoreCircuit::new(Some(w), p)),
            "score with zero age must be satisfied"
        );
    }

    #[test]
    fn test_timestamp_freshness_fails_one_second_over_window() {
        let w = make_test_witness();
        let p = make_test_public_inputs(&w, 1_700_003_601, 3600);
        assert!(
            !satisfied(ScoreCircuit::new(Some(w), p)),
            "score older than max_validity_window must fail constraint check"
        );
    }

    #[test]
    fn test_timestamp_freshness_fails_future_score() {
        let mut w = make_test_witness();
        w.score_timestamp = 1_700_000_005;
        let p = make_test_public_inputs(&w, 1_700_000_000, 3600);
        assert!(
            !satisfied(ScoreCircuit::new(Some(w), p)),
            "score timestamp in the future must fail constraint check"
        );
    }

    /// Run explicitly in CI to report proving time and enforce the R1CS budget.
    #[test]
    #[ignore = "run explicitly for constraint and proving benchmarks"]
    fn benchmark_score_circuit() {
        use ark_bls12_381::Bls12_381;
        use ark_groth16::Groth16;
        use ark_snark::SNARK;
        use ark_std::rand::{rngs::StdRng, SeedableRng};
        use std::time::Instant;

        // Measured baseline before the Poseidon commitment (#1185): 8,235
        // constraints (the Pedersen-style commitment was linear, i.e. free
        // in R1CS). The Poseidon gadget adds exactly 417 (~5.1%), giving
        // 8,652, so the budget was raised from 8,500 to 9,000 (~4% headroom).
        const MAX_CONSTRAINTS: usize = 9_000;
        let cs = ConstraintSystem::<Fr>::new_ref();
        valid_circuit().generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap(), "benchmark witness is invalid");
        let count = cs.num_constraints();
        println!("Score circuit constraints: {count} (budget: < {MAX_CONSTRAINTS})");
        assert!(
            count < MAX_CONSTRAINTS,
            "score circuit constraint budget exceeded: {count} >= {MAX_CONSTRAINTS}"
        );

        // Setup is excluded from the measured proving time; this deterministic
        // key is for benchmarking only and must never be used in production.
        let mut rng = StdRng::from_seed([0u8; 32]);
        let setup_start = Instant::now();
        let (pk, _) =
            Groth16::<Bls12_381>::circuit_specific_setup(valid_circuit(), &mut rng).unwrap();
        println!("Score circuit setup: {:?}", setup_start.elapsed());
        let circuit = valid_circuit();
        let start = Instant::now();
        Groth16::<Bls12_381>::prove(&pk, circuit, &mut rng).unwrap();
        println!("Score circuit proof generation: {:?}", start.elapsed());
    }
}

/// Shared helpers for the adversarial (#1182) and tier-band (#1184) suites.
#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use crate::{generate_test_keys, generate_tier_test_keys, ScoreProof};
    use ark_bls12_381::Bls12_381;
    use ark_groth16::{ProvingKey, VerifyingKey};
    use ark_relations::r1cs::{ConstraintSystem, SynthesisMode};
    use ark_std::rand::rngs::StdRng;
    use ark_std::rand::SeedableRng;
    use std::sync::OnceLock;

    pub type Keys = (ProvingKey<Bls12_381>, VerifyingKey<Bls12_381>);

    pub fn rng() -> StdRng {
        StdRng::from_seed([7u8; 32])
    }

    /// Threshold-circuit keys, generated once per test process.
    pub fn threshold_keys() -> &'static Keys {
        static KEYS: OnceLock<Keys> = OnceLock::new();
        KEYS.get_or_init(|| generate_test_keys(&mut rng()).unwrap())
    }

    /// Tier-circuit keys, generated once per test process.
    pub fn tier_keys() -> &'static Keys {
        static KEYS: OnceLock<Keys> = OnceLock::new();
        KEYS.get_or_init(|| generate_tier_test_keys(&mut rng()).unwrap())
    }

    /// Honest witness whose composite score is exactly `composite` (weights
    /// 100/0/0 make `composite == min(vc_points, 100)`), giving
    /// `score = 300 + composite * 550 / 100`.
    pub fn witness_for_composite(composite: u32) -> ScoreWitness {
        let inputs = ScoreInputs {
            vc_points: composite,
            vc_weight: 100,
            tx_weight: 0,
            repayment_weight: 0,
            vc_count: 10,
            last_updated: 1_700_000_000,
            computed_at_ledger: 1_234_567,
            score_timestamp: 1_700_000_000,
            ..Default::default()
        };
        ScoreWitness::from_inputs(&inputs, Fr::from(42u32)).unwrap()
    }

    /// The reference witness used by the happy-path tests (score 668).
    pub fn reference_witness() -> ScoreWitness {
        let inputs = ScoreInputs {
            vc_points: 50,
            tx_volume_30d: 50_000_000_000,
            avg_counterparties: 10,
            on_time_count: 50,
            total_count: 100,
            total_repaid: 20_000_000_000,
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
            vc_count: 10,
            last_updated: 1_700_000_000,
            computed_at_ledger: 1_234_567,
            stale: false,
            score_timestamp: 1_700_000_000,
        };
        ScoreWitness::from_inputs(&inputs, Fr::from(42u32)).unwrap()
    }

    /// Verifier time used by the shared public inputs: 100 s after the score
    /// was recorded, inside a one-hour validity window.
    pub const NOW: u64 = 1_700_000_100;
    pub const WINDOW: u64 = 3600;

    /// Return `w` after `f`, leaving everything else (including the
    /// commitment, which callers recompute) untouched.
    pub fn tampered(w: &ScoreWitness, f: impl FnOnce(&mut ScoreWitness)) -> ScoreWitness {
        let mut t = w.clone();
        f(&mut t);
        t
    }

    pub fn threshold_inputs(w: &ScoreWitness, threshold: u32) -> ScorePublicInputs {
        ScorePublicInputs {
            threshold,
            subject: Fr::from(1u32),
            credit_oracle_id: Fr::from(2u32),
            score_commitment: w.commitment(),
            snapshot_ledger: 12345,
            domain_separator: Fr::from(3u32),
            current_timestamp: NOW,
            max_validity_window: WINDOW,
        }
    }

    pub fn tier_inputs(w: &ScoreWitness, tier_level: u32) -> TierPublicInputs {
        TierPublicInputs {
            tier_level,
            subject: Fr::from(1u32),
            credit_oracle_id: Fr::from(2u32),
            score_commitment: w.commitment(),
            snapshot_ledger: 12345,
            domain_separator: Fr::from(3u32),
            current_timestamp: NOW,
            max_validity_window: WINDOW,
        }
    }

    /// Synthesize `circuit` and report whether the R1CS is satisfied.
    /// A synthesis error counts as "not satisfied".
    pub fn satisfied<C: ConstraintSynthesizer<Fr>>(circuit: C) -> bool {
        let cs = ConstraintSystem::<Fr>::new_ref();
        cs.set_mode(SynthesisMode::Prove {
            construct_matrices: true,
        });
        match circuit.generate_constraints(cs.clone()) {
            Ok(()) => cs.is_satisfied().unwrap(),
            Err(_) => false,
        }
    }

    pub fn threshold_satisfied(w: &ScoreWitness, p: &ScorePublicInputs) -> bool {
        satisfied(ScoreCircuit::new(Some(w.clone()), p.clone()))
    }

    pub fn tier_satisfied(w: &ScoreWitness, p: &TierPublicInputs) -> bool {
        satisfied(ScoreTierCircuit::new(Some(w.clone()), p.clone()))
    }

    /// In debug builds arkworks' Groth16 prover `debug_assert!`s that the
    /// constraint system is satisfied, so proving an unsatisfiable witness
    /// panics. That is the prover refusing; model it as a rejection. In release
    /// builds the assertion is compiled out, so the real prover runs on the bad
    /// witness and the verifier must still reject the resulting proof.
    fn prover_would_refuse(satisfiable: bool) -> bool {
        cfg!(debug_assertions) && !satisfiable
    }

    /// Full Groth16 path for the threshold circuit. `true` only if a proof
    /// was produced *and* verified; every other outcome (prover refusing,
    /// verifier error, verifier returning false) is a rejection.
    pub fn threshold_proves_and_verifies(w: &ScoreWitness, p: &ScorePublicInputs) -> bool {
        if prover_would_refuse(threshold_satisfied(w, p)) {
            return false;
        }
        let (pk, vk) = threshold_keys();
        match crate::generate_proof(w.clone(), p.clone(), pk, &mut rng()) {
            Err(_) => false,
            Ok(proof) => crate::verify_proof(&proof, p, vk).unwrap_or(false),
        }
    }

    pub fn tier_proves_and_verifies(w: &ScoreWitness, p: &TierPublicInputs) -> bool {
        if prover_would_refuse(tier_satisfied(w, p)) {
            return false;
        }
        let (pk, vk) = tier_keys();
        match crate::generate_tier_proof(w.clone(), p.clone(), pk, &mut rng()) {
            Err(_) => false,
            Ok(proof) => crate::verify_tier_proof(&proof, p, vk).unwrap_or(false),
        }
    }

    pub fn valid_threshold_proof(w: &ScoreWitness, p: &ScorePublicInputs) -> ScoreProof {
        let (pk, vk) = threshold_keys();
        let proof = crate::generate_proof(w.clone(), p.clone(), pk, &mut rng()).unwrap();
        assert!(
            crate::verify_proof(&proof, p, vk).unwrap(),
            "control proof must verify"
        );
        proof
    }
}

/// Issue #1182: adversarial vectors proving soundness of the score circuit.
///
/// Every negative case is checked on two layers: the R1CS must be
/// unsatisfied (witness generation rejects it), and the full Groth16 path
/// must not yield a verifying proof. Each negative case is paired with a
/// positive control so a test cannot pass vacuously.
#[cfg(test)]
mod adversarial_tests {
    use super::test_util::*;
    use super::*;
    use crate::{verify_proof, ScoreProof};
    use ark_ec::{AffineRepr, CurveGroup};
    use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    /// Assert the statement is rejected at witness generation AND by the
    /// Groth16 prover/verifier pair.
    fn assert_rejected(w: &ScoreWitness, p: &ScorePublicInputs, why: &str) {
        assert!(
            !threshold_satisfied(w, p),
            "{why}: R1CS unexpectedly satisfied"
        );
        assert!(
            !threshold_proves_and_verifies(w, p),
            "{why}: proof unexpectedly verified"
        );
    }

    // ---- threshold boundary -------------------------------------------------

    #[test]
    fn control_honest_witness_is_accepted() {
        let w = reference_witness();
        assert_eq!(w.score, 668);
        let p = threshold_inputs(&w, 600);
        assert!(threshold_satisfied(&w, &p));
        assert!(threshold_proves_and_verifies(&w, &p));
    }

    /// Score of 599 when threshold is 600, with the commitment recomputed over
    /// the forged score: the score-formula constraints reject it (599 is not
    /// reachable from any composite) and so does `score > threshold`.
    #[test]
    fn forged_score_599_with_threshold_600_is_rejected() {
        let honest = witness_for_composite(54); // honest score = 597
        let forged = tampered(&honest, |w| w.score = 599);
        let p = threshold_inputs(&forged, 600);
        assert_rejected(&forged, &p, "score 599 vs threshold 600");
    }

    /// An honest, fully consistent score of 597 isolates the
    /// `score > threshold` constraint from the formula constraints.
    #[test]
    fn honest_score_597_below_threshold_600_is_rejected() {
        let w = witness_for_composite(54);
        assert_eq!(w.score, 597);
        assert_rejected(&w, &threshold_inputs(&w, 600), "597 vs 600");
        // positive control: same witness passes a lower threshold.
        let ok = threshold_inputs(&w, 596);
        assert!(threshold_satisfied(&w, &ok));
        assert!(threshold_proves_and_verifies(&w, &ok));
    }

    /// `score > threshold` is strict: score == threshold must fail.
    #[test]
    fn score_equal_to_threshold_is_rejected() {
        let w = witness_for_composite(55);
        assert_eq!(w.score, 602);
        assert_rejected(&w, &threshold_inputs(&w, 602), "602 vs 602");
        let ok = threshold_inputs(&w, 601);
        assert!(threshold_satisfied(&w, &ok));
        assert!(threshold_proves_and_verifies(&w, &ok));
    }

    // ---- out-of-bounds scores ----------------------------------------------

    /// Scores outside [300, 850], including the u32 wrap-arounds that model a
    /// "negative" score (u32::MAX == -1, 0x8000_0000 == i32::MIN).
    #[test]
    fn out_of_range_scores_are_rejected() {
        let honest = witness_for_composite(100);
        assert_eq!(honest.score, 850);
        for bad in [851u32, 900, 1023, 1024, 0, 1, 299, u32::MAX, 0x8000_0000] {
            let forged = tampered(&honest, |w| w.score = bad);
            // Threshold 0 keeps `score > threshold` from masking the failure
            // for the high values; the range/formula constraints must reject.
            let p = threshold_inputs(&forged, 0);
            assert!(
                !threshold_satisfied(&forged, &p),
                "score {bad} satisfied the circuit"
            );
        }
        // Full Groth16 path on a representative subset.
        for bad in [851u32, 0, u32::MAX] {
            let forged = tampered(&honest, |w| w.score = bad);
            assert_rejected(&forged, &threshold_inputs(&forged, 0), "out-of-range score");
        }
    }

    #[test]
    fn boundary_scores_300_and_850_are_accepted() {
        let lo = witness_for_composite(0);
        assert_eq!(lo.score, 300);
        // 300 > 299
        assert!(threshold_satisfied(&lo, &threshold_inputs(&lo, 299)));
        let hi = witness_for_composite(100);
        assert!(threshold_satisfied(&hi, &threshold_inputs(&hi, 849)));
        // 850 > 850 is false.
        assert!(!threshold_satisfied(&hi, &threshold_inputs(&hi, 850)));
    }

    /// A score that disagrees with the composite it was derived from.
    #[test]
    fn score_inconsistent_with_composite_is_rejected() {
        let honest = reference_witness();
        for delta in [1i64, -1, 100, -100] {
            let forged = tampered(&honest, |w| w.score = (w.score as i64 + delta) as u32);
            assert!(
                !threshold_satisfied(&forged, &threshold_inputs(&forged, 0)),
                "score off by {delta} satisfied the circuit"
            );
        }
        let forged = tampered(&honest, |w| w.composite += 1);
        assert!(!threshold_satisfied(&forged, &threshold_inputs(&forged, 0)));
    }

    // ---- falsified commitments ---------------------------------------------

    #[test]
    fn falsified_commitment_is_rejected() {
        let w = reference_witness();
        let honest = threshold_inputs(&w, 600);

        // Off-by-one commitment.
        let mut p = honest.clone();
        p.score_commitment += Fr::from(1u32);
        assert_rejected(&w, &p, "commitment + 1");

        // Zero commitment.
        let mut p = honest.clone();
        p.score_commitment = Fr::from(0u32);
        assert_rejected(&w, &p, "zero commitment");

        // Commitment to a *different* (higher) score than the witness holds.
        let richer = tampered(&w, |x| x.score = 850);
        let mut p = honest.clone();
        p.score_commitment = richer.commitment();
        assert_rejected(&w, &p, "commitment to a different score");

        // Commitment made with a different blinding factor.
        let reblinded = tampered(&w, |x| x.blinding = Fr::from(43u32));
        let mut p = honest;
        p.score_commitment = reblinded.commitment();
        assert_rejected(&w, &p, "commitment with different blinding");
    }

    /// Each committed field is bound: changing any one of them in the witness
    /// while keeping the original commitment must fail.
    #[test]
    fn every_committed_field_is_bound() {
        let w = reference_witness();
        let p = threshold_inputs(&w, 600); // commitment of the honest record
        let edits: [(&str, fn(&mut ScoreWitness)); 7] = [
            ("vc_count", |w| w.vc_count += 1),
            ("avg_counterparties", |w| w.avg_counterparties += 5),
            ("repayment_rate", |w| w.repayment_rate += 1),
            ("last_updated", |w| w.last_updated += 1),
            ("computed_at_ledger", |w| w.computed_at_ledger += 1),
            ("stale", |w| w.stale = !w.stale),
            ("tx_volume_30d", |w| w.tx_volume_30d += 1),
        ];
        for (name, edit) in edits {
            let forged = tampered(&w, edit);
            assert!(
                !threshold_satisfied(&forged, &p),
                "field {name} is not bound by the commitment"
            );
        }
    }

    // ---- corrupted proofs ---------------------------------------------------

    #[test]
    fn corrupted_proof_scalars_are_rejected() {
        let w = reference_witness();
        let p = threshold_inputs(&w, 600);
        let proof = valid_threshold_proof(&w, &p);
        let (_, vk) = threshold_keys();
        let rejects = |bad: &ScoreProof, what: &str| {
            assert!(
                !verify_proof(bad, &p, vk).unwrap_or(false),
                "{what}: corrupted proof verified"
            );
        };

        let g1 = ark_bls12_381::G1Affine::generator();
        let g2 = ark_bls12_381::G2Affine::generator();

        let mut bad = proof.clone();
        bad.a = (proof.a + g1).into_affine();
        rejects(&bad, "A + G");

        let mut bad = proof.clone();
        bad.b = (proof.b + g2).into_affine();
        rejects(&bad, "B + G");

        let mut bad = proof.clone();
        bad.c = (proof.c + g1).into_affine();
        rejects(&bad, "C + G");

        let mut bad = proof.clone();
        bad.a = -proof.a;
        rejects(&bad, "-A");

        let mut bad = proof.clone();
        bad.a = proof.c;
        bad.c = proof.a;
        rejects(&bad, "A <-> C swapped");

        let mut bad = proof.clone();
        bad.a = ark_bls12_381::G1Affine::zero();
        rejects(&bad, "A = infinity");

        let mut bad = proof.clone();
        bad.c = ark_bls12_381::G1Affine::zero();
        rejects(&bad, "C = infinity");

        let mut bad = proof.clone();
        bad.b = ark_bls12_381::G2Affine::zero();
        rejects(&bad, "B = infinity");

        // All-infinity "proof".
        let bad = ScoreProof {
            a: ark_bls12_381::G1Affine::zero(),
            b: ark_bls12_381::G2Affine::zero(),
            c: ark_bls12_381::G1Affine::zero(),
        };
        rejects(&bad, "all-infinity proof");
    }

    /// Flip single bits in the serialized proof: the bytes must either fail to
    /// deserialize (off-curve / wrong subgroup) or deserialize to a proof that
    /// fails verification. Truncated encodings must fail to parse.
    #[test]
    fn bit_flipped_and_truncated_proof_bytes_are_rejected() {
        let w = reference_witness();
        let p = threshold_inputs(&w, 600);
        let proof = valid_threshold_proof(&w, &p);
        let (_, vk) = threshold_keys();

        let mut bytes = Vec::new();
        proof.serialize_compressed(&mut bytes).unwrap();
        assert_eq!(
            ScoreProof::deserialize_compressed(&bytes[..]).unwrap(),
            proof
        );

        let mut tested = 0;
        for byte in (0..bytes.len()).step_by(7) {
            for bit in [0u8, 3, 7] {
                let mut mutated = bytes.clone();
                mutated[byte] ^= 1 << bit;
                match ScoreProof::deserialize_compressed(&mutated[..]) {
                    Err(_) => {}
                    Ok(parsed) => assert!(
                        !verify_proof(&parsed, &p, vk).unwrap_or(false),
                        "bit {bit} of byte {byte} flipped but proof verified"
                    ),
                }
                tested += 1;
            }
        }
        assert!(tested > 50);

        for len in [0, 1, bytes.len() / 2, bytes.len() - 1] {
            assert!(ScoreProof::deserialize_compressed(&bytes[..len]).is_err());
        }
    }

    /// A valid proof is bound to every public input.
    #[test]
    fn proof_is_bound_to_every_public_input() {
        let w = reference_witness();
        let p = threshold_inputs(&w, 600);
        let proof = valid_threshold_proof(&w, &p);
        let (_, vk) = threshold_keys();

        let mutations: [(&str, fn(&mut ScorePublicInputs)); 8] = [
            ("threshold", |p| p.threshold += 1),
            ("subject", |p| p.subject += Fr::from(1u32)),
            ("credit_oracle_id", |p| p.credit_oracle_id += Fr::from(1u32)),
            ("score_commitment", |p| p.score_commitment += Fr::from(1u32)),
            ("snapshot_ledger", |p| p.snapshot_ledger += 1),
            ("domain_separator", |p| p.domain_separator += Fr::from(1u32)),
            ("current_timestamp", |p| p.current_timestamp += 1),
            ("max_validity_window", |p| p.max_validity_window += 1),
        ];
        for (name, mutate) in mutations {
            let mut other = p.clone();
            mutate(&mut other);
            assert!(
                !verify_proof(&proof, &other, vk).unwrap_or(false),
                "proof verified after changing public input {name}"
            );
        }
    }

    // ---- no panics / underflow ---------------------------------------------

    /// Extreme witness values must produce a clean rejection (or acceptance),
    /// never a panic or arithmetic underflow in synthesis, proving or
    /// verification.
    #[test]
    fn extreme_witness_values_never_panic() {
        let honest = reference_witness();
        let extremes: Vec<ScoreWitness> = vec![
            tampered(&honest, |w| {
                w.score = u32::MAX;
                w.vc_count = u32::MAX;
                w.avg_counterparties = u32::MAX;
                w.repayment_rate = u32::MAX;
                w.last_updated = u64::MAX;
                w.computed_at_ledger = u32::MAX;
                w.stale = true;
            }),
            tampered(&honest, |w| {
                w.tx_volume_30d = i128::MAX;
                w.total_repaid = i128::MAX;
                w.q_volume = u32::MAX;
                w.q_rv = u32::MAX;
            }),
            tampered(&honest, |w| {
                w.tx_volume_30d = i128::MIN;
                w.total_repaid = i128::MIN;
            }),
            tampered(&honest, |w| {
                w.vc_weight = u32::MAX;
                w.tx_weight = u32::MAX;
                w.repayment_weight = u32::MAX;
            }),
            tampered(&honest, |w| {
                // total_count == 0 with a non-zero on_time_count.
                w.total_count = 0;
                w.on_time_count = u32::MAX;
            }),
            tampered(&honest, |w| {
                w.on_time_count = u32::MAX;
                w.total_count = 1;
            }),
            tampered(&honest, |w| {
                w.vc_score = u32::MAX;
                w.volume_score = u32::MAX;
                w.counterparty_bonus = u32::MAX;
                w.tx_score = u32::MAX;
                w.repayment_rate_score = u32::MAX;
                w.repayment_volume_score = u32::MAX;
                w.repay_score = u32::MAX;
                w.composite = u32::MAX;
            }),
            ScoreWitness {
                score: 0,
                blinding: Fr::from(0u32),
                ..ScoreWitness::default()
            },
        ];
        for (i, w) in extremes.iter().enumerate() {
            for threshold in [0u32, 600, u32::MAX] {
                let p = threshold_inputs(w, threshold);
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let sat = threshold_satisfied(w, &p);
                    // Debug builds: arkworks panics when asked to prove an
                    // unsatisfiable witness (see `prover_would_refuse`), which
                    // is the prover refusing, not a bug in the circuit.
                    let verified = if cfg!(debug_assertions) && !sat {
                        false
                    } else {
                        let (pk, vk) = threshold_keys();
                        match crate::generate_proof(w.clone(), p.clone(), pk, &mut rng()) {
                            Ok(proof) => verify_proof(&proof, &p, vk).unwrap_or(false),
                            Err(_) => false,
                        }
                    };
                    (sat, verified)
                }));
                let (sat, verified) = outcome.unwrap_or_else(|_| {
                    panic!("panic on extreme witness #{i}, threshold {threshold}")
                });
                assert!(
                    !verified || sat,
                    "extreme witness #{i}: proof verified but R1CS unsatisfied"
                );
                // None of these are honest records for the given statement.
                assert!(
                    !verified,
                    "extreme witness #{i} threshold {threshold} verified"
                );
            }
        }
    }

    /// Witness derivation itself must reject (not panic on) hostile raw inputs.
    #[test]
    fn witness_derivation_rejects_hostile_inputs_without_panicking() {
        let base = ScoreInputs {
            vc_weight: 100,
            ..Default::default()
        };
        let hostile = [
            ScoreInputs {
                vc_weight: 101,
                ..base.clone()
            },
            ScoreInputs {
                vc_weight: u32::MAX,
                tx_weight: u32::MAX,
                repayment_weight: u32::MAX,
                ..base.clone()
            },
            ScoreInputs {
                tx_volume_30d: i128::MAX,
                ..base.clone()
            },
            ScoreInputs {
                total_repaid: i128::MAX,
                ..base.clone()
            },
            ScoreInputs {
                on_time_count: u32::MAX,
                total_count: 1,
                ..base.clone()
            },
            ScoreInputs {
                vc_points: u32::MAX,
                ..base.clone()
            },
            ScoreInputs {
                tx_volume_30d: i128::MIN,
                total_repaid: i128::MIN,
                ..base.clone()
            },
        ];
        for (i, inputs) in hostile.iter().enumerate() {
            let r = catch_unwind(AssertUnwindSafe(|| {
                ScoreWitness::from_inputs(inputs, Fr::from(1u32))
            }));
            assert!(r.is_ok(), "from_inputs panicked on hostile input #{i}");
        }
        assert_eq!(
            ScoreWitness::from_inputs(&hostile[0], Fr::from(1u32)).unwrap_err(),
            WitnessError::WeightsDoNotSumTo100
        );
    }
}

/// Issue #1184: tier-band (`[600, 700, 800]`) statement.
#[cfg(test)]
mod tier_tests {
    use super::test_util::*;
    use super::*;
    use crate::{verify_proof, verify_tier_proof};

    /// (composite, expected score, expected tier). Scores are
    /// `300 + composite * 550 / 100`; these straddle every band edge.
    const BOUNDARIES: [(u32, u32, u32); 8] = [
        (0, 300, 0),
        (54, 597, 0),
        (55, 602, 1),
        (72, 696, 1),
        (73, 701, 2),
        (90, 795, 2),
        (91, 800, 3),
        (100, 850, 3),
    ];

    #[test]
    fn tier_for_score_matches_band_definition() {
        assert_eq!(tier_for_score(0), 0);
        assert_eq!(tier_for_score(599), 0);
        assert_eq!(tier_for_score(600), 1);
        assert_eq!(tier_for_score(699), 1);
        assert_eq!(tier_for_score(700), 2);
        assert_eq!(tier_for_score(799), 2);
        assert_eq!(tier_for_score(800), 3);
        assert_eq!(tier_for_score(850), 3);
        assert_eq!(tier_upper_bound(1), 700);
        assert_eq!(tier_upper_bound(2), 800);
        assert_eq!(tier_upper_bound(3), 851);
    }

    /// Exactly the tier a score belongs to is provable; every other
    /// `tier_level` (including 0 and out-of-range 4, 5, u32::MAX) is not.
    #[test]
    fn only_the_true_tier_is_satisfiable_at_every_band_edge() {
        for (composite, score, tier) in BOUNDARIES {
            let w = witness_for_composite(composite);
            assert_eq!(w.score, score, "composite {composite}");
            assert_eq!(tier_for_score(w.score), tier);
            for claimed in [0u32, 1, 2, 3, 4, 5, u32::MAX] {
                let sat = tier_satisfied(&w, &tier_inputs(&w, claimed));
                assert_eq!(
                    sat,
                    claimed == tier && tier != 0,
                    "score {score} claiming tier {claimed} (true tier {tier})"
                );
            }
        }
    }

    /// Full Groth16 round-trip for a score inside each tier, plus the wrong
    /// tier for each being rejected.
    #[test]
    fn tier_proofs_verify_for_the_true_tier_only() {
        let (_, vk) = tier_keys();
        for (composite, tier) in [(55u32, 1u32), (73, 2), (100, 3)] {
            let w = witness_for_composite(composite);
            let good = tier_inputs(&w, tier);
            assert!(
                tier_proves_and_verifies(&w, &good),
                "tier {tier} should verify"
            );

            for wrong in [1u32, 2, 3].into_iter().filter(|t| *t != tier) {
                let bad = tier_inputs(&w, wrong);
                assert!(
                    !tier_proves_and_verifies(&w, &bad),
                    "score in tier {tier} verified as tier {wrong}"
                );
            }

            // A valid proof for `tier` must not verify under another tier_level.
            let (pk, _) = tier_keys();
            let proof =
                crate::generate_tier_proof(w.clone(), good.clone(), pk, &mut rng()).unwrap();
            for wrong in [0u32, 1, 2, 3, 4].into_iter().filter(|t| *t != tier) {
                let mut other = good.clone();
                other.tier_level = wrong;
                assert!(!verify_tier_proof(&proof, &other, vk).unwrap_or(false));
            }
        }
    }

    /// Tier transitions: a score one point below a band edge cannot claim the
    /// higher tier, and the first score at/above the edge can claim it.
    #[test]
    fn tier_transitions_at_600_700_800() {
        for (below, above, higher_tier) in [(54u32, 55u32, 1u32), (72, 73, 2), (90, 91, 3)] {
            let lo = witness_for_composite(below);
            let hi = witness_for_composite(above);
            assert!(!tier_satisfied(&lo, &tier_inputs(&lo, higher_tier)));
            assert!(tier_satisfied(&hi, &tier_inputs(&hi, higher_tier)));
            assert_eq!(tier_for_score(hi.score), higher_tier);
            assert_eq!(tier_for_score(lo.score), higher_tier - 1);
        }
    }

    /// A forged score cannot be used to claim a tier: the score-formula and
    /// commitment constraints still apply to the tier statement.
    #[test]
    fn forged_scores_cannot_claim_a_tier() {
        let honest = witness_for_composite(54); // 597: no tier
        for forged_score in [600u32, 700, 800, 850] {
            let forged = tampered(&honest, |w| w.score = forged_score);
            for tier in [1u32, 2, 3] {
                assert!(
                    !tier_satisfied(&forged, &tier_inputs(&forged, tier)),
                    "forged score {forged_score} satisfied tier {tier}"
                );
            }
        }
        // Honest witness but commitment to a higher-tier record.
        let gold = tampered(&witness_for_composite(100), |_| {});
        let mut p = tier_inputs(&honest, 3);
        p.score_commitment = gold.commitment();
        assert!(!tier_satisfied(&honest, &p));
    }

    /// The freshness constraint applies to the tier statement too: a stale or
    /// future-dated score cannot claim a tier.
    #[test]
    fn tier_statement_enforces_timestamp_freshness() {
        let w = reference_witness(); // score 668 -> tier 1, recorded at 1_700_000_000
        let mut p = tier_inputs(&w, 1);
        assert!(tier_satisfied(&w, &p));

        p.current_timestamp = w.score_timestamp + WINDOW; // exactly at the window edge
        assert!(tier_satisfied(&w, &p));
        p.current_timestamp = w.score_timestamp + WINDOW + 1; // one second too old
        assert!(!tier_satisfied(&w, &p));
        p.current_timestamp = w.score_timestamp - 1; // score from the future
        assert!(!tier_satisfied(&w, &p));
    }

    /// The same commitment serves both statements: a prover commits once and
    /// can produce either a threshold proof or a tier proof against it.
    #[test]
    fn one_commitment_serves_threshold_and_tier_statements() {
        let w = reference_witness(); // score 668 -> tier 1
        let tp = tier_inputs(&w, tier_for_score(w.score));
        let sp = threshold_inputs(&w, 600);
        assert_eq!(tp.score_commitment, sp.score_commitment);
        assert!(tier_satisfied(&w, &tp));
        assert!(threshold_satisfied(&w, &sp));
    }

    /// Circuits are domain-separated by their verification keys: a threshold
    /// proof is not accepted by the tier VK and vice versa, even though the
    /// public-input vectors have the same shape.
    #[test]
    fn proofs_do_not_cross_verify_between_circuits() {
        let w = reference_witness();
        let sp = threshold_inputs(&w, 600);
        let tp = tier_inputs(&w, 1);
        let (_, threshold_vk) = threshold_keys();
        let (_, tier_vk) = tier_keys();

        let threshold_proof = valid_threshold_proof(&w, &sp);
        // Same numeric public inputs (600 vs 1 differ, but also force equal).
        assert!(!verify_tier_proof(&threshold_proof, &tp, tier_vk).unwrap_or(false));

        let (pk, _) = tier_keys();
        let tier_proof = crate::generate_tier_proof(w.clone(), tp.clone(), pk, &mut rng()).unwrap();
        assert!(verify_tier_proof(&tier_proof, &tp, tier_vk).unwrap());
        assert!(!verify_proof(&tier_proof, &sp, threshold_vk).unwrap_or(false));
        let as_threshold = ScorePublicInputs {
            threshold: tp.tier_level,
            subject: tp.subject,
            credit_oracle_id: tp.credit_oracle_id,
            score_commitment: tp.score_commitment,
            snapshot_ledger: tp.snapshot_ledger,
            domain_separator: tp.domain_separator,
            current_timestamp: tp.current_timestamp,
            max_validity_window: tp.max_validity_window,
        };
        assert!(!verify_proof(&tier_proof, &as_threshold, threshold_vk).unwrap_or(false));
    }
}
