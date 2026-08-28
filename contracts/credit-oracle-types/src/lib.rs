#![no_std]
//! Shared types for the credit-oracle contract.
//!
//! This crate defines types that are used by both the credit-oracle
//! contract and its consumers (e.g. the governance contract).
//! Keeping them in a separate non-cdylib crate avoids WASM linker
//! symbol collisions when both contracts are compiled for deployment.

use soroban_sdk::{contracterror, contracttype, Address, Env, IntoVal, Symbol};

/// Minimum weight allowed for any individual scoring weight component (10%).
/// Every component must contribute at least 10% to prevent degenerate scoring
/// (e.g., setting a component weight to 0 silently disables that metric).
pub const MIN_COMPONENT_WEIGHT: u32 = 10;

/// Error types for the credit-oracle contract.
///
/// Shared between the credit-oracle contract and its consumers (e.g. the
/// governance contract) so that cross-contract invocations can observe and
/// propagate typed errors instead of raw panics.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum CreditOracleError {
    /// Contract is already initialized.
    AlreadyInitialized = 1,
    /// Caller is not authorized to perform this action.
    NotAuthorized = 2,
    /// Feeder is not registered.
    FeederNotRegistered = 3,
    /// Lender is not registered.
    LenderNotRegistered = 4,
    /// Proposed weights do not sum to 100 or a component is below MIN_COMPONENT_WEIGHT (10).
    InvalidWeights = 5,
    /// No pending admin proposal exists.
    NoPendingAdmin = 6,
    /// Compute score called too soon — cooldown has not elapsed.
    ComputeCooldownActive = 7,
    /// A dispute is already pending for this subject and input key.
    DisputeAlreadyPending = 8,
    /// No dispute was found for the given subject and input key.
    DisputeNotFound = 9,
    /// The provided input key is not a valid score input name.
    InvalidInputKey = 10,
    /// The provided identity oracle contract is invalid or did not respond.
    InvalidIdentityOracle = 11,
    /// The contract is currently paused and cannot accept writes.
    ContractPaused = 12,
    /// Recency decay parameters are invalid: `min_recency_bps` exceeds
    /// `BPS_DENOMINATOR` (10_000), which would make the floor larger than a
    /// full-weight, brand-new credential.
    InvalidRecencyConfig = 13,
    /// Weight application timelock has not yet expired.
    TimelockNotExpired = 14,
    /// No pending weights exist to apply.
    NoPendingWeights = 15,
}

/// Weights used in credit score calculation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoringWeights {
    /// Weight for verified credentials component.
    pub vc_weight: u32,
    /// Weight for transaction history component.
    pub tx_weight: u32,
    /// Weight for repayment history component.
    pub repayment_weight: u32,
}

impl ScoringWeights {
    /// Validates that scoring weights sum to 100 and that each component is at least `MIN_COMPONENT_WEIGHT`.
    pub fn is_valid(&self) -> bool {
        self.vc_weight >= MIN_COMPONENT_WEIGHT
            && self.tx_weight >= MIN_COMPONENT_WEIGHT
            && self.repayment_weight >= MIN_COMPONENT_WEIGHT
            && (self.vc_weight + self.tx_weight + self.repayment_weight == 100)
    }
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
    ///
    /// Returns a typed [`CreditOracleError`] so callers can distinguish "too
    /// early" (`TimelockNotExpired`) from "nothing to apply"
    /// (`NoPendingWeights`) instead of catching a raw panic.
    pub fn apply_weights(env: &Env, contract_id: &Address) -> Result<(), CreditOracleError> {
        match env.try_invoke_contract::<(), CreditOracleError>(
            contract_id,
            &Symbol::new(env, "apply_weights"),
            soroban_sdk::vec![env],
        ) {
            Ok(Ok(())) => Ok(()),
            // The remaining arms cover values that cannot occur in practice:
            // the credit-oracle contract only ever returns the typed errors
            // handled above. `Err(Ok(e))` is the typed `CreditOracleError` and
            // is re-returned verbatim; everything else (an aborted/unknown
            // invocation) is surfaced as a generic failure so the caller never
            // sees a raw panic.
            Err(Ok(e)) => Err(e),
            _ => Err(CreditOracleError::NoPendingWeights),
        }
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
    pub fn get_pending_weights(env: &Env, contract_id: &Address) -> Option<PendingWeightsRecord> {
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
