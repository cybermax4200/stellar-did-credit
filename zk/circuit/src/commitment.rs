//! Poseidon commitment for `ScoreRecord` fields.
//!
//! The commitment binds all private witness fields so a prover cannot misreport
//! any component of the score (see `docs/adr-001-avg-counterparties-binding.md`
//! for why `avg_counterparties` is part of the preimage).
//!
//! # Construction
//!
//! The commitment is an algebraic Poseidon hash over the BLS12-381 scalar
//! field `Fr`, used in *fixed-length* mode with a domain tag in the capacity
//! element:
//!
//! ```text
//! state_0 = [ DOMAIN_TAG, f_0, f_1, ..., f_7, blinding ]      (width t = 10)
//! state   = Poseidon-permutation(state_0)
//! C       = state[1]
//! ```
//!
//! All nine inputs fit in the rate (`rate = 9`, `capacity = 1`), so exactly one
//! permutation is needed both natively and in-circuit. The same permutation
//! code is used by the native prover, the WASM/SDK helper and the R1CS gadget
//! (`poseidon_commit_gadget`), so the three agree bit-for-bit.
//!
//! The in-circuit cost is `3 * (8 * 10 + 60) = 420` multiplication
//! constraints (each S-box `x^5` costs 3; the MDS layer and round constants are
//! linear and free), versus ~25,000 for a SHA-256 commitment.
//!
//! Parameters are documented in `docs/zk-spec.md` (section "Commitment").

use ark_bls12_381::Fr;
use ark_crypto_primitives::sponge::poseidon::find_poseidon_ark_and_mds;
use ark_ff::{BigInteger, PrimeField, UniformRand};
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::fields::FieldVar;
use ark_relations::r1cs::SynthesisError;
use ark_std::rand::RngCore;
use std::sync::OnceLock;

/// Number of committed score-record fields (excluding the blinding factor).
pub const COMMIT_FIELDS: usize = 8;
/// Number of Poseidon inputs: the score-record fields plus the blinding factor.
pub const COMMIT_INPUTS: usize = COMMIT_FIELDS + 1;

/// Poseidon state width `t = rate + capacity`.
pub const POSEIDON_WIDTH: usize = 10;
/// Poseidon rate (number of absorbed elements per permutation).
pub const POSEIDON_RATE: usize = POSEIDON_WIDTH - 1;
/// S-box exponent (`x -> x^5`; `gcd(5, r - 1) = 1` for BLS12-381 `Fr`).
pub const POSEIDON_ALPHA: u64 = 5;
/// Number of full rounds `R_F` (half before, half after the partial rounds).
pub const POSEIDON_FULL_ROUNDS: usize = 8;
/// Number of partial rounds `R_P`.
pub const POSEIDON_PARTIAL_ROUNDS: usize = 60;
/// Bit size of the field modulus passed to the Grain LFSR parameter generator.
pub const POSEIDON_PRIME_BITS: u64 = 255;

/// Domain tag placed in the capacity element. Interpreted as a big-endian
/// integer (21 bytes, well below the 255-bit modulus), so no extra hash is
/// involved anywhere in the commitment path.
pub const COMMITMENT_DOMAIN: &[u8] = b"sdc::score-commit::v2";

/// Round constants and MDS matrix for the commitment permutation.
pub struct PoseidonParams {
    /// Additive round constants, one row of `POSEIDON_WIDTH` per round
    /// (`R_F + R_P` rows).
    pub ark: Vec<Vec<Fr>>,
    /// `POSEIDON_WIDTH x POSEIDON_WIDTH` MDS matrix.
    pub mds: Vec<Vec<Fr>>,
}

/// Deterministically generated parameters (Grain LFSR, as in the Poseidon
/// reference implementation), computed once per process.
pub fn poseidon_params() -> &'static PoseidonParams {
    static PARAMS: OnceLock<PoseidonParams> = OnceLock::new();
    PARAMS.get_or_init(|| {
        let (ark, mds) = find_poseidon_ark_and_mds::<Fr>(
            POSEIDON_PRIME_BITS,
            POSEIDON_RATE,
            POSEIDON_FULL_ROUNDS as u64,
            POSEIDON_PARTIAL_ROUNDS as u64,
            0,
        );
        PoseidonParams { ark, mds }
    })
}

/// The domain tag as a field element.
pub fn domain_tag() -> Fr {
    assert!(COMMITMENT_DOMAIN.len() <= 31);
    Fr::from_be_bytes_mod_order(COMMITMENT_DOMAIN)
}

fn sbox(x: Fr) -> Fr {
    let x2 = x * x;
    let x4 = x2 * x2;
    x4 * x
}

/// Apply the Poseidon (Hades) permutation in place.
pub fn poseidon_permute(state: &mut [Fr; POSEIDON_WIDTH]) {
    let params = poseidon_params();
    let half_full = POSEIDON_FULL_ROUNDS / 2;
    let total = POSEIDON_FULL_ROUNDS + POSEIDON_PARTIAL_ROUNDS;
    for round in 0..total {
        for (s, c) in state.iter_mut().zip(params.ark[round].iter()) {
            *s += c;
        }
        let is_full = round < half_full || round >= half_full + POSEIDON_PARTIAL_ROUNDS;
        if is_full {
            for s in state.iter_mut() {
                *s = sbox(*s);
            }
        } else {
            state[0] = sbox(state[0]);
        }
        let mut next = [Fr::from(0u32); POSEIDON_WIDTH];
        for (i, out) in next.iter_mut().enumerate() {
            for (j, s) in state.iter().enumerate() {
                *out += params.mds[i][j] * s;
            }
        }
        *state = next;
    }
}

/// Fixed-length Poseidon hash of the nine commitment inputs with an explicit
/// capacity tag. Exposed so tests can cross-check against arkworks' sponge
/// (tag = 0); production code uses [`poseidon_commit`].
pub fn poseidon_hash_with_tag(inputs: &[Fr; COMMIT_INPUTS], tag: Fr) -> Fr {
    let mut state = [Fr::from(0u32); POSEIDON_WIDTH];
    state[0] = tag;
    state[1..].copy_from_slice(inputs);
    poseidon_permute(&mut state);
    state[1]
}

/// Commit to the 8 score-record fields with the given blinding factor.
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
pub fn poseidon_commit(fields: &[Fr; COMMIT_FIELDS], blinding: Fr) -> Fr {
    let mut inputs = [Fr::from(0u32); COMMIT_INPUTS];
    inputs[..COMMIT_FIELDS].copy_from_slice(fields);
    inputs[COMMIT_FIELDS] = blinding;
    poseidon_hash_with_tag(&inputs, domain_tag())
}

/// Canonical 32-byte big-endian encoding of a commitment (the encoding the
/// SDK and the verifier contract exchange).
pub fn commitment_to_bytes_be(c: Fr) -> [u8; 32] {
    let bytes = c.into_bigint().to_bytes_be();
    let mut out = [0u8; 32];
    out[32 - bytes.len()..].copy_from_slice(&bytes);
    out
}

/// Poseidon commitment scheme over the BLS12-381 scalar field.
///
/// Thin wrapper kept so call sites read `scheme.commit(&fields, blinding)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PoseidonCommitment;

impl PoseidonCommitment {
    pub fn new() -> Self {
        Self
    }

    /// Commit to the 8 score-record fields (see [`poseidon_commit`]).
    pub fn commit(&self, fields: &[Fr; COMMIT_FIELDS], blinding: Fr) -> Fr {
        poseidon_commit(fields, blinding)
    }

    /// Generate a random blinding factor.
    pub fn random_blinding<R: RngCore>(&self, rng: &mut R) -> Fr {
        Fr::rand(rng)
    }
}

// ---------------------------------------------------------------------------
// R1CS gadget
// ---------------------------------------------------------------------------

fn sbox_gadget(x: &FpVar<Fr>) -> Result<FpVar<Fr>, SynthesisError> {
    let x2 = x.square()?;
    let x4 = x2.square()?;
    Ok(x4 * x)
}

/// In-circuit Poseidon commitment. `inputs` are the 8 score-record fields
/// followed by the blinding factor, in the same order as [`poseidon_commit`].
pub fn poseidon_commit_gadget(
    inputs: &[FpVar<Fr>; COMMIT_INPUTS],
) -> Result<FpVar<Fr>, SynthesisError> {
    let params = poseidon_params();
    let half_full = POSEIDON_FULL_ROUNDS / 2;
    let total = POSEIDON_FULL_ROUNDS + POSEIDON_PARTIAL_ROUNDS;

    let mut state: Vec<FpVar<Fr>> = Vec::with_capacity(POSEIDON_WIDTH);
    state.push(FpVar::constant(domain_tag()));
    state.extend(inputs.iter().cloned());

    for round in 0..total {
        for (s, c) in state.iter_mut().zip(params.ark[round].iter()) {
            *s += FpVar::constant(*c);
        }
        let is_full = round < half_full || round >= half_full + POSEIDON_PARTIAL_ROUNDS;
        if is_full {
            for s in state.iter_mut() {
                *s = sbox_gadget(s)?;
            }
        } else {
            state[0] = sbox_gadget(&state[0])?;
        }
        let mut next: Vec<FpVar<Fr>> = Vec::with_capacity(POSEIDON_WIDTH);
        for row in params.mds.iter() {
            let mut acc = FpVar::<Fr>::zero();
            for (s, m) in state.iter().zip(row.iter()) {
                acc += s * FpVar::constant(*m);
            }
            next.push(acc);
        }
        state = next;
    }
    Ok(state[1].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_crypto_primitives::sponge::poseidon::{PoseidonConfig, PoseidonSponge};
    use ark_crypto_primitives::sponge::CryptographicSponge;
    use ark_r1cs_std::alloc::AllocVar;
    use ark_r1cs_std::R1CSVar;
    use ark_relations::r1cs::ConstraintSystem;
    use ark_std::test_rng;

    fn sample_fields() -> [Fr; COMMIT_FIELDS] {
        [
            Fr::from(668u32),
            Fr::from(10u32),
            Fr::from(50_000_000_000u64),
            Fr::from(10u32),
            Fr::from(5000u32),
            Fr::from(1_700_000_000u64),
            Fr::from(1_234_567u32),
            Fr::from(0u32),
        ]
    }

    #[test]
    fn commitment_is_hiding() {
        let mut rng = test_rng();
        let fields = [Fr::from(300u32); COMMIT_FIELDS];
        let c1 = poseidon_commit(&fields, Fr::rand(&mut rng));
        let c2 = poseidon_commit(&fields, Fr::rand(&mut rng));
        assert_ne!(c1, c2);
    }

    #[test]
    fn commitment_is_binding_per_field() {
        let base = sample_fields();
        let b = Fr::from(42u32);
        let c = poseidon_commit(&base, b);
        for i in 0..COMMIT_FIELDS {
            let mut tweaked = base;
            tweaked[i] += Fr::from(1u32);
            assert_ne!(poseidon_commit(&tweaked, b), c, "field {i} not bound");
        }
    }

    #[test]
    fn commitment_is_deterministic() {
        let fields = sample_fields();
        let b = Fr::from(42u32);
        assert_eq!(poseidon_commit(&fields, b), poseidon_commit(&fields, b));
    }

    #[test]
    fn domain_tag_changes_output() {
        let mut inputs = [Fr::from(0u32); COMMIT_INPUTS];
        inputs[0] = Fr::from(1u32);
        assert_ne!(
            poseidon_hash_with_tag(&inputs, domain_tag()),
            poseidon_hash_with_tag(&inputs, Fr::from(0u32))
        );
    }

    /// Independent cross-check of the permutation against arkworks' own
    /// Poseidon sponge (same parameters, capacity tag 0, one absorb of 9
    /// elements followed by one squeeze == one permutation).
    #[test]
    fn matches_arkworks_sponge() {
        let params = poseidon_params();
        let config = PoseidonConfig::new(
            POSEIDON_FULL_ROUNDS,
            POSEIDON_PARTIAL_ROUNDS,
            POSEIDON_ALPHA,
            params.mds.clone(),
            params.ark.clone(),
            POSEIDON_RATE,
            1,
        );
        let mut rng = test_rng();
        for _ in 0..8 {
            let mut inputs = [Fr::from(0u32); COMMIT_INPUTS];
            for i in inputs.iter_mut() {
                *i = Fr::rand(&mut rng);
            }
            let mut sponge = PoseidonSponge::<Fr>::new(&config);
            sponge.absorb(&inputs.to_vec());
            let expected: Fr = sponge.squeeze_field_elements(1)[0];
            assert_eq!(poseidon_hash_with_tag(&inputs, Fr::from(0u32)), expected);
        }
    }

    #[test]
    fn gadget_matches_native_bit_for_bit() {
        let mut rng = test_rng();
        for _ in 0..4 {
            let mut fields = [Fr::from(0u32); COMMIT_FIELDS];
            for f in fields.iter_mut() {
                *f = Fr::rand(&mut rng);
            }
            let blinding = Fr::rand(&mut rng);
            let native = poseidon_commit(&fields, blinding);

            let cs = ConstraintSystem::<Fr>::new_ref();
            let mut vars: Vec<FpVar<Fr>> = fields
                .iter()
                .map(|f| FpVar::new_witness(cs.clone(), || Ok(*f)).unwrap())
                .collect();
            vars.push(FpVar::new_witness(cs.clone(), || Ok(blinding)).unwrap());
            let inputs: [FpVar<Fr>; COMMIT_INPUTS] = vars.try_into().ok().unwrap();
            let out = poseidon_commit_gadget(&inputs).unwrap();
            assert_eq!(out.value().unwrap(), native);
            assert!(cs.is_satisfied().unwrap());
        }
    }

    /// Issue #1185 acceptance: commitment circuit strictly under 600
    /// constraints (SHA-256 would be ~25k).
    #[test]
    fn commitment_gadget_constraint_count_under_600() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let vars: Vec<FpVar<Fr>> = (0..COMMIT_INPUTS)
            .map(|i| FpVar::new_witness(cs.clone(), || Ok(Fr::from(i as u32 + 1))).unwrap())
            .collect();
        let inputs: [FpVar<Fr>; COMMIT_INPUTS] = vars.try_into().ok().unwrap();
        let before = cs.num_constraints();
        let _ = poseidon_commit_gadget(&inputs).unwrap();
        let used = cs.num_constraints() - before;
        let expected_max = 3 * (POSEIDON_WIDTH * POSEIDON_FULL_ROUNDS + POSEIDON_PARTIAL_ROUNDS);
        println!("poseidon commitment constraints: {used} (upper bound {expected_max})");
        assert!(used < 600, "commitment uses {used} constraints");
        assert!(used <= expected_max);
    }

    fn hex32(c: Fr) -> String {
        commitment_to_bytes_be(c)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Precomputed known-answer vectors (issue #1185). They pin the Grain-LFSR
    /// parameters, the permutation and the commitment layout; any change to
    /// round counts, constants, field order or domain tag breaks them. The
    /// permutation itself is independently cross-checked against arkworks'
    /// sponge in `matches_arkworks_sponge`.
    #[test]
    fn known_answer_vectors() {
        let p = poseidon_params();
        assert_eq!(
            hex32(p.ark[0][0]),
            "5ea9e5d10ba6da9024c548dcb8375695c6fc6e02396db37e10f1ee7e17b73339"
        );
        assert_eq!(
            hex32(p.mds[0][0]),
            "69426c93d2c7eca27dfd2adf6d3b4308f812835cda27767f1e74ed19f9d56a86"
        );

        let mut st = [Fr::from(0u32); POSEIDON_WIDTH];
        for (i, s) in st.iter_mut().enumerate() {
            *s = Fr::from(i as u32);
        }
        poseidon_permute(&mut st);
        assert_eq!(
            hex32(st[0]),
            "0b644e76ab8fb286571944fd9a9aaf9e5f07466efb5fc9d2bb2bfd5fafa4364f"
        );
        assert_eq!(
            hex32(st[9]),
            "71fe21a3573ce25c79f2b6b2a592aaaadd8870d3eed7ceceeeb812fdde7b8066"
        );

        // Vector 0: all-zero record, zero blinding.
        let zero = [Fr::from(0u32); COMMIT_FIELDS];
        assert_eq!(
            hex32(poseidon_commit(&zero, Fr::from(0u32))),
            "72648232330eef62ee941a5033161ad21e5d57da5480421506bbe73a9d827c24"
        );

        // Vector 1: the reference witness used across the circuit tests.
        assert_eq!(
            hex32(poseidon_commit(&sample_fields(), Fr::from(42u32))),
            "353d66f7722667f8eeb6000c417883172f1ac2ae5d59cb20bc2d9671a182c2d1"
        );

        // Vector 2: large values near each field's type maximum.
        let big = [
            Fr::from(850u32),
            Fr::from(5u32),
            Fr::from(1u64 << 40),
            Fr::from(100u32),
            Fr::from(10_000u32),
            Fr::from(u64::MAX),
            Fr::from(u32::MAX),
            Fr::from(1u32),
        ];
        assert_eq!(
            hex32(poseidon_commit(&big, Fr::from(u64::MAX))),
            "265efb8caab994f8a5a3913427464591f4bf5c7e38c2677de18262d0d94b2677"
        );
    }

    #[test]
    fn bytes_encoding_is_32_byte_big_endian() {
        let c = Fr::from(0x0102u32);
        let bytes = commitment_to_bytes_be(c);
        assert_eq!(bytes[30], 0x01);
        assert_eq!(bytes[31], 0x02);
        assert!(bytes[..30].iter().all(|b| *b == 0));
        assert_eq!(Fr::from_be_bytes_mod_order(&bytes), c);
    }
}
