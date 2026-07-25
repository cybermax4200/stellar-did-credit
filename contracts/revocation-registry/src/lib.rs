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
        .get(&RevocationKey::Admin)
        .expect("not initialized");
    admin.require_auth();
    admin
}

fn ensure_not_paused(env: &Env) -> Result<(), RevocationRegistryError> {
    if env.storage().instance().get(&RevocationKey::Paused).unwrap_or(false) {
        Err(RevocationRegistryError::ContractPaused)
    } else {
        Ok(())
    }
}

/// Error types for the revocation registry contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum RevocationRegistryError {
    /// Contract is already initialized.
    AlreadyInitialized = 1,
    /// Caller is not authorized to perform this action.
    NotAuthorized = 2,
    /// VC hash was revoked/registered for a different issuer than the caller.
    IssuerMismatch = 3,
    /// No pending admin proposal exists.
    NoPendingAdmin = 4,
    /// Batch size exceeds maximum allowed.
    BatchTooLarge = 5,
    /// The contract is currently paused and cannot accept writes.
    ContractPaused = 6,
}

/// Storage keys for revocation registry contract.
#[contracttype]
pub enum RevocationKey {
    /// Contract administrator address.
    Admin,
    /// Whether the contract is currently paused for writes.
    Paused,
    /// Pending contract admin address for two-step transfer.
    PendingAdmin,
    /// Identity-oracle contract ID for callback sync.
    IdentityOracleId,

    /// Registered authority (first issuer) for a VC hash.
    /// vc_hash → Address
    RegisteredVCIssuer(BytesN<32>),

    /// Revocation status for a VC hash.
    Status(BytesN<32>), // vc_hash → bool
    /// Address of issuer who revoked the VC (latest issuer call).
    IssuerOfVC(BytesN<32>), // vc_hash → Address (who revoked)

    /// List of revoked VC hashes per issuer.
    /// issuer → Vec<BytesN<32>>
    IssuerRevokedList(Address),
}

// ── Instance TTL bump constants ──────────────────────────────────
// Used by admin-gated functions to extend instance storage.
const INSTANCE_BUMP_THRESHOLD: u32 = 5000;
const INSTANCE_BUMP_AMOUNT: u32 = 500_000;

// ── Persistent TTL constants ─────────────────────────────────────
// Extend persistent entries to ~30 days on every write.
const PERS_TTL_THRESHOLD: u32 = 120_960; // ~7 days
const PERS_TTL_EXTEND: u32 = 518_400; // ~30 days

/// On-chain revocation registry contract.
#[contract]
pub struct RevocationRegistry;

#[contractimpl]
impl RevocationRegistry {
    /// Initialize the revocation registry with an administrator address.
    pub fn initialize(env: Env, admin: Address) -> Result<(), RevocationRegistryError> {
        if env.storage().instance().has(&RevocationKey::Admin) {
            return Err(RevocationRegistryError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&RevocationKey::Admin, &admin);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        Ok(())
    }

    /// Set the identity-oracle contract ID for revocation callbacks.
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

    /// Propose a new contract admin (two-step admin transfer).
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

    /// Accept a proposed admin role (two-step admin transfer).
    ///
    /// Panics if the caller address was not proposed as the next admin.
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        let pending: Option<Address> = env.storage().instance().get(&RevocationKey::PendingAdmin);
        match pending {
            Some(p) => {
                if p != new_admin {
                    panic!("not authorized");
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

    /// Revoke a single verifiable credential by its hash.
    pub fn revoke(
        env: Env,
        issuer: Address,
        subject: Address,
        vc_hash: BytesN<32>,
    ) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        issuer.require_auth();

        // Enforce authority per vc_hash: the first issuer that revokes a hash becomes the registered authority.
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
        }
        // Extend TTL for both revocation entries
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
        env.events()
            .publish((symbol_short!("Revoked"),), (issuer, vc_hash));
        Ok(())
    }

    /// Check if a verifiable credential has been revoked.
    pub fn is_revoked(env: Env, vc_hash: BytesN<32>) -> bool {
        env.storage()
            .persistent()
            .get(&RevocationKey::Status(vc_hash))
            .unwrap_or(false)
    }

    /// Get the issuer that most recently revoked the given verifiable credential.
    ///
    /// Returns `Some(Address)` when the VC hash exists in storage under
    /// `IssuerOfVC` and `None` when the VC hash has never been revoked.
    pub fn get_revocation_record(env: Env, vc_hash: BytesN<32>) -> Option<Address> {
        env.storage()
            .persistent()
            .get(&RevocationKey::IssuerOfVC(vc_hash))
    }

    /// Returns the number of revoked VCs for an issuer.
    ///
    /// Read-only function requiring no authorization.
    pub fn get_revocation_count(env: Env, issuer: Address) -> u32 {
        let list_key = RevocationKey::IssuerRevokedList(issuer);
        let list: Vec<BytesN<32>> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or(Vec::new(&env));
        list.len()
    }

    /// Returns a paginated list of revoked VC hashes for an issuer.
    ///
    /// Read-only function requiring no authorization.
    ///
    /// Parameters:
    /// - `issuer`: Address of the issuer whose revocations are queried.
    /// - `cursor`: Starting index (0-based).
    /// - `limit`: Maximum number of items to return.
    pub fn list_revoked(
        env: Env,
        issuer: Address,
        cursor: u32,
        limit: u32,
    ) -> Vec<BytesN<32>> {
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

    /// Revoke multiple verifiable credentials in a single batch operation.
    ///
    /// This operation is atomic (all-or-nothing). If any VC hash in the batch fails
    /// (e.g., due to an `IssuerMismatch`), the entire transaction aborts and no
    /// revocations from the batch are persisted.
    ///
    /// Calling `batch_revoke` with an empty `vc_hashes` vector is a valid no-op,
    /// returning `Ok(())` without modifying state or adding revocations.
    pub fn batch_revoke(
        env: Env,
        issuer: Address,
        vc_hashes: Vec<BytesN<32>>,
    ) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        if vc_hashes.len() > 100 {
            return Err(RevocationRegistryError::BatchTooLarge);
        }
        issuer.require_auth();

        let list_key = RevocationKey::IssuerRevokedList(issuer.clone());
        let mut list: Vec<BytesN<32>> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or(Vec::new(&env));
        let mut list_modified = false;

        for vc_hash in vc_hashes.iter() {
            // Enforce authority per vc_hash: the first issuer that revokes a hash becomes the registered authority.
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
            // Extend TTL for each revocation entry
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
        }

        env.events()
            .publish((symbol_short!("BatchRev"),), (issuer, vc_hashes.len()));
        Ok(())
    }

    /// Admin-only maintenance: extend instance storage TTL so the
    /// Admin entry does not expire on an idle contract.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn maintain_storage(env: Env) -> Result<(), RevocationRegistryError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        Ok(())
    }

    /// Upgrade the contract WASM in-place, preserving address and all stored state.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        ensure_not_paused(&env).unwrap();
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use soroban_sdk::{testutils::Address as _, Env};

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

        // First revoke registers issuer_a for this vc_hash.
        client.revoke(&issuer_a, &subject, &vc_hash);
        client.revoke(&issuer_a, &subject, &vc_hash);

        // issuer_b must not be able to revoke the same hash.
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

        // new admin can perform admin-gated actions
        client.propose_new_admin(&admin3);
    }

    #[test]
    #[should_panic(expected = "not authorized")]
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

        let _ = client.accept_admin(&non_admin);
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

            // Revoking the same hash again should be idempotent
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

        // 1. Single revocation
        let hash1 = BytesN::from_array(&env, &[10u8; 32]);
        let subject = Address::generate(&env);
        client.revoke(&issuer, &subject, &hash1);

        assert_eq!(client.get_revocation_count(&issuer), 1);
        let list1 = client.list_revoked(&issuer, &0, &10);
        assert_eq!(list1.len(), 1);
        assert_eq!(list1.get(0).unwrap(), hash1);

        // 2. Batch revocation of 3 hashes
        let mut batch = Vec::new(&env);
        let hash2 = BytesN::from_array(&env, &[20u8; 32]);
        let hash3 = BytesN::from_array(&env, &[30u8; 32]);
        let hash4 = BytesN::from_array(&env, &[40u8; 32]);
        batch.push_back(hash2.clone());
        batch.push_back(hash3.clone());
        batch.push_back(hash4.clone());

        client.batch_revoke(&issuer, &batch);

        assert_eq!(client.get_revocation_count(&issuer), 4);

        // Test pagination
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

        // Re-revoking the same hash should not increase count or duplicate entry
        client.revoke(&issuer, &subject, &vc_hash);
        assert_eq!(client.get_revocation_count(&issuer), 1);
        let list = client.list_revoked(&issuer, &0, &10);
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).unwrap(), vc_hash);
    }
}
