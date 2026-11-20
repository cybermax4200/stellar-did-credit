#![no_std]
//! On-chain Groth16 verifier for the Stellar DID Credit protocol.
//!
//! Verifies zero-knowledge proofs that a credit score falls within a
//! committed range without revealing the exact score. Uses Stellar's
//! BLS12-381 pairing host functions (CAP-0059) via `env.crypto().bls12_381()`
//! (soroban-sdk 22 API).
use soroban_sdk::crypto::bls12_381::{Fr, G1Affine, G2Affine};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, vec, Address, Bytes, BytesN,
    Env, Vec,
};

/// Fixed-size Groth16 proof encoding (BLS12-381), uncompressed points:
/// A (G1, 96 bytes) || B (G2, 192 bytes) || C (G1, 96 bytes) = 384 bytes.
pub const PROOF_SIZE: u32 = 384;

/// Number of public inputs for the score-range circuit.
pub const NUM_PUBLIC_INPUTS: u32 = 8;

/// Circuit version constant — must match the `domain_separator` in the
/// circuit artifact. Bumped on any breaking circuit change.
///
/// `2`: score commitment switched from Pedersen to Poseidon (issue #1185) and
/// the tier-band circuit was added (issue #1184).
pub const CIRCUIT_VERSION: u32 = 2;

/// Lower bounds (inclusive) of the score tiers, indexed by `tier_level - 1`:
/// Bronze `>= 600`, Silver `>= 700`, Gold `>= 800`. Must match
/// `zk_circuit::TIER_THRESHOLDS`.
pub const TIER_THRESHOLDS: [u32; 3] = [600, 700, 800];

/// Tier levels returned by [`ScoreRangeVerifier::verify_score_tier`].
pub const TIER_BRONZE: u32 = 1;
pub const TIER_SILVER: u32 = 2;
pub const TIER_GOLD: u32 = 3;
/// Returned by the tier verifiers when no tier was verified.
pub const NO_TIER: u32 = 0;

/// Public inputs for the score-range proof.
#[contracttype]
#[derive(Clone)]
pub struct ScoreRangePublicInputs {
    /// Minimum score the prover claims to exceed.
    pub threshold: u32,
    /// Stellar account bound to the proof.
    pub subject: Address,
    /// Source credit-oracle contract.
    pub credit_oracle_id: Address,
    /// Poseidon commitment to the ScoreRecord.
    pub score_commitment: BytesN<32>,
    /// Ledger sequence at computation time.
    pub snapshot_ledger: u32,
    /// Protocol version binding.
    pub domain_separator: BytesN<32>,
    /// Current timestamp in seconds (verifier time).
    pub current_timestamp: u64,
    /// Maximum validity window in seconds.
    pub max_validity_window: u64,
}

/// Public inputs for the tier-band proof. Same shape as
/// [`ScoreRangePublicInputs`] with `tier_level` in place of `threshold`, so the
/// two circuits share one public-input layout.
#[contracttype]
#[derive(Clone)]
pub struct ScoreTierPublicInputs {
    /// Tier the prover claims membership in (`1..=3`).
    pub tier_level: u32,
    /// Stellar account bound to the proof.
    pub subject: Address,
    /// Source credit-oracle contract.
    pub credit_oracle_id: Address,
    /// Poseidon commitment to the ScoreRecord.
    pub score_commitment: BytesN<32>,
    /// Ledger sequence at computation time.
    pub snapshot_ledger: u32,
    /// Protocol version binding.
    pub domain_separator: BytesN<32>,
    /// Current timestamp in seconds (verifier time).
    pub current_timestamp: u64,
    /// Maximum validity window in seconds.
    pub max_validity_window: u64,
}

/// Groth16 verification key for BLS12-381.
#[contracttype]
#[derive(Clone)]
pub struct VerificationKey {
    pub alpha: BytesN<96>,
    pub beta: BytesN<192>,
    pub gamma: BytesN<192>,
    pub delta: BytesN<192>,
    pub gamma_abc: Vec<BytesN<96>>,
}

/// Storage keys.
#[contracttype]
pub enum DataKey {
    Admin,
    VkHash,
    CircuitVersion,
    /// Replay-protection store: proof_hash -> consumed.
    ConsumedProof(BytesN<32>),
}

/// Error types.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum VerifierError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    NotInitialized = 3,
    InvalidProofSize = 4,
    CircuitVersionMismatch = 5,
    ProofAlreadyConsumed = 6,
    /// `tier_level` is not one of `1..=3`.
    InvalidTierLevel = 7,
}

const INSTANCE_BUMP_THRESHOLD: u32 = 5000;
const INSTANCE_BUMP_AMOUNT: u32 = 500_000;

const PERS_TTL_THRESHOLD: u32 = 120_960; // ~7 days
const PERS_TTL_EXTEND: u32 = 518_400; // ~30 days

/// Assemble a [`VerificationKey`] from raw byte arrays (`gamma_abc` must hold
/// one point per public input plus the constant term: `NUM_PUBLIC_INPUTS + 1`).
fn build_verification_key(
    env: &Env,
    alpha: &[u8; 96],
    beta: &[u8; 192],
    gamma: &[u8; 192],
    delta: &[u8; 192],
    gamma_abc_points: &[&[u8; 96]],
) -> VerificationKey {
    let mut gamma_abc = Vec::new(env);
    assert_eq!(gamma_abc_points.len() as u32, NUM_PUBLIC_INPUTS + 1);
    for point in gamma_abc_points {
        gamma_abc.push_back(BytesN::from_array(env, point));
    }
    VerificationKey {
        alpha: BytesN::from_array(env, alpha),
        beta: BytesN::from_array(env, beta),
        gamma: BytesN::from_array(env, gamma),
        delta: BytesN::from_array(env, delta),
        gamma_abc,
    }
}

/// Embedded verification key for the score-range (threshold) circuit.
///
/// These bytes match the development-only fixed-seed output in
/// `zk/circuit/vk_out.txt` (`cargo run -p zk-circuit --bin generate_vk --
/// threshold`); they are not a production ceremony key. The seed in
/// `zk/circuit/src/bin/generate_vk.rs` is public, so this key must not be
/// trusted for production proof verification. A ceremony key and a matching
/// proof fixture are needed before this verifier can be deployed securely.
///
/// The uncompressed key encoding is tested against Soroban's BLS host
/// functions (`test_circuit_proof_with_development_key`). Revalidate encoding
/// and point membership when replacing the key, and regenerate whenever
/// `zk/circuit` changes (circuit version 2: Poseidon commitment).
/// The supplied `vk_hash` is currently stored without being checked against it.
fn embedded_verification_key(env: &Env) -> VerificationKey {
    let alpha_bytes = [
        0x01, 0xFB, 0x00, 0xF7, 0x67, 0xE9, 0x9B, 0x6A, 0x77, 0x57, 0x53, 0x8D, 0x1D, 0x99, 0xB6,
        0x94, 0x31, 0xC0, 0x78, 0x40, 0xEF, 0x24, 0x83, 0xFD, 0xE5, 0x54, 0x84, 0x45, 0xDE, 0x60,
        0x91, 0x6E, 0x5A, 0xF8, 0xC2, 0x85, 0xD9, 0x46, 0x83, 0x8C, 0xB5, 0x72, 0x81, 0x65, 0xF9,
        0x8A, 0x21, 0x55, 0x16, 0xC7, 0xB9, 0x98, 0x45, 0xD2, 0xD7, 0x74, 0x8E, 0x42, 0x2C, 0xFF,
        0xEE, 0x6D, 0x10, 0x2A, 0xF5, 0xEE, 0x6C, 0x55, 0x34, 0xB5, 0xC9, 0x77, 0x32, 0xAB, 0x1E,
        0x18, 0xAC, 0xD4, 0x1A, 0x42, 0x7E, 0x23, 0x0A, 0x8E, 0x47, 0x76, 0x7B, 0x6D, 0xB4, 0x22,
        0x08, 0x8C, 0x0E, 0x26, 0xC7, 0xE9,
    ];

    let beta_bytes = [
        0x0F, 0xE9, 0xF3, 0x4C, 0x58, 0x6E, 0x46, 0x5B, 0xB8, 0xEB, 0xCC, 0xB1, 0x00, 0x82, 0x9D,
        0x7A, 0x54, 0x71, 0x0E, 0xD3, 0x0F, 0x94, 0x46, 0x16, 0x81, 0x71, 0x80, 0x3B, 0x66, 0x0D,
        0xA2, 0x80, 0xC3, 0x0C, 0x12, 0x8C, 0xAF, 0x5B, 0x78, 0x7B, 0x02, 0xB6, 0x70, 0xA0, 0xF6,
        0xD8, 0xBC, 0xE7, 0x0C, 0x86, 0x9C, 0x1B, 0xCE, 0x0B, 0x97, 0x18, 0x6F, 0xCF, 0xD5, 0x22,
        0xFB, 0xAF, 0x43, 0xB1, 0x0C, 0xB6, 0x52, 0xE2, 0x38, 0x9E, 0x36, 0x97, 0xBB, 0x98, 0x87,
        0x9F, 0x4F, 0x38, 0xEC, 0x97, 0xDA, 0x8C, 0x8E, 0x74, 0xEE, 0x5C, 0x05, 0xBB, 0x19, 0x11,
        0x91, 0xF4, 0x95, 0xFB, 0x58, 0xED, 0x11, 0x17, 0x1B, 0x5C, 0x91, 0x41, 0x9E, 0xAF, 0x77,
        0x14, 0x3D, 0x86, 0xCE, 0xB2, 0xD1, 0x50, 0x19, 0x61, 0x80, 0x2C, 0x23, 0xFE, 0xF3, 0x07,
        0x7B, 0x95, 0x88, 0x3D, 0x8A, 0x9A, 0xB4, 0xEB, 0x9D, 0xDE, 0x5B, 0xE4, 0xE2, 0x6F, 0x64,
        0x66, 0xBD, 0xEE, 0x4A, 0x6B, 0xF2, 0xA0, 0x24, 0x7F, 0x09, 0x6B, 0x0D, 0xD1, 0x73, 0xC1,
        0xDD, 0xD2, 0x83, 0x52, 0xFF, 0xC2, 0x0B, 0xEE, 0x8B, 0x9B, 0xA3, 0x91, 0xCE, 0x14, 0x07,
        0x88, 0x34, 0x18, 0xE3, 0xC4, 0xBD, 0x98, 0x89, 0xCB, 0xFB, 0x1C, 0x89, 0xAC, 0x5F, 0x1D,
        0x70, 0x0A, 0xBE, 0x33, 0x98, 0xEB, 0x80, 0xBF, 0xAF, 0xD8, 0xEA, 0x8C,
    ];

    let gamma_bytes = [
        0x0E, 0x92, 0x81, 0xC8, 0x7F, 0x39, 0x0F, 0x55, 0x67, 0x02, 0xA7, 0x47, 0xDE, 0x72, 0x25,
        0x91, 0x1F, 0xCA, 0xFC, 0x22, 0x29, 0x63, 0xC4, 0xC2, 0x02, 0x18, 0x28, 0xB3, 0x6D, 0x48,
        0xC1, 0x45, 0xF1, 0x73, 0xDF, 0x87, 0x89, 0xAE, 0x90, 0x0D, 0x0A, 0xE1, 0x8B, 0x9F, 0xCD,
        0x8C, 0x3E, 0xD8, 0x04, 0x97, 0x54, 0xEE, 0x8D, 0x1A, 0xFC, 0x22, 0xA2, 0x60, 0xBB, 0x47,
        0xE6, 0x9D, 0x25, 0x1F, 0x18, 0xB0, 0xFD, 0x1A, 0x65, 0x29, 0xAD, 0x4C, 0x20, 0x3A, 0x35,
        0xD6, 0xB6, 0x0D, 0x4F, 0x27, 0x66, 0x4C, 0x23, 0x58, 0x56, 0x90, 0x9D, 0x7D, 0x67, 0xD4,
        0x9E, 0x03, 0x39, 0x85, 0x8D, 0xD6, 0x10, 0x75, 0x20, 0x30, 0x2A, 0xA7, 0x48, 0xC6, 0x7E,
        0xE7, 0x8F, 0x55, 0x4A, 0xA3, 0x82, 0xCD, 0xDA, 0xD1, 0x33, 0xE3, 0x42, 0xDC, 0x2B, 0x04,
        0x50, 0xED, 0x42, 0x9E, 0x3A, 0xC9, 0xB1, 0x81, 0xC4, 0xAE, 0x33, 0xF1, 0xB0, 0x0A, 0x10,
        0x2F, 0xA6, 0xDD, 0x83, 0x8F, 0x52, 0x41, 0x1B, 0xBE, 0x12, 0x21, 0xF4, 0x06, 0x6F, 0xAC,
        0x51, 0x31, 0xC5, 0x62, 0x53, 0x9A, 0x69, 0x72, 0xF8, 0x92, 0xE4, 0x2B, 0x2B, 0xAE, 0xC1,
        0xEA, 0xC7, 0x10, 0x9E, 0x38, 0xE4, 0x5E, 0xA6, 0x52, 0x9F, 0xC4, 0x7C, 0x0B, 0x90, 0xCA,
        0x9D, 0xE1, 0x7B, 0x5A, 0xE9, 0x89, 0xF5, 0x88, 0x93, 0x5D, 0x02, 0x8F,
    ];

    let delta_bytes = [
        0x15, 0x19, 0x6C, 0x13, 0xD3, 0x47, 0x8D, 0xDF, 0x21, 0x64, 0x40, 0x4F, 0x53, 0x11, 0x5C,
        0x6B, 0x22, 0x56, 0x93, 0x2C, 0xE5, 0x70, 0x94, 0x63, 0x5D, 0xC3, 0x74, 0xBE, 0xF7, 0xB4,
        0xCA, 0xF2, 0x62, 0x62, 0xBC, 0x2F, 0xE3, 0xF0, 0x7B, 0xDE, 0x59, 0xBC, 0xA0, 0x98, 0xC4,
        0xA6, 0x8F, 0x72, 0x11, 0xA1, 0xDE, 0xC4, 0x2A, 0xE4, 0xE8, 0x97, 0x3E, 0x96, 0x0E, 0xEA,
        0x0F, 0x5B, 0x53, 0xF4, 0x7A, 0x62, 0xCC, 0x84, 0x86, 0x37, 0x8E, 0x89, 0x15, 0xB0, 0x39,
        0x93, 0xC4, 0x88, 0x30, 0x59, 0x0D, 0xAE, 0x62, 0x4F, 0x91, 0xE3, 0xA0, 0xBB, 0x21, 0x56,
        0x7F, 0x28, 0x03, 0x2C, 0x7C, 0xC2, 0x02, 0x38, 0xD4, 0x6A, 0x97, 0x46, 0x29, 0x7D, 0x81,
        0x1D, 0xA6, 0x37, 0xF3, 0x59, 0xA1, 0x26, 0xDF, 0xB5, 0x6E, 0xE7, 0x49, 0x65, 0xA7, 0x4B,
        0xDD, 0x5D, 0x8E, 0x21, 0x98, 0x30, 0x06, 0x00, 0x8F, 0x2C, 0xAC, 0xEF, 0x59, 0x2C, 0xE0,
        0x38, 0xB7, 0xC3, 0xCA, 0x95, 0x76, 0x00, 0x24, 0x41, 0x04, 0x0C, 0x86, 0x22, 0x2F, 0xD5,
        0x11, 0x27, 0x6D, 0x95, 0xA1, 0x63, 0x25, 0x55, 0x15, 0xD3, 0x7B, 0xCE, 0x59, 0xBB, 0x36,
        0xDD, 0x2E, 0x02, 0x05, 0x62, 0x39, 0xB0, 0xDB, 0xA0, 0xBA, 0xAE, 0xDD, 0x58, 0x06, 0x14,
        0x86, 0xFB, 0x80, 0xDA, 0xC2, 0xAC, 0x01, 0x7C, 0xF5, 0x39, 0xB6, 0x5A,
    ];

    let gamma_abc_0 = [
        0x14, 0x1D, 0x38, 0x76, 0x81, 0x24, 0xD0, 0xDD, 0x14, 0x7D, 0x31, 0x6F, 0x8C, 0x20, 0xE0,
        0x7F, 0xDB, 0x26, 0xDD, 0x36, 0x00, 0x12, 0x8C, 0x24, 0x05, 0x2A, 0xD0, 0xA7, 0xF8, 0xDE,
        0x3F, 0x09, 0xC1, 0xE1, 0xA2, 0x7A, 0x90, 0x47, 0x82, 0x3F, 0xDF, 0xCF, 0x25, 0xA5, 0x69,
        0x5B, 0xE6, 0xFB, 0x02, 0x36, 0x49, 0x1D, 0x42, 0xC3, 0x76, 0xDD, 0x2A, 0x78, 0xA3, 0x64,
        0xAD, 0xC5, 0xAF, 0xF1, 0xEE, 0x0B, 0xDE, 0x90, 0x20, 0xE8, 0x46, 0xF0, 0xA3, 0x62, 0xA0,
        0xDC, 0xCC, 0x24, 0xAE, 0x52, 0x3F, 0x28, 0x6D, 0xAB, 0x34, 0x85, 0xDE, 0x25, 0x55, 0xAB,
        0x81, 0xF3, 0xB5, 0xF7, 0x7F, 0xA3,
    ];

    let gamma_abc_1 = [
        0x06, 0x21, 0x2F, 0x5E, 0x95, 0x8F, 0xF3, 0xB0, 0x11, 0x96, 0xAF, 0x70, 0xDD, 0xE4, 0x4A,
        0xE5, 0xAF, 0x6A, 0x49, 0x95, 0x82, 0x41, 0x97, 0x06, 0xED, 0x50, 0xF1, 0xC2, 0x11, 0xE7,
        0x3F, 0xC4, 0xD2, 0x0E, 0x14, 0xB4, 0x48, 0x1C, 0x89, 0x9F, 0xC6, 0x7A, 0x17, 0x3F, 0x5A,
        0x1D, 0x2B, 0xEA, 0x06, 0xFB, 0x01, 0x7F, 0x0D, 0x63, 0xE5, 0x3B, 0x92, 0x09, 0x21, 0x9F,
        0x87, 0xF5, 0xBA, 0xFC, 0x84, 0xEA, 0x2A, 0xCC, 0xCF, 0x53, 0xDF, 0x2F, 0xE0, 0x51, 0xC7,
        0x39, 0x59, 0x1F, 0x98, 0xDD, 0x71, 0x64, 0xA6, 0x46, 0x5E, 0x92, 0xEE, 0x19, 0x41, 0x91,
        0x5C, 0x7F, 0x5C, 0x8E, 0x7D, 0xA1,
    ];

    let gamma_abc_2 = [
        0x01, 0xF0, 0x44, 0x37, 0x50, 0xBB, 0x84, 0x0F, 0x50, 0x6D, 0x96, 0x20, 0x08, 0x48, 0x83,
        0xDE, 0xE3, 0xF7, 0x26, 0x85, 0xE8, 0xE4, 0xE9, 0xEC, 0x51, 0x1C, 0x70, 0xCD, 0x9F, 0x1D,
        0x0D, 0xCB, 0x5E, 0x7C, 0x53, 0x18, 0xB1, 0x96, 0x15, 0xDE, 0xFB, 0xE6, 0x27, 0x7B, 0x9A,
        0x83, 0x86, 0x47, 0x00, 0x50, 0xBE, 0xE0, 0x57, 0xE1, 0xB8, 0x29, 0xB3, 0xDF, 0x40, 0xCF,
        0x49, 0x16, 0xDC, 0xE9, 0x94, 0xBC, 0xCA, 0x66, 0xA1, 0xB4, 0xA4, 0x17, 0x00, 0x27, 0xAC,
        0x26, 0xDE, 0xB6, 0xCD, 0x8A, 0x74, 0xC9, 0xDE, 0x6A, 0x44, 0xEB, 0xB7, 0xCB, 0xA9, 0x33,
        0xFD, 0xA7, 0x8A, 0xB2, 0xE4, 0xF7,
    ];

    let gamma_abc_3 = [
        0x0B, 0xF3, 0xCF, 0xCD, 0x5D, 0x08, 0x44, 0x77, 0xCB, 0x8F, 0x5A, 0x36, 0x1F, 0x5F, 0xB6,
        0x15, 0x42, 0xFE, 0x92, 0xD9, 0x4E, 0x52, 0xB8, 0x0B, 0xA7, 0x44, 0xA2, 0xB6, 0x23, 0xEF,
        0x06, 0xD3, 0xB7, 0x6C, 0xA4, 0x1D, 0xE5, 0xC9, 0x02, 0xDD, 0xDE, 0xAB, 0xAE, 0x8F, 0x57,
        0x15, 0x02, 0x0B, 0x11, 0x49, 0xED, 0x76, 0x26, 0x15, 0x67, 0xD0, 0xC4, 0xDA, 0x11, 0x49,
        0x88, 0x2C, 0x16, 0x59, 0x99, 0x32, 0x8F, 0xFF, 0x42, 0x5A, 0xA1, 0x55, 0xCB, 0x48, 0xE9,
        0x65, 0x10, 0xB9, 0x81, 0x55, 0x12, 0xB9, 0x0D, 0x2E, 0xD0, 0x83, 0x70, 0xDB, 0x6D, 0x07,
        0x55, 0x49, 0x64, 0x9F, 0x38, 0x1E,
    ];

    let gamma_abc_4 = [
        0x08, 0x47, 0x27, 0xE6, 0xB4, 0x90, 0xF6, 0x18, 0x41, 0x7C, 0x0B, 0xD8, 0x79, 0x04, 0x1F,
        0x1E, 0x33, 0xAD, 0x8E, 0x34, 0x14, 0xC4, 0xEF, 0xAA, 0x36, 0xC0, 0x72, 0x7D, 0x03, 0xC5,
        0xC1, 0x36, 0xF6, 0xDC, 0xEF, 0x1B, 0xBC, 0x7B, 0x59, 0x54, 0x7F, 0x58, 0x06, 0x6C, 0x06,
        0x2B, 0x4D, 0x4B, 0x0D, 0xDF, 0x1B, 0x79, 0x5B, 0xFC, 0xC2, 0xF5, 0xFE, 0x7D, 0x4F, 0x4E,
        0x7E, 0x44, 0x4C, 0xE8, 0xC5, 0x35, 0x9C, 0x41, 0x31, 0x52, 0x74, 0x9E, 0x0F, 0x0F, 0xCB,
        0xE2, 0xC6, 0x32, 0xB5, 0x52, 0x33, 0x27, 0x86, 0x12, 0x99, 0xB6, 0xCF, 0x59, 0x6E, 0x8C,
        0x7C, 0x50, 0x23, 0x67, 0x72, 0xA7,
    ];

    let gamma_abc_5 = [
        0x04, 0x29, 0x33, 0xE0, 0xAE, 0xB3, 0xD2, 0x61, 0xD3, 0xCE, 0x38, 0xFB, 0x38, 0x53, 0x2F,
        0x51, 0xF6, 0x8E, 0x2C, 0x4B, 0x99, 0xAE, 0xE6, 0x8E, 0x21, 0xE8, 0x3F, 0x5B, 0x24, 0xF3,
        0x01, 0x30, 0x46, 0x28, 0x72, 0x47, 0x5F, 0xA8, 0xAD, 0x93, 0x53, 0x9E, 0x7A, 0x7F, 0x7F,
        0xD6, 0x49, 0x44, 0x18, 0xBE, 0x3F, 0xA7, 0x02, 0xCF, 0x75, 0xD5, 0x82, 0x0C, 0x51, 0x50,
        0xB3, 0x39, 0x83, 0x67, 0x60, 0x8E, 0xBD, 0x6B, 0xA4, 0x25, 0xAD, 0xF3, 0x9C, 0xA7, 0xFE,
        0xFA, 0x04, 0x2A, 0x2F, 0x91, 0x7E, 0xE0, 0xB4, 0xDA, 0xAB, 0x29, 0xC8, 0x91, 0x7F, 0x81,
        0x0D, 0xF6, 0x8F, 0x8F, 0x0A, 0x9A,
    ];

    let gamma_abc_6 = [
        0x0F, 0xA0, 0x13, 0x4A, 0x8C, 0x5B, 0x55, 0x7D, 0x8A, 0xE0, 0x87, 0x09, 0x9C, 0x49, 0x0F,
        0x60, 0x6B, 0x65, 0x86, 0x2C, 0x36, 0x34, 0x3E, 0xFD, 0x49, 0x45, 0xEE, 0x18, 0x68, 0xF1,
        0xDB, 0xB4, 0x89, 0xC9, 0x6A, 0x0F, 0x14, 0x48, 0xAE, 0x04, 0xBF, 0x32, 0x37, 0x23, 0x6A,
        0x2E, 0xE8, 0xD7, 0x0F, 0x20, 0x7F, 0xB1, 0xBE, 0x41, 0x23, 0x07, 0x73, 0xF4, 0xEB, 0x17,
        0x46, 0x49, 0xDF, 0x1F, 0xA9, 0x2A, 0x6C, 0x28, 0x29, 0xD0, 0xC8, 0x68, 0x70, 0xB8, 0x85,
        0x8C, 0x81, 0xFB, 0x8D, 0x13, 0x47, 0xF9, 0x4C, 0xC8, 0x0F, 0x74, 0x52, 0xB5, 0x07, 0xC7,
        0x53, 0x21, 0x1D, 0x36, 0xD7, 0x29,
    ];

    let gamma_abc_7 = [
        0x13, 0x3E, 0x9A, 0x6A, 0x50, 0xB5, 0x7A, 0xEE, 0x2D, 0x02, 0xA1, 0x26, 0x66, 0xBF, 0xD5,
        0x12, 0x27, 0x82, 0x6B, 0x6B, 0x42, 0x54, 0x39, 0x5D, 0xEF, 0x84, 0x7A, 0x6D, 0x78, 0xE7,
        0x05, 0x97, 0x92, 0x60, 0x05, 0x4D, 0xD7, 0xE6, 0x0E, 0x67, 0xD3, 0xED, 0x11, 0xDA, 0x50,
        0x2D, 0x49, 0x1B, 0x14, 0xEB, 0x91, 0x97, 0x2B, 0x72, 0x21, 0x08, 0xC9, 0x68, 0x4F, 0x41,
        0x6F, 0x94, 0x87, 0x94, 0x2C, 0xBD, 0x50, 0xC7, 0xE0, 0xBA, 0xD3, 0xDD, 0x03, 0xC7, 0x20,
        0x10, 0xD7, 0x8B, 0x58, 0x67, 0x72, 0xB0, 0x86, 0x40, 0x38, 0x0F, 0x97, 0x48, 0x87, 0x1D,
        0xAF, 0x8B, 0x3E, 0xF6, 0xD7, 0xBB,
    ];

    let gamma_abc_8 = [
        0x0C, 0x99, 0xD9, 0x66, 0xC7, 0x3F, 0x5E, 0x9A, 0x0A, 0x4D, 0x70, 0xC5, 0x14, 0x18, 0xE7,
        0xA5, 0x55, 0xD0, 0xDA, 0x82, 0x8A, 0x6C, 0xEC, 0x77, 0x00, 0x11, 0x5F, 0x92, 0xC6, 0xC4,
        0xA9, 0x03, 0x19, 0x4C, 0x54, 0xA0, 0x34, 0xBB, 0xBE, 0xC8, 0x7C, 0xC5, 0xF8, 0x56, 0x16,
        0x26, 0xC3, 0x86, 0x0C, 0x4A, 0x9F, 0x3E, 0x33, 0xB5, 0x48, 0x2C, 0xA0, 0x11, 0xB1, 0xBB,
        0x57, 0x8A, 0xF7, 0x11, 0xEF, 0x14, 0x49, 0xFF, 0xD5, 0x76, 0xA1, 0x4D, 0xA9, 0xDA, 0xA4,
        0x33, 0x4E, 0xDD, 0x69, 0x92, 0x18, 0xB3, 0x84, 0x7B, 0xB1, 0xD0, 0xE9, 0xDB, 0x36, 0xE1,
        0xEB, 0x7C, 0x44, 0xE4, 0x48, 0xED,
    ];

    build_verification_key(
        env,
        &alpha_bytes,
        &beta_bytes,
        &gamma_bytes,
        &delta_bytes,
        &[
            &gamma_abc_0,
            &gamma_abc_1,
            &gamma_abc_2,
            &gamma_abc_3,
            &gamma_abc_4,
            &gamma_abc_5,
            &gamma_abc_6,
            &gamma_abc_7,
            &gamma_abc_8,
        ],
    )
}

/// Embedded verification key for the tier-band circuit (issue #1184).
///
/// Development-only key, same caveats as [`embedded_verification_key`]; the
/// bytes match `zk/circuit/vk_tier_out.txt` (`generate_vk -- tier`) and are
/// checked against a real proof by `test_tier_proof_with_development_key`.
fn embedded_tier_verification_key(env: &Env) -> VerificationKey {
    let alpha_bytes = [
        0x01, 0xFB, 0x00, 0xF7, 0x67, 0xE9, 0x9B, 0x6A, 0x77, 0x57, 0x53, 0x8D, 0x1D, 0x99, 0xB6,
        0x94, 0x31, 0xC0, 0x78, 0x40, 0xEF, 0x24, 0x83, 0xFD, 0xE5, 0x54, 0x84, 0x45, 0xDE, 0x60,
        0x91, 0x6E, 0x5A, 0xF8, 0xC2, 0x85, 0xD9, 0x46, 0x83, 0x8C, 0xB5, 0x72, 0x81, 0x65, 0xF9,
        0x8A, 0x21, 0x55, 0x16, 0xC7, 0xB9, 0x98, 0x45, 0xD2, 0xD7, 0x74, 0x8E, 0x42, 0x2C, 0xFF,
        0xEE, 0x6D, 0x10, 0x2A, 0xF5, 0xEE, 0x6C, 0x55, 0x34, 0xB5, 0xC9, 0x77, 0x32, 0xAB, 0x1E,
        0x18, 0xAC, 0xD4, 0x1A, 0x42, 0x7E, 0x23, 0x0A, 0x8E, 0x47, 0x76, 0x7B, 0x6D, 0xB4, 0x22,
        0x08, 0x8C, 0x0E, 0x26, 0xC7, 0xE9,
    ];

    let beta_bytes = [
        0x0F, 0xE9, 0xF3, 0x4C, 0x58, 0x6E, 0x46, 0x5B, 0xB8, 0xEB, 0xCC, 0xB1, 0x00, 0x82, 0x9D,
        0x7A, 0x54, 0x71, 0x0E, 0xD3, 0x0F, 0x94, 0x46, 0x16, 0x81, 0x71, 0x80, 0x3B, 0x66, 0x0D,
        0xA2, 0x80, 0xC3, 0x0C, 0x12, 0x8C, 0xAF, 0x5B, 0x78, 0x7B, 0x02, 0xB6, 0x70, 0xA0, 0xF6,
        0xD8, 0xBC, 0xE7, 0x0C, 0x86, 0x9C, 0x1B, 0xCE, 0x0B, 0x97, 0x18, 0x6F, 0xCF, 0xD5, 0x22,
        0xFB, 0xAF, 0x43, 0xB1, 0x0C, 0xB6, 0x52, 0xE2, 0x38, 0x9E, 0x36, 0x97, 0xBB, 0x98, 0x87,
        0x9F, 0x4F, 0x38, 0xEC, 0x97, 0xDA, 0x8C, 0x8E, 0x74, 0xEE, 0x5C, 0x05, 0xBB, 0x19, 0x11,
        0x91, 0xF4, 0x95, 0xFB, 0x58, 0xED, 0x11, 0x17, 0x1B, 0x5C, 0x91, 0x41, 0x9E, 0xAF, 0x77,
        0x14, 0x3D, 0x86, 0xCE, 0xB2, 0xD1, 0x50, 0x19, 0x61, 0x80, 0x2C, 0x23, 0xFE, 0xF3, 0x07,
        0x7B, 0x95, 0x88, 0x3D, 0x8A, 0x9A, 0xB4, 0xEB, 0x9D, 0xDE, 0x5B, 0xE4, 0xE2, 0x6F, 0x64,
        0x66, 0xBD, 0xEE, 0x4A, 0x6B, 0xF2, 0xA0, 0x24, 0x7F, 0x09, 0x6B, 0x0D, 0xD1, 0x73, 0xC1,
        0xDD, 0xD2, 0x83, 0x52, 0xFF, 0xC2, 0x0B, 0xEE, 0x8B, 0x9B, 0xA3, 0x91, 0xCE, 0x14, 0x07,
        0x88, 0x34, 0x18, 0xE3, 0xC4, 0xBD, 0x98, 0x89, 0xCB, 0xFB, 0x1C, 0x89, 0xAC, 0x5F, 0x1D,
        0x70, 0x0A, 0xBE, 0x33, 0x98, 0xEB, 0x80, 0xBF, 0xAF, 0xD8, 0xEA, 0x8C,
    ];

    let gamma_bytes = [
        0x0E, 0x92, 0x81, 0xC8, 0x7F, 0x39, 0x0F, 0x55, 0x67, 0x02, 0xA7, 0x47, 0xDE, 0x72, 0x25,
        0x91, 0x1F, 0xCA, 0xFC, 0x22, 0x29, 0x63, 0xC4, 0xC2, 0x02, 0x18, 0x28, 0xB3, 0x6D, 0x48,
        0xC1, 0x45, 0xF1, 0x73, 0xDF, 0x87, 0x89, 0xAE, 0x90, 0x0D, 0x0A, 0xE1, 0x8B, 0x9F, 0xCD,
        0x8C, 0x3E, 0xD8, 0x04, 0x97, 0x54, 0xEE, 0x8D, 0x1A, 0xFC, 0x22, 0xA2, 0x60, 0xBB, 0x47,
        0xE6, 0x9D, 0x25, 0x1F, 0x18, 0xB0, 0xFD, 0x1A, 0x65, 0x29, 0xAD, 0x4C, 0x20, 0x3A, 0x35,
        0xD6, 0xB6, 0x0D, 0x4F, 0x27, 0x66, 0x4C, 0x23, 0x58, 0x56, 0x90, 0x9D, 0x7D, 0x67, 0xD4,
        0x9E, 0x03, 0x39, 0x85, 0x8D, 0xD6, 0x10, 0x75, 0x20, 0x30, 0x2A, 0xA7, 0x48, 0xC6, 0x7E,
        0xE7, 0x8F, 0x55, 0x4A, 0xA3, 0x82, 0xCD, 0xDA, 0xD1, 0x33, 0xE3, 0x42, 0xDC, 0x2B, 0x04,
        0x50, 0xED, 0x42, 0x9E, 0x3A, 0xC9, 0xB1, 0x81, 0xC4, 0xAE, 0x33, 0xF1, 0xB0, 0x0A, 0x10,
        0x2F, 0xA6, 0xDD, 0x83, 0x8F, 0x52, 0x41, 0x1B, 0xBE, 0x12, 0x21, 0xF4, 0x06, 0x6F, 0xAC,
        0x51, 0x31, 0xC5, 0x62, 0x53, 0x9A, 0x69, 0x72, 0xF8, 0x92, 0xE4, 0x2B, 0x2B, 0xAE, 0xC1,
        0xEA, 0xC7, 0x10, 0x9E, 0x38, 0xE4, 0x5E, 0xA6, 0x52, 0x9F, 0xC4, 0x7C, 0x0B, 0x90, 0xCA,
        0x9D, 0xE1, 0x7B, 0x5A, 0xE9, 0x89, 0xF5, 0x88, 0x93, 0x5D, 0x02, 0x8F,
    ];

    let delta_bytes = [
        0x15, 0x19, 0x6C, 0x13, 0xD3, 0x47, 0x8D, 0xDF, 0x21, 0x64, 0x40, 0x4F, 0x53, 0x11, 0x5C,
        0x6B, 0x22, 0x56, 0x93, 0x2C, 0xE5, 0x70, 0x94, 0x63, 0x5D, 0xC3, 0x74, 0xBE, 0xF7, 0xB4,
        0xCA, 0xF2, 0x62, 0x62, 0xBC, 0x2F, 0xE3, 0xF0, 0x7B, 0xDE, 0x59, 0xBC, 0xA0, 0x98, 0xC4,
        0xA6, 0x8F, 0x72, 0x11, 0xA1, 0xDE, 0xC4, 0x2A, 0xE4, 0xE8, 0x97, 0x3E, 0x96, 0x0E, 0xEA,
        0x0F, 0x5B, 0x53, 0xF4, 0x7A, 0x62, 0xCC, 0x84, 0x86, 0x37, 0x8E, 0x89, 0x15, 0xB0, 0x39,
        0x93, 0xC4, 0x88, 0x30, 0x59, 0x0D, 0xAE, 0x62, 0x4F, 0x91, 0xE3, 0xA0, 0xBB, 0x21, 0x56,
        0x7F, 0x28, 0x03, 0x2C, 0x7C, 0xC2, 0x02, 0x38, 0xD4, 0x6A, 0x97, 0x46, 0x29, 0x7D, 0x81,
        0x1D, 0xA6, 0x37, 0xF3, 0x59, 0xA1, 0x26, 0xDF, 0xB5, 0x6E, 0xE7, 0x49, 0x65, 0xA7, 0x4B,
        0xDD, 0x5D, 0x8E, 0x21, 0x98, 0x30, 0x06, 0x00, 0x8F, 0x2C, 0xAC, 0xEF, 0x59, 0x2C, 0xE0,
        0x38, 0xB7, 0xC3, 0xCA, 0x95, 0x76, 0x00, 0x24, 0x41, 0x04, 0x0C, 0x86, 0x22, 0x2F, 0xD5,
        0x11, 0x27, 0x6D, 0x95, 0xA1, 0x63, 0x25, 0x55, 0x15, 0xD3, 0x7B, 0xCE, 0x59, 0xBB, 0x36,
        0xDD, 0x2E, 0x02, 0x05, 0x62, 0x39, 0xB0, 0xDB, 0xA0, 0xBA, 0xAE, 0xDD, 0x58, 0x06, 0x14,
        0x86, 0xFB, 0x80, 0xDA, 0xC2, 0xAC, 0x01, 0x7C, 0xF5, 0x39, 0xB6, 0x5A,
    ];

    let gamma_abc_0 = [
        0x18, 0x11, 0x89, 0x6D, 0x79, 0xE3, 0xEE, 0xB7, 0xC0, 0xF0, 0x1C, 0x3E, 0xED, 0xE4, 0x0A,
        0xC8, 0x36, 0x1B, 0x3C, 0x37, 0xE4, 0x53, 0xCF, 0x5B, 0x0A, 0x94, 0x47, 0x77, 0xF4, 0x60,
        0xF6, 0xE4, 0xFB, 0x5B, 0xB9, 0xD9, 0x37, 0xAD, 0x54, 0x92, 0x6D, 0xE3, 0x42, 0x2C, 0x3B,
        0x23, 0x88, 0xF2, 0x16, 0x87, 0x3E, 0xF1, 0x1C, 0xD3, 0xB2, 0x1C, 0x0F, 0xF3, 0x54, 0x68,
        0x5A, 0x07, 0x53, 0xBD, 0xA1, 0x33, 0x34, 0xF8, 0x3E, 0x7F, 0x24, 0xB1, 0x09, 0xD3, 0x95,
        0xDF, 0x7A, 0x25, 0x5D, 0x4C, 0x00, 0x64, 0xD9, 0x72, 0x5C, 0x33, 0x8D, 0x0B, 0x0F, 0x7C,
        0xB0, 0x20, 0x29, 0x61, 0xF0, 0x7B,
    ];

    let gamma_abc_1 = [
        0x11, 0x69, 0xDD, 0x89, 0xBA, 0xE0, 0x96, 0xAE, 0x12, 0x09, 0x89, 0xD8, 0x75, 0xB9, 0xD2,
        0x1E, 0x88, 0xB2, 0x3C, 0x49, 0xB0, 0x99, 0x90, 0x65, 0xBB, 0x90, 0x89, 0xBC, 0x6D, 0x1E,
        0x58, 0xDE, 0x35, 0x42, 0x23, 0x2E, 0x90, 0xC4, 0xA0, 0x7A, 0xE0, 0xCA, 0xFC, 0x20, 0x5F,
        0xAF, 0x4C, 0x39, 0x10, 0xE6, 0x89, 0xAD, 0x31, 0x0F, 0x83, 0x09, 0x7E, 0x27, 0xD7, 0x19,
        0x8F, 0xF6, 0x57, 0x32, 0xD0, 0xC2, 0x52, 0x0F, 0xD3, 0x14, 0x5A, 0xA6, 0xCF, 0xD0, 0xF8,
        0x7D, 0x51, 0x25, 0xAF, 0x42, 0xAE, 0x5E, 0xF8, 0xDD, 0x03, 0x61, 0xE4, 0x22, 0xAB, 0x9E,
        0x5C, 0x89, 0x96, 0xEF, 0x6A, 0xD3,
    ];

    let gamma_abc_2 = [
        0x00, 0x92, 0xD1, 0x36, 0x62, 0x13, 0xA2, 0xBC, 0x9A, 0x70, 0xD5, 0xC0, 0x56, 0xF3, 0xBE,
        0x31, 0x41, 0x2D, 0x8D, 0x46, 0xC8, 0x20, 0x88, 0x8A, 0x5E, 0x29, 0x67, 0xB2, 0xFF, 0x1C,
        0xD9, 0x59, 0x43, 0x26, 0x3F, 0x38, 0x5B, 0xFB, 0x93, 0x6C, 0xEC, 0x9F, 0xAD, 0x11, 0x4D,
        0x58, 0x4D, 0xFF, 0x15, 0x07, 0x21, 0x08, 0x88, 0x7A, 0xAA, 0xCF, 0xD0, 0x69, 0x04, 0x0D,
        0xAC, 0x4F, 0xA6, 0x1B, 0x10, 0x0F, 0xAD, 0x94, 0x7A, 0xF7, 0x5A, 0x69, 0x8E, 0x09, 0x60,
        0xDF, 0x48, 0x52, 0x3D, 0x7A, 0xDE, 0x1C, 0xC4, 0x70, 0x27, 0xD2, 0x7F, 0x16, 0x17, 0x87,
        0x47, 0xF3, 0x26, 0x3B, 0xA3, 0x73,
    ];

    let gamma_abc_3 = [
        0x14, 0x1D, 0xCE, 0xBC, 0xBC, 0x1D, 0x32, 0x22, 0xB1, 0x08, 0xE1, 0x3B, 0xAF, 0x55, 0xBB,
        0x75, 0x6A, 0x4C, 0x4E, 0xDA, 0x99, 0x35, 0xB6, 0xAA, 0xD0, 0x9C, 0x25, 0xDF, 0x8A, 0x49,
        0x18, 0x50, 0xEC, 0x62, 0x9E, 0x37, 0x0C, 0x40, 0x01, 0x9B, 0xDD, 0xFF, 0xBE, 0xFF, 0xA8,
        0x98, 0x18, 0xC0, 0x04, 0x77, 0x91, 0xF9, 0x81, 0xC5, 0xAB, 0x23, 0x76, 0x52, 0xDE, 0x67,
        0x33, 0x5D, 0xD4, 0xCB, 0x49, 0x01, 0xB8, 0xC3, 0x28, 0xD5, 0xAA, 0xFD, 0x5A, 0x31, 0xAB,
        0xD7, 0xCF, 0x2D, 0xED, 0xF0, 0xB6, 0x4C, 0xA4, 0x8F, 0x35, 0x1B, 0x1F, 0x26, 0x29, 0x61,
        0x59, 0x33, 0xC9, 0x07, 0xC1, 0x1C,
    ];

    let gamma_abc_4 = [
        0x03, 0x32, 0xCD, 0xB9, 0xCD, 0xD3, 0xDF, 0x4A, 0x5E, 0x46, 0x37, 0xB1, 0x4F, 0xC5, 0xC5,
        0x72, 0x7F, 0x89, 0x3C, 0x53, 0xB0, 0x41, 0xA0, 0x15, 0xC7, 0x2D, 0x24, 0xE9, 0xFE, 0xB7,
        0x8F, 0x5A, 0x4C, 0xDA, 0x63, 0x62, 0xD3, 0xC1, 0x2E, 0xAF, 0xD7, 0xA6, 0x48, 0x11, 0xB9,
        0x7F, 0xE4, 0x6D, 0x13, 0x18, 0x47, 0xCE, 0xC4, 0x90, 0x77, 0x18, 0x1B, 0x96, 0x64, 0xB9,
        0xC2, 0xF7, 0x9B, 0x35, 0xEE, 0x4C, 0x47, 0xF8, 0x76, 0x0C, 0xD6, 0xA4, 0x56, 0x58, 0xC4,
        0xB9, 0x35, 0xFB, 0x98, 0x54, 0x24, 0x0A, 0xAB, 0x4A, 0x2D, 0x91, 0x37, 0x68, 0xD9, 0x83,
        0xEC, 0x8B, 0xCB, 0xCD, 0x76, 0xD5,
    ];

    let gamma_abc_5 = [
        0x17, 0xE5, 0x54, 0xDF, 0xE6, 0x88, 0xF3, 0x43, 0x86, 0x67, 0x01, 0x42, 0x91, 0x8A, 0x98,
        0x45, 0xFF, 0x75, 0x69, 0x18, 0x1D, 0xE1, 0xF5, 0x34, 0x3F, 0xA9, 0x1A, 0x70, 0xA1, 0xB2,
        0x01, 0x08, 0xA6, 0xF7, 0x03, 0xB4, 0x44, 0x48, 0xC2, 0xD4, 0x46, 0xAE, 0xB8, 0x50, 0x56,
        0xC4, 0xCC, 0x1A, 0x19, 0xFC, 0x25, 0x19, 0x31, 0x23, 0x59, 0xEA, 0x5A, 0xE7, 0xB8, 0x00,
        0x85, 0x80, 0x6F, 0xBB, 0xBE, 0xB9, 0x3F, 0x73, 0x52, 0x78, 0x7A, 0x1C, 0xE4, 0x1B, 0x8C,
        0x56, 0x0D, 0x41, 0xBA, 0xBA, 0xCB, 0xB1, 0x29, 0x4B, 0xB7, 0xB2, 0x01, 0x91, 0x2D, 0x29,
        0x56, 0x70, 0xD2, 0x98, 0xB0, 0xDC,
    ];

    let gamma_abc_6 = [
        0x04, 0x36, 0xA0, 0x20, 0x93, 0xDE, 0xD3, 0x65, 0xB6, 0xA4, 0x1C, 0x72, 0xE0, 0xD5, 0xC0,
        0x24, 0xAE, 0xF0, 0x42, 0x92, 0x37, 0xD8, 0x8C, 0xE9, 0xC2, 0xEF, 0x5F, 0x4C, 0x04, 0xA0,
        0xE8, 0x79, 0xEA, 0xFA, 0x82, 0x6F, 0xE9, 0x16, 0xAE, 0xBB, 0x61, 0xD3, 0x7D, 0x2B, 0xBD,
        0x32, 0xEB, 0xD1, 0x14, 0x70, 0x03, 0xAA, 0x4E, 0x98, 0xA7, 0xDD, 0x4F, 0x63, 0x06, 0x87,
        0x62, 0xD8, 0x1B, 0x21, 0xF7, 0xFA, 0xDC, 0x48, 0x7B, 0x14, 0xC3, 0xC6, 0x97, 0x7C, 0xD6,
        0xDD, 0x19, 0x2E, 0xB6, 0x1E, 0x4B, 0xB1, 0x5D, 0xD8, 0x5F, 0xB4, 0xA0, 0xF4, 0x35, 0x8E,
        0x1F, 0x12, 0xBF, 0x80, 0xA2, 0x2D,
    ];

    let gamma_abc_7 = [
        0x14, 0x32, 0x9B, 0x50, 0xF4, 0x48, 0xF9, 0x76, 0x97, 0x3B, 0xE7, 0x17, 0x8C, 0xDD, 0x3E,
        0x3F, 0xFF, 0xA3, 0xF6, 0x48, 0xE5, 0xCB, 0x67, 0x57, 0x56, 0xFF, 0x59, 0x1F, 0xEB, 0x3B,
        0x14, 0x54, 0xA6, 0x38, 0x82, 0xD8, 0x88, 0xA2, 0xE9, 0x3A, 0xFA, 0x73, 0xD9, 0xC8, 0x7E,
        0x0B, 0xB0, 0x9A, 0x0C, 0xA2, 0xA5, 0x4F, 0x73, 0x5B, 0x77, 0x8B, 0x9E, 0xF9, 0x3D, 0xA8,
        0x07, 0xAB, 0x7A, 0x16, 0x96, 0x6B, 0x70, 0xE0, 0x36, 0x86, 0xEB, 0x8F, 0x84, 0xB3, 0xC4,
        0x98, 0x08, 0xC6, 0x29, 0x24, 0xF1, 0xDF, 0xB1, 0xEE, 0x48, 0x63, 0x59, 0xAD, 0xAC, 0xF6,
        0x76, 0x54, 0x9D, 0x37, 0xE2, 0x9F,
    ];

    let gamma_abc_8 = [
        0x18, 0xF2, 0x9B, 0x6E, 0x56, 0x3E, 0x90, 0xD8, 0x74, 0xE7, 0x23, 0x7E, 0x0A, 0x87, 0x3F,
        0x2C, 0xF2, 0x9D, 0x3B, 0x81, 0xEA, 0x18, 0x58, 0xF5, 0x5E, 0x22, 0xC9, 0x60, 0xDD, 0xCF,
        0x7F, 0x55, 0x26, 0xA0, 0x45, 0x3F, 0x64, 0x9F, 0x19, 0x14, 0x24, 0xFF, 0x48, 0x95, 0xDD,
        0xB9, 0x5C, 0x8C, 0x14, 0xD2, 0xA7, 0x42, 0xD4, 0xCF, 0xC1, 0x21, 0x49, 0xE1, 0x74, 0x4A,
        0xED, 0xDD, 0xD0, 0x1E, 0x3B, 0x83, 0x98, 0x6D, 0xBC, 0xBB, 0x07, 0x42, 0x99, 0x69, 0x74,
        0x30, 0xD0, 0xA9, 0x8C, 0x7B, 0x51, 0xBC, 0xF4, 0x46, 0x32, 0xEA, 0x96, 0x7D, 0x88, 0x58,
        0x0A, 0xFF, 0x9A, 0x1F, 0xA8, 0xE6,
    ];

    build_verification_key(
        env,
        &alpha_bytes,
        &beta_bytes,
        &gamma_bytes,
        &delta_bytes,
        &[
            &gamma_abc_0,
            &gamma_abc_1,
            &gamma_abc_2,
            &gamma_abc_3,
            &gamma_abc_4,
            &gamma_abc_5,
            &gamma_abc_6,
            &gamma_abc_7,
            &gamma_abc_8,
        ],
    )
}

/// Encode a u32 as a 32-byte big-endian field element.
fn u32_to_field(env: &Env, value: u32) -> BytesN<32> {
    let mut buf = [0u8; 32];
    buf[28..].copy_from_slice(&value.to_be_bytes());
    BytesN::from_array(env, &buf)
}

/// Encode a u64 as a 32-byte big-endian field element.
fn u64_to_field(env: &Env, value: u64) -> BytesN<32> {
    let mut buf = [0u8; 32];
    buf[24..].copy_from_slice(&value.to_be_bytes());
    BytesN::from_array(env, &buf)
}

/// Encode an Address as a 32-byte field element via SHA-256 of its XDR bytes.
fn address_to_field(env: &Env, addr: &Address) -> BytesN<32> {
    let xdr = addr.clone().to_xdr(env);
    env.crypto().sha256(&xdr).to_bytes()
}

/// Map structured public inputs to the ordered list of field elements
/// expected by the Groth16 verifier.
fn public_inputs_to_fields(env: &Env, inputs: &ScoreRangePublicInputs) -> Vec<BytesN<32>> {
    let mut fields = Vec::new(env);
    fields.push_back(u32_to_field(env, inputs.threshold));
    fields.push_back(address_to_field(env, &inputs.subject));
    fields.push_back(address_to_field(env, &inputs.credit_oracle_id));
    fields.push_back(inputs.score_commitment.clone());
    fields.push_back(u32_to_field(env, inputs.snapshot_ledger));
    fields.push_back(inputs.domain_separator.clone());
    fields.push_back(u64_to_field(env, inputs.current_timestamp));
    fields.push_back(u64_to_field(env, inputs.max_validity_window));
    fields
}

/// Compute the combined public-inputs G1 point:
/// `sum_i public_input_i * gamma_abc[i+1] + gamma_abc[0]`.
fn compute_public_inputs_combined(
    env: &Env,
    vk: &VerificationKey,
    public_inputs: &Vec<BytesN<32>>,
) -> G1Affine {
    let bls = env.crypto().bls12_381();
    let mut combined = G1Affine::from_bytes(vk.gamma_abc.get(0).unwrap());
    for (i, input) in public_inputs.iter().enumerate() {
        let index: u32 = (i + 1).try_into().unwrap();
        let ic_point = G1Affine::from_bytes(vk.gamma_abc.get(index).unwrap());
        let term = bls.g1_mul(&ic_point, &Fr::from_bytes(input.clone()));
        combined = bls.g1_add(&combined, &term);
    }
    combined
}

/// Map the tier public inputs to the ordered field elements expected by the
/// tier-band verifier (identical layout to [`public_inputs_to_fields`]).
fn tier_public_inputs_to_fields(env: &Env, inputs: &ScoreTierPublicInputs) -> Vec<BytesN<32>> {
    let mut fields = Vec::new(env);
    fields.push_back(u32_to_field(env, inputs.tier_level));
    fields.push_back(address_to_field(env, &inputs.subject));
    fields.push_back(address_to_field(env, &inputs.credit_oracle_id));
    fields.push_back(inputs.score_commitment.clone());
    fields.push_back(u32_to_field(env, inputs.snapshot_ledger));
    fields.push_back(inputs.domain_separator.clone());
    fields.push_back(u64_to_field(env, inputs.current_timestamp));
    fields.push_back(u64_to_field(env, inputs.max_validity_window));
    fields
}

/// Run the Groth16 pairing check for `proof` against `vk` and the ordered
/// public-input `fields`.
///
/// Note: soroban-sdk 22's BLS12-381 host functions strictly validate that
/// every point is on the curve and in the correct subgroup. A proof whose
/// points fail that validation raises a host error (the invocation fails)
/// rather than returning `false`; only well-formed points that do not satisfy
/// the pairing equation produce a `false` result.
fn groth16_verify_with_vk(
    env: &Env,
    proof: &Bytes,
    vk: &VerificationKey,
    fields: &Vec<BytesN<32>>,
) -> bool {
    if proof.len() != PROOF_SIZE {
        return false;
    }

    // Parse proof: A (G1, 96) || B (G2, 192) || C (G1, 96).
    let a: G1Affine = G1Affine::from_bytes(proof.slice(0..96).try_into().unwrap());
    let b: G2Affine = G2Affine::from_bytes(proof.slice(96..288).try_into().unwrap());
    let c: G1Affine = G1Affine::from_bytes(proof.slice(288..384).try_into().unwrap());

    let bls = env.crypto().bls12_381();

    // Compute combined public-inputs point.
    let combined = compute_public_inputs_combined(env, vk, fields);

    // Groth16 pairing check:
    //   e(-A, B) * e(alpha, beta) * e(combined, gamma) * e(C, delta) == 1
    let neg_a = -a.clone();
    let vp1 = vec![
        &env,
        neg_a,
        G1Affine::from_bytes(vk.alpha.clone()),
        combined,
        c,
    ];
    let vp2 = vec![
        &env,
        b,
        G2Affine::from_bytes(vk.beta.clone()),
        G2Affine::from_bytes(vk.gamma.clone()),
        G2Affine::from_bytes(vk.delta.clone()),
    ];
    bls.pairing_check(vp1, vp2)
}

/// Threshold-circuit verification against the embedded VK.
fn groth16_verify(env: &Env, proof: &Bytes, public_inputs: &ScoreRangePublicInputs) -> bool {
    if proof.len() != PROOF_SIZE {
        return false;
    }
    let vk = embedded_verification_key(env);
    let fields = public_inputs_to_fields(env, public_inputs);
    groth16_verify_with_vk(env, proof, &vk, &fields)
}

/// Tier-band-circuit verification against the embedded tier VK.
fn groth16_verify_tier(env: &Env, proof: &Bytes, public_inputs: &ScoreTierPublicInputs) -> bool {
    if proof.len() != PROOF_SIZE {
        return false;
    }
    let vk = embedded_tier_verification_key(env);
    let fields = tier_public_inputs_to_fields(env, public_inputs);
    groth16_verify_with_vk(env, proof, &vk, &fields)
}

/// `true` iff `tier_level` is one of the defined tiers (`1..=3`).
fn is_valid_tier_level(tier_level: u32) -> bool {
    (TIER_BRONZE..=TIER_GOLD).contains(&tier_level)
}

#[contract]
pub struct ScoreRangeVerifier;

#[contractimpl]
impl ScoreRangeVerifier {
    /// One-time setup: store admin, verification-key hash, and circuit version.
    pub fn initialize(
        env: Env,
        admin: Address,
        vk_hash: BytesN<32>,
        circuit_version: u32,
    ) -> Result<(), VerifierError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(VerifierError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::VkHash, &vk_hash);
        env.storage()
            .instance()
            .set(&DataKey::CircuitVersion, &circuit_version);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.events()
            .publish((symbol_short!("Init"),), (admin, vk_hash, circuit_version));
        Ok(())
    }

    /// Verify a Groth16 proof that the committed score exceeds `threshold`.
    ///
    /// Returns `true` iff the proof is valid for the supplied public inputs.
    /// Does not mutate state — lenders may simulate read-only.
    pub fn verify_score_range(
        env: Env,
        proof: Bytes,
        public_inputs: ScoreRangePublicInputs,
    ) -> bool {
        // Reject proofs of the wrong size early (DoS guard).
        if proof.len() != PROOF_SIZE {
            return false;
        }

        // Ensure the contract is initialized.
        if !env.storage().instance().has(&DataKey::Admin) {
            return false;
        }

        // Verify the circuit version matches the embedded constant.
        let stored_version: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CircuitVersion)
            .unwrap_or(0);
        if stored_version != CIRCUIT_VERSION {
            return false;
        }

        groth16_verify(&env, &proof, &public_inputs)
    }

    /// Stateful verify with replay protection.
    ///
    /// Requires `consumer` auth, verifies the proof, and records a
    /// replay-protection hash so the same proof + nonce cannot be reused.
    pub fn verify_and_consume(
        env: Env,
        consumer: Address,
        proof: Bytes,
        public_inputs: ScoreRangePublicInputs,
        nonce: BytesN<32>,
    ) -> Result<bool, VerifierError> {
        consumer.require_auth();

        // Ensure the contract is initialized.
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(VerifierError::NotInitialized);
        }

        // Reject proofs of the wrong size early.
        if proof.len() != PROOF_SIZE {
            return Err(VerifierError::InvalidProofSize);
        }

        // Compute replay-protection hash: SHA256(proof || public_inputs || nonce).
        let mut preimage = Bytes::new(&env);
        preimage.append(&proof);
        preimage.append(&Bytes::from_array(
            &env,
            &public_inputs.threshold.to_be_bytes(),
        ));
        preimage.append(&public_inputs.subject.clone().to_xdr(&env));
        preimage.append(&public_inputs.credit_oracle_id.clone().to_xdr(&env));
        preimage.append(&Bytes::from(public_inputs.score_commitment.clone()));
        preimage.append(&Bytes::from_array(
            &env,
            &public_inputs.snapshot_ledger.to_be_bytes(),
        ));
        preimage.append(&Bytes::from(public_inputs.domain_separator.clone()));
        preimage.append(&Bytes::from(nonce.clone()));
        let proof_hash = env.crypto().sha256(&preimage);

        // Reject if already consumed.
        let key = DataKey::ConsumedProof(proof_hash.to_bytes());
        if env.storage().persistent().has(&key) {
            return Err(VerifierError::ProofAlreadyConsumed);
        }

        // Verify the proof.
        let valid = groth16_verify(&env, &proof, &public_inputs);
        if !valid {
            return Ok(false);
        }

        // Record the proof as consumed.
        env.storage().persistent().set(&key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);

        Ok(true)
    }

    /// Verify a Groth16 proof that the committed score lies in a tier band
    /// (issue #1184): Bronze `[600, 700)`, Silver `[700, 800)`,
    /// Gold `[800, 850]`.
    ///
    /// Returns the verified tier (`1..=3`) on success and [`NO_TIER`] (`0`)
    /// otherwise. The verifier learns the tier and nothing finer about the
    /// score. Does not mutate state — lenders may simulate read-only.
    ///
    /// A prover whose score is in the Gold band can only produce a proof for
    /// `tier_level = 3`; claiming a lower tier fails in-circuit, so the
    /// returned tier is exactly the band the committed score belongs to.
    pub fn verify_score_tier(env: Env, proof: Bytes, public_inputs: ScoreTierPublicInputs) -> u32 {
        // Reject malformed requests early (DoS guard).
        if proof.len() != PROOF_SIZE || !is_valid_tier_level(public_inputs.tier_level) {
            return NO_TIER;
        }
        if !env.storage().instance().has(&DataKey::Admin) {
            return NO_TIER;
        }
        let stored_version: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CircuitVersion)
            .unwrap_or(0);
        if stored_version != CIRCUIT_VERSION {
            return NO_TIER;
        }

        if groth16_verify_tier(&env, &proof, &public_inputs) {
            public_inputs.tier_level
        } else {
            NO_TIER
        }
    }

    /// Stateful tier verification with replay protection.
    ///
    /// Requires `consumer` auth, verifies the tier proof, and records a
    /// replay-protection hash so the same proof + nonce cannot be reused.
    /// Returns the verified tier (`1..=3`), or [`NO_TIER`] if the proof is
    /// invalid.
    pub fn verify_tier_and_consume(
        env: Env,
        consumer: Address,
        proof: Bytes,
        public_inputs: ScoreTierPublicInputs,
        nonce: BytesN<32>,
    ) -> Result<u32, VerifierError> {
        consumer.require_auth();

        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(VerifierError::NotInitialized);
        }
        if proof.len() != PROOF_SIZE {
            return Err(VerifierError::InvalidProofSize);
        }
        if !is_valid_tier_level(public_inputs.tier_level) {
            return Err(VerifierError::InvalidTierLevel);
        }

        // Replay hash: SHA256("tier" || proof || public_inputs || nonce). The
        // prefix keeps tier and threshold replay keys in disjoint spaces.
        let mut preimage = Bytes::from_slice(&env, b"tier");
        preimage.append(&proof);
        preimage.append(&Bytes::from_array(
            &env,
            &public_inputs.tier_level.to_be_bytes(),
        ));
        preimage.append(&public_inputs.subject.clone().to_xdr(&env));
        preimage.append(&public_inputs.credit_oracle_id.clone().to_xdr(&env));
        preimage.append(&Bytes::from(public_inputs.score_commitment.clone()));
        preimage.append(&Bytes::from_array(
            &env,
            &public_inputs.snapshot_ledger.to_be_bytes(),
        ));
        preimage.append(&Bytes::from(public_inputs.domain_separator.clone()));
        preimage.append(&Bytes::from(nonce.clone()));
        let proof_hash = env.crypto().sha256(&preimage);

        let key = DataKey::ConsumedProof(proof_hash.to_bytes());
        if env.storage().persistent().has(&key) {
            return Err(VerifierError::ProofAlreadyConsumed);
        }

        if !groth16_verify_tier(&env, &proof, &public_inputs) {
            return Ok(NO_TIER);
        }

        env.storage().persistent().set(&key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);

        Ok(public_inputs.tier_level)
    }

    /// The inclusive lower bounds of the tiers, indexed by `tier_level - 1`
    /// (`[600, 700, 800]`). The top tier ends at the maximum score, 850.
    pub fn get_tier_thresholds(env: Env) -> Vec<u32> {
        let mut out = Vec::new(&env);
        for t in TIER_THRESHOLDS {
            out.push_back(t);
        }
        out
    }

    /// Read the stored verification-key hash.
    pub fn get_vk_hash(env: Env) -> Option<BytesN<32>> {
        env.storage().instance().get(&DataKey::VkHash)
    }

    /// Read the stored circuit version.
    pub fn get_circuit_version(env: Env) -> Option<u32> {
        env.storage().instance().get(&DataKey::CircuitVersion)
    }

    /// Read the stored admin.
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Admin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_bls12_381::Fr as ArkFr;
    use ark_ff::PrimeField;
    use ark_serialize::CanonicalSerialize;
    use ark_std::rand::{rngs::StdRng, SeedableRng};
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Bytes, Env};
    use zk_circuit::{
        commitment::commitment_to_bytes_be, generate_proof, generate_test_keys,
        generate_tier_proof, generate_tier_test_keys, ScoreInputs, ScorePublicInputs, ScoreWitness,
        TierPublicInputs,
    };

    #[test]
    fn test_circuit_proof_with_development_key() {
        let (env, _, contract_id) = setup();
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let witness = ScoreWitness {
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
            blinding: ArkFr::from(42u32),
        };
        let commitment = witness.commitment();
        let commitment_bytes = commitment_to_bytes_be(commitment);
        let inputs = ScoreRangePublicInputs {
            threshold: 600,
            subject: Address::generate(&env),
            credit_oracle_id: Address::generate(&env),
            score_commitment: BytesN::from_array(&env, &commitment_bytes),
            snapshot_ledger: 12345,
            domain_separator: BytesN::from_array(&env, &[0u8; 32]),
            current_timestamp: 1_700_000_100,
            max_validity_window: 3600,
        };
        let public = ScorePublicInputs {
            threshold: inputs.threshold,
            subject: ArkFr::from_be_bytes_mod_order(
                &address_to_field(&env, &inputs.subject).to_array(),
            ),
            credit_oracle_id: ArkFr::from_be_bytes_mod_order(
                &address_to_field(&env, &inputs.credit_oracle_id).to_array(),
            ),
            score_commitment: commitment,
            snapshot_ledger: inputs.snapshot_ledger,
            domain_separator: ArkFr::from(0u32),
            current_timestamp: inputs.current_timestamp,
            max_validity_window: inputs.max_validity_window,
        };
        let mut rng = StdRng::from_seed([0u8; 32]);
        let (pk, vk) = generate_test_keys(&mut rng).unwrap();
        let embedded = embedded_verification_key(&env);
        let mut serialized = ark_std::vec::Vec::new();
        vk.alpha_g1.serialize_uncompressed(&mut serialized).unwrap();
        assert_eq!(serialized.as_slice(), &embedded.alpha.to_array());
        serialized.clear();
        vk.beta_g2.serialize_uncompressed(&mut serialized).unwrap();
        assert_eq!(serialized.as_slice(), &embedded.beta.to_array());
        serialized.clear();
        vk.gamma_g2.serialize_uncompressed(&mut serialized).unwrap();
        assert_eq!(serialized.as_slice(), &embedded.gamma.to_array());
        serialized.clear();
        vk.delta_g2.serialize_uncompressed(&mut serialized).unwrap();
        assert_eq!(serialized.as_slice(), &embedded.delta.to_array());
        assert_eq!(vk.gamma_abc_g1.len(), embedded.gamma_abc.len() as usize);
        for (index, point) in vk.gamma_abc_g1.iter().enumerate() {
            serialized.clear();
            point.serialize_uncompressed(&mut serialized).unwrap();
            assert_eq!(
                serialized.as_slice(),
                &embedded.gamma_abc.get(index as u32).unwrap().to_array()
            );
        }
        let proof = generate_proof(witness, public, &pk, &mut rng).unwrap();
        let mut bytes = ark_std::vec::Vec::new();
        proof.a.serialize_uncompressed(&mut bytes).unwrap();
        proof.b.serialize_uncompressed(&mut bytes).unwrap();
        proof.c.serialize_uncompressed(&mut bytes).unwrap();
        let proof = Bytes::from_slice(&env, &bytes);
        assert!(client.verify_score_range(&proof, &inputs));
        let mut wrong = inputs.clone();
        wrong.threshold = 601;
        assert!(!client.verify_score_range(&proof, &wrong));
    }

    /// End-to-end: a real tier-band Groth16 proof verified by Soroban's BLS12-381
    /// host functions against the embedded tier VK (issue #1184).
    #[test]
    fn test_tier_proof_with_development_key() {
        let (env, _, contract_id) = setup();
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        // vc_points 50, volume/repayment as in the threshold test -> score 668.
        let witness = ScoreWitness::from_inputs(
            &ScoreInputs {
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
                computed_at_ledger: 1234567,
                stale: false,
                score_timestamp: 1_700_000_000,
            },
            ArkFr::from(42u32),
        )
        .unwrap();
        assert_eq!(witness.score, 668);
        let commitment = witness.commitment();
        let commitment_bytes = commitment_to_bytes_be(commitment);

        let inputs = ScoreTierPublicInputs {
            tier_level: TIER_BRONZE,
            subject: Address::generate(&env),
            credit_oracle_id: Address::generate(&env),
            score_commitment: BytesN::from_array(&env, &commitment_bytes),
            snapshot_ledger: 12345,
            domain_separator: BytesN::from_array(&env, &[0u8; 32]),
            current_timestamp: 1_700_000_100,
            max_validity_window: 3600,
        };
        let public = TierPublicInputs {
            tier_level: inputs.tier_level,
            subject: ArkFr::from_be_bytes_mod_order(
                &address_to_field(&env, &inputs.subject).to_array(),
            ),
            credit_oracle_id: ArkFr::from_be_bytes_mod_order(
                &address_to_field(&env, &inputs.credit_oracle_id).to_array(),
            ),
            score_commitment: commitment,
            snapshot_ledger: inputs.snapshot_ledger,
            domain_separator: ArkFr::from(0u32),
            current_timestamp: inputs.current_timestamp,
            max_validity_window: inputs.max_validity_window,
        };

        let mut rng = StdRng::from_seed([0u8; 32]);
        let (pk, vk) = generate_tier_test_keys(&mut rng).unwrap();
        // The embedded tier VK is exactly the development setup output.
        let embedded = embedded_tier_verification_key(&env);
        let mut ser = ark_std::vec::Vec::new();
        vk.alpha_g1.serialize_uncompressed(&mut ser).unwrap();
        assert_eq!(ser.as_slice(), &embedded.alpha.to_array());
        ser.clear();
        vk.beta_g2.serialize_uncompressed(&mut ser).unwrap();
        assert_eq!(ser.as_slice(), &embedded.beta.to_array());
        ser.clear();
        vk.gamma_g2.serialize_uncompressed(&mut ser).unwrap();
        assert_eq!(ser.as_slice(), &embedded.gamma.to_array());
        ser.clear();
        vk.delta_g2.serialize_uncompressed(&mut ser).unwrap();
        assert_eq!(ser.as_slice(), &embedded.delta.to_array());
        assert_eq!(vk.gamma_abc_g1.len(), embedded.gamma_abc.len() as usize);
        for (i, point) in vk.gamma_abc_g1.iter().enumerate() {
            ser.clear();
            point.serialize_uncompressed(&mut ser).unwrap();
            assert_eq!(
                ser.as_slice(),
                &embedded.gamma_abc.get(i as u32).unwrap().to_array()
            );
        }

        let proof = generate_tier_proof(witness, public, &pk, &mut rng).unwrap();
        let mut bytes = ark_std::vec::Vec::new();
        proof.a.serialize_uncompressed(&mut bytes).unwrap();
        proof.b.serialize_uncompressed(&mut bytes).unwrap();
        proof.c.serialize_uncompressed(&mut bytes).unwrap();
        let proof = Bytes::from_slice(&env, &bytes);

        // The verifier returns the verified tier...
        assert_eq!(client.verify_score_tier(&proof, &inputs), TIER_BRONZE);
        // ...and no other tier, for the same proof.
        for wrong in [TIER_SILVER, TIER_GOLD] {
            let mut other = inputs.clone();
            other.tier_level = wrong;
            assert_eq!(client.verify_score_tier(&proof, &other), NO_TIER);
        }
        // Bound to the freshness inputs and the commitment.
        let mut stale = inputs.clone();
        stale.current_timestamp += 1;
        assert_eq!(client.verify_score_tier(&proof, &stale), NO_TIER);
        let mut forged = inputs.clone();
        forged.score_commitment = BytesN::from_array(&env, &[0x99; 32]);
        assert_eq!(client.verify_score_tier(&proof, &forged), NO_TIER);

        // The stateful variant returns the tier once, then rejects the replay.
        let consumer = Address::generate(&env);
        let nonce = BytesN::from_array(&env, &[0x55; 32]);
        assert_eq!(
            client.verify_tier_and_consume(&consumer, &proof, &inputs, &nonce),
            TIER_BRONZE
        );
        assert_eq!(
            client.try_verify_tier_and_consume(&consumer, &proof, &inputs, &nonce),
            Err(Ok(VerifierError::ProofAlreadyConsumed))
        );
    }

    /// A threshold proof must not verify as a tier proof (different circuits
    /// and VKs), even with identically-shaped public inputs.
    #[test]
    fn test_threshold_proof_is_not_accepted_as_tier_proof() {
        let (env, _, contract_id) = setup();
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let witness = ScoreWitness::from_inputs(
            &ScoreInputs {
                vc_points: 50,
                vc_weight: 100,
                vc_count: 10,
                last_updated: 1_700_000_000,
                score_timestamp: 1_700_000_000,
                ..Default::default()
            },
            ArkFr::from(42u32),
        )
        .unwrap();
        let commitment = witness.commitment();
        let subject = Address::generate(&env);
        let oracle = Address::generate(&env);
        let public = ScorePublicInputs {
            threshold: 1,
            subject: ArkFr::from_be_bytes_mod_order(&address_to_field(&env, &subject).to_array()),
            credit_oracle_id: ArkFr::from_be_bytes_mod_order(
                &address_to_field(&env, &oracle).to_array(),
            ),
            score_commitment: commitment,
            snapshot_ledger: 1,
            domain_separator: ArkFr::from(0u32),
            current_timestamp: 1_700_000_100,
            max_validity_window: 3600,
        };
        let mut rng = StdRng::from_seed([0u8; 32]);
        let (pk, _) = generate_test_keys(&mut rng).unwrap();
        let proof = generate_proof(witness, public, &pk, &mut rng).unwrap();
        let mut bytes = ark_std::vec::Vec::new();
        proof.a.serialize_uncompressed(&mut bytes).unwrap();
        proof.b.serialize_uncompressed(&mut bytes).unwrap();
        proof.c.serialize_uncompressed(&mut bytes).unwrap();
        let proof = Bytes::from_slice(&env, &bytes);

        let tier_inputs = ScoreTierPublicInputs {
            tier_level: TIER_BRONZE,
            subject,
            credit_oracle_id: oracle,
            score_commitment: BytesN::from_array(&env, &commitment_to_bytes_be(commitment)),
            snapshot_ledger: 1,
            domain_separator: BytesN::from_array(&env, &[0u8; 32]),
            current_timestamp: 1_700_000_100,
            max_validity_window: 3600,
        };
        assert_eq!(client.verify_score_tier(&proof, &tier_inputs), NO_TIER);
    }

    fn setup() -> (Env, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);
        (env, admin, contract_id)
    }

    // The BLS12-381 point at infinity in G1, uncompressed: the infinity flag
    // (bit 1 of the first byte) is set and every other byte is zero.
    fn infinity_g1_bytes() -> [u8; 96] {
        let mut bytes = [0u8; 96];
        bytes[0] = 0x40;
        bytes
    }

    // The BLS12-381 point at infinity in G2, uncompressed.
    fn infinity_g2_bytes() -> [u8; 192] {
        let mut bytes = [0u8; 192];
        bytes[0] = 0x40;
        bytes
    }

    fn make_proof(env: &Env) -> Bytes {
        // 384-byte proof: A (96) || B (192) || C (96).
        // Filled with the point-at-infinity encodings (0x40 first byte, zeros
        // after). These do NOT pass the soroban-sdk 22 host's strict on-curve
        // validation, so a verification of this proof raises a host
        // `Error(Crypto, InvalidInput)` rather than returning `false`. It
        // mirrors a maliciously-crafted proof and is used to exercise the
        // host-validation path.
        let mut proof = Bytes::from_array(env, &infinity_g1_bytes());
        proof.append(&Bytes::from_array(env, &infinity_g2_bytes()));
        proof.append(&Bytes::from_array(env, &infinity_g1_bytes()));
        proof
    }

    fn make_public_inputs(env: &Env) -> ScoreRangePublicInputs {
        ScoreRangePublicInputs {
            threshold: 700,
            subject: Address::generate(env),
            credit_oracle_id: Address::generate(env),
            score_commitment: BytesN::from_array(env, &[0x11; 32]),
            snapshot_ledger: 12345,
            domain_separator: BytesN::from_array(env, &[0x22; 32]),
            current_timestamp: 1_700_000_100,
            max_validity_window: 3600,
        }
    }

    #[test]
    fn test_initialize_stores_config() {
        let (env, admin, _) = setup();
        let client = ScoreRangeVerifierClient::new(&env, &env.register(ScoreRangeVerifier, ()));
        // Re-register to get a fresh client bound to the same contract.
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        assert_eq!(client.get_vk_hash(), Some(vk_hash));
        assert_eq!(client.get_circuit_version(), Some(CIRCUIT_VERSION));
        assert_eq!(client.get_admin(), Some(admin));
    }

    #[test]
    fn test_initialize_rejects_double_init() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        let res = client.try_initialize(&admin, &vk_hash, &CIRCUIT_VERSION);
        assert_eq!(res, Err(Ok(VerifierError::AlreadyInitialized)));
    }

    #[test]
    fn test_verify_score_range_rejects_wrong_size_proof() {
        let (env, _, _) = setup();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        let inputs = make_public_inputs(&env);
        let short_proof = Bytes::from_array(&env, &[0x01u8; 100]);
        assert!(!client.verify_score_range(&short_proof, &inputs));
    }

    #[test]
    fn test_verify_score_range_tampered_proof_raises_host_error() {
        let (env, _, _) = setup();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        let inputs = make_public_inputs(&env);
        let proof = make_proof(&env);
        // The proof's points are point-at-infinity, which are valid on-curve points
        // but do not satisfy the pairing check, so verification returns `false`.
        let res = client.try_verify_score_range(&proof, &inputs);
        assert_eq!(res, Ok(Ok(false)));
    }

    #[test]
    fn test_verify_score_range_rejects_malformed_points() {
        let (env, _, _) = setup();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        let inputs = make_public_inputs(&env);
        // Completely random bytes are almost never valid curve points. The
        // soroban-sdk 22 host strictly validates all points (on-curve and in
        // subgroup), so the invocation fails instead of returning `false`.
        let proof = Bytes::from_array(&env, &[0x42u8; 384]);
        let res = client.try_verify_score_range(&proof, &inputs);
        assert!(res.is_err());
    }

    #[test]
    fn test_verify_and_consume_tampered_proof_raises_host_error() {
        let (env, _, _) = setup();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        let consumer = Address::generate(&env);
        let inputs = make_public_inputs(&env);
        let proof = make_proof(&env);
        let nonce = BytesN::from_array(&env, &[0x33; 32]);

        // The tampered proof fails the pairing check, returning `false`.
        // Because the failed verification is never recorded as consumed, a second
        // identical call fails the same way (not `ProofAlreadyConsumed`).
        let res = client.try_verify_and_consume(&consumer, &proof, &inputs, &nonce);
        assert_eq!(res, Ok(Ok(false)));

        let res2 = client.try_verify_and_consume(&consumer, &proof, &inputs, &nonce);
        assert_eq!(res2, Ok(Ok(false)));
    }

    #[test]
    fn test_verify_and_consume_requires_auth() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        let consumer = Address::generate(&env);
        let inputs = make_public_inputs(&env);
        let proof = make_proof(&env);
        let nonce = BytesN::from_array(&env, &[0x44; 32]);

        // Withdraw the blanket auth mock: with an empty auth list, the
        // consumer's require_auth() inside verify_and_consume() has nothing
        // authorizing the invocation, so it fails before verification runs.
        env.mock_auths(&[]);
        let res = client.try_verify_and_consume(&consumer, &proof, &inputs, &nonce);
        assert!(res.is_err());
    }

    #[test]
    fn test_verify_and_consume_rejects_wrong_size_proof() {
        let (env, _, _) = setup();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        let consumer = Address::generate(&env);
        let inputs = make_public_inputs(&env);
        let short_proof = Bytes::from_array(&env, &[0x01u8; 100]);
        let nonce = BytesN::from_array(&env, &[0x55; 32]);

        let res = client.try_verify_and_consume(&consumer, &short_proof, &inputs, &nonce);
        assert_eq!(res, Err(Ok(VerifierError::InvalidProofSize)));
    }

    #[test]
    fn test_consumed_proof_replay_protection_survives_ledger_advance() {
        use soroban_sdk::testutils::Ledger as _;

        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        assert_eq!(PERS_TTL_THRESHOLD, 120_960);
        assert_eq!(PERS_TTL_EXTEND, 518_400);

        // Write a ConsumedProof entry via the test-only helper (bypasses
        // Groth16 verification, which cannot be run in a unit test without
        // the trusted-setup artifacts).
        let proof_hash = BytesN::from_array(&env, &[0x77; 32]);
        env.as_contract(&contract_id, || {
            let key = DataKey::ConsumedProof(proof_hash.clone());
            env.storage().persistent().set(&key, &true);
            env.storage()
                .persistent()
                .extend_ttl(&key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        });

        // Advance the ledger in chunks smaller than INSTANCE_BUMP_AMOUNT,
        // re-extending the instance's TTL before each step. A single jump
        // past PERS_TTL_EXTEND would archive the instance itself.
        let chunk: u32 = INSTANCE_BUMP_AMOUNT - 1_000;
        let mut current: u32 = env.ledger().sequence();
        let target: u32 = current + PERS_TTL_EXTEND - 1_000;

        while current < target {
            let next = core::cmp::min(current + chunk, target);
            env.as_contract(&contract_id, || {
                env.storage()
                    .instance()
                    .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
            });
            env.ledger().set_sequence_number(next);
            current = next;
        }

        // The ConsumedProof entry must still exist after advancing past
        // PERS_TTL_EXTEND â€” this is the actual replay-protection guarantee.
        env.as_contract(&contract_id, || {
            assert!(env
                .storage()
                .persistent()
                .has(&DataKey::ConsumedProof(proof_hash.clone())));
        });
    }
    #[test]
    fn test_verify_score_range_requires_initialization() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);

        let inputs = make_public_inputs(&env);
        let proof = make_proof(&env);
        // Uninitialized contract should return false.
        assert!(!client.verify_score_range(&proof, &inputs));
    }

    #[test]
    fn test_public_inputs_encoding_is_deterministic() {
        let env = Env::default();
        let inputs = make_public_inputs(&env);
        let fields1 = public_inputs_to_fields(&env, &inputs);
        let fields2 = public_inputs_to_fields(&env, &inputs);
        assert_eq!(fields1.len(), fields2.len());
        assert_eq!(fields1.len(), NUM_PUBLIC_INPUTS);
        for i in 0..fields1.len() {
            assert_eq!(fields1.get(i).unwrap(), fields2.get(i).unwrap());
        }
    }

    // ---- tier-band verification (issue #1184) -------------------------------

    fn make_tier_inputs(env: &Env, tier_level: u32) -> ScoreTierPublicInputs {
        ScoreTierPublicInputs {
            tier_level,
            subject: Address::generate(env),
            credit_oracle_id: Address::generate(env),
            score_commitment: BytesN::from_array(env, &[0x11; 32]),
            snapshot_ledger: 12345,
            domain_separator: BytesN::from_array(env, &[0x22; 32]),
            current_timestamp: 1_700_000_100,
            max_validity_window: 3600,
        }
    }

    fn init_client(env: &Env) -> ScoreRangeVerifierClient<'_> {
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(env, &contract_id);
        let admin = Address::generate(env);
        client.initialize(
            &admin,
            &BytesN::from_array(env, &[0xAB; 32]),
            &CIRCUIT_VERSION,
        );
        client
    }

    #[test]
    fn test_tier_thresholds_are_600_700_800() {
        let env = Env::default();
        env.mock_all_auths();
        let client = init_client(&env);
        let t = client.get_tier_thresholds();
        assert_eq!(t.len(), 3);
        assert_eq!(t.get(0), Some(600));
        assert_eq!(t.get(1), Some(700));
        assert_eq!(t.get(2), Some(800));
    }

    #[test]
    fn test_tier_vk_is_distinct_from_threshold_vk() {
        let env = Env::default();
        let thr = embedded_verification_key(&env);
        let tier = embedded_tier_verification_key(&env);
        // Different circuits must never share a verification key. Both dev VKs
        // use the same fixed setup seed, so alpha/beta/gamma/delta coincide and
        // the circuits are told apart by the public-input commitments
        // (`gamma_abc`).
        assert!(thr.gamma_abc != tier.gamma_abc);
        assert_eq!(thr.gamma_abc.len(), NUM_PUBLIC_INPUTS + 1);
        assert_eq!(tier.gamma_abc.len(), NUM_PUBLIC_INPUTS + 1);
    }

    #[test]
    fn test_tier_public_inputs_encoding_matches_threshold_layout() {
        let env = Env::default();
        let inputs = make_tier_inputs(&env, 2);
        let fields = tier_public_inputs_to_fields(&env, &inputs);
        assert_eq!(fields.len(), NUM_PUBLIC_INPUTS);
        // Slot 0 carries the tier level in the same encoding as a threshold.
        assert_eq!(fields.get(0).unwrap(), u32_to_field(&env, 2));
        assert_eq!(fields.get(3).unwrap(), inputs.score_commitment);
    }

    #[test]
    fn test_verify_score_tier_rejects_invalid_tier_levels() {
        let env = Env::default();
        env.mock_all_auths();
        let client = init_client(&env);
        let proof = make_proof(&env);
        // Out-of-range levels are rejected before any pairing work, so even a
        // well-sized proof returns NO_TIER rather than raising a host error.
        for bad in [0u32, 4, 5, 700, u32::MAX] {
            let inputs = make_tier_inputs(&env, bad);
            assert_eq!(
                client.verify_score_tier(&proof, &inputs),
                NO_TIER,
                "tier {bad}"
            );
        }
    }

    #[test]
    fn test_verify_score_tier_rejects_wrong_size_proof() {
        let env = Env::default();
        env.mock_all_auths();
        let client = init_client(&env);
        let short = Bytes::from_array(&env, &[0x01u8; 100]);
        for tier in [TIER_BRONZE, TIER_SILVER, TIER_GOLD] {
            let inputs = make_tier_inputs(&env, tier);
            assert_eq!(client.verify_score_tier(&short, &inputs), NO_TIER);
        }
    }

    #[test]
    fn test_verify_score_tier_requires_initialization() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let inputs = make_tier_inputs(&env, TIER_GOLD);
        assert_eq!(
            client.verify_score_tier(&make_proof(&env), &inputs),
            NO_TIER
        );
    }

    #[test]
    fn test_verify_score_tier_rejects_circuit_version_mismatch() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(
            &admin,
            &BytesN::from_array(&env, &[0xAB; 32]),
            &(CIRCUIT_VERSION + 1),
        );
        let inputs = make_tier_inputs(&env, TIER_SILVER);
        assert_eq!(
            client.verify_score_tier(&make_proof(&env), &inputs),
            NO_TIER
        );
    }

    #[test]
    fn test_verify_score_tier_malformed_points_do_not_verify() {
        let env = Env::default();
        env.mock_all_auths();
        let client = init_client(&env);
        let inputs = make_tier_inputs(&env, TIER_BRONZE);
        // Same handling as the threshold verifier: strictly-validated host
        // points either error out or fail the pairing; never a verified tier.
        let res = client.try_verify_score_tier(&make_proof(&env), &inputs);
        assert!(matches!(res, Err(_) | Ok(Ok(NO_TIER))));
        let garbage = Bytes::from_array(&env, &[0x42u8; 384]);
        let res = client.try_verify_score_tier(&garbage, &inputs);
        assert!(matches!(res, Err(_) | Ok(Ok(NO_TIER))));
    }

    #[test]
    fn test_verify_tier_and_consume_rejects_bad_requests() {
        let env = Env::default();
        env.mock_all_auths();
        let client = init_client(&env);
        let consumer = Address::generate(&env);
        let nonce = BytesN::from_array(&env, &[0x66; 32]);

        let short = Bytes::from_array(&env, &[0x01u8; 100]);
        let res = client.try_verify_tier_and_consume(
            &consumer,
            &short,
            &make_tier_inputs(&env, TIER_GOLD),
            &nonce,
        );
        assert_eq!(res, Err(Ok(VerifierError::InvalidProofSize)));

        for bad in [0u32, 4, u32::MAX] {
            let res = client.try_verify_tier_and_consume(
                &consumer,
                &make_proof(&env),
                &make_tier_inputs(&env, bad),
                &nonce,
            );
            assert_eq!(res, Err(Ok(VerifierError::InvalidTierLevel)), "tier {bad}");
        }
    }

    #[test]
    fn test_verify_tier_and_consume_requires_initialization_and_auth() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let consumer = Address::generate(&env);
        let nonce = BytesN::from_array(&env, &[0x77; 32]);
        let inputs = make_tier_inputs(&env, TIER_SILVER);
        let res = client.try_verify_tier_and_consume(&consumer, &make_proof(&env), &inputs, &nonce);
        assert_eq!(res, Err(Ok(VerifierError::NotInitialized)));

        let client = init_client(&env);
        env.mock_auths(&[]);
        let res = client.try_verify_tier_and_consume(&consumer, &make_proof(&env), &inputs, &nonce);
        assert!(res.is_err());
    }
}
