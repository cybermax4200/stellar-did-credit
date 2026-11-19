#![no_std]
//! On-chain Groth16 verifier for the Stellar DID Credit protocol.
//!
//! Verifies zero-knowledge proofs that a credit score falls within a
//! committed range without revealing the exact score. Uses Stellar's
//! BLS12-381 pairing host functions (CAP-0059) via `env.crypto().bls12_381()`
//! (soroban-sdk 22 API).
#[cfg(test)]
extern crate std;

use ark_serialize::CanonicalDeserialize;
use soroban_sdk::crypto::bls12_381::{Fr, G1Affine, G2Affine};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, vec, Address, Bytes, BytesN,
    Env, Symbol, Vec,
};

/// Fixed-size Groth16 proof encoding (BLS12-381), uncompressed points:
/// A (G1, 96 bytes) || B (G2, 192 bytes) || C (G1, 96 bytes) = 384 bytes.
pub const PROOF_SIZE: u32 = 384;

/// Number of public inputs for the score-range circuit.
pub const NUM_PUBLIC_INPUTS: u32 = 8;

/// Circuit version constant â€” must match the `domain_separator` in the
/// circuit artifact. Bumped on any breaking circuit change.
pub const CIRCUIT_VERSION: u32 = 1;

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
    /// Pedersen commitment to the ScoreRecord.
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
    /// Malformed, off-curve, non-subgroup, or infinite proof points.
    InvalidProof = 7,
}

const INSTANCE_BUMP_THRESHOLD: u32 = 5000;
const INSTANCE_BUMP_AMOUNT: u32 = 500_000;

const PERS_TTL_THRESHOLD: u32 = 120_960; // ~7 days
const PERS_TTL_EXTEND: u32 = 518_400; // ~30 days

/// Embedded verification key for the score-range circuit.
///
/// These bytes match the development-only fixed-seed output in
/// `zk/circuit/vk_out.txt`; they are not a production ceremony key. The seed
/// in `zk/circuit/src/bin/generate_vk.rs` is public, so this key must not be
/// trusted for production proof verification. A ceremony key and a matching
/// proof fixture are needed before this verifier can be deployed securely.
///
/// The current uncompressed key encoding is tested against Soroban's BLS host
/// functions. Revalidate encoding and point membership when replacing the key.
/// The supplied `vk_hash` is currently stored without being checked against it.
fn embedded_verification_key(env: &Env) -> VerificationKey {
    // Development-only output; see the warning above.
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
        0x01, 0xE9, 0xB2, 0xE5, 0x9F, 0xD1, 0x38, 0xA4, 0x9E, 0xFB, 0x3F, 0xBB, 0x49, 0x3B, 0x28,
        0x7C, 0x04, 0x4E, 0x2A, 0xC0, 0xA1, 0xFC, 0x0B, 0x8E, 0x5A, 0x72, 0x6A, 0xB5, 0x44, 0x55,
        0x04, 0x71, 0xEE, 0xC9, 0x81, 0x52, 0x89, 0xE7, 0x7C, 0x39, 0x86, 0xA1, 0x3D, 0x6A, 0xC5,
        0xE5, 0x80, 0xC8, 0x11, 0x6F, 0x29, 0xFF, 0x6E, 0x55, 0x44, 0xA1, 0x1A, 0xBE, 0x93, 0xE0,
        0xD6, 0x9F, 0x56, 0x8C, 0xF5, 0x28, 0xA1, 0xF5, 0xD8, 0xDB, 0x93, 0x78, 0xD2, 0x00, 0x9F,
        0x91, 0x38, 0x9C, 0x7B, 0x91, 0x99, 0x63, 0xDD, 0xE9, 0x18, 0xA2, 0x13, 0xE6, 0xC4, 0x5F,
        0xF1, 0x09, 0x11, 0x71, 0x4B, 0xEC,
    ];

    let gamma_abc_1 = [
        0x03, 0x15, 0x69, 0x72, 0xAA, 0x68, 0x00, 0x1C, 0xD4, 0x63, 0xBE, 0x4F, 0xD9, 0x0F, 0x12,
        0xD3, 0x9B, 0x21, 0x67, 0x85, 0x49, 0x89, 0xA3, 0x8F, 0xB0, 0x36, 0x77, 0x07, 0x48, 0x16,
        0x07, 0xA9, 0x6C, 0x13, 0x2D, 0x89, 0xDE, 0xAD, 0x81, 0x64, 0x22, 0x0C, 0x3E, 0x7B, 0x39,
        0x3F, 0xDE, 0x37, 0x06, 0xEF, 0x02, 0xF3, 0x0C, 0x69, 0x21, 0x97, 0x37, 0x43, 0xE6, 0xAC,
        0xA5, 0xA4, 0xF6, 0x9F, 0x70, 0xCE, 0x3D, 0x87, 0x28, 0x66, 0x81, 0x95, 0xD9, 0x29, 0x5A,
        0xCB, 0x18, 0xA2, 0x39, 0x48, 0x00, 0x81, 0x86, 0xFA, 0x21, 0xA8, 0xA2, 0x7D, 0xDB, 0x72,
        0x3B, 0x1A, 0x26, 0x0B, 0xA1, 0x77,
    ];

    let gamma_abc_2 = [
        0x0E, 0x80, 0x45, 0xB4, 0xD2, 0x1D, 0x81, 0x28, 0x49, 0xBB, 0x9F, 0x06, 0x7C, 0xD9, 0x19,
        0x22, 0xB0, 0x61, 0x85, 0x51, 0x9E, 0x38, 0x94, 0x84, 0x9A, 0x99, 0xFD, 0x13, 0x27, 0xBD,
        0x79, 0x65, 0x1E, 0xA1, 0x4D, 0x3B, 0xCA, 0x6C, 0x12, 0x97, 0xEB, 0x5C, 0x83, 0x01, 0x76,
        0x28, 0xAB, 0x5E, 0x17, 0x84, 0x14, 0x97, 0xE5, 0xE0, 0x5E, 0x89, 0x7B, 0x31, 0x15, 0x2F,
        0x7E, 0x35, 0x1C, 0xCE, 0x65, 0xD5, 0xDD, 0xA2, 0xA6, 0xE9, 0xE1, 0xD9, 0xD6, 0x0C, 0xBD,
        0x82, 0x90, 0x8A, 0x03, 0xA2, 0x6F, 0x7D, 0xA3, 0x51, 0xCA, 0x06, 0x78, 0xF9, 0x21, 0x3A,
        0xD5, 0x13, 0x9E, 0x82, 0x5B, 0x26,
    ];

    let gamma_abc_3 = [
        0x00, 0x0F, 0x71, 0xCC, 0xE1, 0x4A, 0xF4, 0xB6, 0xDC, 0x57, 0x9E, 0xCA, 0xD3, 0x36, 0xEF,
        0xC8, 0x1A, 0xC3, 0x19, 0x0A, 0xEE, 0x64, 0x1C, 0x62, 0xE8, 0xB4, 0xEE, 0x88, 0x5D, 0xA7,
        0x29, 0x61, 0x7E, 0x1C, 0x25, 0xC5, 0xD6, 0xB8, 0x0F, 0x94, 0xFF, 0x7C, 0x3B, 0x8F, 0x76,
        0xA2, 0x53, 0xFE, 0x11, 0x1E, 0x38, 0x17, 0x99, 0xE2, 0x4C, 0x1E, 0x07, 0x18, 0x37, 0xE9,
        0x8C, 0x85, 0x34, 0x57, 0x22, 0x0C, 0x9C, 0x34, 0xB2, 0x11, 0x01, 0xAF, 0x17, 0x41, 0xE6,
        0x59, 0x7A, 0xBB, 0x07, 0xE0, 0xF9, 0xD5, 0xC1, 0xBA, 0x9C, 0x5D, 0x3D, 0xE7, 0x45, 0xC4,
        0xA9, 0xB8, 0x0D, 0x03, 0x28, 0x17,
    ];

    let gamma_abc_4 = [
        0x14, 0x7C, 0x87, 0x63, 0x50, 0xBA, 0x98, 0x86, 0x73, 0xF6, 0x4F, 0x2C, 0x55, 0x27, 0x5F,
        0x59, 0x06, 0xAF, 0xBE, 0xB9, 0xE3, 0x01, 0x25, 0x76, 0x22, 0xA7, 0x25, 0x3E, 0xF3, 0x54,
        0x9D, 0x37, 0x09, 0xA7, 0x27, 0x49, 0x54, 0xDF, 0x05, 0x78, 0xB6, 0x6B, 0xFB, 0xB9, 0x1C,
        0xBC, 0x45, 0x6B, 0x0F, 0x5D, 0x62, 0x00, 0xF0, 0xD9, 0x6A, 0xB9, 0x88, 0xAE, 0x47, 0x52,
        0x0D, 0x24, 0x32, 0x61, 0x9D, 0xA4, 0x8D, 0x51, 0x89, 0xBD, 0xA0, 0xEC, 0x86, 0x89, 0x3D,
        0xB9, 0xA1, 0xEC, 0x88, 0x38, 0x2E, 0xDE, 0xD8, 0x2C, 0xAA, 0x5B, 0x11, 0x8F, 0xF9, 0xAD,
        0xA0, 0xE5, 0xB3, 0x3E, 0x87, 0x34,
    ];

    let gamma_abc_5 = [
        0x0E, 0x1C, 0x68, 0xA7, 0x51, 0x55, 0x8F, 0xD0, 0x8C, 0x7F, 0x9A, 0x90, 0x81, 0x26, 0xF4,
        0xB2, 0xC6, 0xB3, 0x0E, 0x4A, 0x0A, 0x7C, 0x23, 0x27, 0xC8, 0xC3, 0x23, 0x85, 0xF3, 0x36,
        0xD4, 0x80, 0x9F, 0x37, 0x11, 0x11, 0x88, 0xAC, 0xBA, 0x17, 0x42, 0x9B, 0x4D, 0x87, 0x0C,
        0x38, 0x64, 0x4A, 0x01, 0x40, 0x25, 0x1B, 0xA0, 0x55, 0x5E, 0x6D, 0xD2, 0x29, 0x10, 0x82,
        0x0A, 0xEE, 0xBC, 0x47, 0xAA, 0x42, 0xEB, 0x98, 0x47, 0x95, 0x13, 0x1D, 0x6B, 0x5A, 0x85,
        0x5C, 0xC2, 0xB2, 0x3B, 0x56, 0xF2, 0xA8, 0xDE, 0xCC, 0xF3, 0x05, 0xF8, 0xD3, 0xAA, 0x2B,
        0x05, 0x15, 0x02, 0x0B, 0x34, 0xB6,
    ];

    let gamma_abc_6 = [
        0x05, 0x62, 0xEE, 0x34, 0xE6, 0xDE, 0xAA, 0x15, 0xDC, 0x9C, 0x6A, 0xCF, 0x9E, 0x61, 0xFC,
        0x13, 0x21, 0x64, 0x39, 0x4E, 0x3D, 0x99, 0x0D, 0xC4, 0xE7, 0x7B, 0xCB, 0xFD, 0xA9, 0xB5,
        0xE9, 0xB3, 0x3B, 0xD9, 0xEC, 0x88, 0x80, 0xDC, 0x99, 0x69, 0x45, 0xC9, 0x12, 0x0E, 0x56,
        0x7B, 0x97, 0xC5, 0x13, 0x03, 0x0F, 0xA7, 0x44, 0x7E, 0x62, 0xAF, 0x70, 0xF2, 0x4A, 0xE6,
        0x75, 0xFA, 0x1C, 0x5B, 0x11, 0xEC, 0xFD, 0xE9, 0xC6, 0x6D, 0xA7, 0x05, 0x4D, 0x65, 0x5B,
        0x01, 0xC3, 0xE2, 0xED, 0x0A, 0xF0, 0x58, 0x29, 0x39, 0xD9, 0x85, 0x72, 0xA3, 0xD3, 0x7C,
        0xF0, 0x08, 0xE1, 0xC4, 0x00, 0xEA,
    ];

    let gamma_abc_7 = [
        0x16, 0x73, 0x77, 0x43, 0x31, 0x96, 0xF2, 0x6D, 0xB9, 0x74, 0x84, 0xD0, 0xC3, 0x22, 0x81,
        0x3D, 0xA3, 0x0A, 0xC9, 0x63, 0x69, 0xF8, 0x0B, 0xFC, 0x51, 0xF1, 0x9B, 0x64, 0xB6, 0x1B,
        0xB1, 0xF7, 0xA0, 0xE1, 0x5B, 0xE9, 0xD1, 0x48, 0x40, 0x53, 0xA6, 0x5C, 0x76, 0x63, 0xC5,
        0x16, 0xB0, 0x67, 0x10, 0x0A, 0x13, 0xD4, 0x3C, 0x24, 0x46, 0x5A, 0xA1, 0x26, 0x53, 0x6E,
        0x63, 0xB0, 0x12, 0xC6, 0x20, 0x91, 0xA6, 0x57, 0x3E, 0x8A, 0xF9, 0x86, 0xFD, 0xAF, 0x3D,
        0xD6, 0xB5, 0xDC, 0x6D, 0x2F, 0x1A, 0x99, 0x28, 0xA0, 0x2E, 0x4B, 0x7C, 0xEE, 0x38, 0x04,
        0x8F, 0x0E, 0xEB, 0xFA, 0x59, 0x00,
    ];

    let gamma_abc_8 = [
        0x17, 0x6C, 0xC2, 0x2C, 0x9E, 0x9A, 0x5F, 0x79, 0xBD, 0x67, 0x89, 0x8E, 0xF2, 0x91, 0xB6,
        0x29, 0xD7, 0xF5, 0x09, 0xDB, 0x13, 0x46, 0x18, 0x00, 0xE7, 0x8C, 0x67, 0x9A, 0x59, 0x0D,
        0xF4, 0xA5, 0x5A, 0x48, 0x6C, 0xD6, 0x2A, 0x81, 0x2E, 0xB3, 0xFD, 0xFA, 0x28, 0x77, 0xEE,
        0xFB, 0xDC, 0xA2, 0x0B, 0xF9, 0xBD, 0xBD, 0xB2, 0xF2, 0xA2, 0x17, 0xD4, 0x63, 0xDA, 0x0D,
        0x6F, 0x4B, 0x19, 0x63, 0x63, 0xA4, 0x9C, 0x65, 0x1F, 0xB9, 0x44, 0x81, 0xC3, 0xF5, 0xF2,
        0x72, 0x37, 0x96, 0xEA, 0xEE, 0x94, 0x21, 0xD4, 0xB4, 0xC3, 0xFB, 0x81, 0x07, 0x42, 0x6E,
        0xE2, 0x06, 0xFE, 0x47, 0xC5, 0x74,
    ];
    let alpha = BytesN::from_array(env, &alpha_bytes);
    let beta = BytesN::from_array(env, &beta_bytes);
    let gamma = BytesN::from_array(env, &gamma_bytes);
    let delta = BytesN::from_array(env, &delta_bytes);

    let mut gamma_abc = Vec::new(env);
    gamma_abc.push_back(BytesN::from_array(env, &gamma_abc_0));
    gamma_abc.push_back(BytesN::from_array(env, &gamma_abc_1));
    gamma_abc.push_back(BytesN::from_array(env, &gamma_abc_2));
    gamma_abc.push_back(BytesN::from_array(env, &gamma_abc_3));
    gamma_abc.push_back(BytesN::from_array(env, &gamma_abc_4));
    gamma_abc.push_back(BytesN::from_array(env, &gamma_abc_5));
    gamma_abc.push_back(BytesN::from_array(env, &gamma_abc_6));
    gamma_abc.push_back(BytesN::from_array(env, &gamma_abc_7));
    gamma_abc.push_back(BytesN::from_array(env, &gamma_abc_8));

    VerificationKey {
        alpha,
        beta,
        gamma,
        delta,
        gamma_abc,
    }
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

/// Decode coordinates and check the curve equation before invoking host crypto.
/// Host subgroup queries can trap on malformed encodings and off-curve points.
fn valid_g1_encoding(bytes: &BytesN<96>) -> bool {
    let bytes = bytes.to_array();
    // Uncompressed finite points have no compression, infinity, or sort flags.
    if bytes[0] & 0xe0 != 0 {
        return false;
    }
    ark_bls12_381::G1Affine::deserialize_uncompressed_unchecked(&bytes[..])
        .is_ok_and(|point| !point.infinity && point.is_on_curve())
}

fn valid_g2_encoding(bytes: &BytesN<192>) -> bool {
    let bytes = bytes.to_array();
    if bytes[0] & 0xe0 != 0 {
        return false;
    }
    ark_bls12_381::G2Affine::deserialize_uncompressed_unchecked(&bytes[..])
        .is_ok_and(|point| !point.infinity && point.is_on_curve())
}

/// Run the Groth16 pairing check, rejecting unsafe points before arithmetic.
fn groth16_verify(
    env: &Env,
    proof: &Bytes,
    public_inputs: &ScoreRangePublicInputs,
) -> Result<bool, VerifierError> {
    if proof.len() != PROOF_SIZE {
        return Err(VerifierError::InvalidProof);
    }
    let a_bytes: BytesN<96> = proof
        .slice(0..96)
        .try_into()
        .map_err(|_| VerifierError::InvalidProof)?;
    let b_bytes: BytesN<192> = proof
        .slice(96..288)
        .try_into()
        .map_err(|_| VerifierError::InvalidProof)?;
    let c_bytes: BytesN<96> = proof
        .slice(288..384)
        .try_into()
        .map_err(|_| VerifierError::InvalidProof)?;
    if !valid_g1_encoding(&a_bytes) || !valid_g2_encoding(&b_bytes) || !valid_g1_encoding(&c_bytes)
    {
        return Err(VerifierError::InvalidProof);
    }
    let a = G1Affine::from_bytes(a_bytes);
    let b = G2Affine::from_bytes(b_bytes);
    let c = G1Affine::from_bytes(c_bytes);
    if !a.is_in_subgroup() || !b.is_in_subgroup() || !c.is_in_subgroup() {
        return Err(VerifierError::InvalidProof);
    }

    let vk = embedded_verification_key(env);

    let bls = env.crypto().bls12_381();

    // Map public inputs to field elements.
    let fields = public_inputs_to_fields(env, public_inputs);

    // Compute combined public-inputs point.
    let combined = compute_public_inputs_combined(env, &vk, &fields);

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
    Ok(bls.pairing_check(vp1, vp2))
}

fn publish_proof_verified(env: &Env, verifier: Address, inputs: &ScoreRangePublicInputs) {
    env.events().publish(
        (
            Symbol::new(env, "ProofVerified"),
            verifier,
            inputs.threshold,
        ),
        (inputs.score_commitment.clone(), inputs.snapshot_ledger),
    );
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
    /// Does not mutate state â€” lenders may simulate read-only.
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

        groth16_verify(&env, &proof, &public_inputs).unwrap_or(false)
    }

    /// Verify a proof and publish `ProofVerified` on success.
    ///
    /// Requires the verifier's authorization. Event topics contain the name,
    /// verifier and threshold; data contains the commitment and snapshot ledger
    /// (the proof's epoch). Malformed proofs return `InvalidProof`.
    pub fn verify_proof(
        env: Env,
        verifier: Address,
        proof: Bytes,
        public_inputs: ScoreRangePublicInputs,
    ) -> Result<bool, VerifierError> {
        verifier.require_auth();
        if proof.len() != PROOF_SIZE {
            return Err(VerifierError::InvalidProof);
        }
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(VerifierError::NotInitialized);
        }
        let version: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CircuitVersion)
            .unwrap_or(0);
        if version != CIRCUIT_VERSION {
            return Err(VerifierError::CircuitVersionMismatch);
        }
        let valid = groth16_verify(&env, &proof, &public_inputs)?;
        if valid {
            publish_proof_verified(&env, verifier, &public_inputs);
        }
        Ok(valid)
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
        let valid = groth16_verify(&env, &proof, &public_inputs)?;
        if !valid {
            return Ok(false);
        }

        // Record the proof as consumed.
        env.storage().persistent().set(&key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);

        publish_proof_verified(&env, consumer, &public_inputs);
        Ok(true)
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
    use ark_ff::{BigInteger, PrimeField};
    use ark_serialize::CanonicalSerialize;
    use ark_std::rand::{rngs::StdRng, SeedableRng};
    use soroban_sdk::testutils::{Address as _, Events as _};
    use soroban_sdk::{Bytes, Env};
    use soroban_sdk::{IntoVal, TryFromVal};
    use zk_circuit::{
        commitment::PedersenCommitment, generate_proof, generate_test_keys, ScorePublicInputs,
        ScoreWitness, CIRCUIT_DOMAIN,
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
        let fields = [
            witness.score as u64,
            witness.vc_count as u64,
            witness.tx_volume_30d as u64,
            witness.avg_counterparties as u64,
            witness.repayment_rate as u64,
            witness.last_updated,
            witness.computed_at_ledger as u64,
            witness.stale as u64,
        ]
        .map(ArkFr::from);
        let commitment = PedersenCommitment::new(CIRCUIT_DOMAIN).commit(&fields, witness.blinding);
        let mut commitment_bytes = [0u8; 32];
        let encoded = commitment.into_bigint().to_bytes_be();
        commitment_bytes[32 - encoded.len()..].copy_from_slice(&encoded);
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
        let verifier = Address::generate(&env);
        let event = (
            contract_id.clone(),
            (
                Symbol::new(&env, "ProofVerified"),
                verifier.clone(),
                inputs.threshold,
            )
                .into_val(&env),
            (inputs.score_commitment.clone(), inputs.snapshot_ledger).into_val(&env),
        );
        assert!(client.verify_proof(&verifier, &proof, &inputs));
        std::println!(
            "verify_proof: CPU {}, memory {}",
            env.cost_estimate().budget().cpu_instruction_cost(),
            env.cost_estimate().budget().memory_bytes_cost()
        );
        let events = env.events().all();
        assert_eq!(events.slice(events.len() - 1..), vec![&env, event.clone()]);
        let before = proof_event_count(&env);
        assert!(!client.verify_proof(&verifier, &proof, &wrong));
        assert_eq!(proof_event_count(&env), before);
        let nonce = BytesN::from_array(&env, &[0x99; 32]);
        assert!(!client.verify_and_consume(&verifier, &proof, &wrong, &nonce));
        assert_eq!(proof_event_count(&env), before);
        assert!(client.verify_and_consume(&verifier, &proof, &inputs, &nonce));
        std::println!(
            "verify_and_consume: CPU {}, memory {}",
            env.cost_estimate().budget().cpu_instruction_cost(),
            env.cost_estimate().budget().memory_bytes_cost()
        );
        let events = env.events().all();
        assert_eq!(events.slice(events.len() - 1..), vec![&env, event]);
        let before = proof_event_count(&env);
        assert_eq!(
            client.try_verify_and_consume(&verifier, &proof, &inputs, &nonce),
            Err(Ok(VerifierError::ProofAlreadyConsumed))
        );
        assert_eq!(proof_event_count(&env), before);
    }

    fn proof_event_count(env: &Env) -> usize {
        let name = Symbol::new(env, "ProofVerified");
        env.events()
            .all()
            .iter()
            .filter(|(_, topics, _)| {
                topics
                    .first()
                    .and_then(|topic| Symbol::try_from_val(env, &topic).ok())
                    == Some(name.clone())
            })
            .count()
    }

    #[test]
    fn test_verify_proof_rejects_invalid_inputs_without_events() {
        let (env, admin, id) = setup();
        let client = ScoreRangeVerifierClient::new(&env, &id);
        let inputs = make_public_inputs(&env);
        for bytes in [
            ark_std::vec![],
            ark_std::vec![0; 383],
            ark_std::vec![0x42; 384],
            ark_std::vec![0; 385],
        ] {
            let proof = Bytes::from_slice(&env, &bytes);
            assert_eq!(
                client.try_verify_proof(&admin, &proof, &inputs),
                Err(Ok(VerifierError::InvalidProof))
            );
            assert!(!client.verify_score_range(&proof, &inputs));
        }
        assert_eq!(
            client.try_verify_proof(&admin, &make_proof(&env), &inputs),
            Err(Ok(VerifierError::InvalidProof))
        );
        assert_eq!(proof_event_count(&env), 0);
        env.mock_auths(&[]);
        assert!(client
            .try_verify_proof(&admin, &make_proof(&env), &inputs)
            .is_err());
        assert_eq!(proof_event_count(&env), 0);
    }

    #[test]
    fn test_verify_proof_initialization_and_version_guards() {
        let (env, admin, id) = setup();
        let inputs = make_public_inputs(&env);
        let proof = make_proof(&env);
        let fresh = env.register_contract(None, ScoreRangeVerifier);
        let client = ScoreRangeVerifierClient::new(&env, &fresh);
        assert_eq!(
            client.try_verify_proof(&admin, &proof, &inputs),
            Err(Ok(VerifierError::NotInitialized))
        );
        env.as_contract(&id, || {
            env.storage()
                .instance()
                .set(&DataKey::CircuitVersion, &2u32)
        });
        let client = ScoreRangeVerifierClient::new(&env, &id);
        assert_eq!(
            client.try_verify_proof(&admin, &proof, &inputs),
            Err(Ok(VerifierError::CircuitVersionMismatch))
        );
        assert_eq!(proof_event_count(&env), 0);
    }

    #[test]
    fn test_non_subgroup_points_are_rejected_in_every_position() {
        use ark_bls12_381::{Fq, Fq2, G1Affine as ArkG1, G2Affine as ArkG2};
        let (env, verifier, id) = setup();
        let client = ScoreRangeVerifierClient::new(&env, &id);
        let inputs = make_public_inputs(&env);
        let key = embedded_verification_key(&env);
        let mut baseline = ark_std::vec::Vec::new();
        baseline.extend_from_slice(&key.alpha.to_array());
        baseline.extend_from_slice(&key.beta.to_array());
        baseline.extend_from_slice(&key.alpha.to_array());
        let g1 = (0..100u64)
            .find_map(|x| {
                ArkG1::get_point_from_x_unchecked(Fq::from(x), false)
                    .filter(|p| !p.is_in_correct_subgroup_assuming_on_curve())
            })
            .unwrap();
        let g2 = (0..100u64)
            .find_map(|x| {
                ArkG2::get_point_from_x_unchecked(Fq2::new(Fq::from(x), Fq::from(1u64)), false)
                    .filter(|p| !p.is_in_correct_subgroup_assuming_on_curve())
            })
            .unwrap();
        assert!(g1.is_on_curve());
        assert!(g2.is_on_curve());
        let mut g1_bytes = ark_std::vec::Vec::new();
        let mut g2_bytes = ark_std::vec::Vec::new();
        g1.serialize_uncompressed(&mut g1_bytes).unwrap();
        g2.serialize_uncompressed(&mut g2_bytes).unwrap();
        for (start, bytes) in [(0, &g1_bytes), (96, &g2_bytes), (288, &g1_bytes)] {
            let mut data = baseline.clone();
            data[start..start + bytes.len()].copy_from_slice(bytes);
            let proof = Bytes::from_slice(&env, &data);
            assert_eq!(
                client.try_verify_proof(&verifier, &proof, &inputs),
                Err(Ok(VerifierError::InvalidProof))
            );
            assert_eq!(
                client.try_verify_and_consume(
                    &verifier,
                    &proof,
                    &inputs,
                    &BytesN::from_array(&env, &[0; 32])
                ),
                Err(Ok(VerifierError::InvalidProof))
            );
            assert!(!client.verify_score_range(&proof, &inputs));
        }
        // Infinity and illegal flags are rejected at A, B and C independently.
        for (start, len) in [(0, 96), (96, 192), (288, 96)] {
            for flag in [0x40, 0x80, 0x20, 0xe0] {
                let mut data = baseline.clone();
                data[start..start + len].fill(0);
                data[start] = flag;
                assert_eq!(
                    client.try_verify_proof(&verifier, &Bytes::from_slice(&env, &data), &inputs),
                    Err(Ok(VerifierError::InvalidProof))
                );
            }
        }
        assert_eq!(proof_event_count(&env), 0);
    }

    #[test]
    fn fuzz_verify_proof_bytes() {
        use proptest::{collection, prelude::*, test_runner::TestRunner};
        let template_env = Env::new_with_config(soroban_sdk::testutils::EnvTestConfig {
            capture_snapshot_at_drop: false,
            ..Default::default()
        });
        let key = embedded_verification_key(&template_env);
        let mut template = ark_std::vec::Vec::new();
        template.extend_from_slice(&key.alpha.to_array());
        template.extend_from_slice(&key.beta.to_array());
        template.extend_from_slice(&key.alpha.to_array());
        let strategy = prop_oneof![
            collection::vec(any::<u8>(), 0..768),
            collection::vec(any::<u8>(), 384),
            // Valid prefixes force validation to reach B and C as well as A.
            (0..3usize, collection::vec(any::<u8>(), 192)).prop_map(move |(position, random)| {
                let mut bytes = template.clone();
                let (start, len) = [(0, 96), (96, 192), (288, 96)][position];
                bytes[start..start + len].copy_from_slice(&random[..len]);
                bytes
            }),
        ];
        TestRunner::default()
            .run(&strategy, |data| {
                let env = Env::new_with_config(soroban_sdk::testutils::EnvTestConfig {
                    capture_snapshot_at_drop: false,
                    ..Default::default()
                });
                env.mock_all_auths();
                let id = env.register_contract(None, ScoreRangeVerifier);
                let client = ScoreRangeVerifierClient::new(&env, &id);
                let verifier = Address::generate(&env);
                client.initialize(
                    &verifier,
                    &BytesN::from_array(&env, &[0; 32]),
                    &CIRCUIT_VERSION,
                );
                let proof = Bytes::from_slice(&env, &data);
                let inputs = make_public_inputs(&env);
                prop_assert_eq!(
                    client.try_verify_proof(&verifier, &proof, &inputs),
                    Err(Ok(VerifierError::InvalidProof))
                );
                prop_assert_eq!(proof_event_count(&env), 0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn test_verify_proof_rejects_off_curve_and_noncanonical_coordinates() {
        use ark_bls12_381::Fq;
        let (env, verifier, id) = setup();
        let client = ScoreRangeVerifierClient::new(&env, &id);
        let inputs = make_public_inputs(&env);
        let key = embedded_verification_key(&env);
        let mut baseline = ark_std::vec::Vec::new();
        baseline.extend_from_slice(&key.alpha.to_array());
        baseline.extend_from_slice(&key.beta.to_array());
        baseline.extend_from_slice(&key.alpha.to_array());
        let modulus = Fq::MODULUS.to_bytes_be();
        assert_eq!(modulus.len(), 48);
        for (start, len) in [(0, 96), (96, 192), (288, 96)] {
            // All-zero finite coordinates fail the curve equation.
            let mut off_curve = baseline.clone();
            off_curve[start..start + len].fill(0);
            let mut corruptions = ark_std::vec![off_curve];
            // Exercise every Fp coordinate, including both Fp2 components in B.
            // A coordinate equal to the modulus is noncanonical without flags.
            for offset in (start..start + len).step_by(48) {
                let mut noncanonical = baseline.clone();
                noncanonical[offset..offset + 48].copy_from_slice(&modulus);
                corruptions.push(noncanonical);
            }
            for data in corruptions {
                let proof = Bytes::from_slice(&env, &data);
                assert_eq!(
                    client.try_verify_proof(&verifier, &proof, &inputs),
                    Err(Ok(VerifierError::InvalidProof))
                );
                assert_eq!(
                    client.try_verify_and_consume(
                        &verifier,
                        &proof,
                        &inputs,
                        &BytesN::from_array(&env, &[0; 32])
                    ),
                    Err(Ok(VerifierError::InvalidProof))
                );
                assert!(!client.verify_score_range(&proof, &inputs));
            }
        }
        assert_eq!(proof_event_count(&env), 0);
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
        // Infinity is a valid curve point but is not accepted in a proof.
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
    fn test_verify_score_range_rejects_infinity() {
        let (env, _, _) = setup();
        let contract_id = env.register(ScoreRangeVerifier, ());
        let client = ScoreRangeVerifierClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let vk_hash = BytesN::from_array(&env, &[0xAB; 32]);
        client.initialize(&admin, &vk_hash, &CIRCUIT_VERSION);

        let inputs = make_public_inputs(&env);
        let proof = make_proof(&env);
        // The read-only compatibility query maps invalid points to false.
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
        // Malformed points are rejected before invoking host arithmetic.
        let proof = Bytes::from_array(&env, &[0x42u8; 384]);
        let res = client.try_verify_score_range(&proof, &inputs);
        assert_eq!(res, Ok(Ok(false)));
    }

    #[test]
    fn test_verify_and_consume_rejects_infinity() {
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

        // Failed verification never records the proof as consumed.
        let res = client.try_verify_and_consume(&consumer, &proof, &inputs, &nonce);
        assert_eq!(res, Err(Ok(VerifierError::InvalidProof)));

        let res2 = client.try_verify_and_consume(&consumer, &proof, &inputs, &nonce);
        assert_eq!(res2, Err(Ok(VerifierError::InvalidProof)));
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
}
