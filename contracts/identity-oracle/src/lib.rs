#![no_std]
#[allow(unused_imports)]
use soroban_sdk::{contract, contractimpl, contracttype, Address, BytesN, Env, String, Vec};

/// Storage key variants for the identity-oracle contract.
#[contracttype]
pub enum DataKey {
    /// The contract administrator address.
    Admin,
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

#[contractimpl]
impl IdentityOracle {
    pub fn initialize(_env: Env, _admin: Address) {
        // TODO: store admin in instance storage
        panic!("not yet implemented")
    }

    pub fn register_issuer(_env: Env, _admin: Address, _issuer: Address) {
        // TODO: require admin auth, store issuer as trusted
        panic!("not yet implemented")
    }

    pub fn anchor_did(_env: Env, _subject: Address, _did_doc_cid: String) {
        // TODO: require subject auth, store CID, emit DIDAnchored event
        panic!("not yet implemented")
    }

    pub fn anchor_vc(
        _env: Env,
        _issuer: Address,
        _subject: Address,
        _vc_hash: BytesN<32>,
    ) {
        // TODO: require issuer auth, check issuer is trusted, store VCRecord
        panic!("not yet implemented")
    }

    pub fn is_verified(_env: Env, _subject: Address) -> bool {
        // TODO: return true if subject has >= 1 non-revoked VC
        panic!("not yet implemented")
    }

    pub fn get_vc_count(_env: Env, _subject: Address) -> u32 {
        panic!("not yet implemented")
    }

    pub fn verify_vc(_env: Env, _subject: Address, _vc_hash: BytesN<32>) -> bool {
        panic!("not yet implemented")
    }
}

#[cfg(test)]
mod tests {}
