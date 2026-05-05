#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, Address, BytesN, Env, Vec};

#[contracttype]
pub enum RevocationKey {
    Admin,
    Status(BytesN<32>),    // vc_hash → bool
    IssuerOfVC(BytesN<32>), // vc_hash → Address (who revoked)
}

#[contract]
pub struct RevocationRegistry;

#[contractimpl]
impl RevocationRegistry {
    pub fn initialize(_env: Env, _admin: Address) {
        panic!("not yet implemented")
    }

    pub fn revoke(_env: Env, _issuer: Address, _vc_hash: BytesN<32>) {
        panic!("not yet implemented")
    }

    pub fn is_revoked(_env: Env, _vc_hash: BytesN<32>) -> bool {
        panic!("not yet implemented")
    }

    pub fn batch_revoke(_env: Env, _issuer: Address, _vc_hashes: Vec<BytesN<32>>) {
        panic!("not yet implemented")
    }
}

#[cfg(test)]
mod tests {}
