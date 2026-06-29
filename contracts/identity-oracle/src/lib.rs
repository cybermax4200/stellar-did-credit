#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, symbol_short, Address, BytesN, Env, String, Vec, IntoVal};

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
}


    /// Storage key variants for the identity-oracle contract.
#[contracttype]
pub enum DataKey {
    /// The contract administrator address.
    Admin,
    /// Pending contract admin address for two-step transfer.
    PendingAdmin,
    /// Global index of currently registered trusted issuers.
    IssuersIndex,
    /// Whether the given address is a trusted credential issuer.
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
    if let Some(registry_id) = env.storage().instance().get::<_, Address>(&DataKey::RevocationRegistryId) {
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
        Ok(())
    }

    /// Set the revocation registry ID to allow checking global revocations.
    pub fn set_revocation_registry(env: Env, admin: Address, registry_id: Address) -> Result<(), IdentityOracleError> {
        if admin != require_admin(&env) {
            return Err(IdentityOracleError::NotAuthorized);
        }
        env.storage().instance().set(&DataKey::RevocationRegistryId, &registry_id);
        Ok(())
    }

    /// Register a trusted credential issuer authorized to anchor verifiable credentials.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn register_issuer(env: Env, admin: Address, issuer: Address) -> Result<(), IdentityOracleError> {
        let stored = require_admin(&env);
        if admin != stored {
            return Err(IdentityOracleError::NotAuthorized);
        }

        let issuer_key = DataKey::TrustedIssuer(issuer.clone());
        if !env.storage().persistent().has(&issuer_key) {
            let mut issuers: Vec<Address> = env
                .storage()
                .persistent()
                .get(&DataKey::IssuersIndex)
                .unwrap_or(Vec::new(&env));
            issuers.push_back(issuer.clone());
            env.storage().persistent().set(&DataKey::IssuersIndex, &issuers);
        }

        env.storage().persistent().set(&issuer_key, &true);
        env.events()
            .publish((symbol_short!("IssReg"),), issuer);
        Ok(())
    }

    /// Deregister a trusted credential issuer, preventing future credential anchoring.
    ///
    /// Does NOT retroactively revoke existing VCs anchored by this issuer.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn deregister_issuer(env: Env, admin: Address, issuer: Address) -> Result<(), IdentityOracleError> {
        let stored = require_admin(&env);
        if admin != stored {
            return Err(IdentityOracleError::NotAuthorized);
        }

        env.storage().persistent().remove(&DataKey::TrustedIssuer(issuer.clone()));

        let issuers: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::IssuersIndex)
            .unwrap_or(Vec::new(&env));
        let mut updated = Vec::new(&env);
        for registered_issuer in issuers.iter() {
            if registered_issuer != issuer {
                updated.push_back(registered_issuer);
            }
        }
        env.storage().persistent().set(&DataKey::IssuersIndex, &updated);

        env.events()
            .publish((symbol_short!("IssDeReg"),), issuer);
        Ok(())
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
    pub fn anchor_did(env: Env, subject: Address, did_doc_cid: String) -> Result<(), IdentityOracleError> {
        subject.require_auth();

        let len = did_doc_cid.len();
        if len < 7 {
            return Err(IdentityOracleError::InvalidCID);
        }

        // Accept "ipfs://", "bafy", or "Qm" prefixes
        let ipfs_prefix = String::from_str(&env, "ipfs://");
        let bafy_prefix = String::from_str(&env, "bafy");
        let qm_prefix   = String::from_str(&env, "Qm");

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
    pub fn anchor_vc(
        env: Env,
        issuer: Address,
        subject: Address,
        vc_hash: BytesN<32>,
    ) -> Result<(), IdentityOracleError> {
        issuer.require_auth();
        if !env
            .storage()
            .persistent()
            .has(&DataKey::TrustedIssuer(issuer.clone()))
        {
            return Err(IdentityOracleError::IssuerNotRegistered);
        }

        let key = DataKey::VCAnchors(subject.clone());
        let mut anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        let record = VCRecord {
            vc_hash: vc_hash.clone(),
            issuer: issuer.clone(),
            anchored_at: env.ledger().timestamp(),
            revoked: false,
        };

        anchors.push_back(record);
        env.storage().persistent().set(&key, &anchors);

        env.events()
            .publish((symbol_short!("VCAnch"),), (issuer, subject, vc_hash));
        Ok(())
    }

    /// Mark a previously anchored VC as revoked by its issuer.
    pub fn mark_vc_revoked(env: Env, issuer: Address, subject: Address, vc_hash: BytesN<32>) -> Result<(), IdentityOracleError> {
        issuer.require_auth();
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        let mut found = false;
        let mut updated = Vec::new(&env);
        for mut record in anchors.iter() {
            if record.vc_hash == vc_hash && record.issuer == issuer {
                record.revoked = true;
                found = true;
            }
            updated.push_back(record);
        }

        if !found {
            panic!("vc not found");
        }

        env.storage().persistent().set(&key, &updated);
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
    /// NOTE: This includes revoked entries. Prefer `get_active_vc_count` for scoring/verification.
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

    /// Propose a new contract admin (two-step admin transfer).
    pub fn propose_new_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), IdentityOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if current_admin != stored_admin {
            return Err(IdentityOracleError::NotAuthorized);
        }
        current_admin.require_auth();
        env.storage().instance().set(&DataKey::PendingAdmin, &new_admin);
        Ok(())
    }

    /// Accept a proposed admin role (two-step admin transfer).
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), IdentityOracleError> {
        let pending: Option<Address> = env.storage().instance().get(&DataKey::PendingAdmin);
        match pending {
            Some(p) => {
                if p != new_admin {
                    panic!("not authorized");
                }
            }
            None => return Err(IdentityOracleError::NoPendingAdmin),
        }
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        Ok(())
    }

    /// Upgrade the contract WASM in-place, preserving address and all stored state.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        let stored = require_admin(&env);
        if admin != stored {
            panic!("not authorized");
        }
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Return the `IssuersIndex` vector of currently registered trusted issuers.
    pub fn list_issuers(env: Env) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::IssuersIndex)
            .unwrap_or(Vec::new(&env))
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
        client.register_issuer(&admin, &issuer);

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
        client.register_issuer(&admin, &issuer);
        client.deregister_issuer(&admin, &issuer);

        let is_trusted: bool = env.as_contract(&contract_id, || {
            env.storage().persistent().has(&DataKey::TrustedIssuer(issuer.clone()))
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
        client.register_issuer(&admin, &issuer);

        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);

        client.deregister_issuer(&admin, &issuer);

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

        client.register_issuer(&admin, &issuer1);
        assert_eq!(client.list_issuers(), Vec::from_array(&env, [issuer1.clone()]));

        client.register_issuer(&admin, &issuer2);
        assert_eq!(
            client.list_issuers(),
            Vec::from_array(&env, [issuer1.clone(), issuer2.clone()])
        );

        client.register_issuer(&admin, &issuer1);
        assert_eq!(
            client.list_issuers(),
            Vec::from_array(&env, [issuer1.clone(), issuer2.clone()])
        );

        client.deregister_issuer(&admin, &issuer1);
        assert_eq!(client.list_issuers(), Vec::from_array(&env, [issuer2]));
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
        client.register_issuer(&admin, &issuer);

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
        let cid = String::from_str(&env, "ipfs://QmYwAPJzagoJzrKSTTkG8w6zWZSNxrCYhpDkxQottEwHym");
        client.anchor_did(&subject, &cid);

        let subject2 = Address::generate(&env);
        let cid2 = String::from_str(&env, "bafy2bzacedw4hc6k2vxtcmfmr3jtcl6yvqohqmvtqj7lhyzuejcxgxvl6yv4");
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
            env.storage().persistent().get(&DataKey::DIDDocument(subject.clone())).unwrap()
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
        client.register_issuer(&admin, &issuer);

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
        client.register_issuer(&admin, &issuer);

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
    #[should_panic(expected = "vc not found")]
    fn test_mark_vc_revoked_panics_for_unknown_hash() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&admin, &issuer);

        let subject = Address::generate(&env);
        let known_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &known_hash);

        let unknown_hash = BytesN::from_array(&env, &[2u8; 32]);
        client.mark_vc_revoked(&issuer, &subject, &unknown_hash);
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
        client.register_issuer(&admin, &issuer);

        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);

        assert!(client.is_verified(&subject));

        client.mark_vc_revoked(&issuer, &subject, &vc_hash);

        assert!(!client.is_verified(&subject));
    }

    #[test]
    #[should_panic(expected = "not authorized")]
    fn test_upgrade_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        client.initialize(&admin);
        // Pass a zeroed hash — upgrade will fail on auth check before using it
        client.upgrade(&non_admin, &BytesN::from_array(&env, &[0u8; 32]));
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
        client.propose_new_admin(&admin1, &admin2);

        // accept by proposed admin
        client.accept_admin(&admin2);

        // new admin can register issuer
        client.register_issuer(&admin2, &issuer);

        // old admin cannot register issuer
        let issuer2 = Address::generate(&env);
        let res = client.try_register_issuer(&admin1, &issuer2);
        assert_eq!(res, Err(Ok(IdentityOracleError::NotAuthorized)));
    }

    #[test]
    #[should_panic(expected = "not authorized")]
    fn test_non_pending_admin_cannot_accept() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let non_admin = Address::generate(&env);

        client.initialize(&admin1);
        client.propose_new_admin(&admin1, &admin2);

        // non_admin tries to accept
        let _ = client.accept_admin(&non_admin);
    }
}
