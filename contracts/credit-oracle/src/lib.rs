#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, symbol_short, Address, BytesN, Env};

/// Minimum credit score (no history).
pub const MIN_SCORE: u32 = 300;
/// Maximum credit score (exceptional history).
pub const MAX_SCORE: u32 = 850;

/// Error types for the credit-oracle contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum CreditOracleError {
    /// Contract is already initialized.
    AlreadyInitialized = 1,
    /// Caller is not authorized to perform this action.
    NotAuthorized = 2,
    /// Feeder is not registered as a trusted feeder.
    FeederNotRegistered = 3,
    /// Lender is not registered as a trusted lender.
    LenderNotRegistered = 4,
    /// Proposed weights do not sum to 100.
    InvalidWeights = 5,
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

    /// Update transaction statistics for a subject.
    ///
    /// **WARNING: This is a full replace, not a partial update.** Every call overwrites the entire
    /// `TxStats` struct. If you only intend to update `volume_30d`, you MUST also supply the
    /// current values of `tx_count_30d` and `avg_counterparties`; omitting them will silently
    /// zero those fields and degrade the subject's credit score.
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

        let weights: ScoringWeights = env.storage().instance().get(&DataKey::Config).expect("not initialized");
        let composite = (vc_score * weights.vc_weight
            + tx_score * weights.tx_weight
            + repay_score * weights.repayment_weight)
            / 100;

        let score = (MIN_SCORE + composite * 550 / 100).clamp(MIN_SCORE, MAX_SCORE);

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

    /// Get credit score for a user; returns None if score has not been computed yet
    pub fn get_score(env: Env, subject: Address) -> Option<ScoreRecord> {
        env.storage().persistent().get(&DataKey::Score(subject))
    }

    /// Propose new scoring weights with a 24-hour timelock. Admin only.
    pub fn propose_weights(env: Env, weights: ScoringWeights) -> Result<(), CreditOracleError> {
        if weights.vc_weight + weights.tx_weight + weights.repayment_weight != 100 {
            return Err(CreditOracleError::InvalidWeights);
        }
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        stored_admin.require_auth();

        let effective_ledger = env.ledger().sequence() + TIMELOCK_LEDGERS;
        env.storage().instance().set(&DataKey::PendingWeights, &weights);
        env.storage().instance().set(&DataKey::PendingWeightsEffectiveLedger, &effective_ledger);
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
        env.storage().instance().get(&DataKey::Config).expect("not initialized")
    }

    /// Get pending weights proposal (if any)
    pub fn get_pending_weights(env: Env) -> Option<PendingWeightsRecord> {
        env.storage().instance().get(&DataKey::PendingWeights)
    }

    /// Upgrade the contract WASM in-place, preserving address and all stored state.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
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
    use soroban_sdk::testutils::Address as _;

    fn setup() -> (soroban_sdk::Env, Address, CreditOracleClient) {
        let env = soroban_sdk::Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        (env, admin, client)
    }

    #[test]
    fn test_default_weights_sum_to_100() {
        let (_, _, client) = setup();
        let w = client.get_scoring_weights();
        assert_eq!(w.vc_weight + w.tx_weight + w.repayment_weight, 100);
    }

    #[test]
    fn test_only_admin_can_register_feeder() {
        let (env, _, client) = setup();
        let non_admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let result = client.register_feeder(&non_admin, &feeder);
        assert_eq!(result, Err(Ok(CreditOracleError::NotAuthorized)));
    }

    #[test]
    fn test_register_lender_succeeds() {
        let (env, admin, client) = setup();
        let contract_id = env.register_contract(None, CreditOracle);
        let lender = Address::generate(&env);
        let result = client.register_lender(&admin, &lender);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tx_stats_stored_and_retrieved() {
        let (env, admin, client) = setup();
        let contract_id = env.register_contract(None, CreditOracle);
        let feeder = Address::generate(&env);
        let subject = Address::generate(&env);
        client.register_feeder(&admin, &feeder);
        let result = client.update_tx_stats(&feeder, &subject, &TxStats {
            volume_30d: 5000,
            tx_count_30d: 10,
            avg_counterparties: 3,
        });
        assert!(result.is_ok());
    }

    #[test]
    fn test_repayment_rate_calculated_correctly() {
        let (env, admin, client) = setup();
        let contract_id = env.register_contract(None, CreditOracle);
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);
        client.register_lender(&admin, &lender);
        for _ in 0..8 {
            client.record_repayment(&lender, &subject, &1000, &true);
        }
        for _ in 0..2 {
            client.record_repayment(&lender, &subject, &1000, &false);
        }
        // 8 on-time out of 10 = 80%
        let score = client.compute_score(&subject);
        assert!(score > MIN_SCORE);
    }

    #[test]
    fn test_base_score_is_300() {
        let (env, _, client) = setup();
        let subject = Address::generate(&env);
        let score = client.compute_score(&subject);
        assert_eq!(score, MIN_SCORE);
    }

    #[test]
    fn test_score_increases_with_repayments() {
        let (env, admin, client) = setup();
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);
        client.register_lender(&admin, &lender);
        for _ in 0..10 {
            client.record_repayment(&lender, &subject, &1000, &true);
        }
        let score = client.compute_score(&subject);
        assert!(score > MIN_SCORE);
    }

    #[test]
    fn test_score_bounded_300_850() {
        let (env, admin, client) = setup();
        let feeder = Address::generate(&env);
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);
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
    #[should_panic]
    fn test_weights_must_sum_to_100() {
        let (_, _, client) = setup();
        // 40+40+40 = 120, should return InvalidWeights error (panics via unwrap in test client)
        client.propose_weights(&ScoringWeights { vc_weight: 40, tx_weight: 40, repayment_weight: 40 });
    }

    #[test]
    fn test_propose_weights_unchanged_until_applied() {
        let (_, _, client) = setup();
        let original = client.get_scoring_weights();
        assert_eq!(original.vc_weight, 40);
        client.propose_weights(&ScoringWeights { vc_weight: 50, tx_weight: 30, repayment_weight: 20 });
        let current = client.get_scoring_weights();
        assert_eq!(current.vc_weight, 40);
    }

    #[test]
    #[should_panic(expected = "timelock not expired")]
    fn test_apply_weights_before_timelock_fails() {
        let (_, _, client) = setup();
        client.propose_weights(&ScoringWeights { vc_weight: 50, tx_weight: 30, repayment_weight: 20 });
        client.apply_weights();
    }

    #[test]
    fn test_apply_weights_after_timelock_succeeds() {
        let (env, _, client) = setup();
        client.propose_weights(&ScoringWeights { vc_weight: 50, tx_weight: 25, repayment_weight: 25 });
        env.ledger().set_sequence_number(env.ledger().sequence() + TIMELOCK_LEDGERS + 1);
        client.apply_weights();
        let w = client.get_scoring_weights();
        assert_eq!(w.vc_weight, 50);
        assert_eq!(w.tx_weight, 25);
        assert_eq!(w.repayment_weight, 25);
    }

    #[test]
    #[should_panic]
    fn test_deregistered_feeder_cannot_update_tx_stats() {
        let (env, admin, client) = setup();
        let feeder = Address::generate(&env);
        let subject = Address::generate(&env);
        client.register_feeder(&admin, &feeder);
        client.update_tx_stats(&feeder, &subject, &TxStats { volume_30d: 5000, tx_count_30d: 10, avg_counterparties: 3 });
        client.deregister_feeder(&admin, &feeder);
        client.update_tx_stats(&feeder, &subject, &TxStats { volume_30d: 6000, tx_count_30d: 11, avg_counterparties: 4 });
    }

    #[test]
    #[should_panic]
    fn test_deregistered_lender_cannot_record_repayment() {
        let (env, admin, client) = setup();
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);
        client.register_lender(&admin, &lender);
        client.record_repayment(&lender, &subject, &1000, &true);
        client.deregister_lender(&admin, &lender);
        client.record_repayment(&lender, &subject, &1000, &true);
    }

    #[test]
    fn test_upgrade_preserves_contract_address() {
        let env = soroban_sdk::Env::default();
        env.mock_all_auths();
        let new_wasm_hash = env.deployer().upload_contract_wasm(CreditOracle::WASM);
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        client.upgrade(&admin, &new_wasm_hash);
        let w = client.get_scoring_weights();
        assert_eq!(w.vc_weight + w.tx_weight + w.repayment_weight, 100);
    }

    #[test]
    #[should_panic(expected = "not authorized")]
    fn test_upgrade_rejects_non_admin() {
        let env = soroban_sdk::Env::default();
        env.mock_all_auths();
        let new_wasm_hash = env.deployer().upload_contract_wasm(CreditOracle::WASM);
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        client.initialize(&admin);
        client.upgrade(&non_admin, &new_wasm_hash);
    }

    // --- #32: update_tx_stats full-replace test ---

    #[test]
    fn test_update_tx_stats_full_replace() {
        let (env, admin, client) = setup();
        let contract_id = env.register_contract(None, CreditOracle);
        let feeder = Address::generate(&env);
        let subject = Address::generate(&env);
        client.register_feeder(&admin, &feeder);

        // Set all three fields
        client.update_tx_stats(&feeder, &subject, &TxStats {
            volume_30d: 500_000_000,
            tx_count_30d: 10,
            avg_counterparties: 5,
        });

        // Call again with only volume — other fields will be zeroed
        client.update_tx_stats(&feeder, &subject, &TxStats {
            volume_30d: 800_000_000,
            tx_count_30d: 0,
            avg_counterparties: 0,
        });

        // Score reflects the new stats; tx_count_30d and avg_counterparties are zero
        let score = client.compute_score(&subject);
        // Only volume contributes — 800_000_000 / 100_000_000 = 8 tx_score points
        // composite = (0*40 + 8*30 + 0*30) / 100 = 2
        // score = 300 + 2*550/100 = 300 + 11 = 311
        assert_eq!(score, 311);
    }

    // --- #33: non-admin cannot call propose_weights ---

    #[test]
    #[should_panic]
    fn test_non_admin_cannot_update_weights() {
        let env = soroban_sdk::Env::default();
        // Do NOT use mock_all_auths — we need real auth enforcement
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let admin_b = Address::generate(&env);

        // Initialize with admin_a; must mock auth just for initialize
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &admin,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &contract_id,
                fn_name: "initialize",
                args: (admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.initialize(&admin);

        // Attempt propose_weights with admin_b — no auth mocked, should panic with auth error
        client.propose_weights(&ScoringWeights { vc_weight: 50, tx_weight: 30, repayment_weight: 20 });
    }
}
