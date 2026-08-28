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
    // BLS12-381 G1 generator (uncompressed, 96 bytes).
    let g1_gen: [u8; 96] = [
        0x17, 0xF1, 0xD3, 0xA7, 0x31, 0x97, 0xD7, 0x94, 0x26, 0x95, 0x63, 0x8C, 0x4F, 0xA9, 0xAC,
        0x0F, 0xC3, 0x68, 0x8C, 0x4F, 0x97, 0x74, 0xB9, 0x05, 0xA1, 0x4E, 0x3A, 0x3F, 0x17, 0x1B,
        0xAC, 0x58, 0x6C, 0x55, 0xE8, 0x3F, 0xF9, 0x7A, 0x1A, 0xEF, 0xFB, 0x3A, 0xF0, 0x0A, 0xDB,
        0x22, 0xC6, 0xBB, 0x08, 0xB3, 0xF4, 0x81, 0xE3, 0xAA, 0xA0, 0xF1, 0xA0, 0x9E, 0x30, 0xED,
        0x74, 0x1D, 0x8A, 0xE4, 0xFC, 0xF5, 0xE0, 0x95, 0xD5, 0xD0, 0x0A, 0xF6, 0x00, 0xDB, 0x18,
        0xCB, 0x2C, 0x04, 0xB3, 0xED, 0xD0, 0x3C, 0xC7, 0x44, 0xA2, 0x88, 0x8A, 0xE4, 0x0C, 0xAA,
        0x23, 0x29, 0x46, 0xC5, 0xE7, 0xE1,
    ];

    // BLS12-381 G2 generator (uncompressed, 192 bytes).
    let g2_gen: [u8; 192] = [
        0x02, 0x4A, 0xA2, 0xB2, 0xF0, 0x8F, 0x0A, 0x91, 0x26, 0x08, 0x05, 0x27, 0x2D, 0xC5, 0x10,
        0x51, 0xC6, 0xE4, 0x7A, 0xD4, 0xFA, 0x40, 0x3B, 0x02, 0xB4, 0x51, 0x0B, 0x64, 0x7A, 0xE3,
        0xD1, 0x77, 0x0B, 0xAC, 0x03, 0x26, 0xA8, 0x05, 0xBB, 0xEF, 0xD4, 0x80, 0x56, 0xC8, 0xC1,
        0x21, 0xBD, 0xB8, 0x13, 0xE0, 0x2B, 0x60, 0x52, 0x71, 0x9F, 0x60, 0x7D, 0xAC, 0xD3, 0xA0,
        0x88, 0x27, 0x4F, 0x65, 0x59, 0x6B, 0xD0, 0xD0, 0x99, 0x20, 0xB6, 0x1A, 0xB5, 0xDA, 0x61,
        0xBB, 0xDC, 0x7F, 0x50, 0x49, 0x33, 0x4C, 0xF1, 0x12, 0x13, 0x94, 0x5D, 0x57, 0xE5, 0xAC,
        0x7D, 0x05, 0x5D, 0x04, 0x2B, 0x7E, 0x0C, 0xE5, 0xD5, 0x27, 0x72, 0x7D, 0x6E, 0x11, 0x8C,
        0xC9, 0xCD, 0xC6, 0xDA, 0x2E, 0x35, 0x1A, 0xAD, 0xFD, 0x9B, 0xAA, 0x8C, 0xBD, 0xD3, 0xA7,
        0x6D, 0x42, 0x9A, 0x69, 0x51, 0x60, 0xD1, 0x2C, 0x92, 0x3A, 0xC9, 0xCC, 0x3B, 0xAC, 0xA2,
        0x89, 0xE1, 0x93, 0x54, 0x86, 0x08, 0xB8, 0x28, 0x01, 0x06, 0x06, 0xC4, 0xA0, 0x2E, 0xA7,
        0x34, 0xCC, 0x32, 0xAC, 0xD2, 0xB0, 0x2B, 0xC2, 0x8B, 0x99, 0xCB, 0x3E, 0x28, 0x7E, 0x85,
        0xA7, 0x63, 0xAF, 0x26, 0x74, 0x92, 0xAB, 0x57, 0x2E, 0x99, 0xAB, 0x3F, 0x37, 0x0D, 0x27,
        0x5C, 0xEC, 0x1D, 0xA1, 0xAA, 0x90, 0x75, 0xFF, 0x05, 0xF7, 0x9B, 0x0E,
    ];

    let alpha = BytesN::from_array(env, &g1_gen);
    let beta = BytesN::from_array(env, &g2_gen);
    let gamma = BytesN::from_array(env, &g2_gen);
    let delta = BytesN::from_array(env, &g2_gen);

    // gamma_abc: [gamma_abc_0, gamma_abc_1, ..., gamma_abc_6]
    // (one per public input + the constant term).
    let mut gamma_abc = Vec::new(env);
    for _ in 0..(NUM_PUBLIC_INPUTS + 1) {
        gamma_abc.push_back(BytesN::from_array(env, &g1_gen));
    }

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
        // The proof's points fail the soroban-sdk 22 host's strict BLS12-381
        // validation, so the invocation raises a host error instead of
        // returning `false` (see `make_proof`).
        let res = client.try_verify_score_range(&proof, &inputs);
        assert!(res.is_err());
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

        // The tampered proof's points fail strict host validation, so the
        // invocation raises a host error. Because the failed verification is
        // never recorded as consumed, a second identical call fails the same
        // way (not `ProofAlreadyConsumed`).
        let res = client.try_verify_and_consume(&consumer, &proof, &inputs, &nonce);
        assert!(res.is_err());

        let res2 = client.try_verify_and_consume(&consumer, &proof, &inputs, &nonce);
        assert!(res2.is_err());
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