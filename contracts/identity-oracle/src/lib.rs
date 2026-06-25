#![no_std]
#[allow(unused_imports)]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, symbol_short, Address, BytesN, Env, String, Vec};

/// Maximum number of VCs that can be anchored per subject.
/// Soroban ledger entries have a ~64KB size limit; capping at 50 prevents overflow.
pub const MAX_VC_PER_SUBJECT: u32 = 50;

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
    /// The contract is paused.
    ContractPaused = 5,
    /// Subject has reached the maximum number of anchored VCs.
    VcLimitReached = 6,
}

/// Storage key variants for the identity-oracle contract.
#[contracttype]
pub enum DataKey {
    /// The contract administrator address.
    Admin,
    /// Whether the contract is paused.
    Paused,
    /// Whether the given address is a trusted credential issuer.
    TrustedIssuer(Address),
    /// The DID document hash anchored for the given subject address.
    DIDDocument(Address),
    /// The list of VC anchors associated with the given subject address.
    VCAnchors(Address),
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

#[contract]
pub struct IdentityOracle;

fn check_not_paused(env: &Env) {
    let paused: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
    if paused {
        panic!("contract paused");
    }
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

    /// Pause the contract, blocking all state-mutating functions. Admin only.
    pub fn pause(env: Env, admin: Address) {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            panic!("not authorized");
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
    }

    /// Unpause the contract, restoring normal operation. Admin only.
    pub fn unpause(env: Env, admin: Address) {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            panic!("not authorized");
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
    }

    /// Register a trusted credential issuer authorized to anchor verifiable credentials.
    pub fn register_issuer(env: Env, admin: Address, issuer: Address) -> Result<(), IdentityOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            return Err(IdentityOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage().persistent().set(&DataKey::TrustedIssuer(issuer.clone()), &true);
        env.events().publish((symbol_short!("IssReg"),), issuer);
        Ok(())
    }

    /// Deregister a trusted credential issuer, preventing future credential anchoring.
    ///
    /// Removes the issuer's trusted status. This does NOT retroactively revoke existing VCs
    /// anchored by this issuer — the deregistration only prevents the issuer from anchoring new VCs.
    pub fn deregister_issuer(env: Env, admin: Address, issuer: Address) -> Result<(), IdentityOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            return Err(IdentityOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage().persistent().remove(&DataKey::TrustedIssuer(issuer.clone()));
        env.events().publish((symbol_short!("IssDeReg"),), issuer);
        Ok(())
    }

    /// Anchor the IPFS CID of a DID document for the calling subject.
    pub fn anchor_did(env: Env, subject: Address, did_doc_cid: String) -> Result<(), IdentityOracleError> {
        check_not_paused(&env);
        subject.require_auth();

        if did_doc_cid.len() < 7 {
            return Err(IdentityOracleError::InvalidCID);
        }

        let cid_str = did_doc_cid.clone();
        let starts_with_valid = cid_str.starts_with(&String::from_str(&env, "ipfs://"))
            || cid_str.starts_with(&String::from_str(&env, "bafy"))
            || cid_str.starts_with(&String::from_str(&env, "Qm"));

        if !starts_with_valid {
            return Err(IdentityOracleError::InvalidCID);
        }

        env.storage().persistent().set(&DataKey::DIDDocument(subject.clone()), &did_doc_cid);
        env.events().publish((symbol_short!("DIDAnch"),), (subject, did_doc_cid));
        Ok(())
    }

    /// Anchor a verifiable credential (VC) for a subject issued by a trusted issuer.
    pub fn anchor_vc(
        env: Env,
        issuer: Address,
        subject: Address,
        vc_hash: BytesN<32>,
    ) -> Result<(), IdentityOracleError> {
        check_not_paused(&env);
        issuer.require_auth();
        if !env.storage().persistent().has(&DataKey::TrustedIssuer(issuer.clone())) {
            return Err(IdentityOracleError::IssuerNotRegistered);
        }

        let key = DataKey::VCAnchors(subject.clone());
        let mut anchors: Vec<VCRecord> = env.storage().persistent().get(&key).unwrap_or(Vec::new(&env));

        if anchors.len() >= MAX_VC_PER_SUBJECT {
            panic!("vc limit reached");
        }

        let record = VCRecord {
            vc_hash: vc_hash.clone(),
            issuer: issuer.clone(),
            anchored_at: env.ledger().timestamp(),
            revoked: false,
        };

        anchors.push_back(record);
        env.storage().persistent().set(&key, &anchors);
        env.events().publish((symbol_short!("VCAnch"),), (issuer, subject, vc_hash));
        Ok(())
    }

    /// Mark a previously anchored VC as revoked by its issuer.
    pub fn mark_vc_revoked(env: Env, issuer: Address, subject: Address, vc_hash: BytesN<32>) -> Result<(), IdentityOracleError> {
        check_not_paused(&env);
        issuer.require_auth();
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env.storage().persistent().get(&key).unwrap_or(Vec::new(&env));

        let mut updated = Vec::new(&env);
        for mut record in anchors.iter() {
            if record.vc_hash == vc_hash && record.issuer == issuer {
                record.revoked = true;
            }
            updated.push_back(record);
        }
        env.storage().persistent().set(&key, &updated);
        Ok(())
    }

    /// Check if a subject has at least one non-revoked verifiable credential anchored.
    pub fn is_verified(env: Env, subject: Address) -> bool {
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env.storage().persistent().get(&key).unwrap_or(Vec::new(&env));
        for record in anchors.iter() {
            if !record.revoked {
                return true;
            }
        }
        false
    }

    /// Returns the total number of anchored VC records for `subject`, including revoked entries.
    pub fn get_total_vc_count(env: Env, subject: Address) -> u32 {
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env.storage().persistent().get(&key).unwrap_or(Vec::new(&env));
        anchors.len()
    }

    /// Returns the number of anchored VC records for `subject` that are **not revoked**.
    pub fn get_active_vc_count(env: Env, subject: Address) -> u32 {
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env.storage().persistent().get(&key).unwrap_or(Vec::new(&env));
        let mut count: u32 = 0;
        for record in anchors.iter() {
            if !record.revoked {
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

    /// Check if a specific VC hash is valid (anchored and not revoked) for a subject.
    pub fn verify_vc(env: Env, subject: Address, vc_hash: BytesN<32>) -> bool {
        let key = DataKey::VCAnchors(subject);
        let anchors: Vec<VCRecord> = env.storage().persistent().get(&key).unwrap_or(Vec::new(&env));
        for record in anchors.iter() {
            if record.vc_hash == vc_hash && !record.revoked {
                return true;
            }
        }
        false
    }

    /// Upgrade the contract WASM in-place, preserving address and all stored state.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            panic!("not authorized");
        }
        admin.require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup() -> (Env, soroban_sdk::Address, IdentityOracleClient) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        (env, admin, client)
    }

    #[test]
    fn test_anchor_vc_by_trusted_issuer() {
        let (env, admin, client) = setup();
        let issuer = Address::generate(&env);
        client.register_issuer(&admin, &issuer);
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        let result = client.anchor_vc(&issuer, &subject, &vc_hash);
        assert!(result.is_ok());
    }

    #[test]
    fn test_unregistered_issuer_fails() {
        let (env, _admin, client) = setup();
        let issuer = Address::generate(&env);
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        let result = client.anchor_vc(&issuer, &subject, &vc_hash);
        assert_eq!(result, Err(Ok(IdentityOracleError::IssuerNotRegistered)));
    }

    #[test]
    fn test_deregister_issuer_succeeds() {
        let (env, admin, client) = setup();
        let issuer = Address::generate(&env);
        client.register_issuer(&admin, &issuer);
        let result = client.deregister_issuer(&admin, &issuer);
        assert!(result.is_ok());
    }

    #[test]
    fn test_deregistered_issuer_cannot_anchor_vc() {
        let (env, admin, client) = setup();
        let issuer = Address::generate(&env);
        client.register_issuer(&admin, &issuer);
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);
        client.deregister_issuer(&admin, &issuer);
        let vc_hash2 = BytesN::from_array(&env, &[2u8; 32]);
        let result = client.anchor_vc(&issuer, &subject, &vc_hash2);
        assert_eq!(result, Err(Ok(IdentityOracleError::IssuerNotRegistered)));
    }

    #[test]
    fn test_is_verified_true_after_vc_anchored() {
        let (env, admin, client) = setup();
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
        let (env, _admin, client) = setup();
        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://Qm...");
        let result = client.anchor_did(&subject, &cid);
        assert!(result.is_ok());
    }

    #[test]
    fn test_anchor_did_rejects_empty_cid() {
        let (env, _admin, client) = setup();
        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "");
        let result = client.anchor_did(&subject, &cid);
        assert_eq!(result, Err(Ok(IdentityOracleError::InvalidCID)));
    }

    #[test]
    fn test_anchor_did_rejects_single_space_cid() {
        let (env, _admin, client) = setup();
        let subject = Address::generate(&env);
        let cid = String::from_str(&env, " ");
        let result = client.anchor_did(&subject, &cid);
        assert_eq!(result, Err(Ok(IdentityOracleError::InvalidCID)));
    }

    #[test]
    fn test_anchor_did_rejects_invalid_prefix() {
        let (env, _admin, client) = setup();
        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "invalid-cid-data");
        let result = client.anchor_did(&subject, &cid);
        assert_eq!(result, Err(Ok(IdentityOracleError::InvalidCID)));
    }

    #[test]
    fn test_anchor_did_accepts_valid_ipfs_cid() {
        let (env, _admin, client) = setup();
        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmYwAPJzagoJzrKSTTkG8w6zWZSNxrCYhpDkxQottEwHym");
        assert!(client.anchor_did(&subject, &cid).is_ok());

        let subject2 = Address::generate(&env);
        let cid2 = String::from_str(&env, "bafy2bzacedw4hc6k2vxtcmfmr3jtcl6yvqohqmvtqj7lhyzuejcxgxvl6yv4");
        assert!(client.anchor_did(&subject2, &cid2).is_ok());

        let subject3 = Address::generate(&env);
        let cid3 = String::from_str(&env, "QmVocdeKSNbd9jkc3pDjq9FdAVLpiHrfQFwcJMgB7aXZi3");
        assert!(client.anchor_did(&subject3, &cid3).is_ok());
    }

    #[test]
    fn test_vc_count_increments_correctly() {
        let (env, admin, client) = setup();
        let issuer = Address::generate(&env);
        client.register_issuer(&admin, &issuer);
        let subject = Address::generate(&env);
        assert_eq!(client.get_vc_count(&subject), 0);
        for i in 0..3 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = i as u8;
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            client.anchor_vc(&issuer, &subject, &vc_hash);
        }
        assert_eq!(client.get_vc_count(&subject), 3);
    }

    #[test]
    fn test_get_active_vc_count_excludes_revoked() {
        let (env, admin, client) = setup();
        let issuer = Address::generate(&env);
        client.register_issuer(&admin, &issuer);
        let subject = Address::generate(&env);
        for i in 0..3u8 {
            let vc_hash = BytesN::from_array(&env, &[i; 32]);
            client.anchor_vc(&issuer, &subject, &vc_hash);
        }
        for i in 0..2u8 {
            let vc_hash = BytesN::from_array(&env, &[i; 32]);
            client.mark_vc_revoked(&issuer, &subject, &vc_hash);
        }
        assert_eq!(client.get_active_vc_count(&subject), 1);
    }

    #[test]
    fn test_revoked_vc_fails_is_verified() {
        let (env, admin, client) = setup();
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
    fn test_upgrade_preserves_contract_address() {
        let env = Env::default();
        env.mock_all_auths();
        let new_wasm_hash = env.deployer().upload_contract_wasm(IdentityOracle::WASM);
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        client.upgrade(&admin, &new_wasm_hash);
        let subject = Address::generate(&env);
        assert!(!client.is_verified(&subject));
    }

    #[test]
    #[should_panic(expected = "not authorized")]
    fn test_upgrade_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let new_wasm_hash = env.deployer().upload_contract_wasm(IdentityOracle::WASM);
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        client.initialize(&admin);
        client.upgrade(&non_admin, &new_wasm_hash);
    }

    // --- #30 pause tests ---

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_contract_rejects_mutations() {
        let (env, admin, client) = setup();
        let issuer = Address::generate(&env);
        client.register_issuer(&admin, &issuer);
        client.pause(&admin);
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);
    }

    #[test]
    fn test_unpause_restores_functionality() {
        let (env, admin, client) = setup();
        let issuer = Address::generate(&env);
        client.register_issuer(&admin, &issuer);
        client.pause(&admin);
        client.unpause(&admin);
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);
        let result = client.anchor_vc(&issuer, &subject, &vc_hash);
        assert!(result.is_ok());
    }

    // --- #31 VC limit test ---

    #[test]
    #[should_panic(expected = "vc limit reached")]
    fn test_anchor_vc_rejects_at_limit() {
        let (env, admin, client) = setup();
        let issuer = Address::generate(&env);
        client.register_issuer(&admin, &issuer);
        let subject = Address::generate(&env);
        for i in 0..MAX_VC_PER_SUBJECT {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = (i >> 8) as u8;
            hash_arr[1] = (i & 0xff) as u8;
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            client.anchor_vc(&issuer, &subject, &vc_hash);
        }
        // This 51st call should panic
        let vc_hash = BytesN::from_array(&env, &[255u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);
    }
}
