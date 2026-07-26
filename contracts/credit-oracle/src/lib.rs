#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, symbol_short, Address, BytesN, Env, IntoVal};

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
    /// Identity oracle contract ID for cross-contract lookups
    IdentityOracleId,
    /// Number of ledgers to wait before recomputing score
    ComputeCooldownLedgers,
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
    /// Previous credit score, if one exists
    pub previous_score: Option<u32>,
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

        let mut vc_count: u32 = env.storage().persistent()
            .get(&DataKey::VcCount(subject.clone()))
            .unwrap_or(0u32);

        // Cross-contract lookup takes precedence if configured
        if let Some(identity_oracle_id) = env.storage().instance().get::<_, Address>(&DataKey::IdentityOracleId) {
            vc_count = env.invoke_contract::<u32>(
                &identity_oracle_id,
                &soroban_sdk::Symbol::new(&env, "get_active_vc_count"),
                soroban_sdk::vec![&env, subject.clone().into_val(&env)],
            );
        }

        let vc_score = (vc_count * 20).min(100);

        // tx_score is the sum of the volume sub-score and the counterparty bonus.
        // The counterparty bonus awards up to 20 extra points for network diversity
        // (1 point per 5 unique counterparties, capped at 20), making it a
        // sub-component of the transaction score.
        //
        // NOTE: Because the bonus is multiplied by tx_weight in the composite
        // calculation, setting tx_weight = 0 also suppresses the counterparty bonus.
        // This is intentional — see docs/scoring-spec.md for rationale.
        let volume_score = ((tx_stats.volume_30d / 100_000_000i128) as u32).min(80);
        let counterparty_bonus = (tx_stats.avg_counterparties / 5).min(20);
        let tx_score = (volume_score + counterparty_bonus).min(100);

        let repay_score = (repayment.on_time_count * 10000)
            .checked_div(repayment.total_count)
            .map(|r| r / 100)
            .unwrap_or(0);

        let weights: ScoringWeights = env.storage().instance().get(&DataKey::Config).unwrap();
        let composite = (vc_score * weights.vc_weight
            + tx_score * weights.tx_weight
            + repay_score * weights.repayment_weight)
            / 100;

        let score = (MIN_SCORE + composite * 550 / 100).clamp(MIN_SCORE, MAX_SCORE);

        let repayment_rate = (repayment.on_time_count * 10000)
                                .checked_div(repayment.total_count)
                                .unwrap_or(0);

        let mut previous_score: Option<u32> = None;
        let mut needs_write = true;
        if let Some(prev) = env.storage().persistent().get::<_, ScoreRecord>(&DataKey::Score(subject.clone())) {
            if prev.score == score 
                && prev.vc_count == vc_count 
                && prev.repayment_rate == repayment_rate 
                && prev.tx_volume_30d == tx_stats.volume_30d 
            {
                needs_write = false;
            }
            previous_score = Some(prev.score);
        }

        if needs_write {
            env.storage().persistent().set(&DataKey::Score(subject.clone()), &ScoreRecord {
                score,
                last_updated: env.ledger().timestamp(),
                vc_count,
                repayment_rate,
                tx_volume_30d: tx_stats.volume_30d,
                previous_score,
            });
        }

        score
    }

    /// Get credit score for a user; returns None if score has not been computed yet
    pub fn get_score(env: Env, subject: Address) -> Option<ScoreRecord> {
        env.storage().persistent().get(&DataKey::Score(subject))
    }

    /// Propose new scoring weights with timelock
    /// Propose new scoring weights with timelock
    pub fn propose_weights(env: Env, weights: ScoringWeights) -> Result<(), CreditOracleError> {
        if weights.vc_weight + weights.tx_weight + weights.repayment_weight != 100 {
            return Err(CreditOracleError::InvalidWeights);
        }
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        stored_admin.require_auth();

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
        let weights: Option<ScoringWeights> = env.storage().instance().get(&DataKey::PendingWeights);
        weights.map(|w| {
            let effective_ledger: u32 = env.storage()
                .instance()
                .get(&DataKey::PendingWeightsEffectiveLedger)
                .expect("effective ledger should exist if weights exist");
            PendingWeightsRecord {
                weights: w,
                effective_ledger,
            }
        })
    }

    /// Propose a new admin for the contract (first step of two-step admin transfer).
    /// The proposed admin must call `accept_admin` to complete the transfer.
    pub fn propose_new_admin(env: Env, new_admin: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        stored_admin.require_auth();
        env.storage().instance().set(&DataKey::PendingAdmin, &new_admin);
        env.events().publish((symbol_short!("AdmProp"),), new_admin);
        Ok(())
    }

    /// Accept the admin role (second step of two-step admin transfer).
    /// Must be called by the address that was proposed via `propose_new_admin`.
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), CreditOracleError> {
        new_admin.require_auth();
        let pending_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .expect("no pending admin");
        if new_admin != pending_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        env.events().publish((symbol_short!("AdmAccept"),), new_admin);
        Ok(())
    }

    /// Propose a new contract admin (two-step admin transfer).
    pub fn propose_new_admin(env: Env, new_admin: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        stored_admin.require_auth();
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

    /// Set the identity-oracle contract ID for cross-contract VC count lookup.
    ///
    /// Auth: admin only — verified via stored_admin.require_auth()
    pub fn set_identity_oracle(env: Env, identity_oracle_id: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        stored_admin.require_auth();
        env.storage().instance().set(&DataKey::IdentityOracleId, &identity_oracle_id);
        
        env.events().publish((soroban_sdk::symbol_short!("OrclSet"),), identity_oracle_id);
        Ok(())
    }

    /// Update the compute cooldown ledgers
    pub fn update_compute_cooldown(env: Env, ledgers: u32) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        stored_admin.require_auth();
        env.storage().instance().set(&DataKey::ComputeCooldownLedgers, &ledgers);
        
        env.events().publish(
            (symbol_short!("CdSet"),),
            (ledgers, stored_admin),
        );
        Ok(())
    }

    /// Get current compute cooldown in ledgers
    pub fn get_compute_cooldown(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::ComputeCooldownLedgers)
            .unwrap_or(1) // Default to 1 ledger as described in docs
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

    /// When tx_weight = 0, the counterparty bonus contributes nothing to the final score
    /// because it is a sub-component of tx_score, which is multiplied by tx_weight.
    #[test]
    fn test_counterparty_bonus_zero_when_tx_weight_is_zero() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        client.initialize(&admin);

        // Propose and apply weights with tx_weight = 0
        client.propose_weights(&ScoringWeights { vc_weight: 60, tx_weight: 0, repayment_weight: 40 });
        let jump = TIMELOCK_LEDGERS + 2;
        env.as_contract(&contract_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger().set_sequence_number(env.ledger().sequence() + jump);
        client.apply_weights();

        client.register_feeder(&admin, &feeder);

        let subject_with_counterparties = Address::generate(&env);
        let subject_without_counterparties = Address::generate(&env);

        // Give first subject 100 counterparties (max bonus)
        client.update_tx_stats(&feeder, &subject_with_counterparties, &TxStats {
            volume_30d: 0,
            tx_count_30d: 0,
            avg_counterparties: 100,
        });
        // Second subject has no counterparties
        client.update_tx_stats(&feeder, &subject_without_counterparties, &TxStats {
            volume_30d: 0,
            tx_count_30d: 0,
            avg_counterparties: 0,
        });

        let score_with = client.compute_score(&subject_with_counterparties);
        let score_without = client.compute_score(&subject_without_counterparties);

        // Both scores must be identical — tx_weight=0 suppresses the counterparty bonus
        assert_eq!(score_with, score_without,
            "counterparty bonus should have no effect when tx_weight is 0");
    }

    /// When tx_weight = 100, the counterparty bonus is fully applied and
    /// a subject with 100+ counterparties scores higher than one with none.
    #[test]
    fn test_counterparty_bonus_applied_when_tx_weight_is_100() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        client.initialize(&admin);

        // Propose and apply weights with tx_weight = 100
        client.propose_weights(&ScoringWeights { vc_weight: 0, tx_weight: 100, repayment_weight: 0 });
        let jump = TIMELOCK_LEDGERS + 2;
        env.as_contract(&contract_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger().set_sequence_number(env.ledger().sequence() + jump);
        client.apply_weights();

        client.register_feeder(&admin, &feeder);

        let subject_with = Address::generate(&env);
        let subject_without = Address::generate(&env);

        // Same volume, but subject_with has 100 counterparties (max bonus = 20 pts)
        client.update_tx_stats(&feeder, &subject_with, &TxStats {
            volume_30d: 0,
            tx_count_30d: 0,
            avg_counterparties: 100,
        });
        client.update_tx_stats(&feeder, &subject_without, &TxStats {
            volume_30d: 0,
            tx_count_30d: 0,
            avg_counterparties: 0,
        });

        let score_with = client.compute_score(&subject_with);
        let score_without = client.compute_score(&subject_without);

        assert!(score_with > score_without,
            "subject with 100 counterparties should score higher when tx_weight=100");
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

        client.propose_new_admin(&admin2);
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
        client.propose_new_admin(&admin2);

        let _ = client.accept_admin(&non_admin);
    }

    #[test]
    fn test_compute_score_skips_write_when_inputs_unchanged() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        // First computation sets initial values
        client.compute_score(&subject);
        let record1 = client.get_score(&subject).unwrap();

        // Advance ledger time by 100 seconds
        env.ledger().set_timestamp(env.ledger().timestamp() + 100);

        // Second computation with identical inputs
        client.compute_score(&subject);
        let record2 = client.get_score(&subject).unwrap();

        // Timestamp shouldn't change because write was skipped
        assert_eq!(record1.last_updated, record2.last_updated);

        // Change an input (VC count)
        let feeder = Address::generate(&env);
        client.register_feeder(&admin, &feeder);
        client.set_vc_count(&feeder, &subject, &2);

        // Advance ledger time again
        env.ledger().set_timestamp(env.ledger().timestamp() + 100);

        // Third computation with changed input
        client.compute_score(&subject);
        let record3 = client.get_score(&subject).unwrap();

        // Write occurred, so timestamp is updated
        assert!(record3.last_updated > record2.last_updated);
        assert_eq!(record3.vc_count, 2);
    }
}
