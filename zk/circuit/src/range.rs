//! Range-proof helpers for the score circuit.
//!
//! These implement bit-decomposition range checks as R1CS constraints using
//! arkworks' `UInt32` / `UInt64` gadgets.

use ark_ff::PrimeField;
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::prelude::*;
use ark_r1cs_std::uint32::UInt32;
use ark_r1cs_std::uint64::UInt64;
use ark_relations::r1cs::SynthesisError;

/// Enforce that `value < bound` for a 32-bit value.
///
/// Constraint cost: `32` bit-decomposition constraints + `1` comparison
/// constraint = 33 constraints.
pub fn enforce_u32_less_than<F: PrimeField>(
    value: &UInt32<F>,
    bound: u32,
) -> Result<(), SynthesisError> {
    // value < bound  <=>  value <= bound - 1
    let bound_minus_one = bound.saturating_sub(1);
    let bound_fp = FpVar::<F>::constant(F::from(bound_minus_one as u64));
    let value_fp = value.to_fp()?;
    // diff = bound - 1 - value, must be in [0, 2^32)
    let diff = bound_fp - value_fp;
    let diff_bits = diff.to_bits_le()?;
    for b in diff_bits.iter().skip(32) {
        b.enforce_equal(&Boolean::constant(false))?;
    }
    Ok(())
}

/// Enforce that `value >= bound` for a 32-bit value.
pub fn enforce_u32_gte<F: PrimeField>(
    value: &UInt32<F>,
    bound: u32,
) -> Result<(), SynthesisError> {
    let bound_fp = FpVar::<F>::constant(F::from(bound as u64));
    let value_fp = value.to_fp()?;
    let diff = value_fp - bound_fp;
    let diff_bits = diff.to_bits_le()?;
    for b in diff_bits.iter().skip(32) {
        b.enforce_equal(&Boolean::constant(false))?;
    }
    Ok(())
}

/// Enforce that `value > bound` for a 32-bit value.
pub fn enforce_u32_gt<F: PrimeField>(
    value: &UInt32<F>,
    bound: u32,
) -> Result<(), SynthesisError> {
    // value > bound  <=>  value >= bound + 1
    enforce_u32_gte(value, bound.saturating_add(1))
}

/// Enforce that `value` is in `[0, 2^bits)` via bit decomposition.
pub fn enforce_u32_range<F: PrimeField>(
    value: &UInt32<F>,
    bits: usize,
) -> Result<(), SynthesisError> {
    let value_bits = value.to_bits_le()?;
    for b in value_bits.iter().skip(bits) {
        b.enforce_equal(&Boolean::constant(false))?;
    }
    Ok(())
}

/// Enforce that a 64-bit value is in `[0, 2^bits)`.
pub fn enforce_u64_range<F: PrimeField>(
    value: &UInt64<F>,
    bits: usize,
) -> Result<(), SynthesisError> {
    let value_bits = value.to_bits_le()?;
    for b in value_bits.iter().skip(bits) {
        b.enforce_equal(&Boolean::constant(false))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_bls12_381::Fr;
    use ark_relations::r1cs::{ConstraintSystem, SynthesisMode};

    #[test]
    fn test_enforce_u32_gt() -> Result<(), SynthesisError> {
        let cs = ConstraintSystem::<Fr>::new_ref();
        cs.set_mode(SynthesisMode::Prove {
            construct_matrices: true,
        });
        let value = UInt32::new_witness(cs.clone(), || Ok(650u32))?;
        enforce_u32_gt(&value, 600)?;
        assert!(cs.is_satisfied()?);
        Ok(())
    }

    #[test]
    fn test_enforce_u32_gt_fails() -> Result<(), SynthesisError> {
        let cs = ConstraintSystem::<Fr>::new_ref();
        cs.set_mode(SynthesisMode::Prove {
            construct_matrices: true,
        });
        let value = UInt32::new_witness(cs.clone(), || Ok(500u32))?;
        enforce_u32_gt(&value, 600)?;
        assert!(!cs.is_satisfied()?);
        Ok(())
    }
}
