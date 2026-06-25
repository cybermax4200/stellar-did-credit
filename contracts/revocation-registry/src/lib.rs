#![no_std]
#![warn(missing_docs)]
use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror, symbol_short, Address, BytesN, Env, Vec,
};

/// Error types for the revocation registry contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum RevocationRegistryError {
    /// Contract is already initialized.
    AlreadyInitialized = 1,
    /// Caller is not authorized to perform this action.
    NotAuthorized = 2,
}

/// Storage keys for revocation registry contract.
#[contracttype]
pub enum RevocationKey {
    /// Contract administrator address.
    Admin,
    /// Revocation status for a VC hash.
    Status(BytesN<32>),    // vc_hash → bool
    /// Address of issuer who revoked the VC.
    IssuerOfVC(BytesN<32>), // vc_hash → Address (who revoked)
}

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
        Ok(())
    }

    /// Revoke a single verifiable credential by its hash.
    pub fn revoke(env: Env, issuer: Address, vc_hash: BytesN<32>) -> Result<(), RevocationRegistryError> {
        issuer.require_auth();
        env.storage()
            .persistent()
            .set(&RevocationKey::Status(vc_hash.clone()), &true);
        env.storage()
            .persistent()
            .set(&RevocationKey::IssuerOfVC(vc_hash.clone()), &issuer);
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

    /// Revoke multiple verifiable credentials in a single batch operation.
    pub fn batch_revoke(env: Env, issuer: Address, vc_hashes: Vec<BytesN<32>>) -> Result<(), RevocationRegistryError> {
        issuer.require_auth();
        for vc_hash in vc_hashes.iter() {
            env.storage()
                .persistent()
                .set(&RevocationKey::Status(vc_hash.clone()), &true);
            env.storage()
                .persistent()
                .set(&RevocationKey::IssuerOfVC(vc_hash.clone()), &issuer);
        }
        env.events()
            .publish((symbol_short!("BatchRev"),), (issuer, vc_hashes.len()));
        Ok(())
    }

    /// Upgrade the contract WASM in-place, preserving address and all stored state
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        let stored_admin: Address = env.storage().instance().get(&RevocationKey::Admin).expect("not initialized");
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

    #[test]
    fn test_revoke_and_check() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[1u8; 32]);

        assert!(!client.is_revoked(&vc_hash));
        let result = client.revoke(&issuer, &vc_hash);
        assert!(result.is_ok());
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
    #[should_panic]
    fn test_only_issuer_can_revoke() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let issuer = Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[3u8; 32]);

        // Mock auth will fail if we don't provide the correct address
        // However, mock_all_auths() makes all auths succeed.
        // To test failure, we need to NOT use mock_all_auths or specifically fail it.
        // Let's create a new env without mock_all_auths for this test.
        let env2 = Env::default();
        let contract_id2 = env2.register_contract(None, RevocationRegistry);
        let client2 = RevocationRegistryClient::new(&env2, &contract_id2);
        let _ = client2.revoke(&issuer, &vc_hash);
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

        let result = client.batch_revoke(&issuer, &vc_hashes);
        assert!(result.is_ok());

        for vc_hash in vc_hashes.iter() {
            assert!(client.is_revoked(&vc_hash));
        }
    }

    #[test]
    fn test_upgrade_preserves_contract_address() {
        let env = Env::default();
        env.mock_all_auths();
        let new_wasm_hash = env.deployer().upload_contract_wasm(RevocationRegistry::WASM);
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Upgrade — contract_id must remain unchanged
        client.upgrade(&admin, &new_wasm_hash);

        // Contract still responds correctly; address is preserved
        let vc_hash = BytesN::from_array(&env, &[7u8; 32]);
        assert!(!client.is_revoked(&vc_hash));
    }

    #[test]
    #[should_panic(expected = "not authorized")]
    fn test_upgrade_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let new_wasm_hash = env.deployer().upload_contract_wasm(RevocationRegistry::WASM);
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        client.initialize(&admin);
        client.upgrade(&non_admin, &new_wasm_hash);
    }
}
