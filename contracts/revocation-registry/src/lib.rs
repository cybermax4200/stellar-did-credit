#![no_std]
//! Revocation registry contract for the Stellar DID Credit protocol.
//!
//! Maintains an on-chain list of revoked verifiable credential hashes.
use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror, symbol_short, Address, BytesN, Env,
    Vec,
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

/// Error types for the revocation registry contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum RevocationRegistryError {
    /// Contract is already initialized.
    AlreadyInitialized = 1,
    /// Caller is not authorized to perform this action.
    NotAuthorized = 2,
    /// VC hash was revoked/registered for a different issuer than the caller.
    IssuerMismatch = 3,
    /// No pending admin proposal exists.
    NoPendingAdmin = 4,
}

/// Storage keys for revocation registry contract.
#[contracttype]
#[allow(missing_docs)]
pub enum RevocationKey {
    /// Contract administrator address.
    Admin,
    /// Pending contract admin address for two-step transfer.
    PendingAdmin,

    /// Registered authority (first issuer) for a VC hash.
    /// vc_hash → Address
    RegisteredVCIssuer(BytesN<32>),

    /// Revocation status for a VC hash.
    Status(BytesN<32>), // vc_hash → bool
    /// Address of issuer who revoked the VC (latest issuer call).
    IssuerOfVC(BytesN<32>), // vc_hash → Address (who revoked)
}

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
        Ok(())
    }

    /// Propose a new contract admin (two-step admin transfer).
    pub fn propose_new_admin(
        env: Env,
        current_admin: Address,
        new_admin: Address,
    ) -> Result<(), RevocationRegistryError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&RevocationKey::Admin)
            .expect("not initialized");
        if current_admin != stored_admin {
            return Err(RevocationRegistryError::NotAuthorized);
        }
        current_admin.require_auth();
        env.storage()
            .instance()
            .set(&RevocationKey::PendingAdmin, &new_admin);
        Ok(())
    }

    /// Accept a proposed admin role (two-step admin transfer).
    ///
    /// Panics if the caller address was not proposed as the next admin.
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), RevocationRegistryError> {
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
            .set(&RevocationKey::Admin, &new_admin);
        env.storage().instance().remove(&RevocationKey::PendingAdmin);
        Ok(())
    }

    /// Revoke a single verifiable credential by its hash.
    pub fn revoke(env: Env, issuer: Address, vc_hash: BytesN<32>) -> Result<(), RevocationRegistryError> {
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
    pub fn batch_revoke(
        env: Env,
        issuer: Address,
        vc_hashes: Vec<BytesN<32>>,
    ) -> Result<(), RevocationRegistryError> {
        issuer.require_auth();
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
        }
        env.events()
            .publish((symbol_short!("BatchRev"),), (issuer, vc_hashes.len()));
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
        client.revoke(&issuer, &vc_hash);
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
        let vc_hash = BytesN::from_array(&env, &[3u8; 32]);

        // First revoke registers issuer_a for this vc_hash.
        client.revoke(&issuer_a, &vc_hash).unwrap();
        assert!(client.is_revoked(&vc_hash));

        // issuer_b must not be able to revoke the same hash.
        let res = client.revoke(&issuer_b, &vc_hash);
        assert_eq!(res, Err(RevocationRegistryError::IssuerMismatch));
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
    fn test_admin_transfer_two_step() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);

        client.initialize(&admin1);
        client.propose_new_admin(&admin1, &admin2);
        client.accept_admin(&admin2);

        // new admin can upgrade
        client.upgrade(&admin2, &BytesN::from_array(&env, &[0u8; 32]));

        // old admin cannot upgrade
        let res = std::panic::catch_unwind(|| {
            client.upgrade(&admin1, &BytesN::from_array(&env, &[1u8; 32]));
        });
        assert!(res.is_err());
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
        client.propose_new_admin(&admin1, &admin2);

        let _ = client.accept_admin(&non_admin);
    }

    #[test]
    #[should_panic(expected = "not authorized")]
    fn test_upgrade_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RevocationRegistry);
        let client = RevocationRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        client.initialize(&admin);
        client.upgrade(&non_admin, &BytesN::from_array(&env, &[0u8; 32]));
    }
}

