#![no_std]
//! Revocation registry contract for the Stellar DID Credit protocol.
//!
//! Maintains an on-chain list of revoked verifiable credential hashes.
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, BytesN, Env,
    IntoVal, Vec,
};

// ---------------------------------------------------------------------------
// Auth helper
// ---------------------------------------------------------------------------

fn require_admin(env: &Env) -> Address {
    let admin: Address = env
        .storage()
        .instance()
        .get(&RevocationKey::Admin)
        .expect("not initialized");
    admin.require_auth();
    admin
}

fn ensure_not_paused(env: &Env) -> Result<(), RevocationRegistryError> {
    if env
        .storage()
        .instance()
        .get(&RevocationKey::Paused)
        .unwrap_or(false)
    {
        Err(RevocationRegistryError::ContractPaused)
    } else {
        Ok(())
    }
}

fn enter_guard(env: &Env) -> Result<(), RevocationRegistryError> {
    if env.storage().temporary().has(&RevocationKey::ReentrancyLock) {
        return Err(RevocationRegistryError::ReentrancyDetected);
    }
    env.storage()
        .temporary()
        .set(&RevocationKey::ReentrancyLock, &true);
    Ok(())
}

fn exit_guard(env: &Env) {
    env.storage()
        .temporary()
        .remove(&RevocationKey::ReentrancyLock);
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum RevocationRegistryError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    IssuerMismatch = 3,
    NoPendingAdmin = 4,
    BatchTooLarge = 5,
    ContractPaused = 6,
    ReentrancyDetected = 7,
    /// Batch limit must be between 1 and MAX_BATCH_SIZE.
    InvalidBatchLimit = 8,
}

#[contracttype]
pub enum RevocationKey {
    Admin,
    Paused,
    PendingAdmin,
    IdentityOracleId,
    RegisteredVCIssuer(BytesN<32>),
    Status(BytesN<32>),
    IssuerOfVC(BytesN<32>),
    IssuerRevokedList(Address),
    ReentrancyLock,
    BatchLimit,
}

const INSTANCE_BUMP_THRESHOLD: u32 = 5000;
const INSTANCE_BUMP_AMOUNT: u32 = 500_000;

const PERS_TTL_THRESHOLD: u32 = 120_960; // ~7 days
const PERS_TTL_EXTEND: u32 = 518_400; // ~30 days

/// Hard safety cap for batch revoke operations.
const MAX_BATCH_SIZE: u32 = 100;

#[contract]
pub struct RevocationRegistry;

#[contractimpl]
impl RevocationRegistry {
    pub fn initialize(env: Env, admin: Address) -> Result<(), RevocationRegistryError> {
        if env.storage().instance().has(&RevocationKey::Admin) {
            return Err(RevocationRegistryError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&RevocationKey::Admin, &admin);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.events()
            .publish((symbol_short!("Init"),), admin);
        env.storage()
            .instance()
            .set(&RevocationKey::BatchLimit, &MAX_BATCH_SIZE);
        Ok(())
    }

    pub fn get_batch_limit(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&RevocationKey::BatchLimit)
            .unwrap_or(MAX_BATCH_SIZE)
    }

    pub fn set_batch_limit(
        env: Env,
        admin: Address,
        limit: u32,
    ) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        let stored_admin = require_admin(&env);
        if admin != stored_admin {
            return Err(RevocationRegistryError::NotAuthorized);
        }
        if limit == 0 || limit > MAX_BATCH_SIZE {
            return Err(RevocationRegistryError::InvalidBatchLimit);
        }
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .instance()
            .set(&RevocationKey::BatchLimit, &limit);
        Ok(())
    }

    pub fn set_identity_oracle(
        env: Env,
        identity_oracle_id: Address,
    ) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .instance()
            .set(&RevocationKey::IdentityOracleId, &identity_oracle_id);
        Ok(())
    }

    pub fn propose_new_admin(env: Env, new_admin: Address) -> Result<(), RevocationRegistryError> {
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .instance()
            .set(&RevocationKey::PendingAdmin, &new_admin);
        Ok(())
    }

    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        let pending: Option<Address> = env.storage().instance().get(&RevocationKey::PendingAdmin);
        match pending {
            Some(p) => {
                if p != new_admin {
                    return Err(RevocationRegistryError::NotAuthorized);
                }
            }
            None => return Err(RevocationRegistryError::NoPendingAdmin),
        }

        new_admin.require_auth();
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .instance()
            .set(&RevocationKey::Admin, &new_admin);
        env.storage()
            .instance()
            .remove(&RevocationKey::PendingAdmin);
        Ok(())
    }

    pub fn revoke(
        env: Env,
        issuer: Address,
        subject: Address,
        vc_hash: BytesN<32>,
    ) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        issuer.require_auth();

        let registered: Option<Address> = env
            .storage()
            .persistent()
            .get(&RevocationKey::RegisteredVCIssuer(vc_hash.clone()));

        match registered {
            Some(existing) => {
                if existing != issuer {
                    return Err(RevocationRegistryError::IssuerMismatch);
                }
            }
            None => {
                env.storage()
                    .persistent()
                    .set(&RevocationKey::RegisteredVCIssuer(vc_hash.clone()), &issuer);
            }
        }

        env.storage()
            .persistent()
            .set(&RevocationKey::Status(vc_hash.clone()), &true);
        env.storage()
            .persistent()
            .set(&RevocationKey::IssuerOfVC(vc_hash.clone()), &issuer);
        if let Some(identity_oracle_id) = env
            .storage()
            .instance()
            .get::<_, Address>(&RevocationKey::IdentityOracleId)
        {
            enter_guard(&env)?;
            env.invoke_contract::<()>(
                &identity_oracle_id,
                &soroban_sdk::Symbol::new(&env, "mark_vc_revoked"),
                soroban_sdk::vec![
                    &env,
                    issuer.into_val(&env),
                    subject.into_val(&env),
                    vc_hash.clone().into_val(&env)
                ],
            );
            exit_guard(&env);
        }
        env.storage().persistent().extend_ttl(
            &RevocationKey::Status(vc_hash.clone()),
            PERS_TTL_THRESHOLD,
            PERS_TTL_EXTEND,
        );
        env.storage().persistent().extend_ttl(
            &RevocationKey::IssuerOfVC(vc_hash.clone()),
            PERS_TTL_THRESHOLD,
            PERS_TTL_EXTEND,
        );

        let list_key = RevocationKey::IssuerRevokedList(issuer.clone());
        let mut list: Vec<BytesN<32>> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or(Vec::new(&env));
        if !list.contains(vc_hash.clone()) {
            list.push_back(vc_hash.clone());
            env.storage().persistent().set(&list_key, &list);
            env.storage()
                .persistent()
                .extend_ttl(&list_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        }
        env.events()
            .publish((symbol_short!("Revoked"),), (issuer, vc_hash));
        Ok(())
    }

    pub fn is_revoked(env: Env, vc_hash: BytesN<32>) -> bool {
        env.storage()
            .persistent()
            .get(&RevocationKey::Status(vc_hash))
            .unwrap_or(false)
    }

    pub fn get_revocation_record(env: Env, vc_hash: BytesN<32>) -> Option<Address> {
        env.storage()
            .persistent()
            .get(&RevocationKey::IssuerOfVC(vc_hash))
    }

    pub fn get_revocation_count(env: Env, issuer: Address) -> u32 {
        let list_key = RevocationKey::IssuerRevokedList(issuer);
        let list: Vec<BytesN<32>> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or(Vec::new(&env));
        list.len()
    }

    pub fn list_revoked(env: Env, issuer: Address, cursor: u32, limit: u32) -> Vec<BytesN<32>> {
        let list_key = RevocationKey::IssuerRevokedList(issuer);
        let list: Vec<BytesN<32>> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or(Vec::new(&env));

        let total = list.len();
        let mut result = Vec::new(&env);

        if cursor >= total || limit == 0 {
            return result;
        }

        let end = (cursor + limit).min(total);
        for i in cursor..end {
            result.push_back(list.get(i).unwrap());
        }
        result
    }

    pub fn batch_revoke(
        env: Env,
        issuer: Address,
        vc_hashes: Vec<BytesN<32>>,
    ) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        issuer.require_auth();
        let batch_limit: u32 = env
            .storage()
            .instance()
            .get(&RevocationKey::BatchLimit)
            .unwrap_or(MAX_BATCH_SIZE);
        if vc_hashes.len() > batch_limit {
            return Err(RevocationRegistryError::BatchTooLarge);
        }

        let list_key = RevocationKey::IssuerRevokedList(issuer.clone());
        let mut list: Vec<BytesN<32>> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or(Vec::new(&env));
        let mut list_modified = false;

        for vc_hash in vc_hashes.iter() {
            let registered: Option<Address> = env
                .storage()
                .persistent()
                .get(&RevocationKey::RegisteredVCIssuer(vc_hash.clone()));

            match registered {
                Some(existing) => {
                    if existing != issuer {
                        return Err(RevocationRegistryError::IssuerMismatch);
                    }
                }
                None => {
                    env.storage()
                        .persistent()
                        .set(&RevocationKey::RegisteredVCIssuer(vc_hash.clone()), &issuer);
                }
            }

            env.storage()
                .persistent()
                .set(&RevocationKey::Status(vc_hash.clone()), &true);
            env.storage()
                .persistent()
                .set(&RevocationKey::IssuerOfVC(vc_hash.clone()), &issuer);
            env.storage().persistent().extend_ttl(
                &RevocationKey::Status(vc_hash.clone()),
                PERS_TTL_THRESHOLD,
                PERS_TTL_EXTEND,
            );
            env.storage().persistent().extend_ttl(
                &RevocationKey::IssuerOfVC(vc_hash.clone()),
                PERS_TTL_THRESHOLD,
                PERS_TTL_EXTEND,
            );

            if !list.contains(vc_hash.clone()) {
                list.push_back(vc_hash.clone());
                list_modified = true;
            }
        }

        if list_modified {
            env.storage().persistent().set(&list_key, &list);
            env.storage()
                .persistent()
                .extend_ttl(&list_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        }

        env.events()
            .publish((symbol_short!("BatchRev"),), (issuer, vc_hashes.len()));
        Ok(())
    }

    pub fn maintain_storage(env: Env) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        Ok(())
    }

    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use soroban_sdk::{testutils::Address as _, Env};

    #[test]
    fn test_set_batch_limit_and_enforce() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        assert_eq!(client.get_batch_limit(), 100);

        client.set_batch_limit(&admin, &5);
        assert_eq!(client.get_batch_limit(), 5);

        let issuer = Address::generate(&env);
        let mut vc_hashes = Vec::new(&env);
        for i in 0..6 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = i as u8;
            vc_hashes.push_back(BytesN::from_array(&env, &hash_arr));
        }

        let res = client.try_batch_revoke(&issuer, &vc_hashes);
        assert_eq!(res, Err(Ok(RevocationRegistryError::BatchTooLarge)));
    }

    #[test]
    fn test_set_batch_limit_rejects_invalid() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let res = client.try_set_batch_limit(&admin, &0);
        assert_eq!(res, Err(Ok(RevocationRegistryError::InvalidBatchLimit)));

        let res = client.try_set_batch_limit(&admin, &101);
        assert_eq!(res, Err(Ok(RevocationRegistryError::InvalidBatchLimit)));
    }

    #[test]
    fn test_revoke_and_check() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);

        assert!(!client.is_revoked(&vc_hash));
        client.revoke(&issuer, &subject, &vc_hash);
        assert!(client.is_revoked(&vc_hash));
    }

    #[test]
    fn test_unknown_hash_not_revoked() {
        let env = Env::default();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let vc_hash = BytesN::from_array(&env, &[2u8; 32]);
        assert!(!client.is_revoked(&vc_hash));
    }

    #[test]
    fn test_only_registered_issuer_can_revoke() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer_a = Address::generate(&env);
        let issuer_b = Address::generate(&env);
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[3u8; 32]);

        client.revoke(&issuer_a, &subject, &vc_hash);
        client.revoke(&issuer_a, &subject, &vc_hash);

        let res = client.try_revoke(&issuer_b, &subject, &vc_hash);
        assert_eq!(res, Err(Ok(RevocationRegistryError::IssuerMismatch)));
    }

    #[test]
    fn test_batch_revoke_five_hashes() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        let mut vc_hashes = Vec::new(&env);
        for i in 0..5 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = i as u8;
            vc_hashes.push_back(BytesN::from_array(&env, &hash_arr));
        }

        client.batch_revoke(&issuer, &vc_hashes);

        for vc_hash in vc_hashes.iter() {
            assert!(client.is_revoked(&vc_hash));
        }
    }

    #[test]
    fn test_batch_revoke_exceeds_limit() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        let mut vc_hashes = Vec::new(&env);
        for i in 0..101 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = (i % 256) as u8;
            hash_arr[1] = (i / 256) as u8;
            vc_hashes.push_back(BytesN::from_array(&env, &hash_arr));
        }

        let res = client.try_batch_revoke(&issuer, &vc_hashes);
        assert_eq!(res, Err(Ok(RevocationRegistryError::BatchTooLarge)));
    }

    #[test]
    fn test_admin_transfer_two_step() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let admin3 = Address::generate(&env);

        client.initialize(&admin1);
        client.propose_new_admin(&admin2);
        client.accept_admin(&admin2);

        client.propose_new_admin(&admin3);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn test_non_pending_admin_cannot_accept() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let non_admin = Address::generate(&env);

        client.initialize(&admin1);
        client.propose_new_admin(&admin2);

        client.accept_admin(&non_admin);
    }

    proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]
        #[test]
        fn proptest_batch_revoke_all_marked(
            hash_bytes in proptest::collection::vec(any::<[u8; 32]>(), 0..=50),
        ) {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register_contract(None, RevocationRegistry);
            let client = RevocationRegistryClient::new(&env, &contract_id);

            let issuer = Address::generate(&env);
            let mut vc_hashes = Vec::new(&env);
            for h in &hash_bytes {
                vc_hashes.push_back(BytesN::from_array(&env, h));
            }

            let result = client.try_batch_revoke(&issuer, &vc_hashes);
            assert!(result.is_ok());

            for h in vc_hashes.iter() {
                prop_assert!(client.is_revoked(&h));
            }
        }
    }

    proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]
        #[test]
        fn proptest_is_revoked_idempotent(
            hash_bytes in any::<[u8; 32]>(),
        ) {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register_contract(None, RevocationRegistry);
            let client = RevocationRegistryClient::new(&env, &contract_id);

            let issuer = Address::generate(&env);
            let subject = Address::generate(&env);
            let vc_hash = BytesN::from_array(&env, &hash_bytes);

            let result = client.try_revoke(&issuer, &subject, &vc_hash);
            assert!(result.is_ok());
            prop_assert!(client.is_revoked(&vc_hash));

            let result = client.try_revoke(&issuer, &subject, &vc_hash);
            assert!(result.is_ok());
            prop_assert!(client.is_revoked(&vc_hash));
        }
    }

    #[test]
    fn test_maintain_storage_succeeds_for_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let res = client.try_maintain_storage();
        assert!(res.is_ok());
    }

    #[test]
    fn test_maintain_storage_fails_for_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        env.mock_auths(&[]);
        let res = client.try_maintain_storage();
        assert!(res.is_err());
    }

    #[test]
    fn test_get_revocation_count_and_list_revoked_single_and_batch() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        assert_eq!(client.get_revocation_count(&issuer), 0);
        assert_eq!(client.list_revoked(&issuer, &0, &10), Vec::new(&env));

        let hash1 = BytesN::from_array(&env, &[10u8; 32]);
        let subject = Address::generate(&env);
        client.revoke(&issuer, &subject, &hash1);

        assert_eq!(client.get_revocation_count(&issuer), 1);
        let list1 = client.list_revoked(&issuer, &0, &10);
        assert_eq!(list1.len(), 1);
        assert_eq!(list1.get(0).unwrap(), hash1);

        let mut batch = Vec::new(&env);
        let hash2 = BytesN::from_array(&env, &[20u8; 32]);
        let hash3 = BytesN::from_array(&env, &[30u8; 32]);
        let hash4 = BytesN::from_array(&env, &[40u8; 32]);
        batch.push_back(hash2.clone());
        batch.push_back(hash3.clone());
        batch.push_back(hash4.clone());

        client.batch_revoke(&issuer, &batch);

        assert_eq!(client.get_revocation_count(&issuer), 4);

        let page1 = client.list_revoked(&issuer, &0, &2);
        assert_eq!(page1.len(), 2);
        assert_eq!(page1.get(0).unwrap(), hash1);
        assert_eq!(page1.get(1).unwrap(), hash2);

        let page2 = client.list_revoked(&issuer, &2, &2);
        assert_eq!(page2.len(), 2);
        assert_eq!(page2.get(0).unwrap(), hash3);
        assert_eq!(page2.get(1).unwrap(), hash4);

        let page3 = client.list_revoked(&issuer, &4, &2);
        assert_eq!(page3.len(), 0);

        let zero_limit = client.list_revoked(&issuer, &0, &0);
        assert_eq!(zero_limit.len(), 0);
    }

    #[test]
    fn test_get_revocation_count_and_list_revoked_idempotent() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[50u8; 32]);

        let subject = Address::generate(&env);
        client.revoke(&issuer, &subject, &vc_hash);
        assert_eq!(client.get_revocation_count(&issuer), 1);

        client.revoke(&issuer, &subject, &vc_hash);
        assert_eq!(client.get_revocation_count(&issuer), 1);
        let list = client.list_revoked(&issuer, &0, &10);
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).unwrap(), vc_hash);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #1)")]
    fn test_initialize_already_initialized() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let admin2 = Address::generate(&env);
        client.initialize(&admin2);
    }

    #[test]
    fn test_guard_absent_without_identity_oracle() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        let subject = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[0xAAu8; 32]);

        client.revoke(&issuer, &subject, &vc_hash);
        assert!(client.is_revoked(&vc_hash));

        let lock_present: bool = env.as_contract(&contract_id, || {
            env.storage().temporary().has(&RevocationKey::ReentrancyLock)
        });
        assert!(
            !lock_present,
            "reentrancy lock must be released after revoke"
        );
    }

    #[test]
    fn test_enter_guard_detects_locked_state() {
        let env = Env::default();
        let contract_id = env.register_contract(None, RevocationRegistry);

        env.as_contract(&contract_id, || {
            env.storage()
                .temporary()
                .set(&RevocationKey::ReentrancyLock, &true);
        });

        let result = env.as_contract(&contract_id, || enter_guard(&env));
        assert_eq!(result, Err(RevocationRegistryError::ReentrancyDetected));
    }

    #[test]
    fn test_exit_guard_releases_lock() {
        let env = Env::default();
        let contract_id = env.register_contract(None, RevocationRegistry);

        env.as_contract(&contract_id, || {
            env.storage()
                .instance()
                .set(&RevocationKey::ReentrancyLock, &true);
            exit_guard(&env);
            // After exit_guard the lock must be gone.
            assert!(!env.storage().temporary().has(&RevocationKey::ReentrancyLock));
            // And enter_guard must now succeed.
            assert!(enter_guard(&env).is_ok());
        });
    }

    #[test]
    fn test_sequential_revokes_succeed() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        let subject = Address::generate(&env);

        for i in 0u8..4 {
            let vc_hash = BytesN::from_array(&env, &[i; 32]);
            assert!(client.try_revoke(&issuer, &subject, &vc_hash).is_ok());
            assert!(client.is_revoked(&vc_hash));
        }
    }
}