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

        let current_ledger = env.ledger().sequence();
        env.storage().persistent().remove(&DataKey::VcAnchor(subject.clone(), vc_hash));
        env.storage().persistent().set(&DataKey::LastStateChange(subject.clone()), &current_ledger);
    }

    pub fn get_last_state_change_ledger(env: Env, subject: Address) -> u32 {
        env.storage().persistent().get(&DataKey::LastStateChange(subject)).unwrap_or(0)
    }
}
