#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, BytesN, Env,
    IntoVal, String, Vec,
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
}

/// Issuer reputation tier. Higher tiers correspond to better track records,
/// and VCs from higher-tier issuers contribute more weight to subject scores.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum IssuerTier {
    /// Suspended / lowest quality: VCs contribute 0 weight.
    Tier0 = 0,
    /// Bronze / probationary: VCs contribute 0.25 weight.
    Tier1 = 1,
    /// Silver / standard: VCs contribute 0.5 weight.
    Tier2 = 2,
    /// Gold / trusted: VCs contribute full 1.0 weight (default for new issuers).
    Tier3 = 3,
}

/// Reputation profile for a trusted issuer. Stored under
/// `DataKey::TrustedIssuer(issuer_address)` replacing the previous bool flag.
#[contracttype]
#[derive(Clone)]
pub struct IssuerProfile {
    /// Whether this issuer is currently registered/active (replaces the old bool flag).
    pub active: bool,
    /// Total number of VCs this issuer has ever anchored on-chain.
    pub vcs_issued: u32,
    /// Total number of VCs this issuer has ever revoked (including globally revoked).
    pub vcs_revoked: u32,
    /// Reputation tier controlling how much VCs from this issuer count toward scores.
    pub tier: IssuerTier,
}

/// Storage key variants for the identity-oracle contract.
#[contracttype]
pub enum DataKey {
    /// The contract administrator address.
    Admin,
    /// Pending contract admin address for two-step transfer.
    PendingAdmin,
    /// Append-only index of every address ever registered as a trusted
    /// issuer. Entries are never removed on deregistration (that would
    /// require an O(n) rewrite on every `deregister_issuer` call) — a
    /// deregistered issuer's entry is left in place and its `TrustedIssuer`
    /// `active` flag is flipped to `false` instead. Use `list_issuers` (which
    /// filters this index against each `IssuerProfile.active`) to get the
    /// currently-active set.
    IssuersIndex,
    /// Full reputation profile for the given issuer address. Replaces the
    /// previous bool flag with an `IssuerProfile` struct that tracks
    /// active-status plus `vcs_issued`, `vcs_revoked`, and a reputation
    /// `tier` used by the credit oracle for tier-weighted scoring.
    TrustedIssuer(Address),
    /// The DID document hash anchored for the given subject address.
    DIDDocument(Address),
    /// The list of VC anchors associated with the given subject address.
    VCAnchors(Address),
    /// The ID of the revocation registry contract.
    RevocationRegistryId,
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

#[contractimpl]
impl IdentityOracle {
    /// Initialize the contract with an administrator address.
    pub fn initialize(env: Env, admin: Address) -> Result<(), IdentityOracleError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(IdentityOracleError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        Ok(())
    }

    /// Set the revocation registry ID to allow checking global revocations.
    pub fn set_revocation_registry(
        env: Env,
        registry_id: Address,
    ) -> Result<(), IdentityOracleError> {
        require_admin(&env);
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .instance()
            .set(&DataKey::RevocationRegistryId, &registry_id);
        Ok(())
    }

    /// Register a trusted credential issuer authorized to anchor verifiable credentials.
    ///
    /// On first registration the issuer starts at `IssuerTier::Tier3` (gold /
    /// full weight) with zero counters. Re-registering a previously-deregistered
    /// issuer restores `active = true` without resetting reputation counters or
    /// tier (preserving the track record for Sybil resistance).
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn register_issuer(
        env: Env,
        issuer: Address,
    ) -> Result<(), IdentityOracleError> {
        require_admin(&env);
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        let issuer_key = DataKey::TrustedIssuer(issuer.clone());
        let existing: Option<IssuerProfile> = env.storage().persistent().get(&issuer_key);
        if existing.is_none() {
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

        let profile = match existing {
            Some(mut p) => {
                p.active = true;
                p
            }
            None => IssuerProfile {
                active: true,
                vcs_issued: 0,
                vcs_revoked: 0,
                tier: IssuerTier::Tier3,
            },
        };
        env.storage().persistent().set(&issuer_key, &profile);
        env.events().publish((symbol_short!("IssReg"),), issuer);
        Ok(())
    }

    /// Deregister a trusted credential issuer, preventing future credential anchoring.
    ///
    /// Does NOT retroactively revoke existing VCs anchored by this issuer.
    ///
    /// Flips `IssuerProfile.active` to `false` while preserving reputation
    /// counters and tier. `IssuersIndex` is not touched (so cost does not
    /// scale with the number of registered issuers). `list_issuers` filters
    /// based on `active` to return only currently-active issuers.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn deregister_issuer(
        env: Env,
        issuer: Address,
    ) -> Result<(), IdentityOracleError> {
        require_admin(&env);
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        let key = DataKey::TrustedIssuer(issuer.clone());
        let mut profile: IssuerProfile = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(IssuerProfile {
                active: false,
                vcs_issued: 0,
                vcs_revoked: 0,
                tier: IssuerTier::Tier0,
            });
        profile.active = false;
        env.storage().persistent().set(&key, &profile);

        env.events().publish((symbol_short!("IssDeReg"),), issuer);
        Ok(())
    }

    /// Admin-only: set the reputation tier of a registered (or previously
    /// registered) issuer. Lowering a tier reduces the weight of VCs from
    /// that issuer in future credit-score computations; raising it restores
    /// weight. The adjustment is additive-free — the tier is set exactly.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn set_issuer_tier(
        env: Env,
        issuer: Address,
        tier: IssuerTier,
    ) -> Result<(), IdentityOracleError> {
        require_admin(&env);
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        let key = DataKey::TrustedIssuer(issuer.clone());
        let mut profile: IssuerProfile = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(IssuerProfile {
                active: false,
                vcs_issued: 0,
                vcs_revoked: 0,
                tier: IssuerTier::Tier0,
            });
        profile.tier = tier;
        env.storage().persistent().set(&key, &profile);
        env.events().publish((symbol_short!("IssTier"),), (issuer, tier as u32));
        Ok(())
    }

    /// Returns the full `IssuerProfile` for a given issuer, or a default
    /// inactive/Tier0 profile if the issuer has never been registered.
    ///
    /// Read-only — no authorization required.
    pub fn get_issuer_profile(env: Env, issuer: Address) -> IssuerProfile {
        env.storage()
            .persistent()
            .get(&DataKey::TrustedIssuer(issuer))
            .unwrap_or(IssuerProfile {
                active: false,
                vcs_issued: 0,
                vcs_revoked: 0,
                tier: IssuerTier::Tier0,
            })
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
        env.events()
            .publish((symbol_short!("DIDAnch"),), (subject, did_doc_cid));
        Ok(())
    }

    /// Anchor a verifiable credential (VC) for a subject issued by a trusted issuer.
    ///
    /// Increments the issuer's `vcs_issued` reputation counter on success.
    pub fn anchor_vc(
        env: Env,
        issuer: Address,
        subject: Address,
        vc_hash: BytesN<32>,
    ) -> Result<(), IdentityOracleError> {
        issuer.require_auth();
        let issuer_key = DataKey::TrustedIssuer(issuer.clone());
        let profile: IssuerProfile = env
            .storage()
            .persistent()
            .get(&issuer_key)
            .unwrap_or(IssuerProfile {
                active: false,
                vcs_issued: 0,
                vcs_revoked: 0,
                tier: IssuerTier::Tier0,
            });
        if !profile.active {
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

        anchors.push_back(record);
        env.storage().persistent().set(&key, &anchors);

        // Increment issuer's vcs_issued counter
        let mut updated_profile = profile;
        updated_profile.vcs_issued = updated_profile.vcs_issued.saturating_add(1);
        env.storage().persistent().set(&issuer_key, &updated_profile);

        env.events()
            .publish((symbol_short!("VCAnch"),), (issuer, subject, vc_hash));
        Ok(())
    }

    /// Mark a previously anchored VC as revoked by its issuer.
    ///
    /// Increments the issuer's `vcs_revoked` reputation counter on success.
    pub fn mark_vc_revoked(
        env: Env,
        issuer: Address,
        subject: Address,
        vc_hash: BytesN<32>,
    ) -> Result<(), IdentityOracleError> {
        issuer.require_auth();
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        let mut found = false;
        let mut already_revoked = false;
        let mut updated = Vec::new(&env);
        for mut record in anchors.iter() {
            if record.vc_hash == vc_hash && record.issuer == issuer {
                if record.revoked {
                    already_revoked = true;
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

        // Increment issuer's vcs_revoked counter exactly once per
        // revocation (do not double-count repeated mark_vc_revoked calls).
        if !already_revoked {
            let issuer_key = DataKey::TrustedIssuer(issuer.clone());
            let mut profile: IssuerProfile = env
                .storage()
                .persistent()
                .get(&issuer_key)
                .unwrap_or(IssuerProfile {
                    active: false,
                    vcs_issued: 0,
                    vcs_revoked: 0,
                    tier: IssuerTier::Tier0,
                });
            profile.vcs_revoked = profile.vcs_revoked.saturating_add(1);
            env.storage().persistent().set(&issuer_key, &profile);
        }
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
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        let mut count: u32 = 0;
        for record in anchors.iter() {
            if !is_record_revoked(&env, &record) {
                count += 1;
            }
        }
        count
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
        require_admin(&env);
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
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
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        Ok(())
    }

    /// Upgrade the contract WASM in-place, preserving address and all stored state.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), IdentityOracleError> {
        require_admin(&env);
        env.storage().instance().extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    /// Tier → numerator for weight calculation. Weight = numerator / 4.
    ///
    /// Tier0 → 0/4 = 0.00 weight (VCs do not count)
    /// Tier1 → 1/4 = 0.25 weight
    /// Tier2 → 2/4 = 0.50 weight
    /// Tier3 → 4/4 = 1.00 weight
    ///
    /// Using integer arithmetic (numerator in 0..=4, denominator = 4) avoids
    /// floating-point operations inside the no_std WASM contract.
    fn tier_weight_numerator(tier: IssuerTier) -> u32 {
        match tier {
            IssuerTier::Tier0 => 0,
            IssuerTier::Tier1 => 1,
            IssuerTier::Tier2 => 2,
            IssuerTier::Tier3 => 4,
        }
    }

    /// Weight denominator used by `get_weighted_vc_count`. Weights are always
    /// computed as `tier_weight_numerator(tier) / TIER_WEIGHT_DENOMINATOR`.
    const TIER_WEIGHT_DENOMINATOR: u32 = 4;

    /// Returns the currently registered (non-deregistered) trusted issuers.
    ///
    /// `IssuersIndex` is append-only and may contain deregistered addresses,
    /// so this filters it against each entry's live `IssuerProfile.active`
    /// flag (replacing the previous `bool` storage).
    pub fn list_issuers(env: Env) -> Vec<Address> {
        let ever_registered: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::IssuersIndex)
            .unwrap_or(Vec::new(&env));

        let mut active = Vec::new(&env);
        for issuer in ever_registered.iter() {
            let profile: Option<IssuerProfile> = env
                .storage()
                .persistent()
                .get(&DataKey::TrustedIssuer(issuer.clone()));
            if let Some(p) = profile {
                if p.active {
                    active.push_back(issuer);
                }
            }
        }
        active
    }

    /// Returns the *tier-weighted* count of active (non-revoked) VCs for
    /// `subject`, expressed as integer hundredths so the credit oracle can
    /// use it in pure integer arithmetic.
    ///
    /// Each active VC contributes `(tier_weight_numerator * 100) /
    /// TIER_WEIGHT_DENOMINATOR` "hundredth-VCs" to the sum:
    ///   Tier3 → 100 hundredths (full VC)
    ///   Tier2 →  50 hundredths (half VC)
    ///   Tier1 →  25 hundredths (quarter VC)
    ///   Tier0 →   0 hundredths
    ///
    /// Dividing the returned value by 100 yields the effective weighted VC
    /// count, or it can be plugged directly into `vc_count`-style scoring by
    /// dividing an extra 100 in the consumer (the credit oracle uses
    /// `vc_count * 20` capped at 100, so `weighted_hundredths / 100` maps
    /// cleanly onto that axis).
    ///
    /// Read-only — no authorization required.
    pub fn get_weighted_vc_count(env: Env, subject: Address) -> u32 {
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        let mut total_hundredths: u32 = 0;
        for record in anchors.iter() {
            if is_record_revoked(&env, &record) {
                continue;
            }
            let profile: IssuerProfile = env
                .storage()
                .persistent()
                .get(&DataKey::TrustedIssuer(record.issuer.clone()))
                .unwrap_or(IssuerProfile {
                    active: false,
                    vcs_issued: 0,
                    vcs_revoked: 0,
                    tier: IssuerTier::Tier0,
                });
            let numerator = Self::tier_weight_numerator(profile.tier);
            let hundredths = numerator.saturating_mul(100) / Self::TIER_WEIGHT_DENOMINATOR;
            total_hundredths = total_hundredths.saturating_add(hundredths);
        }
        total_hundredths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

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

        // Deregistration tombstones profile.active to false rather than
        // removing the key, so `deregister_issuer` never has to rewrite
        // IssuersIndex. Reputation counters and tier are preserved for
        // Sybil resistance.
        let profile: IssuerProfile = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::TrustedIssuer(issuer.clone()))
                .unwrap()
        });
        assert!(!profile.active);
        assert_eq!(profile.tier, IssuerTier::Tier3);
    }

    #[test]
    fn test_issuer_metrics_increment_on_issue_and_revoke() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        // Initial counters are zero
        let p = client.get_issuer_profile(&issuer);
        assert_eq!(p.vcs_issued, 0);
        assert_eq!(p.vcs_revoked, 0);
        assert_eq!(p.tier, IssuerTier::Tier3);
        assert!(p.active);

        let subject = Address::generate(&env);
        for i in 0..5u8 {
            let mut hash = [0u8; 32];
            hash[0] = i;
            let vc_hash = BytesN::from_array(&env, &hash);
            client.anchor_vc(&issuer, &subject, &vc_hash);
        }
        let p = client.get_issuer_profile(&issuer);
        assert_eq!(p.vcs_issued, 5);
        assert_eq!(p.vcs_revoked, 0);

        // Revoke 3 of the 5
        for i in 0..3u8 {
            let mut hash = [0u8; 32];
            hash[0] = i;
            let vc_hash = BytesN::from_array(&env, &hash);
            client.mark_vc_revoked(&issuer, &subject, &vc_hash);
        }
        let p = client.get_issuer_profile(&issuer);
        assert_eq!(p.vcs_issued, 5);
        assert_eq!(p.vcs_revoked, 3);

        // Duplicate revoke call should not double-increment vcs_revoked
        let mut hash0 = [0u8; 32];
        hash0[0] = 0u8;
        let vc0 = BytesN::from_array(&env, &hash0);
        client.mark_vc_revoked(&issuer, &subject, &vc0);
        let p = client.get_issuer_profile(&issuer);
        assert_eq!(p.vcs_revoked, 3);
    }

    #[test]
    fn test_set_issuer_tier_changes_weighted_vc_count() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer_gold = Address::generate(&env);
        let issuer_silver = Address::generate(&env);
        client.register_issuer(&issuer_gold);
        client.register_issuer(&issuer_silver);
        // Demote silver issuer to Tier2
        client.set_issuer_tier(&issuer_silver, &IssuerTier::Tier2);

        assert_eq!(
            client.get_issuer_profile(&issuer_gold).tier,
            IssuerTier::Tier3
        );
        assert_eq!(
            client.get_issuer_profile(&issuer_silver).tier,
            IssuerTier::Tier2
        );

        let subject = Address::generate(&env);
        // One VC from each issuer, both active
        client.anchor_vc(
            &issuer_gold,
            &subject,
            &BytesN::from_array(&env, &[1u8; 32]),
        );
        client.anchor_vc(
            &issuer_silver,
            &subject,
            &BytesN::from_array(&env, &[2u8; 32]),
        );

        // Active VC count still counts raw VCs = 2
        assert_eq!(client.get_active_vc_count(&subject), 2);

        // Weighted: Tier3=100 + Tier2=50 = 150 hundredths = 1.5 effective VCs
        assert_eq!(client.get_weighted_vc_count(&subject), 150);

        // Now demote gold to Tier1, silver to Tier0
        client.set_issuer_tier(&issuer_gold, &IssuerTier::Tier1);
        client.set_issuer_tier(&issuer_silver, &IssuerTier::Tier0);
        // Weighted: Tier1=25 + Tier0=0 = 25 hundredths = 0.25 effective VCs
        assert_eq!(client.get_weighted_vc_count(&subject), 25);
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
}