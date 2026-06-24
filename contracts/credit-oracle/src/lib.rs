#![no_std]
#![warn(missing_docs)]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, symbol_short, Address, Env};

/// Error types for the credit-oracle contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum CreditOracleError {
    /// Contract is already initialized.
    AlreadyInitialized = 1,
    /// Caller is not authorized to perform this action.
    NotAuthorized = 2,
    /// Feeder is not registered with the contract.
    FeederNotRegistered = 3,
    /// Lender is not registered with the contract.
    LenderNotRegistered = 4,
    /// Weights must sum to 100.
    InvalidWeights = 5,
    /// Timelock has not expired yet.
    TimelockNotExpired = 6,
    /// No pending weights to apply.
    NoPendingWeights = 7,
}

/// Storage keys for the credit oracle contract
#[contracttype]
pub enum DataKey {
    /// Contract administrator address
    Admin,
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
    /// Pending weights proposed by admin
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

    /// Register a trusted feeder address
    pub fn register_feeder(env: Env, admin: Address, feeder: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage().persistent().set(&DataKey::TrustedFeeder(feeder.clone()), &true);
        env.events().publish((symbol_short!("FdrReg"),), feeder);
        Ok(())
    }

    /// Deregister a trusted feeder address
    pub fn deregister_feeder(env: Env, admin: Address, feeder: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage().persistent().remove(&DataKey::TrustedFeeder(feeder.clone()));
        env.events().publish((symbol_short!("FdrDeReg"),), feeder);
        Ok(())
    }

    /// Register a trusted lender address
    pub fn register_lender(env: Env, admin: Address, lender: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage().persistent().set(&DataKey::TrustedLender(lender.clone()), &true);
        env.events().publish((symbol_short!("LndReg"),), lender);
        Ok(())
    }

    /// Deregister a trusted lender address
    pub fn deregister_lender(env: Env, admin: Address, lender: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
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
            record.on_time_count += 1;
        }
        record.total_count += 1;
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

    /// Compute and store credit score for a user
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

        let vc_score = (vc_count * 20).min(100);
        let tx_score = ((tx_stats.volume_30d / 100_000_000i128) as u32).min(100);
        let repay_score = (repayment.on_time_count * 10000)
            .checked_div(repayment.total_count)
            .map(|r| r / 100)
            .unwrap_or(0);

        let weights: ScoringWeights = env.storage().instance().get(&DataKey::Config).unwrap();
        let composite = (vc_score * weights.vc_weight
            + tx_score * weights.tx_weight
            + repay_score * weights.repayment_weight)
            / 100;

        let score = (300 + composite * 550 / 100).clamp(300, 850);

        env.storage().persistent().set(&DataKey::Score(subject.clone()), &ScoreRecord {
            score,
            last_updated: env.ledger().timestamp(),
            vc_count,
            repayment_rate: (repayment.on_time_count * 10000)
                                .checked_div(repayment.total_count)
                                .unwrap_or(0),
            tx_volume_30d: tx_stats.volume_30d,
        });

        score
    }

    /// Get credit score for a user
    pub fn get_score(env: Env, subject: Address) -> ScoreRecord {
        env.storage().persistent()
            .get(&DataKey::Score(subject))
            .expect("score not computed")
    }

    /// Update scoring weights (must sum to 100)
    pub fn update_weights(env: Env, weights: ScoringWeights) -> Result<(), CreditOracleError> {
        if weights.vc_weight + weights.tx_weight + weights.repayment_weight != 100 {
            return Err(CreditOracleError::InvalidWeights);
        }
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        stored_admin.require_auth();
        env.storage().instance().set(&DataKey::Config, &weights);
        Ok(())
    }

    /// Propose new weights with timelock (~24 hours)
    pub fn propose_weights(env: Env, admin: Address, weights: ScoringWeights) -> Result<(), CreditOracleError> {
        if weights.vc_weight + weights.tx_weight + weights.repayment_weight != 100 {
            return Err(CreditOracleError::InvalidWeights);
        }
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();

        const TIMELOCK_LEDGERS: u32 = 17280; // approximately 24 hours at 5-second blocks
        let current_ledger = env.ledger().sequence();
        let effective_ledger = current_ledger + TIMELOCK_LEDGERS;

        let pending = PendingWeightsRecord {
            weights: weights.clone(),
            effective_ledger,
        };

        env.storage().instance().set(&DataKey::PendingWeights, &pending);
        env.events().publish(
            (symbol_short!("WtProp"),),
            (weights.vc_weight, weights.tx_weight, weights.repayment_weight, effective_ledger),
        );
        Ok(())
    }

    /// Apply pending weights after timelock expires
    pub fn apply_weights(env: Env, admin: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();

        let pending: PendingWeightsRecord = env.storage()
            .instance()
            .get(&DataKey::PendingWeights)
            .ok_or(CreditOracleError::NoPendingWeights)?;

        let current_ledger = env.ledger().sequence();
        if current_ledger < pending.effective_ledger {
            return Err(CreditOracleError::TimelockNotExpired);
        }

        let weights = pending.weights.clone();
        env.storage().instance().set(&DataKey::Config, &weights);
        env.storage().instance().remove(&DataKey::PendingWeights);

        env.events().publish(
            (symbol_short!("WtApply"),),
            (weights.vc_weight, weights.tx_weight, weights.repayment_weight),
        );
        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_default_weights_sum_to_100() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let result = client.initialize(&admin);
        assert!(result.is_ok());

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

        let _ = client.initialize(&admin);
        let result = client.register_feeder(&non_admin, &feeder);
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

        let _ = client.initialize(&admin);
        let result = client.register_lender(&admin, &lender);
        assert!(result.is_ok());

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

        let _ = client.initialize(&admin);
        let _ = client.register_feeder(&admin, &feeder);
        let result = client.update_tx_stats(&feeder, &subject, &TxStats {
            volume_30d: 5000,
            tx_count_30d: 10,
            avg_counterparties: 3,
        });
        assert!(result.is_ok());

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

        let _ = client.initialize(&admin);
        let _ = client.register_lender(&admin, &lender);

        for _ in 0..8 {
            let _ = client.record_repayment(&lender, &subject, &1000, &true);
        }
        for _ in 0..2 {
            let _ = client.record_repayment(&lender, &subject, &1000, &false);
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
        let _ = client.initialize(&admin);

        let score = client.compute_score(&subject);
        assert_eq!(score, 300);
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
        let _ = client.initialize(&admin);
        let _ = client.register_lender(&admin, &lender);

        for _ in 0..10 {
            let _ = client.record_repayment(&lender, &subject, &1000, &true);
        }

        let score = client.compute_score(&subject);
        assert!(score > 300);
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
        let _ = client.initialize(&admin);
        let _ = client.register_feeder(&admin, &feeder);
        let _ = client.register_lender(&admin, &lender);

        // max vc_count
        let _ = client.set_vc_count(&feeder, &subject, &5);
        // max tx volume
        let _ = client.update_tx_stats(&feeder, &subject, &TxStats {
            volume_30d: 100_000_000_000i128,
            tx_count_30d: 1000,
            avg_counterparties: 100,
        });
        // 100% repayment
        for _ in 0..100 {
            let _ = client.record_repayment(&lender, &subject, &1000, &true);
        }

        let score = client.compute_score(&subject);
        assert!(score >= 300);
        assert!(score <= 850);
    }

    #[test]
    fn test_weights_must_sum_to_100() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let _ = client.initialize(&admin);
        let result = client.update_weights(&ScoringWeights { vc_weight: 40, tx_weight: 40, repayment_weight: 40 });
        assert_eq!(result, Err(Ok(CreditOracleError::InvalidWeights)));
    }

    #[test]
    fn test_propose_weights_stores_pending() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let _ = client.initialize(&admin);

        let new_weights = ScoringWeights { vc_weight: 50, tx_weight: 30, repayment_weight: 20 };
        let result = client.propose_weights(&admin, &new_weights);
        assert!(result.is_ok());

        let original_weights = client.get_scoring_weights();
        assert_eq!(original_weights.vc_weight, 40);
        assert_eq!(original_weights.tx_weight, 30);
        assert_eq!(original_weights.repayment_weight, 30);

        let pending = client.get_pending_weights();
        assert!(pending.is_some());
        let pending_record = pending.unwrap();
        assert_eq!(pending_record.weights.vc_weight, 50);
        assert_eq!(pending_record.weights.tx_weight, 30);
        assert_eq!(pending_record.weights.repayment_weight, 20);
    }

    #[test]
    fn test_apply_weights_fails_before_timelock() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let _ = client.initialize(&admin);

        let new_weights = ScoringWeights { vc_weight: 50, tx_weight: 30, repayment_weight: 20 };
        let _ = client.propose_weights(&admin, &new_weights);

        let result = client.apply_weights(&admin);
        assert_eq!(result, Err(Ok(CreditOracleError::TimelockNotExpired)));
    }

    #[test]
    fn test_apply_weights_succeeds_after_timelock() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let _ = client.initialize(&admin);

        let new_weights = ScoringWeights { vc_weight: 50, tx_weight: 30, repayment_weight: 20 };
        let _ = client.propose_weights(&admin, &new_weights);

        env.as_contract(&contract_id, || {
            let pending: PendingWeightsRecord = env.storage()
                .instance()
                .get(&DataKey::PendingWeights)
                .unwrap();
            let effective_ledger = pending.effective_ledger;

            env.ledger().with_mut(|mut ledger| {
                ledger.sequence = effective_ledger;
            });
        });

        let result = client.apply_weights(&admin);
        assert!(result.is_ok());

        let updated_weights = client.get_scoring_weights();
        assert_eq!(updated_weights.vc_weight, 50);
        assert_eq!(updated_weights.tx_weight, 30);
        assert_eq!(updated_weights.repayment_weight, 20);
    }

    #[test]
    fn test_deregister_feeder_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);

        let _ = client.initialize(&admin);
        let _ = client.register_feeder(&admin, &feeder);

        let is_trusted_before: bool = env.as_contract(&contract_id, || {
            env.storage().persistent().get(&DataKey::TrustedFeeder(feeder.clone())).unwrap_or(false)
        });
        assert!(is_trusted_before);

        let result = client.deregister_feeder(&admin, &feeder);
        assert!(result.is_ok());

        let is_trusted_after: bool = env.as_contract(&contract_id, || {
            env.storage().persistent().get(&DataKey::TrustedFeeder(feeder.clone())).unwrap_or(false)
        });
        assert!(!is_trusted_after);
    }

    #[test]
    fn test_deregistered_feeder_cannot_update_stats() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let subject = Address::generate(&env);

        let _ = client.initialize(&admin);
        let _ = client.register_feeder(&admin, &feeder);

        let result = client.update_tx_stats(&feeder, &subject, &TxStats {
            volume_30d: 5000,
            tx_count_30d: 10,
            avg_counterparties: 3,
        });
        assert!(result.is_ok());

        let _ = client.deregister_feeder(&admin, &feeder);

        let result = client.update_tx_stats(&feeder, &subject, &TxStats {
            volume_30d: 6000,
            tx_count_30d: 11,
            avg_counterparties: 4,
        });
        assert_eq!(result, Err(Ok(CreditOracleError::FeederNotRegistered)));
    }

    #[test]
    fn test_deregister_lender_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let lender = Address::generate(&env);

        let _ = client.initialize(&admin);
        let _ = client.register_lender(&admin, &lender);

        let is_trusted_before: bool = env.as_contract(&contract_id, || {
            env.storage().persistent().get(&DataKey::TrustedLender(lender.clone())).unwrap_or(false)
        });
        assert!(is_trusted_before);

        let result = client.deregister_lender(&admin, &lender);
        assert!(result.is_ok());

        let is_trusted_after: bool = env.as_contract(&contract_id, || {
            env.storage().persistent().get(&DataKey::TrustedLender(lender.clone())).unwrap_or(false)
        });
        assert!(!is_trusted_after);
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

        let _ = client.initialize(&admin);
        let _ = client.register_lender(&admin, &lender);

        let result = client.record_repayment(&lender, &subject, &1000, &true);
        assert!(result.is_ok());

        let _ = client.deregister_lender(&admin, &lender);

        let result = client.record_repayment(&lender, &subject, &1000, &true);
        assert_eq!(result, Err(Ok(CreditOracleError::LenderNotRegistered)));
    }
}

