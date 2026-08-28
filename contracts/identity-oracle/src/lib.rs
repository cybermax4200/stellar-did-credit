#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env};

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Admin,
    Issuer(Address),
    VcAnchor(Address, soroban_sdk::BytesN<32>),
    LastStateChange(Address),
}

#[contract]
pub struct IdentityOracleContract;

#[contractimpl]
impl IdentityOracleContract {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    pub fn register_issuer(env: Env, admin: Address, issuer: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if admin != stored_admin {
            panic!("not authorized");
        }
        env.storage().persistent().set(&DataKey::Issuer(issuer), &true);
    }

    pub fn anchor_vc(env: Env, issuer: Address, subject: Address, vc_hash: soroban_sdk::BytesN<32>) {
        issuer.require_auth();
        let is_issuer: bool = env.storage().persistent().get(&DataKey::Issuer(issuer)).unwrap_or(false);
        if !is_issuer {
            panic!("unregistered issuer");
        }

        let current_ledger = env.ledger().sequence();
        env.storage().persistent().set(&DataKey::VcAnchor(subject.clone(), vc_hash), &true);
        env.storage().persistent().set(&DataKey::LastStateChange(subject.clone()), &current_ledger);
    }

    pub fn mark_vc_revoked(env: Env, issuer: Address, subject: Address, vc_hash: soroban_sdk::BytesN<32>) {
        issuer.require_auth();
        let is_issuer: bool = env.storage().persistent().get(&DataKey::Issuer(issuer)).unwrap_or(false);
        if !is_issuer {
            panic!("unregistered issuer");
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

    pub fn get_active_vc_count(env: Env, subject: Address) -> u32 {
        load_active_vc_count(&env, &subject).unwrap_or_else(|| seed_active_vc_count(&env, &subject))
    }

        let current_ledger = env.ledger().sequence();
        env.storage().persistent().remove(&DataKey::VcAnchor(subject.clone(), vc_hash));
        env.storage().persistent().set(&DataKey::LastStateChange(subject.clone()), &current_ledger);
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

    /// Returns the currently configured revocation registry contract ID, or
    /// `None` if no registry has been configured yet.
    ///
    /// # Important
    ///
    /// If `None` is returned, `is_verified`, `get_active_vc_count`, and
    /// `verify_vc` will **only** check the local `mark_vc_revoked` flag —
    /// any revocations performed through the `RevocationRegistry` contract
    /// will be **silently ignored**.
    ///
    /// Deployers must call `set_revocation_registry` after deploying the
    /// revocation-registry contract to enable cross-contract revocation
    /// checking.
    ///
    /// See `docs/mainnet-deployment.md` for the required deployment order.
    pub fn get_revocation_registry(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::RevocationRegistryId)
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

    /// Returns aggregate protocol-level counters.
    ///
    /// These counters are updated on every write operation and provide
    /// on-chain operational metrics without requiring an external indexer.
    pub fn get_protocol_stats(env: Env) -> ProtocolStats {
        load_protocol_stats(&env)
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[contract]
    pub struct MockRevocationRegistry;

    #[contractimpl]
    impl MockRevocationRegistry {
        pub fn is_revoked(_env: Env, _vc_hash: BytesN<32>) -> bool {
            false
        }
        pub fn set_identity_oracle(_env: Env, _oracle: Address) {}
    }

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
    fn test_get_did_document_returns_cid_after_anchor() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDIDDocument");

        // Before anchoring, get_did_document returns None
        assert!(client.get_did_document(&subject).is_none());

        // Anchor the DID
        client.anchor_did(&subject, &cid);

        // After anchoring, get_did_document returns the CID
        let result = client.get_did_document(&subject);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), cid);
    }

    #[test]
    fn test_get_did_document_returns_none_for_unknown_subject() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);

        // Subject has never anchored a DID
        assert!(client.get_did_document(&subject).is_none());
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
    fn test_duplicate_vc_hash_same_issuer_is_noop() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[42u8; 32]);

        // First anchor should succeed
        assert!(client.try_anchor_vc(&issuer, &subject, &vc_hash).is_ok());

        // Second anchor with same issuer + same hash should be a no-op (not error)
        assert!(client.try_anchor_vc(&issuer, &subject, &vc_hash).is_ok());

        // Count should be 1, not 2
        assert_eq!(client.get_total_vc_count(&subject), 1);
        assert_eq!(client.get_active_vc_count(&subject), 1);
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
    fn test_get_active_vc_count_cached_cost_stays_flat_with_registry() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let registry_id = env.register_contract(None, MockRevocationRegistry);
        client.set_revocation_registry(&registry_id);

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
            "get_active_vc_count cached (registry configured) cpu instructions: 5 VCs = {}, 10 VCs = {}, 20 VCs = {}",
            cost_5,
            cost_10,
            cost_20
        );

        let max_cost = core::cmp::max(core::cmp::max(cost_5, cost_10), cost_20);
        let min_cost = core::cmp::min(core::cmp::min(cost_5, cost_10), cost_20);
        assert!(
            max_cost - min_cost <= 25_000,
            "expected get_active_vc_count with registry configured to cost O(1) (flat), got 5={} 10={} 20={}",
            cost_5,
            cost_10,
            cost_20
        );

        const MAINNET_CPU_LIMIT: u64 = 600_000_000;
        assert!(
            max_cost < MAINNET_CPU_LIMIT,
            "expected get_active_vc_count with registry configured to stay under the mainnet CPU limit"
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
    fn test_initialize_sets_admin_and_emits_event() {
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

    #[test]
    #[should_panic(expected = "Error(Contract, #1)")]
    fn test_initialize_already_initialized() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let admin2 = Address::generate(&env);
        client.initialize(&admin2);
    }

    // -----------------------------------------------------------------------
    // Revocation Registry configuration tests
    // -----------------------------------------------------------------------

    /// Verifies that `get_revocation_registry` returns `None` before
    /// configuration, matching the intended initial state.
    #[test]
    fn test_get_revocation_registry_returns_none_after_initialize() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let registry = client.get_revocation_registry();
        assert!(registry.is_none(), "RevocationRegistryId should be None after initialization");
    }

    // -----------------------------------------------------------------------
    // Revocation Registry configuration tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_protocol_stats_default_zero() {
        let env = Env::default();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_dids_anchored, 0);
        assert_eq!(stats.total_vcs_anchored, 0);
        assert_eq!(stats.total_vcs_revoked, 0);
    }

    #[test]
    fn test_protocol_stats_increments_on_anchor_did() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDIDStats");
        client.anchor_did(&subject, &cid);

        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_dids_anchored, 1);

        let subject2 = Address::generate(&env);
        let cid2 = String::from_str(&env, "ipfs://QmTestDIDStats2");
        client.anchor_did(&subject2, &cid2);

        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_dids_anchored, 2);
    }

    /// Verifies that `set_revocation_registry` correctly stores the address
    /// and `get_revocation_registry` returns it.
    #[test]
    fn test_set_revocation_registry_sets_address() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let registry_id = env.register_contract(None, MockRevocationRegistry);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Initially None
        assert!(client.get_revocation_registry().is_none());

        // Set the registry
        client.set_revocation_registry(&registry_id);

        // Now should return Some
        let stored = client.get_revocation_registry();
        assert!(stored.is_some(), "RevocationRegistryId should be Some after set_revocation_registry");
        assert_eq!(stored.unwrap(), registry_id);
    }

    #[test]
    fn test_protocol_stats_increments_on_anchor_vc() {
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

        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_vcs_anchored, 1);

        let vc_hash2 = BytesN::from_array(&env, &[2u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash2);

        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_vcs_anchored, 2);
    }

    /// Verifies that `set_revocation_registry` can update an existing
    /// registry address to a new one.
    #[test]
    fn test_set_revocation_registry_can_update() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let registry_id_1 = env.register_contract(None, MockRevocationRegistry);
        let registry_id_2 = env.register_contract(None, MockRevocationRegistry);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Set to first registry
        client.set_revocation_registry(&registry_id_1);
        assert_eq!(client.get_revocation_registry().unwrap(), registry_id_1);

        // Update to second registry
        client.set_revocation_registry(&registry_id_2);
        assert_eq!(client.get_revocation_registry().unwrap(), registry_id_2);
    }

    #[test]
    fn test_protocol_stats_increments_on_mark_vc_revoked() {
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

        client.mark_vc_revoked(&issuer, &subject, &vc_hash);

        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_vcs_anchored, 1);
        assert_eq!(stats.total_vcs_revoked, 1);
    }

    #[test]
    fn test_protocol_stats_increments_on_deactivate_did() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDIDDeact");
        client.anchor_did(&subject, &cid);

        let vc_hash1 = BytesN::from_array(&env, &[1u8; 32]);
        let vc_hash2 = BytesN::from_array(&env, &[2u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash1);
        client.anchor_vc(&issuer, &subject, &vc_hash2);

        client.deactivate_did(&subject);

        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_dids_anchored, 1);
        assert_eq!(stats.total_vcs_anchored, 2);
        assert_eq!(stats.total_vcs_revoked, 2);
    }

    #[test]
    fn test_deactivate_did_sets_deactivated_flag_and_removes_did_document() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDeactivateDidFlag");
        client.anchor_did(&subject, &cid);

        let vc_hash = BytesN::from_array(&env, &[7u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash);

        assert!(client.is_verified(&subject));
        assert!(!client.is_deactivated(&subject));
        assert!(client.get_did_document(&subject).is_some());

        client.deactivate_did(&subject);

        // Regression test for the bug this issue reports: previously
        // `deactivate_did` never set the `Deactivated` flag, so
        // `is_deactivated` (which `credit-oracle` relies on via
        // cross-contract call) kept returning `false` after a subject
        // called `deactivate_did`, letting their score still be computed.
        assert!(client.is_deactivated(&subject));

        // Full deactivation also removes the DID document.
        assert!(client.get_did_document(&subject).is_none());

        // And, as before, VCs are revoked / not verified / active count 0.
        assert!(!client.is_verified(&subject));
        assert_eq!(client.get_active_vc_count(&subject), 0);
    }

    #[test]
    fn test_deactivate_identity_does_not_remove_did_document() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let subject = Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDeactivateIdentityKeepsDid");
        client.anchor_did(&subject, &cid);

        client.deactivate_identity(&subject);

        // Unlike `deactivate_did`, `deactivate_identity` leaves the DID
        // document CID in place.
        assert_eq!(client.get_did_document(&subject), Some(cid));
        assert!(client.is_deactivated(&subject));
    }

    #[test]
    fn test_deactivate_identity_sets_flag_and_revokes_vcs() {
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

        // Anchor 3 VCs
        let vc_hash1 = BytesN::from_array(&env, &[1u8; 32]);
        let vc_hash2 = BytesN::from_array(&env, &[2u8; 32]);
        let vc_hash3 = BytesN::from_array(&env, &[3u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash1);
        client.anchor_vc(&issuer, &subject, &vc_hash2);
        client.anchor_vc(&issuer, &subject, &vc_hash3);

        assert!(client.is_verified(&subject));
        assert_eq!(client.get_active_vc_count(&subject), 3);
        assert!(!client.is_deactivated(&subject));

        // Deactivate
        let revoked = client.deactivate_identity(&subject);
        assert_eq!(revoked, 3);

        // Verify flag is set
        assert!(client.is_deactivated(&subject));

        // is_verified returns false
        assert!(!client.is_verified(&subject));

        // Active VC count is 0
        assert_eq!(client.get_active_vc_count(&subject), 0);

        // Total VC count unchanged
        assert_eq!(client.get_total_vc_count(&subject), 3);
    }

    #[test]
    fn test_reactivate_identity_clears_flag() {
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

        // Deactivate
        client.deactivate_identity(&subject);
        assert!(client.is_deactivated(&subject));
        assert!(!client.is_verified(&subject));

        // Reactivate
        client.reactivate_identity(&subject);

        // Flag is cleared
        assert!(!client.is_deactivated(&subject));

        // is_verified still returns false because VCs were revoked during deactivation
        assert!(!client.is_verified(&subject));

        // Active VC count is still 0
        assert_eq!(client.get_active_vc_count(&subject), 0);
    }

    #[test]
    fn test_deactivate_reactivate_full_lifecycle() {
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

        // Initial: verified
        assert!(client.is_verified(&subject));
        assert_eq!(client.get_active_vc_count(&subject), 1);

        // Deactivate
        let revoked = client.deactivate_identity(&subject);
        assert_eq!(revoked, 1);
        assert!(client.is_deactivated(&subject));
        assert!(!client.is_verified(&subject));
        assert_eq!(client.get_active_vc_count(&subject), 0);

        // Reactivate
        client.reactivate_identity(&subject);
        assert!(!client.is_deactivated(&subject));

        // VCs are still revoked, so not verified
        assert!(!client.is_verified(&subject));
        assert_eq!(client.get_active_vc_count(&subject), 0);

        // Issue a new VC - should become verified again
        let vc_hash2 = BytesN::from_array(&env, &[2u8; 32]);
        client.anchor_vc(&issuer, &subject, &vc_hash2);
        assert!(client.is_verified(&subject));
        assert_eq!(client.get_active_vc_count(&subject), 1);
    }

    #[test]
    fn test_deactivate_identity_with_no_vcs_returns_zero() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let subject = Address::generate(&env);

        // No VCs at all
        assert!(!client.is_verified(&subject));
        assert!(!client.is_deactivated(&subject));

        let revoked = client.deactivate_identity(&subject);
        assert_eq!(revoked, 0);
        assert!(client.is_deactivated(&subject));
    }

    #[test]
    fn test_protocol_stats_no_increment_on_dedup_vc() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[42u8; 32]);

        client.anchor_vc(&issuer, &subject, &vc_hash);
        let stats_after_first = client.get_protocol_stats();
        assert_eq!(stats_after_first.total_vcs_anchored, 1);

        // Duplicate (issuer, hash) pair should be a no-op
        client.anchor_vc(&issuer, &subject, &vc_hash);
        let stats_after_dedup = client.get_protocol_stats();
        assert_eq!(stats_after_dedup.total_vcs_anchored, 1);
    }

    #[test]
    fn test_vc_limit_reached_at_max() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);

        for i in 0..100u8 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = i;
            hash_arr[1] = 1;
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            client.anchor_vc(&issuer, &subject, &vc_hash);
        }

        let mut hash_arr = [0u8; 32];
        hash_arr[0] = 101;
        hash_arr[1] = 1;
        let vc_hash_101 = BytesN::from_array(&env, &hash_arr);
        let result = client.try_anchor_vc(&issuer, &subject, &vc_hash_101);
        assert_eq!(result, Err(Ok(IdentityOracleError::VCLimitReached)));
    }

    #[test]
    fn test_revoked_vcs_do_not_count_toward_limit() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let subject = Address::generate(&env);

        for i in 0..100u8 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = i;
            hash_arr[1] = 2;
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            client.anchor_vc(&issuer, &subject, &vc_hash);
        }
        for i in 0..50u8 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = i;
            hash_arr[1] = 2;
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            client.mark_vc_revoked(&issuer, &subject, &vc_hash);
        }

        for i in 0..50u8 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = i;
            hash_arr[1] = 3;
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            client.anchor_vc(&issuer, &subject, &vc_hash);
        }

        assert_eq!(client.get_active_vc_count(&subject), 100);
    }

    #[test]
    fn test_anchor_did_accepts_exactly_65_byte_cid() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);
        let mut cid_str = std::string::String::from("ipfs://");
        while cid_str.len() < 65 {
            cid_str.push('a');
        }
        let cid = String::from_str(&env, &cid_str);
        
        client.anchor_did(&subject, &cid);
        
        let stored = client.get_did_document(&subject);
        assert_eq!(stored.unwrap(), cid);
    }

    #[test]
    fn test_anchor_did_rejects_256_byte_cid() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, IdentityOracle);
        let client = IdentityOracleClient::new(&env, &contract_id);

        let subject = Address::generate(&env);
        let mut cid_str = std::string::String::from("ipfs://");
        while cid_str.len() < 256 {
            cid_str.push('a');
        }
        let cid = String::from_str(&env, &cid_str);
        
        let result = client.try_anchor_did(&subject, &cid);
        assert_eq!(result, Err(Ok(IdentityOracleError::InvalidCID)));
    }
}
