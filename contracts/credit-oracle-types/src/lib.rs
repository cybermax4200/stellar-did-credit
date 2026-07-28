#![no_std]
//! Shared types for the credit-oracle contract.
//!
//! This crate defines types that are used by both the credit-oracle
//! contract and its consumers (e.g. the governance contract).
//! Keeping them in a separate non-cdylib crate avoids WASM linker
//! symbol collisions when both contracts are compiled for deployment.

use soroban_sdk::{contracttype, Address, Env, IntoVal, Symbol};

/// Weights used in credit score calculation.
#[contracttype]
#[derive(Clone)]
pub struct ScoringWeights {
    /// Weight for verified credentials component.
    pub vc_weight: u32,
    /// Weight for transaction history component.
    pub tx_weight: u32,
    /// Weight for repayment history component.
    pub repayment_weight: u32,
}

/// Pending weights proposal with timelock.
#[contracttype]
#[derive(Clone)]
pub struct PendingWeightsRecord {
    /// Proposed weights.
    pub weights: ScoringWeights,
    /// Ledger number when these weights become effective.
    pub effective_ledger: u32,
}

/// Cross-contract client for the credit-oracle contract.
///
/// This manual client replaces the auto-generated `CreditOracleClient`
/// so that the governance contract does not need to depend on the
/// credit-oracle `cdylib` crate directly, avoiding WASM linker
/// symbol collisions.
pub struct CreditOracleClient;

impl CreditOracleClient {
    /// Queue new scoring weights in the credit-oracle (starts timelock).
    pub fn propose_weights(env: &Env, contract_id: &Address, weights: &ScoringWeights) {
        let _: () = env.invoke_contract(
            contract_id,
            &Symbol::new(env, "propose_weights"),
            soroban_sdk::vec![env, weights.clone().into_val(env)],
        );
    }

    /// Apply pending weights after the timelock expires.
    pub fn apply_weights(env: &Env, contract_id: &Address) {
        let _: () = env.invoke_contract(
            contract_id,
            &Symbol::new(env, "apply_weights"),
            soroban_sdk::vec![env],
        );
    }

    /// Accept admin role on the credit-oracle contract.
    pub fn accept_admin(env: &Env, contract_id: &Address, new_admin: &Address) {
        let _: () = env.invoke_contract(
            contract_id,
            &Symbol::new(env, "accept_admin"),
            soroban_sdk::vec![env, new_admin.clone().into_val(env)],
        );
    }

    /// Get pending weights from the credit-oracle.
    pub fn get_pending_weights(
        env: &Env,
        contract_id: &Address,
    ) -> Option<PendingWeightsRecord> {
        env.invoke_contract(
            contract_id,
            &Symbol::new(env, "get_pending_weights"),
            soroban_sdk::vec![env],
        )
    }

    /// Get the current scoring weights from the credit-oracle.
    pub fn get_scoring_weights(env: &Env, contract_id: &Address) -> ScoringWeights {
        env.invoke_contract(
            contract_id,
            &Symbol::new(env, "get_scoring_weights"),
            soroban_sdk::vec![env],
        )
    }
}


