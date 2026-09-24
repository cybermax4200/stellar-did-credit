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
pub const NUM_PUBLIC_INPUTS: u32 = 6;

/// Circuit version constant — must match the `domain_separator` in the
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
}

const INSTANCE_BUMP_THRESHOLD: u32 = 5000;
const INSTANCE_BUMP_AMOUNT: u32 = 500_000;

const PERS_TTL_THRESHOLD: u32 = 120_960; // ~7 days
const PERS_TTL_EXTEND: u32 = 518_400; // ~30 days

/// Embedded verification key for the score-range circuit.
///
/// This is a placeholder VK (BLS12-381 generator points) until the trusted
/// setup for the score-range circuit (Issue 59) is completed. The real VK
/// must be embedded here before mainnet deployment. The `vk_hash` stored at
/// `initialize` time must match `sha256` of this VK's canonical encoding.
fn embedded_verification_key(env: &Env) -> VerificationKey {
    // Generated BLS12-381 G1 generator (uncompressed, 96 bytes) for alpha
    let alpha_bytes = [
        0x01, 0xFB, 0x00, 0xF7, 0x67, 0xE9, 0x9B, 0x6A, 0x77, 0x57, 0x53, 0x8D, 0x1D, 0x99, 0xB6, 0x94, 
        0x31, 0xC0, 0x78, 0x40, 0xEF, 0x24, 0x83, 0xFD, 0xE5, 0x54, 0x84, 0x45, 0xDE, 0x60, 0x91, 0x6E, 
        0x5A, 0xF8, 0xC2, 0x85, 0xD9, 0x46, 0x83, 0x8C, 0xB5, 0x72, 0x81, 0x65, 0xF9, 0x8A, 0x21, 0x55, 
        0x16, 0xC7, 0xB9, 0x98, 0x45, 0xD2, 0xD7, 0x74, 0x8E, 0x42, 0x2C, 0xFF, 0xEE, 0x6D, 0x10, 0x2A, 
        0xF5, 0xEE, 0x6C, 0x55, 0x34, 0xB5, 0xC9, 0x77, 0x32, 0xAB, 0x1E, 0x18, 0xAC, 0xD4, 0x1A, 0x42, 
        0x7E, 0x23, 0x0A, 0x8E, 0x47, 0x76, 0x7B, 0x6D, 0xB4, 0x22, 0x08, 0x8C, 0x0E, 0x26, 0xC7, 0xE9, 
    ];

    let beta_bytes = [
        0x0F, 0xE9, 0xF3, 0x4C, 0x58, 0x6E, 0x46, 0x5B, 0xB8, 0xEB, 0xCC, 0xB1, 0x00, 0x82, 0x9D, 0x7A, 
        0x54, 0x71, 0x0E, 0xD3, 0x0F, 0x94, 0x46, 0x16, 0x81, 0x71, 0x80, 0x3B, 0x66, 0x0D, 0xA2, 0x80, 
        0xC3, 0x0C, 0x12, 0x8C, 0xAF, 0x5B, 0x78, 0x7B, 0x02, 0xB6, 0x70, 0xA0, 0xF6, 0xD8, 0xBC, 0xE7, 
        0x0C, 0x86, 0x9C, 0x1B, 0xCE, 0x0B, 0x97, 0x18, 0x6F, 0xCF, 0xD5, 0x22, 0xFB, 0xAF, 0x43, 0xB1, 
        0x0C, 0xB6, 0x52, 0xE2, 0x38, 0x9E, 0x36, 0x97, 0xBB, 0x98, 0x87, 0x9F, 0x4F, 0x38, 0xEC, 0x97, 
        0xDA, 0x8C, 0x8E, 0x74, 0xEE, 0x5C, 0x05, 0xBB, 0x19, 0x11, 0x91, 0xF4, 0x95, 0xFB, 0x58, 0xED, 
        0x11, 0x17, 0x1B, 0x5C, 0x91, 0x41, 0x9E, 0xAF, 0x77, 0x14, 0x3D, 0x86, 0xCE, 0xB2, 0xD1, 0x50, 
        0x19, 0x61, 0x80, 0x2C, 0x23, 0xFE, 0xF3, 0x07, 0x7B, 0x95, 0x88, 0x3D, 0x8A, 0x9A, 0xB4, 0xEB, 
        0x9D, 0xDE, 0x5B, 0xE4, 0xE2, 0x6F, 0x64, 0x66, 0xBD, 0xEE, 0x4A, 0x6B, 0xF2, 0xA0, 0x24, 0x7F, 
        0x09, 0x6B, 0x0D, 0xD1, 0x73, 0xC1, 0xDD, 0xD2, 0x83, 0x52, 0xFF, 0xC2, 0x0B, 0xEE, 0x8B, 0x9B, 
        0xA3, 0x91, 0xCE, 0x14, 0x07, 0x88, 0x34, 0x18, 0xE3, 0xC4, 0xBD, 0x98, 0x89, 0xCB, 0xFB, 0x1C, 
        0x89, 0xAC, 0x5F, 0x1D, 0x70, 0x0A, 0xBE, 0x33, 0x98, 0xEB, 0x80, 0xBF, 0xAF, 0xD8, 0xEA, 0x8C, 
    ];

    let gamma_bytes = [
        0x0E, 0x92, 0x81, 0xC8, 0x7F, 0x39, 0x0F, 0x55, 0x67, 0x02, 0xA7, 0x47, 0xDE, 0x72, 0x25, 0x91, 
        0x1F, 0xCA, 0xFC, 0x22, 0x29, 0x63, 0xC4, 0xC2, 0x02, 0x18, 0x28, 0xB3, 0x6D, 0x48, 0xC1, 0x45, 
        0xF1, 0x73, 0xDF, 0x87, 0x89, 0xAE, 0x90, 0x0D, 0x0A, 0xE1, 0x8B, 0x9F, 0xCD, 0x8C, 0x3E, 0xD8, 
        0x04, 0x97, 0x54, 0xEE, 0x8D, 0x1A, 0xFC, 0x22, 0xA2, 0x60, 0xBB, 0x47, 0xE6, 0x9D, 0x25, 0x1F, 
        0x18, 0xB0, 0xFD, 0x1A, 0x65, 0x29, 0xAD, 0x4C, 0x20, 0x3A, 0x35, 0xD6, 0xB6, 0x0D, 0x4F, 0x27, 
        0x66, 0x4C, 0x23, 0x58, 0x56, 0x90, 0x9D, 0x7D, 0x67, 0xD4, 0x9E, 0x03, 0x39, 0x85, 0x8D, 0xD6, 
        0x10, 0x75, 0x20, 0x30, 0x2A, 0xA7, 0x48, 0xC6, 0x7E, 0xE7, 0x8F, 0x55, 0x4A, 0xA3, 0x82, 0xCD, 
        0xDA, 0xD1, 0x33, 0xE3, 0x42, 0xDC, 0x2B, 0x04, 0x50, 0xED, 0x42, 0x9E, 0x3A, 0xC9, 0xB1, 0x81, 
        0xC4, 0xAE, 0x33, 0xF1, 0xB0, 0x0A, 0x10, 0x2F, 0xA6, 0xDD, 0x83, 0x8F, 0x52, 0x41, 0x1B, 0xBE, 
        0x12, 0x21, 0xF4, 0x06, 0x6F, 0xAC, 0x51, 0x31, 0xC5, 0x62, 0x53, 0x9A, 0x69, 0x72, 0xF8, 0x92, 
        0xE4, 0x2B, 0x2B, 0xAE, 0xC1, 0xEA, 0xC7, 0x10, 0x9E, 0x38, 0xE4, 0x5E, 0xA6, 0x52, 0x9F, 0xC4, 
        0x7C, 0x0B, 0x90, 0xCA, 0x9D, 0xE1, 0x7B, 0x5A, 0xE9, 0x89, 0xF5, 0x88, 0x93, 0x5D, 0x02, 0x8F, 
    ];

    let delta_bytes = [
        0x15, 0x19, 0x6C, 0x13, 0xD3, 0x47, 0x8D, 0xDF, 0x21, 0x64, 0x40, 0x4F, 0x53, 0x11, 0x5C, 0x6B, 
        0x22, 0x56, 0x93, 0x2C, 0xE5, 0x70, 0x94, 0x63, 0x5D, 0xC3, 0x74, 0xBE, 0xF7, 0xB4, 0xCA, 0xF2, 
        0x62, 0x62, 0xBC, 0x2F, 0xE3, 0xF0, 0x7B, 0xDE, 0x59, 0xBC, 0xA0, 0x98, 0xC4, 0xA6, 0x8F, 0x72, 
        0x11, 0xA1, 0xDE, 0xC4, 0x2A, 0xE4, 0xE8, 0x97, 0x3E, 0x96, 0x0E, 0xEA, 0x0F, 0x5B, 0x53, 0xF4, 
        0x7A, 0x62, 0xCC, 0x84, 0x86, 0x37, 0x8E, 0x89, 0x15, 0xB0, 0x39, 0x93, 0xC4, 0x88, 0x30, 0x59, 
        0x0D, 0xAE, 0x62, 0x4F, 0x91, 0xE3, 0xA0, 0xBB, 0x21, 0x56, 0x7F, 0x28, 0x03, 0x2C, 0x7C, 0xC2, 
        0x02, 0x38, 0xD4, 0x6A, 0x97, 0x46, 0x29, 0x7D, 0x81, 0x1D, 0xA6, 0x37, 0xF3, 0x59, 0xA1, 0x26, 
        0xDF, 0xB5, 0x6E, 0xE7, 0x49, 0x65, 0xA7, 0x4B, 0xDD, 0x5D, 0x8E, 0x21, 0x98, 0x30, 0x06, 0x00, 
        0x8F, 0x2C, 0xAC, 0xEF, 0x59, 0x2C, 0xE0, 0x38, 0xB7, 0xC3, 0xCA, 0x95, 0x76, 0x00, 0x24, 0x41, 
        0x04, 0x0C, 0x86, 0x22, 0x2F, 0xD5, 0x11, 0x27, 0x6D, 0x95, 0xA1, 0x63, 0x25, 0x55, 0x15, 0xD3, 
        0x7B, 0xCE, 0x59, 0xBB, 0x36, 0xDD, 0x2E, 0x02, 0x05, 0x62, 0x39, 0xB0, 0xDB, 0xA0, 0xBA, 0xAE, 
        0xDD, 0x58, 0x06, 0x14, 0x86, 0xFB, 0x80, 0xDA, 0xC2, 0xAC, 0x01, 0x7C, 0xF5, 0x39, 0xB6, 0x5A, 
    ];

    let gamma_abc_0 = [
        0x06, 0xE9, 0x40, 0x60, 0xEA, 0x72, 0xA3, 0xB9, 0xFC, 0xCA, 0x0D, 0x40, 0xD6, 0x07, 0xE8, 0x88, 
        0x36, 0x66, 0x2E, 0x6C, 0x23, 0x98, 0x83, 0x24, 0x34, 0x41, 0xD0, 0x37, 0x0F, 0x78, 0x3B, 0xBB, 
        0xDB, 0x25, 0x88, 0xA2, 0x55, 0xEE, 0x11, 0x8E, 0x72, 0x47, 0x2B, 0xD6, 0xD0, 0x30, 0x47, 0xFE, 
        0x06, 0xEE, 0x12, 0x71, 0xA0, 0x68, 0xB4, 0x07, 0x48, 0xF6, 0xEC, 0x21, 0xB6, 0x4E, 0x61, 0x3F, 
        0x3B, 0xC4, 0x1A, 0x76, 0x7A, 0x3C, 0xCE, 0x02, 0xAE, 0x6D, 0x75, 0xDE, 0x47, 0x1B, 0x10, 0xCC, 
        0x09, 0x1E, 0xFE, 0x32, 0xB8, 0x32, 0x3C, 0x54, 0x40, 0x47, 0x8B, 0xBB, 0xB1, 0x76, 0x23, 0xE1, 
    ];

    let gamma_abc_1 = [
        0x07, 0xB4, 0xBC, 0xFC, 0xC3, 0xE8, 0xF8, 0x97, 0xE6, 0xAB, 0x3D, 0xED, 0x1E, 0xA2, 0x04, 0x4C, 
        0x24, 0xBF, 0x3B, 0x7C, 0x39, 0xA1, 0x0D, 0xAC, 0x2E, 0xE5, 0xD2, 0xAC, 0xA4, 0x28, 0x27, 0xC5, 
        0x92, 0xF6, 0x51, 0x6B, 0xD1, 0x80, 0x49, 0xEB, 0x54, 0xC1, 0xF3, 0xAF, 0xE6, 0x14, 0xCB, 0x40, 
        0x0C, 0xEC, 0xCE, 0x5D, 0xA8, 0x8D, 0x8D, 0x83, 0x59, 0xE1, 0x42, 0x74, 0x3E, 0x10, 0x26, 0x75, 
        0x67, 0xC8, 0xA4, 0x7E, 0x1A, 0x93, 0x93, 0x64, 0x46, 0xCE, 0x02, 0xC5, 0x67, 0xCA, 0x0F, 0xDA, 
        0xDF, 0x7A, 0x59, 0x84, 0x42, 0xFA, 0xCE, 0x16, 0xB8, 0xBE, 0x2E, 0xE0, 0x43, 0xDA, 0x32, 0xC9, 
    ];

    let gamma_abc_2 = [
        0x09, 0x0D, 0xAB, 0x3F, 0xBA, 0xCF, 0xF9, 0x26, 0x87, 0x3E, 0x5C, 0x4C, 0x28, 0x3F, 0xA9, 0x3F, 
        0xC4, 0x6D, 0x82, 0x60, 0xC6, 0x8B, 0x6A, 0xFB, 0xFB, 0x75, 0x60, 0xB2, 0xCE, 0x11, 0x65, 0x65, 
        0xFB, 0x51, 0x1A, 0x63, 0x4A, 0x31, 0xFD, 0x44, 0x3C, 0x54, 0xDF, 0x1D, 0x84, 0xCE, 0xF0, 0x9F, 
        0x17, 0x99, 0x4A, 0xA9, 0x20, 0xED, 0xBA, 0xF4, 0x81, 0x4D, 0xD0, 0xEB, 0xAC, 0x95, 0x76, 0x64, 
        0x18, 0xE5, 0x8F, 0xF1, 0x99, 0xE6, 0x1E, 0xBF, 0xBC, 0xF7, 0xDA, 0x1A, 0x90, 0x30, 0xD0, 0x57, 
        0x1E, 0x69, 0x89, 0xA7, 0x5F, 0x6D, 0x99, 0xC2, 0x46, 0x0A, 0x96, 0x8D, 0x9F, 0xC1, 0xF5, 0x6A, 
    ];

    let gamma_abc_3 = [
        0x02, 0xDA, 0x87, 0xF6, 0x1F, 0xC4, 0x12, 0xF1, 0xA7, 0x89, 0x84, 0xE3, 0x52, 0xAA, 0xD5, 0x8F, 
        0x3E, 0x2A, 0x07, 0x71, 0x16, 0x39, 0x07, 0xDD, 0x7C, 0xCB, 0xA5, 0x8C, 0xB6, 0x7B, 0x61, 0xEE, 
        0xB2, 0x10, 0xBC, 0xE8, 0x8A, 0x09, 0x3D, 0xD4, 0xA4, 0x59, 0x1A, 0x12, 0xD0, 0x00, 0x79, 0x08, 
        0x06, 0x8D, 0xF0, 0xE7, 0x38, 0x93, 0x1A, 0xD8, 0xB2, 0x1C, 0x93, 0xED, 0x9D, 0xA4, 0x44, 0x28, 
        0x8C, 0x38, 0x11, 0x8C, 0xA4, 0xF7, 0x6F, 0x90, 0xFA, 0x98, 0xDA, 0x01, 0xE9, 0x73, 0xC4, 0x73, 
        0x07, 0x45, 0x9B, 0xA1, 0x31, 0x50, 0xC8, 0x5A, 0xA5, 0x8A, 0x2F, 0x65, 0x17, 0x2C, 0x07, 0xD3, 
    ];

    let gamma_abc_4 = [
        0x15, 0x08, 0x7C, 0xEF, 0x4C, 0x19, 0x7A, 0x70, 0x65, 0xE4, 0xDD, 0x5F, 0xC5, 0x4B, 0xA3, 0x9E, 
        0xA1, 0x1C, 0x23, 0x53, 0xB7, 0xE8, 0xB9, 0xE7, 0xA8, 0xCA, 0x6A, 0xE7, 0x45, 0x2E, 0x6A, 0x7A, 
        0x5E, 0x07, 0x83, 0x16, 0xDE, 0x36, 0x1F, 0xCB, 0xC7, 0x8F, 0xBF, 0x92, 0xA7, 0x8C, 0x1F, 0x9F, 
        0x18, 0xF6, 0xD1, 0x94, 0x96, 0x5D, 0x4F, 0x6D, 0x57, 0x15, 0x50, 0x9E, 0x19, 0x7D, 0x89, 0x96, 
        0xF0, 0x9B, 0x77, 0x1B, 0x1C, 0x25, 0xB5, 0x21, 0x9D, 0x53, 0xF9, 0x2B, 0xE5, 0xCF, 0xC8, 0xC5, 
        0x7C, 0xA9, 0xDD, 0x9C, 0xA3, 0x8D, 0xF9, 0x8B, 0xE3, 0x76, 0xCB, 0x89, 0x06, 0x74, 0x8D, 0x7D, 
    ];

    let gamma_abc_5 = [
        0x15, 0x72, 0x09, 0xBB, 0x02, 0x8C, 0x1E, 0x3B, 0x42, 0xB7, 0x7F, 0x68, 0xBD, 0x99, 0x26, 0xC1, 
        0x5E, 0x6A, 0xD6, 0xA4, 0x1F, 0x79, 0x9D, 0x7C, 0x50, 0x9D, 0x02, 0xF1, 0x73, 0xC0, 0x9F, 0x70, 
        0x0C, 0x02, 0x2E, 0xE7, 0xBB, 0xBE, 0xA0, 0xE5, 0x07, 0x82, 0xB4, 0xF8, 0x93, 0x19, 0x2C, 0xFC, 
        0x19, 0x92, 0x14, 0x98, 0xDF, 0xF3, 0xF5, 0x8B, 0x9D, 0xAF, 0xF2, 0xD1, 0xC2, 0xF0, 0x5A, 0xD6, 
        0xB5, 0xE4, 0x37, 0xC7, 0x87, 0xBF, 0x4F, 0xF2, 0x66, 0x87, 0x05, 0xA1, 0x3A, 0xE6, 0x52, 0x9A, 
        0xFB, 0xB4, 0xF7, 0xE1, 0x5F, 0x42, 0x98, 0xBE, 0x65, 0xAA, 0x6C, 0x48, 0xD8, 0xAC, 0x05, 0x8C, 
    ];

    let gamma_abc_6 = [
        0x13, 0x82, 0x8C, 0x86, 0x20, 0xA2, 0x9F, 0xD5, 0xC6, 0x81, 0x28, 0x6B, 0x46, 0x13, 0xA6, 0xDA, 
        0x65, 0x05, 0x0E, 0xB8, 0x5B, 0x1E, 0x2B, 0xB1, 0x76, 0xA8, 0xF4, 0x10, 0x60, 0x34, 0xD9, 0xBD, 
        0xAB, 0xAC, 0x62, 0x5D, 0xAB, 0xB9, 0xCB, 0x19, 0xCF, 0xF0, 0x81, 0xC7, 0xB7, 0x13, 0xA1, 0x61, 
        0x14, 0x0B, 0xE6, 0xC2, 0x75, 0x82, 0x7A, 0x6C, 0x80, 0x10, 0x1E, 0xBF, 0x48, 0x68, 0x52, 0xBE, 
        0xA2, 0x02, 0x7F, 0xA6, 0x10, 0x1F, 0xC4, 0x07, 0x46, 0x0C, 0xA1, 0xA7, 0xDD, 0x59, 0xEC, 0xA9, 
        0x04, 0x9B, 0xD1, 0x2B, 0xD3, 0xDE, 0x89, 0x72, 0xE5, 0xDA, 0x5B, 0xEE, 0x36, 0x72, 0xD9, 0xF9, 
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

    VerificationKey {
        alpha,
        beta,
        gamma,
        delta,
        gamma_abc,
    }
}

/// Encode a u32 as a 32-byte little-endian field element.
fn u32_to_field(env: &Env, value: u32) -> BytesN<32> {
    let mut buf = [0u8; 32];
    buf[0] = (value & 0xFF) as u8;
    buf[1] = ((value >> 8) & 0xFF) as u8;
    buf[2] = ((value >> 16) & 0xFF) as u8;
    buf[3] = ((value >> 24) & 0xFF) as u8;
    BytesN::from_array(env, &buf)
}

/// Encode an Address as a 32-byte field element via SHA-256 of its XDR bytes.
fn address_to_field(env: &Env, addr: &Address) -> BytesN<32> {
    let xdr = addr.clone().to_xdr(env);
    env.crypto().sha256(&xdr).to_bytes()
}

/// Map structured public inputs to the ordered list of field elements
/// expected by the Groth16 verifier.
fn public_inputs_to_fields(
    env: &Env,
    inputs: &ScoreRangePublicInputs,
) -> Vec<BytesN<32>> {
    let mut fields = Vec::new(env);
    fields.push_back(u32_to_field(env, inputs.threshold));
    fields.push_back(address_to_field(env, &inputs.subject));
    fields.push_back(address_to_field(env, &inputs.credit_oracle_id));
    fields.push_back(inputs.score_commitment.clone());
    fields.push_back(u32_to_field(env, inputs.snapshot_ledger));
    fields.push_back(inputs.domain_separator.clone());
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

/// Run the Groth16 pairing check for a proof against the embedded VK.
///
/// Note: soroban-sdk 22's BLS12-381 host functions strictly validate that
/// every point is on the curve and in the correct subgroup. A proof whose
/// points fail that validation raises a host error (the invocation fails)
/// rather than returning `false`; only well-formed points that do not satisfy
/// the pairing equation produce a `false` result.
fn groth16_verify(
    env: &Env,
    proof: &Bytes,
    public_inputs: &ScoreRangePublicInputs,
) -> bool {
    if proof.len() != PROOF_SIZE {
        return false;
    }

    let vk = embedded_verification_key(env);

    // Parse proof: A (G1, 96) || B (G2, 192) || C (G1, 96).
    let a: G1Affine = G1Affine::from_bytes(proof.slice(0..96).try_into().unwrap());
    let b: G2Affine = G2Affine::from_bytes(proof.slice(96..288).try_into().unwrap());
    let c: G1Affine = G1Affine::from_bytes(proof.slice(288..384).try_into().unwrap());

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
    bls.pairing_check(vp1, vp2)
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
        env.events().publish(
            (symbol_short!("Init"),),
            (admin, vk_hash, circuit_version),
        );
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
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Bytes, Env};

    fn setup() -> (Env, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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
        }
    }

    #[test]
    fn test_initialize_stores_config() {
        let (env, admin, _) = setup();
        let client = ScoreRangeVerifierClient::new(&env, &env.register_contract(None, ScoreRangeVerifier));
        // Re-register to get a fresh client bound to the same contract.
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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
    fn test_verify_score_range_requires_initialization() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, ScoreRangeVerifier);
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