#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, symbol_short, Address, BytesN, Env};

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
/// 3. Return the address so callers can use it for equality checks if needed.
///
/// All admin functions call this helper instead of duplicating the two-step
/// lookup + auth inline.
fn require_admin(env: &Env) -> Address {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("not initialized");
    admin.require_auth();
    admin
}

pub const MIN_SCORE: u32 = 300;
pub const MAX_SCORE: u32 = 850;

/// Error types for the credit-oracle contract.
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
    /// Proposed weights do not sum to 100.
    InvalidWeights = 5,
    /// No pending admin proposal exists.
    NoPendingAdmin = 6,
}


/// Storage keys for the credit oracle contract
#[contracttype]
pub enum DataKey {
    /// Contract administrator address
    Admin,
    /// Pending contract admin address for two-step transfer
    PendingAdmin,

    /// Global configuration
    Config,
    /// Trusted feeder address authorized to update transaction stats
    TrustedFeeder(Address),
    /// Trusted lender address authorized to record repayments
    TrustedLender(Address),
    /// Transaction statistics for a user
    TxStats(Address),
    /// Repayment record for a user
    RepaymentRecord(Address),
    /// Credit score for a user
    Score(Address),
    /// Cached VC count for a user
    VcCount(Address),
    /// Pending weights awaiting timelock
    PendingWeights,
    /// Ledger number when pending weights become effective
    PendingWeightsEffectiveLedger,
}

/// Credit score record with metadata
#[contracttype]
#[derive(Clone)]
pub struct ScoreRecord {
    /// Credit score value
    pub score: u32,
    /// Timestamp of last update
    pub last_updated: u64,
    /// Number of verified credentials
    pub vc_count: u32,
    /// Repayment rate in basis points (0-10000)
    pub repayment_rate: u32,
    /// Transaction volume in last 30 days
    pub tx_volume_30d: i128,
}

/// Transaction statistics for a user
#[contracttype]
#[derive(Clone)]
pub struct TxStats {
    /// Total transaction volume in last 30 days
    pub volume_30d: i128,
    /// Transaction count in last 30 days
    pub tx_count_30d: u32,
    /// Average number of counterparties
    pub avg_counterparties: u32,
}

/// Weights used in credit score calculation
#[contracttype]
#[derive(Clone)]
pub struct ScoringWeights {
    /// Weight for verified credentials component
    pub vc_weight: u32,
    /// Weight for transaction history component
    pub tx_weight: u32,
    /// Weight for repayment history component
    pub repayment_weight: u32,
}

/// Pending weights proposal with timelock
#[contracttype]
#[derive(Clone)]
pub struct PendingWeightsRecord {
    /// Proposed weights
    pub weights: ScoringWeights,
    /// Ledger number when these weights become effective
    pub effective_ledger: u32,
}

/// Internal repayment counters for a subject
#[contracttype]
#[derive(Clone)]
pub struct RepaymentRecord {
    pub on_time_count: u32,
    pub total_count: u32,
}

const TIMELOCK_LEDGERS: u32 = 17_280; // approximately 24 hours

#[contract]
pub struct CreditOracle;

#[contractimpl]
impl CreditOracle {
    /// Initialize the contract with admin and default scoring weights
    pub fn initialize(env: Env, admin: Address) -> Result<(), CreditOracleError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(CreditOracleError::AlreadyInitialized);
        }
        admin.require_auth();

        env.storage().instance().set(&DataKey::Admin, &admin);

        let default_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        env.storage().instance().set(&DataKey::Config, &default_weights);
        Ok(())
    }

    /// Register a trusted feeder address.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn register_feeder(env: Env, admin: Address, feeder: Address) -> Result<(), CreditOracleError> {
        // Verify that the supplied `admin` matches storage and has signed the tx.
        let stored = require_admin(&env);
        if admin != stored {
            return Err(CreditOracleError::NotAuthorized);
        }
        env.storage().persistent().set(&DataKey::TrustedFeeder(feeder.clone()), &true);
        env.events().publish((symbol_short!("FdrReg"),), feeder);
        Ok(())
    }

    /// Deregister a trusted feeder address.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn deregister_feeder(env: Env, admin: Address, feeder: Address) -> Result<(), CreditOracleError> {
        let stored = require_admin(&env);
        if admin != stored {
            return Err(CreditOracleError::NotAuthorized);
        }
        env.storage().persistent().remove(&DataKey::TrustedFeeder(feeder.clone()));
        env.events().publish((symbol_short!("FdrDeReg"),), feeder);
        Ok(())
    }

    /// Register a trusted lender address.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn register_lender(env: Env, admin: Address, lender: Address) -> Result<(), CreditOracleError> {
        let stored = require_admin(&env);
        if admin != stored {
            return Err(CreditOracleError::NotAuthorized);
        }
        env.storage().persistent().set(&DataKey::TrustedLender(lender.clone()), &true);
        env.events().publish((symbol_short!("LndReg"),), lender);
        Ok(())
    }

    /// Deregister a trusted lender address.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn deregister_lender(env: Env, admin: Address, lender: Address) -> Result<(), CreditOracleError> {
        let stored = require_admin(&env);
        if admin != stored {
            return Err(CreditOracleError::NotAuthorized);
        }
        env.storage().persistent().remove(&DataKey::TrustedLender(lender.clone()));
        env.events().publish((symbol_short!("LndDeReg"),), lender);
        Ok(())
    }

    /// Update transaction statistics for a user
    pub fn update_tx_stats(env: Env, feeder: Address, subject: Address, stats: TxStats) -> Result<(), CreditOracleError> {
        feeder.require_auth();
        if !env.storage().persistent().has(&DataKey::TrustedFeeder(feeder.clone())) {
            return Err(CreditOracleError::FeederNotRegistered);
        }
        env.storage().persistent().set(&DataKey::TxStats(subject), &stats);
        Ok(())
    }

    /// Record a repayment event for a user
    pub fn record_repayment(env: Env, lender: Address, subject: Address, _amount: i128, on_time: bool) -> Result<(), CreditOracleError> {
        lender.require_auth();
        if !env.storage().persistent().has(&DataKey::TrustedLender(lender.clone())) {
            return Err(CreditOracleError::LenderNotRegistered);
        }
        let mut record: RepaymentRecord = env.storage().persistent()
            .get(&DataKey::RepaymentRecord(subject.clone()))
            .unwrap_or(RepaymentRecord { on_time_count: 0, total_count: 0 });
        if on_time {
            // saturating_add prevents a wrap-around panic if on_time_count
            // ever reaches u32::MAX (e.g. during fuzz / adversarial input).
            record.on_time_count = record.on_time_count.saturating_add(1);
        }
        // Same reasoning for total_count.
        record.total_count = record.total_count.saturating_add(1);
        env.storage().persistent().set(&DataKey::RepaymentRecord(subject), &record);
        Ok(())
    }

    /// Cache VC count for a subject (feeder-only)
    pub fn set_vc_count(env: Env, feeder: Address, subject: Address, count: u32) -> Result<(), CreditOracleError> {
        feeder.require_auth();
        if !env.storage().persistent().has(&DataKey::TrustedFeeder(feeder.clone())) {
            return Err(CreditOracleError::FeederNotRegistered);
        }
        env.storage().persistent().set(&DataKey::VcCount(subject), &count);
        Ok(())
    }

    /// Compute and store the credit score for `subject`.
    ///
    /// # Open-call design (no auth required)
    ///
    /// This function intentionally requires **no authorization**. Any address on
    /// any ledger may call it for any subject. The rationale is:
    ///
    /// - **Benefit to subject.** Score computation is a pure read + write of
    ///   on-chain data that already belongs to the subject. There is no way to
    ///   harm a subject by computing their score with the data currently in
    ///   storage.
    /// - **Lender convenience.** A lender or application can refresh a score
    ///   immediately before reading it without needing the subject's signature.
    /// - **Feeder tooling.** The off-chain feeder that keeps `TxStats` and
    ///   `VcCount` current can also drive score refresh in the same transaction.
    ///
    /// # Known gap — recomputation spam (Issue 78)
    ///
    /// Because there is no cooldown, a subject (or anyone) could call
    /// `compute_score` many times in rapid succession to land on a favourable
    /// `last_updated` ledger timestamp. A minimum recomputation interval (e.g.
    /// one ledger per subject) would close this gap. Full implementation is
    /// tracked in Issue 78; it is logged here as a known limitation of the
    /// current version.
    pub fn compute_score(env: Env, subject: Address) -> u32 {
        let tx_stats: TxStats = env.storage().persistent()
            .get(&DataKey::TxStats(subject.clone()))
            .unwrap_or(TxStats { volume_30d: 0, tx_count_30d: 0, avg_counterparties: 0 });

        let repayment: RepaymentRecord = env.storage().persistent()
            .get(&DataKey::RepaymentRecord(subject.clone()))
            .unwrap_or(RepaymentRecord { on_time_count: 0, total_count: 0 });

        let vc_count: u32 = env.storage().persistent()
            .get(&DataKey::VcCount(subject.clone()))
            .unwrap_or(0u32);

        // saturating_mul prevents overflow when vc_count is very large (e.g. u32::MAX).
        // The subsequent .min(100) clamp preserves the original scoring cap.
        let vc_score = vc_count.saturating_mul(20).min(100);
        let tx_score = ((tx_stats.volume_30d / 100_000_000i128) as u32).min(100);
        // saturating_mul prevents overflow if on_time_count is pathologically large;
        // checked_div still guards against division by zero (total_count == 0).
        let repay_score = repayment.on_time_count.saturating_mul(10000)
            .checked_div(repayment.total_count)
            .map(|r| r / 100)
            .unwrap_or(0);

        let weights: ScoringWeights = env.storage().instance().get(&DataKey::Config).unwrap();
        // Each component is already clamped to ≤ 100 and weights sum to 100,
        // so the intermediate sums fit comfortably in u32; saturating_add is
        // used as a defence-in-depth measure.
        let composite = vc_score.saturating_mul(weights.vc_weight)
            .saturating_add(tx_score.saturating_mul(weights.tx_weight))
            .saturating_add(repay_score.saturating_mul(weights.repayment_weight))
            / 100;

        // composite ≤ 100, so composite * 550 ≤ 55_000 – well within u32 range;
        // saturating_mul is used for uniformity and future-proofing.
        let score = (MIN_SCORE + composite.saturating_mul(550) / 100).clamp(MIN_SCORE, MAX_SCORE);

        env.storage().persistent().set(&DataKey::Score(subject.clone()), &ScoreRecord {
            score,
            last_updated: env.ledger().timestamp(),
            vc_count,
            // Mirror the saturating_mul used above so the stored rate is
            // computed consistently with the scoring path.
            repayment_rate: repayment.on_time_count.saturating_mul(10000)
                                .checked_div(repayment.total_count)
                                .unwrap_or(0),
            tx_volume_30d: tx_stats.volume_30d,
        });

        score
    }

    /// Get credit score for a user; returns None if score has not been computed yet
    pub fn get_score(env: Env, subject: Address) -> Option<ScoreRecord> {
        env.storage().persistent().get(&DataKey::Score(subject))
    }

    /// Propose new scoring weights with timelock.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn propose_weights(env: Env, weights: ScoringWeights) -> Result<(), CreditOracleError> {
        if weights.vc_weight + weights.tx_weight + weights.repayment_weight != 100 {
            return Err(CreditOracleError::InvalidWeights);
        }
        // require_admin loads the stored admin and calls require_auth() on it.
        require_admin(&env);

        let effective_ledger = env.ledger().sequence() + TIMELOCK_LEDGERS;

        env.storage().instance().set(&DataKey::PendingWeights, &weights);
        env.storage()
            .instance()
            .set(&DataKey::PendingWeightsEffectiveLedger, &effective_ledger);

        env.events().publish(
            (symbol_short!("WtProp"),),
            (weights.vc_weight, weights.tx_weight, weights.repayment_weight, effective_ledger),
        );
        Ok(())
    }

    /// Apply pending weights after timelock expires
    pub fn apply_weights(env: Env) {
        let effective_ledger: u32 = env.storage()
            .instance()
            .get(&DataKey::PendingWeightsEffectiveLedger)
            .expect("no pending weights");

        if env.ledger().sequence() < effective_ledger {
            panic!("timelock not expired");
        }

        let weights: ScoringWeights = env.storage()
            .instance()
            .get(&DataKey::PendingWeights)
            .expect("no pending weights");

        env.storage().instance().set(&DataKey::Config, &weights);

        env.storage().instance().remove(&DataKey::PendingWeights);
        env.storage().instance().remove(&DataKey::PendingWeightsEffectiveLedger);

        env.events().publish(
            (symbol_short!("WtApply"),),
            (weights.vc_weight, weights.tx_weight, weights.repayment_weight),
        );
    }

    /// Get current scoring weights
    pub fn get_scoring_weights(env: Env) -> ScoringWeights {
        env.storage()
            .instance()
            .get(&DataKey::Config)
            .unwrap()
    }

    /// Get pending weights (if any)
    pub fn get_pending_weights(env: Env) -> Option<PendingWeightsRecord> {
        env.storage().instance().get(&DataKey::PendingWeights)
    }

    /// Propose a new contract admin (two-step admin transfer).
    pub fn propose_new_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if current_admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        current_admin.require_auth();
        env.storage().instance().set(&DataKey::PendingAdmin, &new_admin);
        Ok(())
    }

    /// Accept a proposed admin role (two-step admin transfer).
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), CreditOracleError> {
        let pending: Option<Address> = env.storage().instance().get(&DataKey::PendingAdmin);
        match pending {
            Some(p) => {
                if p != new_admin {
                    panic!("not authorized");
                }
            }
            None => return Err(CreditOracleError::NoPendingAdmin),
        }
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
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
    use proptest::prelude::*;
    use soroban_sdk::testutils::{Address as _, Ledger as _};


    #[test]
    fn test_default_weights_sum_to_100() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let w = client.get_scoring_weights();
        assert_eq!(w.vc_weight + w.tx_weight + w.repayment_weight, 100);
    }

    #[test]
    fn test_only_admin_can_register_feeder() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        let feeder = Address::generate(&env);

        client.initialize(&admin);
        let result = client.try_register_feeder(&non_admin, &feeder);
        assert_eq!(result, Err(Ok(CreditOracleError::NotAuthorized)));
    }

    #[test]
    fn test_register_lender_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let lender = Address::generate(&env);

        client.initialize(&admin);
        client.register_lender(&admin, &lender);

        let is_trusted: bool = env.as_contract(&contract_id, || {
            env.storage().persistent().get(&DataKey::TrustedLender(lender.clone())).unwrap_or(false)
        });
        assert!(is_trusted);
    }

    #[test]
    fn test_tx_stats_stored_and_retrieved() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let subject = Address::generate(&env);

        client.initialize(&admin);
        client.register_feeder(&admin, &feeder);
        client.update_tx_stats(&feeder, &subject, &TxStats {
            volume_30d: 5000,
            tx_count_30d: 10,
            avg_counterparties: 3,
        });

        let stored: TxStats = env.as_contract(&contract_id, || {
            env.storage().persistent().get(&DataKey::TxStats(subject.clone())).unwrap()
        });
        assert_eq!(stored.volume_30d, 5000);
        assert_eq!(stored.tx_count_30d, 10);
    }

    #[test]
    fn test_repayment_rate_calculated_correctly() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);

        client.initialize(&admin);
        client.register_lender(&admin, &lender);

        for _ in 0..8 {
            client.record_repayment(&lender, &subject, &1000, &true);
        }
        for _ in 0..2 {
            client.record_repayment(&lender, &subject, &1000, &false);
        }

        let record: RepaymentRecord = env.as_contract(&contract_id, || {
            env.storage().persistent().get(&DataKey::RepaymentRecord(subject.clone())).unwrap()
        });
        let rate = record.on_time_count * 10000 / record.total_count;
        assert_eq!(rate, 8000);
    }

    #[test]
    fn test_base_score_is_300() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let score = client.compute_score(&subject);
        assert_eq!(score, MIN_SCORE);
    }

    #[test]
    fn test_score_increases_with_repayments() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);
        client.register_lender(&admin, &lender);

        for _ in 0..10 {
            client.record_repayment(&lender, &subject, &1000, &true);
        }

        let score = client.compute_score(&subject);
        assert!(score > MIN_SCORE);
    }

    #[test]
    fn test_score_bounded_300_850() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);
        client.register_feeder(&admin, &feeder);
        client.register_lender(&admin, &lender);

        client.set_vc_count(&feeder, &subject, &5);
        client.update_tx_stats(&feeder, &subject, &TxStats {
            volume_30d: 100_000_000_000i128,
            tx_count_30d: 1000,
            avg_counterparties: 100,
        });
        for _ in 0..100 {
            client.record_repayment(&lender, &subject, &1000, &true);
        }

        let score = client.compute_score(&subject);
        assert!(score >= MIN_SCORE);
        assert!(score <= MAX_SCORE);
    }

    #[test]
    fn test_weights_must_sum_to_100() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);
        // Invalid weights — should return error via try_
        let result = client.try_propose_weights(&ScoringWeights { vc_weight: 40, tx_weight: 40, repayment_weight: 40 });
        assert_eq!(result, Err(Ok(CreditOracleError::InvalidWeights)));
    }

    #[test]
    fn test_propose_weights_unchanged_until_applied() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let original_weights = client.get_scoring_weights();
        assert_eq!(original_weights.vc_weight, 40);

        client.propose_weights(&ScoringWeights { vc_weight: 50, tx_weight: 30, repayment_weight: 20 });

        let current_weights = client.get_scoring_weights();
        assert_eq!(current_weights.vc_weight, 40);
    }

    #[test]
    #[should_panic(expected = "timelock not expired")]
    fn test_apply_weights_before_timelock_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);
        client.propose_weights(&ScoringWeights { vc_weight: 50, tx_weight: 30, repayment_weight: 20 });
        client.apply_weights();
    }

    #[test]
    fn test_apply_weights_after_timelock_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);
        client.propose_weights(&ScoringWeights { vc_weight: 50, tx_weight: 25, repayment_weight: 25 });

        // Extend instance TTL before jumping the ledger so it isn't archived.
        let jump = TIMELOCK_LEDGERS + 2;
        env.as_contract(&contract_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger().set_sequence_number(env.ledger().sequence() + jump);
        client.apply_weights();

        let w = client.get_scoring_weights();
        assert_eq!(w.vc_weight, 50);
        assert_eq!(w.tx_weight, 25);
        assert_eq!(w.repayment_weight, 25);
    }

    #[test]
    fn test_deregistered_feeder_cannot_update_tx_stats() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let subject = Address::generate(&env);

        client.initialize(&admin);
        client.register_feeder(&admin, &feeder);
        client.update_tx_stats(&feeder, &subject, &TxStats { volume_30d: 5000, tx_count_30d: 10, avg_counterparties: 3 });
        client.deregister_feeder(&admin, &feeder);
        let result = client.try_update_tx_stats(&feeder, &subject, &TxStats { volume_30d: 6000, tx_count_30d: 11, avg_counterparties: 4 });
        assert_eq!(result, Err(Ok(CreditOracleError::FeederNotRegistered)));
    }

    #[test]
    fn test_deregistered_lender_cannot_record_repayment() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);

        client.initialize(&admin);
        client.register_lender(&admin, &lender);
        client.record_repayment(&lender, &subject, &1000, &true);
        client.deregister_lender(&admin, &lender);
        let result = client.try_record_repayment(&lender, &subject, &1000, &true);
        assert_eq!(result, Err(Ok(CreditOracleError::LenderNotRegistered)));
    }

    /// Verifies that a u32::MAX vc_count does not panic and that the final
    /// score stays within the documented [MIN_SCORE, MAX_SCORE] range.
    #[test]
    fn test_vc_score_saturating_at_max() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);
        client.register_feeder(&admin, &feeder);

        // Feed an extreme vc_count; saturating_mul must prevent a panic here.
        client.set_vc_count(&feeder, &subject, &u32::MAX);

        // Should not panic.
        let score = client.compute_score(&subject);

        // The VC contribution is clamped to 100 before weighting, so the
        // final score must still fall within the documented bounds.
        assert!(score >= MIN_SCORE, "score below MIN_SCORE: {score}");
        assert!(score <= MAX_SCORE, "score above MAX_SCORE: {score}");

        // With only vc_count set (no tx stats, no repayments) and default
        // weights (vc=40), the VC component contributes:
        //   vc_score=100, composite = 100*40/100 = 40
        //   score = 300 + 40*550/100 = 300 + 220 = 520
        assert_eq!(score, 520, "unexpected score with max vc_count");
    }

    #[test]
    #[should_panic(expected = "not authorized")]
    fn test_upgrade_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        client.initialize(&admin);
        client.upgrade(&non_admin, &BytesN::from_array(&env, &[0u8; 32]));
    }

    #[test]
    fn test_admin_transfer_two_step() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let feeder = Address::generate(&env);

        client.initialize(&admin1);

        client.propose_new_admin(&admin1, &admin2);
        client.accept_admin(&admin2);

        // new admin can register feeder
        client.register_feeder(&admin2, &feeder);

        // old admin cannot register feeder
        let feeder2 = Address::generate(&env);
        let res = client.try_register_feeder(&admin1, &feeder2);
        assert_eq!(res, Err(Ok(CreditOracleError::NotAuthorized)));
    }

    #[test]
    #[should_panic(expected = "not authorized")]
    fn test_non_pending_admin_cannot_accept() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let non_admin = Address::generate(&env);

        client.initialize(&admin1);
        client.propose_new_admin(&admin1, &admin2);

        let _ = client.accept_admin(&non_admin);
    }

    fn setup_and_compute_score(
        vc_count: u32,
        volume_30d: i64,
        on_time_count: u32,
        total_count: u32,
        weights: ScoringWeights,
    ) -> u32 {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);

        client.initialize(&admin);
        client.register_feeder(&admin, &feeder);
        client.register_lender(&admin, &lender);

        // Apply weights immediately by setting pending weights and jumping beyond timelock.
        client.propose_weights(&weights);
        let jump = TIMELOCK_LEDGERS + 2;
        env.as_contract(&contract_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger().set_sequence_number(env.ledger().sequence() + jump);
        client.apply_weights();

        client.set_vc_count(&feeder, &subject, &vc_count);
        client.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: volume_30d as i128,
                tx_count_30d: 0,
                avg_counterparties: 0,
            },
        );

        // Record repayments to build the repayment counters.
        // Use exact counts instead of relying on randomness for test stability.
        for _ in 0..on_time_count {
            client.record_repayment(&lender, &subject, &0, &true);
        }
        let late = total_count.saturating_sub(on_time_count);
        for _ in 0..late {
            client.record_repayment(&lender, &subject, &0, &false);
        }

        client.compute_score(&subject)
    }

    proptest! {
        #[test]
        fn proptest_score_always_in_range(
            vc_count in any::<u32>(),
            volume_30d in any::<i64>(),
            on_time in any::<u32>(),
            total in any::<u32>(),
        ) {
            // Ensure a valid repayment state: on_time <= total.
            let total_count = total;
            let on_time_count = on_time.min(total_count);

            let weights = ScoringWeights { vc_weight: 40, tx_weight: 30, repayment_weight: 30 };
            let score = setup_and_compute_score(
                vc_count,
                volume_30d,
                on_time_count,
                total_count,
                weights,
            );
            prop_assert!(score >= MIN_SCORE && score <= MAX_SCORE);
        }
    }

    proptest! {
        #[test]
        fn proptest_score_monotone_on_repayment(
            vc_count in 0u32..100u32,
            volume_30d in any::<i64>(),
            total1 in 1u32..500u32,
            on_time1 in 0u32..500u32,
            extra in 0u32..500u32,
        ) {
            let on_time1 = on_time1.min(total1);
            let total2 = total1 + 1; // keep close to maximize boundary coverage

            // Construct on-time ratio that is >= ratio1 after truncation effects.
            // We do it via counts: target ratio2 uses on_time2 = on_time1*(total2)/total1 rounded up.
            let on_time2 = ((on_time1 as u128) * (total2 as u128) + (total1 as u128) - 1) / (total1 as u128);
            let on_time2 = on_time2.min(total2 as u128) as u32;

            let weights = ScoringWeights { vc_weight: 40, tx_weight: 30, repayment_weight: 30 };

            let score1 = setup_and_compute_score(
                vc_count,
                volume_30d,
                on_time1,
                total1,
                weights.clone(),
            );

            let score2 = setup_and_compute_score(
                vc_count,
                volume_30d,
                on_time2,
                total2,
                weights,
            );

            prop_assert!(score2 >= score1);
        }
    }

    proptest! {
        #[test]
        fn proptest_no_panic_on_any_valid_weights(
            a in 0u32..=100u32,
            b in 0u32..=100u32,
            c in 0u32..=100u32,
            vc_count in any::<u32>(),
            volume_30d in any::<i64>(),
            on_time in any::<u32>(),
            total in any::<u32>(),
        ) {
            prop_assume!(a + b + c == 100);

            let on_time_count = on_time.min(total);
            let weights = ScoringWeights { vc_weight: a, tx_weight: b, repayment_weight: c };

            // Should never panic for valid weights; also should always be bounded.
            let score = setup_and_compute_score(
                vc_count,
                volume_30d,
                on_time_count,
                total,
                weights,
            );
            prop_assert!(score >= MIN_SCORE && score <= MAX_SCORE);
        }
    }
}



