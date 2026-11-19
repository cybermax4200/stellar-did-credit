//! Poseidon Merkle membership proof gadget (depth 20).
//!
//! Implements the optional on-chain state-binding step proposed in
//! `docs/zk-proof-design.md` (Research Q1 / Phase 4b): a subject proves that a
//! private credential commitment is a leaf of a Merkle tree whose root is
//! anchored (e.g. published by the credit-oracle) **without** revealing which
//! leaf or identity was used.
//!
//! * **Public input:** the anchored Merkle `root`.
//! * **Private witness:** the `leaf` and the 20-level authentication path
//!   (`siblings` hashes plus the `is_right` direction bits).
//!
//! Hashing uses the Poseidon sponge as a two-to-one compression function
//! (rate 2, capacity 1), which is SNARK-friendly and aligns with the proposed
//! CAP-0075 Poseidon host function.

use ark_bls12_381::Fr;
use ark_crypto_primitives::sponge::constraints::CryptographicSpongeVar;
use ark_crypto_primitives::sponge::poseidon::constraints::PoseidonSpongeVar;
use ark_crypto_primitives::sponge::poseidon::{PoseidonConfig, PoseidonSponge};
use ark_crypto_primitives::sponge::CryptographicSponge;
use ark_ff::{Field, PrimeField};
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::prelude::*;
use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};
use sha2::{Digest, Sha256};

/// Depth of the Merkle tree: 20 levels, i.e. up to `2^20` leaves.
pub const MERKLE_DEPTH: usize = 20;

/// Domain separator used to derive the Poseidon round constants.
pub const POSEIDON_DOMAIN: &[u8] = b"stellar-did-credit::poseidon-merkle::v1";

/// Poseidon sponge rate (field elements absorbed per permutation).
const POSEIDON_RATE: usize = 2;
/// Poseidon sponge capacity.
const POSEIDON_CAPACITY: usize = 1;
/// Number of Poseidon full rounds.
const POSEIDON_FULL_ROUNDS: usize = 8;
/// Number of Poseidon partial rounds.
const POSEIDON_PARTIAL_ROUNDS: usize = 56;
/// S-box exponent. `x^5` is a permutation over the BLS12-381 scalar field
/// because `gcd(5, p - 1) = 1`.
const POSEIDON_ALPHA: u64 = 5;

/// Upper bound on the R1CS constraints added by a depth-20 membership proof.
///
/// The bound is asserted by `tests::constraint_budget` and documents the budget
/// used in `docs/zk-proof-design.md`. The headroom above the measured count
/// keeps the budget stable across `ark-r1cs-std` patch releases.
pub const MERKLE_CONSTRAINT_BUDGET: usize = 8_000;

/// Build the deterministic Poseidon parameters used by the Merkle gadget.
///
/// The MDS matrix uses the Cauchy construction `M[i][j] = 1 / (i + t + j)`,
/// which is provably MDS. Round constants are derived "nothing up my sleeve"
/// from [`POSEIDON_DOMAIN`] with SHA-256.
///
/// These parameters are for the Phase 4 research circuit. Production
/// deployments should swap in the reference Poseidon parameter generator (or
/// the CAP-0075 host-function constants) and pin the chosen set in the
/// verification key.
pub fn poseidon_config() -> PoseidonConfig<Fr> {
    let width = POSEIDON_RATE + POSEIDON_CAPACITY;

    let mut mds = Vec::with_capacity(width);
    for i in 0..width {
        let mut row = Vec::with_capacity(width);
        for j in 0..width {
            let denominator = Fr::from((i + width + j) as u64);
            row.push(denominator.inverse().expect("non-zero Cauchy denominator"));
        }
        mds.push(row);
    }

    let total_rounds = POSEIDON_FULL_ROUNDS + POSEIDON_PARTIAL_ROUNDS;
    let mut ark = Vec::with_capacity(total_rounds);
    for round in 0..total_rounds {
        let mut row = Vec::with_capacity(width);
        for column in 0..width {
            row.push(round_constant(round, column));
        }
        ark.push(row);
    }

    PoseidonConfig::new(
        POSEIDON_FULL_ROUNDS,
        POSEIDON_PARTIAL_ROUNDS,
        POSEIDON_ALPHA,
        mds,
        ark,
        POSEIDON_RATE,
        POSEIDON_CAPACITY,
    )
}

/// Derive the round constant for `(round, column)` from the domain separator.
fn round_constant(round: usize, column: usize) -> Fr {
    let mut hasher = Sha256::new();
    hasher.update(POSEIDON_DOMAIN);
    hasher.update(b"::ark::");
    hasher.update((round as u32).to_le_bytes());
    hasher.update((column as u32).to_le_bytes());
    Fr::from_le_bytes_mod_order(&hasher.finalize())
}

/// Poseidon two-to-one compression (native, off-circuit).
///
/// This is the same function the gadget enforces: absorb `left`, absorb
/// `right`, squeeze one field element.
pub fn hash_two(cfg: &PoseidonConfig<Fr>, left: Fr, right: Fr) -> Fr {
    let mut sponge = PoseidonSponge::<Fr>::new(cfg);
    sponge.absorb(&left);
    sponge.absorb(&right);
    sponge.squeeze_field_elements::<Fr>(1)[0]
}

/// Private witness for the Merkle membership proof.
#[derive(Clone, Debug)]
pub struct MerkleWitness {
    /// The private leaf value (e.g. `H(ScoreRecord)`).
    pub leaf: Fr,
    /// Sibling hashes, indexed from the leaf level (`0`) up to the level below
    /// the root.
    pub siblings: [Fr; MERKLE_DEPTH],
    /// Direction bits: `true` when the current node is the right child at that
    /// level.
    pub is_right: [bool; MERKLE_DEPTH],
}

impl MerkleWitness {
    /// Recompute the Merkle root from this witness (native, off-circuit).
    pub fn root(&self, cfg: &PoseidonConfig<Fr>) -> Fr {
        let mut current = self.leaf;
        for (sibling, is_right) in self.siblings.iter().zip(self.is_right.iter()) {
            let (left, right) = if *is_right {
                (*sibling, current)
            } else {
                (current, *sibling)
            };
            current = hash_two(cfg, left, right);
        }
        current
    }
}

/// Public inputs for the Merkle membership proof.
#[derive(Clone, Debug)]
pub struct MerklePublicInputs {
    /// The anchored Merkle root the leaf must be included under.
    pub root: Fr,
}

/// The depth-20 Poseidon Merkle membership circuit.
pub struct MerkleMembershipCircuit {
    /// Private witness (leaf + authentication path).
    pub witness: Option<MerkleWitness>,
    /// Public anchored root.
    pub public_inputs: MerklePublicInputs,
}

impl MerkleMembershipCircuit {
    /// Build the circuit from an optional witness and its public inputs.
    pub fn new(witness: Option<MerkleWitness>, public_inputs: MerklePublicInputs) -> Self {
        Self {
            witness,
            public_inputs,
        }
    }
}

impl ConstraintSynthesizer<Fr> for MerkleMembershipCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        let cfg = poseidon_config();
        let w = self.witness.as_ref();

        // ---- Public input: the anchored root ----
        let root = FpVar::new_input(cs.clone(), || Ok(self.public_inputs.root))?;

        // ---- Private witness: leaf + authentication path ----
        let leaf = FpVar::new_witness(cs.clone(), || {
            Ok(w.map(|w| w.leaf).unwrap_or_else(|| Fr::from(0u32)))
        })?;

        let mut siblings: [FpVar<Fr>; MERKLE_DEPTH] = core::array::from_fn(|_| FpVar::zero());
        let mut directions: [Boolean<Fr>; MERKLE_DEPTH] =
            core::array::from_fn(|_| Boolean::constant(false));
        for level in 0..MERKLE_DEPTH {
            siblings[level] = FpVar::new_witness(cs.clone(), || {
                Ok(w.map(|w| w.siblings[level])
                    .unwrap_or_else(|| Fr::from(0u32)))
            })?;
            directions[level] = Boolean::new_witness(cs.clone(), || {
                Ok(w.map(|w| w.is_right[level]).unwrap_or(false))
            })?;
        }

        // ---- Constraint: recomputing the root from the private path equals
        // the public anchored root ----
        let computed = merkle_root_gadget(cs, &cfg, &leaf, &siblings, &directions)?;
        computed.enforce_equal(&root)?;

        Ok(())
    }
}

/// Recompute the Merkle root from a private leaf and 20-level authentication
/// path.
///
/// Each level selects the child order with the private direction bit (in
/// constant time) and applies one Poseidon two-to-one compression, so a
/// depth-20 proof costs 20 Poseidon permutations.
pub fn merkle_root_gadget(
    cs: ConstraintSystemRef<Fr>,
    cfg: &PoseidonConfig<Fr>,
    leaf: &FpVar<Fr>,
    siblings: &[FpVar<Fr>; MERKLE_DEPTH],
    directions: &[Boolean<Fr>; MERKLE_DEPTH],
) -> Result<FpVar<Fr>, SynthesisError> {
    let mut current = leaf.clone();
    for (sibling, direction) in siblings.iter().zip(directions.iter()) {
        let left = FpVar::conditionally_select(direction, sibling, &current)?;
        let right = FpVar::conditionally_select(direction, &current, sibling)?;
        current = hash_two_gadget(cs.clone(), cfg, &left, &right)?;
    }
    Ok(current)
}

/// Poseidon two-to-one compression as R1CS constraints.
fn hash_two_gadget(
    cs: ConstraintSystemRef<Fr>,
    cfg: &PoseidonConfig<Fr>,
    left: &FpVar<Fr>,
    right: &FpVar<Fr>,
) -> Result<FpVar<Fr>, SynthesisError> {
    let mut sponge = PoseidonSpongeVar::<Fr>::new(cs, cfg);
    sponge.absorb(left)?;
    sponge.absorb(right)?;
    Ok(sponge.squeeze_field_elements(1)?[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_relations::r1cs::{ConstraintSystem, SynthesisMode};

    fn synthesize(circuit: MerkleMembershipCircuit) -> ConstraintSystemRef<Fr> {
        let cs = ConstraintSystem::<Fr>::new_ref();
        cs.set_mode(SynthesisMode::Prove {
            construct_matrices: true,
        });
        circuit.generate_constraints(cs.clone()).unwrap();
        cs
    }

    /// Root of an all-empty subtree (zero-valued leaves) at the given height.
    fn empty_subtree_root(cfg: &PoseidonConfig<Fr>, height: usize) -> Fr {
        let mut node = Fr::from(0u32);
        for _ in 0..height {
            node = hash_two(cfg, node, node);
        }
        node
    }

    /// Witness for the leftmost leaf of a tree where every other subtree is
    /// empty: sibling `level` is the empty subtree of height `level`.
    fn leftmost_witness(cfg: &PoseidonConfig<Fr>, leaf: Fr) -> MerkleWitness {
        MerkleWitness {
            leaf,
            siblings: core::array::from_fn(|level| empty_subtree_root(cfg, level)),
            is_right: [false; MERKLE_DEPTH],
        }
    }

    #[test]
    fn valid_membership_path_is_satisfied() {
        let cfg = poseidon_config();
        let witness = leftmost_witness(&cfg, Fr::from(1234u32));
        let root = witness.root(&cfg);

        let cs = synthesize(MerkleMembershipCircuit::new(
            Some(witness),
            MerklePublicInputs { root },
        ));

        assert!(
            cs.is_satisfied().unwrap(),
            "a valid 20-level path must satisfy the circuit"
        );
    }

    #[test]
    fn valid_right_child_path_is_satisfied() {
        let cfg = poseidon_config();
        let leaf = Fr::from(99u32);
        let witness = MerkleWitness {
            leaf,
            siblings: core::array::from_fn(|level| empty_subtree_root(&cfg, level)),
            is_right: [true; MERKLE_DEPTH],
        };
        let root = witness.root(&cfg);

        let cs = synthesize(MerkleMembershipCircuit::new(
            Some(witness),
            MerklePublicInputs { root },
        ));

        assert!(cs.is_satisfied().unwrap());
    }

    #[test]
    fn tampered_sibling_fails() {
        let cfg = poseidon_config();
        let mut witness = leftmost_witness(&cfg, Fr::from(1234u32));
        let root = witness.root(&cfg);

        // Tamper with one level of the authentication path.
        witness.siblings[7] += Fr::from(1u32);

        let cs = synthesize(MerkleMembershipCircuit::new(
            Some(witness),
            MerklePublicInputs { root },
        ));

        assert!(
            !cs.is_satisfied().unwrap(),
            "an invalid authentication path must not satisfy the circuit"
        );
    }

    #[test]
    fn wrong_leaf_fails() {
        let cfg = poseidon_config();
        let mut witness = leftmost_witness(&cfg, Fr::from(1234u32));
        let root = witness.root(&cfg);

        // Claim to hold a different leaf than the one in the tree.
        witness.leaf = Fr::from(4321u32);

        let cs = synthesize(MerkleMembershipCircuit::new(
            Some(witness),
            MerklePublicInputs { root },
        ));

        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn wrong_root_fails() {
        let cfg = poseidon_config();
        let witness = leftmost_witness(&cfg, Fr::from(1234u32));
        let mut root = witness.root(&cfg);
        root += Fr::from(1u32);

        let cs = synthesize(MerkleMembershipCircuit::new(
            Some(witness),
            MerklePublicInputs { root },
        ));

        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn constraint_budget() {
        let cfg = poseidon_config();
        let witness = leftmost_witness(&cfg, Fr::from(7u32));
        let root = witness.root(&cfg);
        let cs = synthesize(MerkleMembershipCircuit::new(
            Some(witness),
            MerklePublicInputs { root },
        ));

        let count = cs.num_constraints();
        assert!(
            count <= MERKLE_CONSTRAINT_BUDGET,
            "Merkle gadget used {count} constraints, budget is {MERKLE_CONSTRAINT_BUDGET}"
        );
    }
}
