#![no_std]
//! Identity oracle contract for the Stellar DID Credit protocol.
//!
//! Manages trusted credential issuers, DID document anchoring, and
//! verifiable credential (VC) lifecycle — including anchoring, revocation,
//! and active-count queries used by the credit-oracle.
#[cfg(test)]
extern crate std;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, BytesN, Env,
    IntoVal, String, Symbol, Vec,
};

// ---------------------------------------------------------------------------
// Auth helper
// ---------------------------------------------------------------------------

/// Load the stored admin address and call `require_auth()` on it.
///
/// This is the single canonical admin-auth pattern used by every admin-gated
/// function in this contract:
///
/// 1. Read the `Admin` key from instance storage (panics if not yet
///    initialized, which should never happen in normal operation).
/// 2. Call `require_auth()` so Soroban validates the invoker's signature.
/// 3. Return the address so callers can compare it against the `admin`
///    parameter passed in by the caller.
///
/// All admin functions call this helper instead of duplicating the two-step
/// lookup + auth inline.
fn require_admin(env: &Env) -> Address {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("not initialized");
    admin.require_auth();
    admin
}

fn ensure_not_paused(env: &Env) -> Result<(), IdentityOracleError> {
    if env.storage().instance().get(&DataKey::Paused).unwrap_or(false) {
        Err(IdentityOracleError::ContractPaused)
    } else {
        Ok(())
    }
} // ── Persistent TTL constants ─────────────────────────────────────
  // Persistent entries are extended to ~30 days on every write.
  //
  // Threshold: if remaining TTL drops below this, extend.
  // Extend to: the new TTL value in ledger counts (≈5 s/ledger).
  //
const PERS_TTL_THRESHOLD: u32 = 120_960; // ~7 days
const PERS_TTL_EXTEND: u32 = 518_400; // ~30 days

/// Error types for the identity-oracle contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum IdentityOracleError {
    /// Contract is already initialized.
    AlreadyInitialized = 1,
    /// Caller is not authorized to perform this action.
    NotAuthorized = 2,
    /// Issuer is not registered as a trusted issuer.
    IssuerNotRegistered = 3,
    /// The provided CID is invalid.
    InvalidCID = 4,
    /// No pending admin proposal exists.
    NoPendingAdmin = 5,
    /// A VC with the same hash has already been anchored for this subject.
    DuplicateVC = 6,
    /// No matching VC record was found for the given hash/issuer.
    VCNotFound = 7,
    /// The contract is currently paused and cannot accept writes.
    ContractPaused = 8,
}

/// Storage key variants for the identity-oracle contract.
#[contracttype]
pub enum DataKey {
    /// The contract administrator address.
    Admin,
    /// Whether the contract is currently paused for writes.
    Paused,
    /// Pending contract admin address for two-step transfer.
    PendingAdmin,
    /// Append-only index of every address ever registered as a trusted
    /// issuer. Entries are never removed on deregistration (that would
    /// require an O(n) rewrite on every `deregister_issuer` call) — a
    /// deregistered issuer's entry is left in place and its `TrustedIssuer`
    /// flag is flipped to `false` instead. Use `list_issuers` (which filters
    /// this index against `TrustedIssuer`) to get the currently-active set.
    IssuersIndex,
    /// Whether the given address is a *currently* trusted credential issuer.
    /// Present and `true` while registered; present and `false` once
    /// deregistered (a tombstone, not removed) so re-registration can be
    /// told apart from first-time registration without rescanning
    /// `IssuersIndex`.
    TrustedIssuer(Address),
    /// The DID document hash anchored for the given subject address.
    DIDDocument(Address),
    /// The list of VC anchors associated with the given subject address.
    VCAnchors(Address),
    /// Cached count of active, non-revoked VC anchors for the subject.
    ///
    /// This counter is seeded lazily from `VCAnchors(Address)` for legacy
    /// subjects and then maintained incrementally on `anchor_vc` and
    /// `mark_vc_revoked`.
    ActiveVCCount(Address),
    /// The ID of the revocation registry contract.
    RevocationRegistryId,
    /// Issuer trust multiplier in basis points (100 = 1×). Defaults to 100 when unset.
    IssuerTier(Address),
    /// Credential type label for a subject's anchored VC hash.
    VCCredentialType(Address, BytesN<32>),
}

/// An on-chain anchor record for a verifiable credential.
#[contracttype]
#[derive(Clone)]
pub struct VCRecord {
    /// SHA-256 hash of the off-chain verifiable credential JSON.
    pub vc_hash: BytesN<32>,
    /// Address of the issuer who anchored this credential.
    pub issuer: Address,
    /// Ledger timestamp (Unix seconds) when this credential was anchored.
    pub anchored_at: u64,
    /// Whether this credential has been revoked by the issuer.
    pub revoked: bool,
}

const INSTANCE_BUMP_THRESHOLD: u32 = 5000;
const INSTANCE_BUMP_AMOUNT: u32 = 500_000;

/// Default issuer trust multiplier: 100 basis points (1×).
pub const DEFAULT_ISSUER_TIER_BPS: u32 = 100;
/// Maximum allowed issuer trust multiplier.
pub const MAX_ISSUER_TIER_BPS: u32 = 300;

fn generic_credential_type(_env: &Env) -> Symbol {
    symbol_short!("generic")
}

fn get_stored_credential_type(env: &Env, subject: &Address, vc_hash: &BytesN<32>) -> Symbol {
    env.storage()
        .persistent()
        .get(&DataKey::VCCredentialType(subject.clone(), vc_hash.clone()))
        .unwrap_or(generic_credential_type(env))
}

fn store_credential_type(
    env: &Env,
    subject: &Address,
    vc_hash: &BytesN<32>,
    credential_type: Symbol,
) {
    env.storage().persistent().set(
        &DataKey::VCCredentialType(subject.clone(), vc_hash.clone()),
        &credential_type,
    );
}

/// Returns true if `s` starts with `prefix` by comparing their leading bytes on the stack.
/// `prefix` must be ≤ 32 bytes.
fn cid_starts_with(_env: &Env, s: &String, prefix: &String) -> bool {
    let plen = prefix.len() as usize;
    if (s.len() as usize) < plen {
        return false;
    }
    let mut sbuf = [0u8; 64];
    let mut pbuf = [0u8; 32];
    s.copy_into_slice(&mut sbuf[..s.len() as usize]);
    prefix.copy_into_slice(&mut pbuf[..plen]);
    sbuf[..plen] == pbuf[..plen]
}

#[contract]
pub struct IdentityOracle;

fn is_record_revoked(env: &Env, record: &VCRecord) -> bool {
    if record.revoked {
        return true;
    }
    if let Some(registry_id) = env
        .storage()
        .instance()
        .get::<_, Address>(&DataKey::RevocationRegistryId)
    {
        let is_revoked: bool = env.invoke_contract(
            &registry_id,
            &soroban_sdk::Symbol::new(env, "is_revoked"),
            soroban_sdk::vec![env, record.vc_hash.into_val(env)],
        );
        if is_revoked {
            return true;
        }
    }
    false
}

fn compute_active_vc_count(env: &Env, subject: &Address) -> u32 {
    let key = DataKey::VCAnchors(subject.clone());
    let anchors: Vec<VCRecord> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or(Vec::new(env));

    let mut count: u32 = 0;
    for record in anchors.iter() {
        if !is_record_revoked(env, &record) {
            count += 1;
        }
    }
    count
}

fn seed_active_vc_count(env: &Env, subject: &Address) -> u32 {
    let count = compute_active_vc_count(env, subject);
    env.storage()
        .persistent()
        .set(&DataKey::ActiveVCCount(subject.clone()), &count);
    count
}

fn load_active_vc_count(env: &Env, subject: &Address) -> Option<u32> {
    env.storage()
        .persistent()
        .get(&DataKey::ActiveVCCount(subject.clone()))
}

#[contractimpl]
impl IdentityOracle {
    /// Initialize the contract with an administrator address.
    pub fn initialize(env: Env, admin: Address) -> Result<(), IdentityOracleError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(IdentityOracleError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        Ok(())
    }

    /// Pause all writes on the contract.
    pub fn pause(env: Env) -> Result<(), IdentityOracleError> {
        require_admin(&env);
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish((symbol_short!("Paused"),), ());
        Ok(())
    }

    /// Resume the contract and allow writes again.
    pub fn unpause(env: Env) -> Result<(), IdentityOracleError> {
        require_admin(&env);
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish((symbol_short!("Unpaused"),), ());
        Ok(())
    }

    /// Set the revocation registry contract ID used to check global revocations.
    ///
    /// When set, `is_verified`, `get_active_vc_count`, and `verify_vc` will
    /// additionally consult the registry before returning results.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn set_revocation_registry(
        env: Env,
        registry_id: Address,
    ) -> Result<(), IdentityOracleError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .instance()
            .set(&DataKey::RevocationRegistryId, &registry_id);

        env.invoke_contract::<()>(
            &registry_id,
            &soroban_sdk::Symbol::new(&env, "set_identity_oracle"),
            soroban_sdk::vec![&env, env.current_contract_address().into_val(&env)],
        );
        Ok(())
    }

    /// Register a trusted credential issuer authorized to anchor verifiable credentials.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn register_issuer(env: Env, issuer: Address) -> Result<(), IdentityOracleError> {
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        let issuer_key = DataKey::TrustedIssuer(issuer.clone());
        if !env.storage().persistent().has(&issuer_key) {
            let mut issuers: Vec<Address> = env
                .storage()
                .persistent()
                .get(&DataKey::IssuersIndex)
                .unwrap_or(Vec::new(&env));
            issuers.push_back(issuer.clone());
            env.storage()
                .persistent()
                .set(&DataKey::IssuersIndex, &issuers);
        }

        env.storage().persistent().set(&issuer_key, &true);
        env.events().publish((symbol_short!("IssReg"),), issuer);
        Ok(())
    }

    /// Deregister a trusted credential issuer, preventing future credential anchoring.
    ///
    /// Does NOT retroactively revoke existing VCs anchored by this issuer.
    ///
    /// This is a single tombstone write (`TrustedIssuer(issuer) = false`) —
    /// it does not touch `IssuersIndex`, so cost does not scale with the
    /// number of registered issuers. `list_issuers` is what hides
    /// deregistered issuers from the returned set.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn deregister_issuer(env: Env, issuer: Address) -> Result<(), IdentityOracleError> {
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        // Mark the issuer as not trusted (tombstone). We will rebuild the
        // compact `IssuersIndex` in-memory and write it once so that the
        // rewrite is atomic from the perspective of contract storage: either
        // the function completes and both the tombstone + new index are
        // written, or the call aborts and nothing is changed.
        env.storage()
            .persistent()
            .set(&DataKey::TrustedIssuer(issuer.clone()), &false);

        // Read the append-only index and construct a compacted vector of
        // currently-trusted issuers. Do all work in-memory and perform a
        // single `set` at the end so partial progress is never persisted.
        let ever_registered: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::IssuersIndex)
            .unwrap_or(Vec::new(&env));

        let mut compacted = Vec::new(&env);
        for addr in ever_registered.iter() {
            let is_trusted: bool = env
                .storage()
                .persistent()
                .get(&DataKey::TrustedIssuer(addr.clone()))
                .unwrap_or(false);
            if is_trusted {
                compacted.push_back(addr);
            }
        }

        // Write the compacted index once. If this call fails (e.g. out of
        // gas), the entire transaction will abort and the previous
        // `TrustedIssuer` tombstone write will be rolled back too.
        env.storage()
            .persistent()
            .set(&DataKey::IssuersIndex, &compacted);

        env.events().publish((symbol_short!("IssDeReg"),), issuer);
        Ok(())
    }

    /// Check whether a subject has already anchored a DID document.
    pub fn has_anchored_did(env: Env, subject: Address) -> bool {
        env.storage().persistent().has(&DataKey::DIDDocument(subject))
    }

    /// Anchor a DID document on-chain by storing its IPFS CID.
    ///
    /// **Authentication:** The `subject` must provide a valid signature.
    ///
    /// **Overwrite behavior:** This function is idempotent — calling it multiple times with
    /// different CIDs will silently replace the previous value in storage. Each call emits
    /// a `DIDAnch` event. DID documents are considered **mutable** in this protocol;
    /// subjects may update their DID document (e.g., to rotate keys or add service
    /// endpoints) by calling this function again. Consumers should always resolve the
    /// current CID from storage rather than relying on historical events.
    pub fn anchor_did(
        env: Env,
        subject: Address,
        did_doc_cid: String,
    ) -> Result<(), IdentityOracleError> {
        ensure_not_paused(&env)?;
        subject.require_auth();

        let len = did_doc_cid.len();
        if len < 7 {
            return Err(IdentityOracleError::InvalidCID);
        }

        // Accept "ipfs://", "bafy", or "Qm" prefixes
        let ipfs_prefix = String::from_str(&env, "ipfs://");
        let bafy_prefix = String::from_str(&env, "bafy");
        let qm_prefix = String::from_str(&env, "Qm");

        let valid = cid_starts_with(&env, &did_doc_cid, &ipfs_prefix)
            || cid_starts_with(&env, &did_doc_cid, &bafy_prefix)
            || cid_starts_with(&env, &did_doc_cid, &qm_prefix);

        if !valid {
            return Err(IdentityOracleError::InvalidCID);
        }

        env.storage()
            .persistent()
            .set(&DataKey::DIDDocument(subject.clone()), &did_doc_cid);
        env.storage().persistent().extend_ttl(
            &DataKey::DIDDocument(subject.clone()),
            PERS_TTL_THRESHOLD,
            PERS_TTL_EXTEND,
        );
        env.events()
            .publish((symbol_short!("DIDAnch"),), (subject, did_doc_cid));
        Ok(())
    }

    /// Returns the DID document CID anchored for the given subject, if any.
    pub fn get_did_document(env: Env, subject: Address) -> Option<String> {
        env.storage()
            .persistent()
            .get(&DataKey::DIDDocument(subject))
    }

    /// Deactivate the subject's DID.
    ///
    /// 1. Revokes all active verifiable credentials anchored for the subject.
    /// 2. Removes the anchored DID Document CID.
    /// 3. Emits a `DIDDeact` event.
    ///
    /// Auth: The `subject` must provide a valid signature.
    pub fn deactivate_did(env: Env, subject: Address) -> Result<(), IdentityOracleError> {
        subject.require_auth();

        // 1. Revoke all VCs
        let key = DataKey::VCAnchors(subject.clone());
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        if !anchors.is_empty() {
            let mut updated = Vec::new(&env);
            for mut record in anchors.iter() {
                record.revoked = true;
                updated.push_back(record);
            }
            env.storage().persistent().set(&key, &updated);
        }

        // 2. Remove DID Document
        env.storage().persistent().remove(&DataKey::DIDDocument(subject.clone()));

        // 3. Emit event
        env.events().publish((symbol_short!("DIDDeact"),), subject);

        Ok(())
    }

    /// Anchor a verifiable credential (VC) for a subject issued by a trusted issuer.
    pub fn anchor_vc(
        env: Env,
        issuer: Address,
        subject: Address,
        vc_hash: BytesN<32>,
    ) -> Result<(), IdentityOracleError> {
        let credential_type = generic_credential_type(&env);
        Self::anchor_vc_typed(env, issuer, subject, vc_hash, credential_type)
    }

    /// Anchor a VC with an explicit credential type label (e.g. `kyc`, `employment`).
    pub fn anchor_vc_typed(
        env: Env,
        issuer: Address,
        subject: Address,
        vc_hash: BytesN<32>,
        credential_type: Symbol,
    ) -> Result<(), IdentityOracleError> {
        ensure_not_paused(&env)?;
        issuer.require_auth();
        let is_trusted: bool = env
            .storage()
            .persistent()
            .get(&DataKey::TrustedIssuer(issuer.clone()))
            .unwrap_or(false);
        if !is_trusted {
            return Err(IdentityOracleError::IssuerNotRegistered);
        }

        let key = DataKey::VCAnchors(subject.clone());
        let mut anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        // Reject duplicate vc_hash for this subject
        for i in 0..anchors.len() {
            if anchors.get(i).unwrap().vc_hash == vc_hash {
                return Err(IdentityOracleError::DuplicateVC);
            }
        }

        let record = VCRecord {
            vc_hash: vc_hash.clone(),
            issuer: issuer.clone(),
            anchored_at: env.ledger().timestamp(),
            revoked: false,
        };
        let is_active = !is_record_revoked(&env, &record);

        store_credential_type(&env, &subject, &vc_hash, credential_type);

        anchors.push_back(record);
        env.storage().persistent().set(&key, &anchors);
        env.storage()
            .persistent()
            .extend_ttl(&key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);

        if let Some(mut active_count) = load_active_vc_count(&env, &subject) {
            if is_active {
                active_count = active_count
                    .checked_add(1)
                    .expect("active VC count overflow");
            }
            env.storage()
                .persistent()
                .set(&DataKey::ActiveVCCount(subject.clone()), &active_count);
        } else {
            seed_active_vc_count(&env, &subject);
        }

        env.events()
            .publish((symbol_short!("VCAnch"),), (issuer, subject, vc_hash));
        Ok(())
    }

    /// Mark a previously anchored VC as revoked by its issuer.
    pub fn mark_vc_revoked(
        env: Env,
        issuer: Address,
        subject: Address,
        vc_hash: BytesN<32>,
    ) -> Result<(), IdentityOracleError> {
        ensure_not_paused(&env)?;
        issuer.require_auth();
        let key = DataKey::VCAnchors(subject.clone());
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        let mut found = false;
        let mut transitioned_to_revoked = false;
        let mut updated = Vec::new(&env);
        for mut record in anchors.iter() {
            if record.vc_hash == vc_hash && record.issuer == issuer {
                if !record.revoked {
                    transitioned_to_revoked = true;
                }
                record.revoked = true;
                found = true;
            }
            updated.push_back(record);
        }

        if !found {
            return Err(IdentityOracleError::VCNotFound);
        }

        env.storage().persistent().set(&key, &updated);
        if transitioned_to_revoked {
            if let Some(mut active_count) = load_active_vc_count(&env, &subject) {
                active_count = active_count
                    .checked_sub(1)
                    .expect("active VC count underflow");
                env.storage()
                    .persistent()
                    .set(&DataKey::ActiveVCCount(subject.clone()), &active_count);
            } else {
                seed_active_vc_count(&env, &subject);
            }
        } else if load_active_vc_count(&env, &subject).is_none() {
            seed_active_vc_count(&env, &subject);
        }
        env.storage()
            .persistent()
            .extend_ttl(&key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        Ok(())
    }

    /// Check if a subject has at least one non-revoked verifiable credential anchored.
    pub fn is_verified(env: Env, subject: Address) -> bool {
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        for record in anchors.iter() {
            if !is_record_revoked(&env, &record) {
                return true;
            }
        }
        false
    }

    /// Returns the total number of anchored VC records for `subject`, including revoked entries.
    pub fn get_total_vc_count(env: Env, subject: Address) -> u32 {
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));
        anchors.len()
    }

    /// Returns the number of anchored VC records for `subject` that are **not revoked**.
    pub fn get_active_vc_count(env: Env, subject: Address) -> u32 {
        load_active_vc_count(&env, &subject).unwrap_or_else(|| seed_active_vc_count(&env, &subject))
    }

    /// Returns active (non-revoked) VC anchor records for `subject`.
    ///
    /// Revocations from both on-chain flags and the linked revocation registry
    /// are excluded. Use this for credit scoring and verification audits.
    pub fn get_vc_details(env: Env, subject: Address) -> Vec<VCRecord> {
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        let mut active = Vec::new(&env);
        for record in anchors.iter() {
            if !is_record_revoked(&env, &record) {
                active.push_back(record);
            }
        }
        active
    }

    /// Returns the credential type label for an anchored VC, defaulting to `generic`.
    pub fn get_vc_credential_type(env: Env, subject: Address, vc_hash: BytesN<32>) -> Symbol {
        get_stored_credential_type(&env, &subject, &vc_hash)
    }

    /// Set an issuer's trust multiplier in basis points (100 = 1×, 200 = 2×).
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn set_issuer_tier(
        env: Env,
        admin: Address,
        issuer: Address,
        weight_bps: u32,
    ) -> Result<(), IdentityOracleError> {
        ensure_not_paused(&env)?;
        let stored = require_admin(&env);
        if admin != stored {
            return Err(IdentityOracleError::NotAuthorized);
        }
        if weight_bps == 0 || weight_bps > MAX_ISSUER_TIER_BPS {
            panic!("invalid issuer tier");
        }
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .persistent()
            .set(&DataKey::IssuerTier(issuer.clone()), &weight_bps);
        env.events()
            .publish((symbol_short!("IssTier"),), (issuer, weight_bps));
        Ok(())
    }

    /// Returns the issuer trust multiplier in basis points (default 100).
    pub fn get_issuer_tier(env: Env, issuer: Address) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::IssuerTier(issuer))
            .unwrap_or(DEFAULT_ISSUER_TIER_BPS)
    }

    /// Backwards-compatible wrapper.
    ///
    /// **Deprecated:** This function includes revoked entries in its count.
    /// For credit scoring and verification, use `get_active_vc_count` instead,
    /// which excludes revoked credentials and provides accurate scores.
    #[deprecated(note = "use get_active_vc_count for accurate non-revoked VC counts")]
    pub fn get_vc_count(env: Env, subject: Address) -> u32 {
        Self::get_total_vc_count(env, subject)
    }

    /// Verify whether a subject has a matching active verifiable credential anchor.
    ///
    /// Parameters:
    /// - `env`: Soroban contract environment used to read persistent storage.
    /// - `subject`: Address whose anchored VC records are searched.
    /// - `vc_hash`: SHA-256 hash of the off-chain VC JSON to verify.
    ///
    /// Returns `true` when `subject` has an anchored VC record with `vc_hash`
    /// that has not been revoked, and `false` when no matching active record
    /// exists. This function is read-only and does not require authentication.
    pub fn verify_vc(env: Env, subject: Address, vc_hash: BytesN<32>) -> bool {
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        for record in anchors.iter() {
            if record.vc_hash == vc_hash && !is_record_revoked(&env, &record) {
                return true;
            }
        }
        false
    }

    /// Propose a new contract admin (step 1 of two-step admin transfer).
    ///
    /// Stores `new_admin` under `DataKey::PendingAdmin` in instance storage.
    /// The transfer only completes once `new_admin` calls `accept_admin`.
    ///
    /// Auth: current admin only — verified via `require_admin`.
    pub fn propose_new_admin(env: Env, new_admin: Address) -> Result<(), IdentityOracleError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .instance()
            .set(&DataKey::PendingAdmin, &new_admin);
        Ok(())
    }

    /// Accept a pending admin proposal (step 2 of two-step admin transfer).
    ///
    /// Reads `DataKey::PendingAdmin` from instance storage and verifies that
    /// `new_admin` matches, then promotes `new_admin` to `DataKey::Admin` and
    /// clears the pending entry.
    ///
    /// Auth: the proposed `new_admin` address must sign the transaction.
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), IdentityOracleError> {
        ensure_not_paused(&env)?;
        let pending: Option<Address> = env.storage().instance().get(&DataKey::PendingAdmin);
        match pending {
            Some(p) => {
                if p != new_admin {
                    return Err(IdentityOracleError::NotAuthorized);
                }
            }
            None => return Err(IdentityOracleError::NoPendingAdmin),
        }
        new_admin.require_auth();
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        Ok(())
    }

    /// Upgrade the contract WASM in-place, preserving address and all stored state.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), IdentityOracleError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    /// Admin-only maintenance: extend instance storage TTL so critical
    /// configuration (Admin, RevocationRegistryId) does not expire on
    /// an idle contract.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn maintain_storage(env: Env) -> Result<(), IdentityOracleError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        Ok(())
    }

    /// Returns the currently registered (non-deregistered) trusted issuers.
    ///
    /// `IssuersIndex` is append-only and may contain deregistered addresses,
    /// so this filters it against each entry's live `TrustedIssuer` flag.
    pub fn list_issuers(env: Env) -> Vec<Address> {
        let ever_registered: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::IssuersIndex)
            .unwrap_or(Vec::new(&env));

        let mut active = Vec::new(&env);
        for issuer in ever_registered.iter() {
            let is_trusted: bool = env
                .storage()
                .persistent()
                .get(&DataKey::TrustedIssuer(issuer.clone()))
                .unwrap_or(false);
            if is_trusted {
                active.push_back(issuer);
            }
        }
        active
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    #[test]
    fn test_deactivate_did_removes_did_and_revokes_vcs() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDID");
        client.anchor_did(&subject, &cid);

        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);

        assert!(client.is_verified(&subject));
        assert!(client.get_did_document(&subject).is_some());

        client.deactivate_did(&subject);

        assert!(!client.is_verified(&subject));
        assert!(client.get_did_document(&subject).is_none());
    }


    #[test]
    fn test_anchor_vc_by_trusted_issuer() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);
    }

    #[test]
    fn test_unregistered_issuer_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        let result = client.try_anchor_vc(&issuer, &subject, &vc_hash);
        assert_eq!(result, Err(Ok(IdentityOracleError::IssuerNotRegistered)));
    }

    #[test]
    fn test_deregister_issuer_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);
        client.deregister_issuer(&issuer);

        // Deregistration tombstones the flag (sets it false) rather than
        // removing the key, so `deregister_issuer` never has to rewrite
        // IssuersIndex.
        let is_trusted: bool = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::TrustedIssuer(issuer.clone()))
                .unwrap_or(true)
        });
        assert!(!is_trusted);
    }

    #[test]
    fn test_deregistered_issuer_cannot_anchor_vc() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);

        client.deregister_issuer(&issuer);

        let vc_hash2 = BytesN::from_array(&env, &[2u8; 32]);
        let result = client.try_anchor_vc(&issuer, &subject, &vc_hash2);
        assert_eq!(result, Err(Ok(IdentityOracleError::IssuerNotRegistered)));
    }

    #[test]
    fn test_list_issuers_reflects_register_and_deregister_operations() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer1 = Address::generate(&env);
        let issuer2 = Address::generate(&env);

        assert_eq!(client.list_issuers(), Vec::new(&env));

        client.register_issuer(&issuer1);
        assert_eq!(
            client.list_issuers(),
            Vec::from_array(&env, [issuer1.clone()])
        );

        client.register_issuer(&issuer2);
        assert_eq!(
            client.list_issuers(),
            Vec::from_array(&env, [issuer1.clone(), issuer2.clone()])
        );

        client.register_issuer(&issuer1);
        assert_eq!(
            client.list_issuers(),
            Vec::from_array(&env, [issuer1.clone(), issuer2.clone()])
        );

        client.deregister_issuer(&issuer1);
        assert_eq!(client.list_issuers(), Vec::from_array(&env, [issuer2]));
    }

    #[test]
    fn test_reregistering_deregistered_issuer_does_not_duplicate_index() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);
        client.deregister_issuer(&issuer);
        client.register_issuer(&issuer);

        // list_issuers must show the issuer exactly once even though it went
        // through register -> deregister -> register.
        assert_eq!(
            client.list_issuers(),
            Vec::from_array(&env, [issuer.clone()])
        );

        // And it must be able to anchor VCs again now that it's re-trusted.
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[3u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);
    }

    #[test]
    fn test_is_verified_true_after_vc_anchored() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        assert!(!client.is_verified(&subject));

        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);

        assert!(client.is_verified(&subject));
    }

    #[test]
    fn test_anchor_did_stores_cid() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://Qm...");
        client.anchor_did(&subject, &cid);
    }

    #[test]
    fn test_anchor_did_rejects_empty_cid() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "");
        let result = client.try_anchor_did(&subject, &cid);
        assert_eq!(result, Err(Ok(IdentityOracleError::InvalidCID)));
    }

    #[test]
    fn test_anchor_did_rejects_single_space_cid() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, " ");
        let result = client.try_anchor_did(&subject, &cid);
        assert_eq!(result, Err(Ok(IdentityOracleError::InvalidCID)));
    }

    #[test]
    fn test_anchor_did_rejects_invalid_prefix() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "invalid-cid-data");
        let result = client.try_anchor_did(&subject, &cid);
        assert_eq!(result, Err(Ok(IdentityOracleError::InvalidCID)));
    }

    #[test]
    fn test_anchor_did_accepts_valid_ipfs_cid() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);
        let cid = String::from_str(
            &env,
            "ipfs://QmYwAPJzagoJzrKSTTkG8w6zWZSNxrCYhpDkxQottEwHym",
        );
        client.anchor_did(&subject, &cid);

        let subject2 = Address::generate(&env);
        let cid2 = String::from_str(
            &env,
            "bafy2bzacedw4hc6k2vxtcmfmr3jtcl6yvqohqmvtqj7lhyzuejcxgxvl6yv4",
        );
        client.anchor_did(&subject2, &cid2);

        let subject3 = Address::generate(&env);
        let cid3 = String::from_str(&env, "QmVocdeKSNbd9jkc3pDjq9FdAVLpiHrfQFwcJMgB7aXZi3");
        client.anchor_did(&subject3, &cid3);
    }

    #[test]
    fn test_anchor_did_overwrite() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);
        let cid_first = String::from_str(&env, "ipfs://QmFirstCID123456789");
        client.anchor_did(&subject, &cid_first);

        // Second call with different CID overwrites the first
        let cid_second = String::from_str(&env, "ipfs://QmSecondCID987654321");
        client.anchor_did(&subject, &cid_second);

        // Verify storage contains the second CID
        let stored: String = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::DIDDocument(subject.clone()))
                .unwrap()
        });
        assert_eq!(stored, cid_second);
    }

    #[test]
    fn test_vc_count_increments_correctly() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        assert_eq!(client.get_vc_count(&subject), 0);

        for i in 0..3u8 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = i;
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            client.anchor_vc(&issuer, &subject, &vc_hash);
        }

        assert_eq!(client.get_vc_count(&subject), 3);
    }

    #[test]
    fn test_get_active_vc_count_excludes_revoked() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);

        for i in 0..3u8 {
            let hash_arr = [i; 32];
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            client.anchor_vc(&issuer, &subject, &vc_hash);
        }

        for i in 0..2u8 {
            let hash_arr = [i; 32];
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            client.mark_vc_revoked(&issuer, &subject, &vc_hash);
        }

        assert_eq!(client.get_active_vc_count(&subject), 1);
    }

    #[test]
    fn test_get_active_vc_count_cached_cost_stays_flat() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let mut costs = Vec::new(&env);
        for vc_total in [5u32, 10u32, 20u32] {
            let subject = Address::generate(&env);
            for i in 0..vc_total {
                let mut hash_arr = [0u8; 32];
                hash_arr[0] = i as u8;
                let vc_hash = BytesN::from_array(&env, &hash_arr);
                client.anchor_vc(&issuer, &subject, &vc_hash);
            }

            let count = client.get_active_vc_count(&subject);
            assert_eq!(count, vc_total);

            costs.push_back(env.cost_estimate().budget().cpu_instruction_cost());
        }

        let cost_5 = costs.get(0).unwrap();
        let cost_10 = costs.get(1).unwrap();
        let cost_20 = costs.get(2).unwrap();

        std::println!(
            "get_active_vc_count cached cpu instructions: 5 VCs = {}, 10 VCs = {}, 20 VCs = {}",
            cost_5,
            cost_10,
            cost_20
        );

        let max_cost = core::cmp::max(core::cmp::max(cost_5, cost_10), cost_20);
        let min_cost = core::cmp::min(core::cmp::min(cost_5, cost_10), cost_20);
        assert!(
            max_cost - min_cost <= 25_000,
            "expected cached get_active_vc_count costs to stay roughly flat, got 5={} 10={} 20={}",
            cost_5,
            cost_10,
            cost_20
        );

        const MAINNET_CPU_LIMIT: u64 = 600_000_000;
        assert!(
            max_cost < MAINNET_CPU_LIMIT,
            "expected cached get_active_vc_count to stay under the mainnet CPU limit"
        );
    }

    #[test]
    fn test_mark_vc_revoked_panics_for_unknown_hash() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        let known_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &known_hash);

        let unknown_hash = BytesN::from_array(&env, &[2u8; 32]);
        let res = client.try_mark_vc_revoked(&issuer, &subject, &unknown_hash);
        assert_eq!(res, Err(Ok(IdentityOracleError::VCNotFound)));
    }

    #[test]
    fn test_revoked_vc_fails_is_verified() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);

        assert!(client.is_verified(&subject));

        client.mark_vc_revoked(&issuer, &subject, &vc_hash);

        assert!(!client.is_verified(&subject));
    }

    #[test]
    fn test_upgrade_rejects_without_admin_auth() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Withdraw the blanket auth mock: with an empty auth list, admin's
        // require_auth() inside require_admin() has nothing authorizing the
        // invocation, so it fails before the call ever reaches
        // deployer().update_current_contract_wasm() (which would separately
        // fail on an unregistered hash regardless of auth — that's not what
        // this test is checking).
        env.mock_auths(&[]);
        let res = client.try_upgrade(&BytesN::from_array(&env, &[0u8; 32]));
        assert!(res.is_err());
    }

    #[test]
    fn test_initialize_sets_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let stored: Address = env.as_contract(&contract_id, || {
            env.storage().instance().get(&DataKey::Admin).unwrap()
        });
        assert_eq!(stored, admin);
    }

    #[test]
    fn test_admin_transfer_two_step() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let issuer = Address::generate(&env);

        client.initialize(&admin1);

        // propose new admin
        client.propose_new_admin(&admin2);

        // accept by proposed admin
        client.accept_admin(&admin2);

        // new admin can register issuer
        client.register_issuer(&issuer);
    }

    #[test]
    fn test_non_pending_admin_cannot_accept() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let non_admin = Address::generate(&env);

        client.initialize(&admin1);
        client.propose_new_admin(&admin2);

        // non_admin tries to accept
        let res = client.try_accept_admin(&non_admin);
        assert_eq!(res, Err(Ok(IdentityOracleError::NotAuthorized)));
    }
    #[test]
    fn test_maintain_storage_succeeds_for_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Admin can call maintain_storage without error
        let res = client.try_maintain_storage();
        assert!(res.is_ok());
    }

    #[test]
    fn test_maintain_storage_fails_for_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Withdraw blanket auth so require_admin fails
        env.mock_auths(&[]);
        let res = client.try_maintain_storage();
        assert!(res.is_err());
    }
}
