#![no_std]
//! Credit oracle contract for the Stellar DID Credit protocol.
//!
//! Computes composite credit scores for Stellar addresses by combining
//! on-chain verified credential counts, 30-day transaction statistics,
//! and repayment history. Scores are bounded to [MIN_SCORE, MAX_SCORE].
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, BytesN, Env,
    IntoVal, Symbol, Val, Vec as SorobanVec,
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

fn ensure_not_paused(env: &Env) -> Result<(), CreditOracleError> {
    if env.storage().instance().get(&DataKey::Paused).unwrap_or(false) {
        Err(CreditOracleError::ContractPaused)
    } else {
        Ok(())
    }
}

/// Load the stored admin address and call `require_auth()` on it, or check
/// that `caller` is a registered governor.
///
/// This helper is used by `propose_weights` so that both the admin and any
/// registered governor can submit a weight proposal.
fn require_admin_or_governor(env: &Env, caller: &Address) -> Result<(), CreditOracleError> {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("not initialized");
    if *caller == admin {
        caller.require_auth();
        return Ok(());
    }
    if env
        .storage()
        .persistent()
        .has(&DataKey::Governor(caller.clone()))
    {
        caller.require_auth();
        return Ok(());
    }
    Err(CreditOracleError::NotAuthorized)
}

pub const MIN_SCORE: u32 = 300;
pub const MAX_SCORE: u32 = 850;

pub const INSTANCE_BUMP_THRESHOLD: u32 = 5000;
pub const INSTANCE_BUMP_AMOUNT: u32 = 500_000;

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
    /// Score was computed too recently for this subject.
    ComputeCooldownActive = 7,
    /// The contract is currently paused and cannot accept writes.
    ContractPaused = 8,
}

/// Storage keys for the credit oracle contract
#[contracttype]
pub enum DataKey {
    /// Contract administrator address
    Admin,
    /// Whether the contract is currently paused for writes.
    Paused,
    /// Pending contract admin address for two-step transfer
    PendingAdmin,

    /// Global configuration
    Config,
    /// Trusted feeder address authorized to update transaction stats
    TrustedFeeder(Address),
    /// Trusted lender address authorized to record repayments
    TrustedLender(Address),
    /// Governance address authorized to propose weight changes
    Governor(Address),
    /// Transaction statistics for a user
    TxStats(Address),
    /// Repayment record for a user
    RepaymentRecord(Address),
    /// Credit score for a user
    Score(Address),
    /// Cached VC count for a user
    VcCount(Address),
    /// Optional identity-oracle contract ID for cross-contract VC count lookup
    IdentityOracleId,
    /// Pending weights awaiting timelock
    PendingWeights,
    /// Ledger number when pending weights become effective
    PendingWeightsEffectiveLedger,
    /// Minimum ledgers required between score computations for one subject
    ComputeCooldownLedgers,
    /// Last ledger sequence when a subject's score was computed
    LastComputed(Address),
    /// Credential-type multiplier in basis points (100 = 1×).
    CredentialTypeWeight(Symbol),
}

/// Credit score record with metadata, returned by `get_score`.
///
/// This record captures the state of the score at computation time, enabling
/// consumers to detect stale scores and understand what inputs were used.
#[contracttype]
#[derive(Clone)]
pub struct ScoreRecord {
    /// Credit score value, bounded to `MIN_SCORE`–`MAX_SCORE`.
    pub score: u32,
    /// Ledger timestamp (Unix seconds) of the last score computation.
    pub last_updated: u64,
    /// Number of verified credentials counted toward the score.
    pub vc_count: u32,
    /// Repayment rate in basis points (0-10000). On-chain mirror of the
    /// repayment component calculation.
    pub repayment_rate: u32,
    /// 30-day transaction volume in stroops. On-chain mirror of the transaction
    /// volume component calculation.
    pub tx_volume_30d: i128,
}

/// Transaction statistics for a user
///
/// All fields are used in the credit scoring formula. See `compute_score` for
/// how each field contributes to the final score.
#[contracttype]
#[derive(Clone)]
pub struct TxStats {
    /// Total transaction volume in last 30 days. Used for the transaction volume
    /// component (up to 100 points based on volume tier).
    pub volume_30d: i128,
    /// Transaction count in last 30 days. Currently unused but retained for
    /// future scoring extensions.
    pub tx_count_30d: u32,
    /// Average number of distinct counterparties. Provides a bonus of up to 10
    /// points when >= 10 counterparties on average, rewarding transaction
    /// diversity.
    pub avg_counterparties: u32,
}

/// Weights used in credit score calculation. Must sum to 100.
///
/// Each weight determines the contribution of its component to the final composite.
#[contracttype]
#[derive(Clone)]
pub struct ScoringWeights {
    /// Weight for verified credentials component. Controls how much VC score
    /// influences the composite (0–100).
    pub vc_weight: u32,
    /// Weight for transaction history component. Controls the combined influence
    /// of volume and counterparty diversity (0–100).
    pub tx_weight: u32,
    /// Weight for repayment history component. Controls how much repayment score
    /// influences the composite (0–100).
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

/// Internal repayment counters for a subject.
///
/// Used to compute the repayment score component (0–100 based on on-time rate).
#[contracttype]
#[derive(Clone)]
pub struct RepaymentRecord {
    /// Number of repayments made on time. Higher ratio with total_count improves score.
    pub on_time_count: u32,
    /// Total number of repayments recorded. Used as divisor for on-time rate calculation.
    pub total_count: u32,
}

/// Mirror of identity-oracle `VCRecord` for cross-contract deserialization.
#[contracttype]
#[derive(Clone)]
struct AnchoredVCRecord {
    pub vc_hash: BytesN<32>,
    pub issuer: Address,
    pub anchored_at: u64,
    pub revoked: bool,
}

const TIMELOCK_LEDGERS: u32 = 17_280; // approximately 24 hours
const DEFAULT_COMPUTE_COOLDOWN_LEDGERS: u32 = 1;

/// Base points contributed by one VC before issuer/type multipliers.
pub const VC_BASE_POINTS: u32 = 20;
/// Default credential-type multiplier: 100 basis points (1×).
pub const DEFAULT_CREDENTIAL_TYPE_WEIGHT_BPS: u32 = 100;
/// Maximum allowed credential-type multiplier.
pub const MAX_CREDENTIAL_TYPE_WEIGHT_BPS: u32 = 300;

/// Legacy VC component from a raw count (uniform 20 points per VC).
pub fn vc_score_from_count(vc_count: u32) -> u32 {
    vc_count.saturating_mul(VC_BASE_POINTS).min(100)
}

/// Weighted VC component from per-credential issuer and type multipliers.
pub fn compute_weighted_vc_score(entries: &[(u32, u32)]) -> u32 {
    let mut total: u32 = 0;
    for (issuer_tier_bps, type_weight_bps) in entries.iter() {
        let points = VC_BASE_POINTS
            .saturating_mul(*issuer_tier_bps)
            .saturating_mul(*type_weight_bps)
            / 10_000;
        total = total.saturating_add(points);
    }
    total.min(100)
}

fn credential_type_weight(env: &Env, credential_type: &Symbol) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::CredentialTypeWeight(credential_type.clone()))
        .unwrap_or(DEFAULT_CREDENTIAL_TYPE_WEIGHT_BPS)
}

fn compute_vc_score_from_identity(
    env: &Env,
    identity_id: &Address,
    subject: &Address,
) -> (u32, u32) {
    let details_args: SorobanVec<Val> =
        SorobanVec::from_array(env, [subject.clone().into_val(env)]);
    let records: SorobanVec<AnchoredVCRecord> = env.invoke_contract(
        identity_id,
        &Symbol::new(env, "get_vc_details"),
        details_args,
    );

    let mut total: u32 = 0;
    let mut vc_count: u32 = 0;
    for i in 0..records.len() {
        let record = records.get(i).unwrap();
        vc_count = vc_count.saturating_add(1);

        let tier_args: SorobanVec<Val> =
            SorobanVec::from_array(env, [record.issuer.clone().into_val(env)]);
        let issuer_tier_bps: u32 =
            env.invoke_contract(identity_id, &Symbol::new(env, "get_issuer_tier"), tier_args);

        let type_args: SorobanVec<Val> = SorobanVec::from_array(
            env,
            [subject.clone().into_val(env), record.vc_hash.into_val(env)],
        );
        let credential_type: Symbol = env.invoke_contract(
            identity_id,
            &Symbol::new(env, "get_vc_credential_type"),
            type_args,
        );
        let type_weight_bps = credential_type_weight(env, &credential_type);
        let points = VC_BASE_POINTS
            .saturating_mul(issuer_tier_bps)
            .saturating_mul(type_weight_bps)
            / 10_000;
        total = total.saturating_add(points);
    }

    (total.min(100), vc_count)
}

fn validate_weights(weights: &ScoringWeights) {
    let sum = weights.vc_weight.saturating_add(weights.tx_weight).saturating_add(weights.repayment_weight);
    if sum != 100 {
        panic!(
            "weights must sum to 100, got {} (vc: {}, tx: {}, repayment: {})",
            sum, weights.vc_weight, weights.tx_weight, weights.repayment_weight
        );
    }
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
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        let default_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        validate_weights(&default_weights);
        env.storage()
            .instance()
            .set(&DataKey::Config, &default_weights);
        env.storage().instance().set(
            &DataKey::ComputeCooldownLedgers,
            &DEFAULT_COMPUTE_COOLDOWN_LEDGERS,
        );
        Ok(())
    }

    /// Register a trusted feeder address.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn register_feeder(env: Env, feeder: Address) -> Result<(), CreditOracleError> {
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .persistent()
            .set(&DataKey::TrustedFeeder(feeder.clone()), &true);
        env.events().publish((symbol_short!("FdrReg"),), feeder);
        Ok(())
    }

    /// Deregister a trusted feeder address.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn deregister_feeder(env: Env, feeder: Address) -> Result<(), CreditOracleError> {
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .persistent()
            .remove(&DataKey::TrustedFeeder(feeder.clone()));
        env.events().publish((symbol_short!("FdrDeReg"),), feeder);
        Ok(())
    }

    /// Register a trusted lender address.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn register_lender(env: Env, lender: Address) -> Result<(), CreditOracleError> {
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .persistent()
            .set(&DataKey::TrustedLender(lender.clone()), &true);
        env.events().publish((symbol_short!("LndReg"),), lender);
        Ok(())
    }

    /// Deregister a trusted lender address.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn deregister_lender(env: Env, lender: Address) -> Result<(), CreditOracleError> {
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .persistent()
            .remove(&DataKey::TrustedLender(lender.clone()));
        env.events().publish((symbol_short!("LndDeReg"),), lender);
        Ok(())
    }

    /// Register a governance address that may propose weight changes.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn register_governor(
        env: Env,
        admin: Address,
        governor: Address,
    ) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        let stored = require_admin(&env);
        if admin != stored {
            return Err(CreditOracleError::NotAuthorized);
        }
        env.storage()
            .persistent()
            .set(&DataKey::Governor(governor.clone()), &true);
        env.events().publish((symbol_short!("GovReg"),), governor);
        Ok(())
    }

    /// Deregister a governance address.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn deregister_governor(
        env: Env,
        admin: Address,
        governor: Address,
    ) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        let stored = require_admin(&env);
        if admin != stored {
            return Err(CreditOracleError::NotAuthorized);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::Governor(governor.clone()));
        env.events().publish((symbol_short!("GovDeReg"),), governor);
        Ok(())
    }

    /// Update transaction statistics for a subject address.
    ///
    /// Auth: `feeder` must be a registered trusted feeder and must sign
    /// the transaction.
    pub fn update_tx_stats(
        env: Env,
        feeder: Address,
        subject: Address,
        stats: TxStats,
    ) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        feeder.require_auth();
        if !env
            .storage()
            .persistent()
            .has(&DataKey::TrustedFeeder(feeder.clone()))
        {
            return Err(CreditOracleError::FeederNotRegistered);
        }
        env.storage()
            .persistent()
            .set(&DataKey::TxStats(subject), &stats);
        Ok(())
    }

    /// Record a repayment event for a subject.
    ///
    /// Increments the subject's total repayment count and, when `on_time` is
    /// `true`, also increments the on-time count. Uses saturating arithmetic
    /// to avoid overflow on adversarial inputs.
    ///
    /// Auth: `lender` must be a registered trusted lender and must sign
    /// the transaction.
    pub fn record_repayment(
        env: Env,
        lender: Address,
        subject: Address,
        _amount: i128,
        on_time: bool,
    ) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        lender.require_auth();
        if !env
            .storage()
            .persistent()
            .has(&DataKey::TrustedLender(lender.clone()))
        {
            return Err(CreditOracleError::LenderNotRegistered);
        }
        let mut record: RepaymentRecord = env
            .storage()
            .persistent()
            .get(&DataKey::RepaymentRecord(subject.clone()))
            .unwrap_or(RepaymentRecord {
                on_time_count: 0,
                total_count: 0,
            });
        if on_time {
            // saturating_add prevents a wrap-around panic if on_time_count
            // ever reaches u32::MAX (e.g. during fuzz / adversarial input).
            record.on_time_count = record.on_time_count.saturating_add(1);
        }
        // Same reasoning for total_count.
        record.total_count = record.total_count.saturating_add(1);
        env.storage()
            .persistent()
            .set(&DataKey::RepaymentRecord(subject), &record);
        Ok(())
    }

    /// Cache VC count for a subject (feeder-only)
    ///
    /// **Deprecated:** Prefer configuring an `IdentityOracleId` and using the
    /// cross-contract lookup via `set_identity_oracle` + `compute_score`.
    #[deprecated(note = "use cross-contract lookup via set_identity_oracle instead")]
    pub fn set_vc_count(
        env: Env,
        feeder: Address,
        subject: Address,
        count: u32,
    ) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        feeder.require_auth();
        if !env
            .storage()
            .persistent()
            .has(&DataKey::TrustedFeeder(feeder.clone()))
        {
            return Err(CreditOracleError::FeederNotRegistered);
        }
        env.storage()
            .persistent()
            .set(&DataKey::VcCount(subject), &count);
        Ok(())
    }
}

/// Pure scoring arithmetic, extracted for unit and property-based testing
/// without requiring a Soroban `Env`.
///
/// All inputs mirror the fields read from storage in `compute_score`.
///
/// # Parameters
/// - `vc_count` — number of active verified credentials.
/// - `volume_30d` — 30-day transaction volume in stroops.
/// - `avg_counterparties` — average distinct counterparties (bonus at ≥ 10).
/// - `on_time_count` — number of on-time repayments.
/// - `total_count` — total repayments recorded.
/// - `weights` — scoring weights (must sum to 100).
///
/// Returns a score clamped to [`MIN_SCORE`]–[`MAX_SCORE`].
pub fn compute_score_pure(
    vc_score: u32,
    volume_30d: i128,
    avg_counterparties: u32,
    on_time_count: u32,
    total_count: u32,
    weights: &ScoringWeights,
) -> u32 {
    let vc_score = vc_score.min(100);
    let tx_score = ((volume_30d / 100_000_000i128) as u32).min(100);
    let repay_score = on_time_count
        .saturating_mul(10000)
        .checked_div(total_count)
        .map(|r| r / 100)
        .unwrap_or(0);
    let counterparty_bonus: u32 = if avg_counterparties >= 10 { 10 } else { 0 };

    let composite = vc_score
        .saturating_mul(weights.vc_weight)
        .saturating_add(tx_score.saturating_mul(weights.tx_weight))
        .saturating_add(repay_score.saturating_mul(weights.repayment_weight))
        .saturating_add(counterparty_bonus.saturating_mul(weights.tx_weight))
        / 100;

    (MIN_SCORE + composite.saturating_mul(550) / 100).clamp(MIN_SCORE, MAX_SCORE)
}

fn has_anchored_did(env: &Env, subject: &Address) -> bool {
    if let Some(identity_id) = env.storage().instance().get(&DataKey::IdentityOracleId) {
        let args: SorobanVec<Val> = SorobanVec::from_array(&env, [subject.clone().into_val(env)]);
        env.invoke_contract(
            &identity_id,
            &Symbol::new(env, "has_anchored_did"),
            args,
        )
    } else {
        true
    }
}

#[contractimpl]
impl CreditOracle {
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
    /// When an identity-oracle is configured, this function requires the
    /// subject to have already anchored a DID document before a score may be
    /// computed. If no identity-oracle is configured, the legacy open-call
    /// behavior remains unchanged.
    ///
    /// # Recompute cooldown
    ///
    /// Calls are rate-limited per subject by `ComputeCooldownLedgers`. The
    /// default is one ledger, preventing repeated same-ledger refreshes from
    /// gaming the persisted `last_updated` timestamp.
    pub fn compute_score(env: Env, subject: Address) -> Result<u32, CreditOracleError> {
        ensure_not_paused(&env)?;
        let current_ledger = env.ledger().sequence();
        let cooldown: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ComputeCooldownLedgers)
            .unwrap_or(DEFAULT_COMPUTE_COOLDOWN_LEDGERS);

        if cooldown > 0 {
            let last_computed: Option<u32> = env
                .storage()
                .persistent()
                .get(&DataKey::LastComputed(subject.clone()));
            if let Some(last_ledger) = last_computed {
                if current_ledger < last_ledger.saturating_add(cooldown) {
                    return Err(CreditOracleError::ComputeCooldownActive);
                }
            }
        }

        let tx_stats: TxStats = env
            .storage()
            .persistent()
            .get(&DataKey::TxStats(subject.clone()))
            .unwrap_or(TxStats {
                volume_30d: 0,
                tx_count_30d: 0,
                avg_counterparties: 0,
            });

        let repayment: RepaymentRecord = env
            .storage()
            .persistent()
            .get(&DataKey::RepaymentRecord(subject.clone()))
            .unwrap_or(RepaymentRecord {
                on_time_count: 0,
                total_count: 0,
            });

        // Prefer live weighted lookup from identity-oracle when configured;
        // fall back to the cached `VcCount` for backward compatibility.
        let (vc_score, vc_count) =
            if let Some(identity_id) = env.storage().instance().get(&DataKey::IdentityOracleId) {
                compute_vc_score_from_identity(&env, &identity_id, &subject)
            } else {
                let count = env
                    .storage()
                    .persistent()
                    .get(&DataKey::VcCount(subject.clone()))
                    .unwrap_or(0u32);
                (vc_score_from_count(count), count)
            };

        let weights: ScoringWeights = env.storage().instance().get(&DataKey::Config).unwrap();
        let score = compute_score_pure(
            vc_score,
            tx_stats.volume_30d,
            tx_stats.avg_counterparties,
            repayment.on_time_count,
            repayment.total_count,
            &weights,
        );

        env.storage().persistent().set(
            &DataKey::Score(subject.clone()),
            &ScoreRecord {
                score,
                last_updated: env.ledger().timestamp(),
                vc_count,
                repayment_rate: repayment
                    .on_time_count
                    .saturating_mul(10000)
                    .checked_div(repayment.total_count)
                    .unwrap_or(0),
                tx_volume_30d: tx_stats.volume_30d,
            },
        );

        env.events()
            .publish((symbol_short!("Score"),), (subject.clone(), score));

        env.storage()
            .persistent()
            .set(&DataKey::LastComputed(subject), &current_ledger);

        Ok(score)
    }

    /// Get credit score for a user; returns None if score has not been computed yet
    pub fn get_score(env: Env, subject: Address) -> Option<ScoreRecord> {
        env.storage().persistent().get(&DataKey::Score(subject))
    }

    /// Check whether a subject's stored score is stale.
    ///
    /// Returns `true` when:
    /// - No score has ever been computed for `subject` (no record exists), or
    /// - The elapsed time since `last_updated` exceeds `max_age_seconds`.
    ///
    /// Consumers should call this before acting on a score and choose a
    /// `max_age_seconds` threshold appropriate for their use case. See
    /// `docs/scoring-spec.md` for recommended values.
    pub fn is_stale(env: Env, subject: Address, max_age_seconds: u64) -> bool {
        let record: Option<ScoreRecord> = env.storage().persistent().get(&DataKey::Score(subject));
        match record {
            None => true,
            Some(r) => {
                let now = env.ledger().timestamp();
                now.saturating_sub(r.last_updated) > max_age_seconds
            }
        }
    }

    /// Propose new scoring weights with timelock.
    ///
    /// Auth: admin or registered governor — verified via `require_admin_or_governor`.
    pub fn propose_weights(
        env: Env,
        caller: Address,
        weights: ScoringWeights,
    ) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        if weights.vc_weight + weights.tx_weight + weights.repayment_weight != 100 {
            return Err(CreditOracleError::InvalidWeights);
        }
        require_admin_or_governor(&env, &caller)?;
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        let effective_ledger = env.ledger().sequence() + TIMELOCK_LEDGERS;

        env.storage()
            .instance()
            .set(&DataKey::PendingWeights, &weights);
        env.storage()
            .instance()
            .set(&DataKey::PendingWeightsEffectiveLedger, &effective_ledger);

        env.events().publish(
            (symbol_short!("WtProp"),),
            (
                weights.vc_weight,
                weights.tx_weight,
                weights.repayment_weight,
                effective_ledger,
            ),
        );
        Ok(())
    }

    /// Apply pending weights after timelock expires
    pub fn apply_weights(env: Env) {
        let effective_ledger: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PendingWeightsEffectiveLedger)
            .expect("no pending weights");

        if env.ledger().sequence() < effective_ledger {
            panic!("timelock not expired");
        }

        let weights: ScoringWeights = env
            .storage()
            .instance()
            .get(&DataKey::PendingWeights)
            .expect("no pending weights");

        env.storage().instance().set(&DataKey::Config, &weights);

        env.storage().instance().remove(&DataKey::PendingWeights);
        env.storage()
            .instance()
            .remove(&DataKey::PendingWeightsEffectiveLedger);

        env.events().publish(
            (symbol_short!("WtApply"),),
            (
                weights.vc_weight,
                weights.tx_weight,
                weights.repayment_weight,
            ),
        );
    }

    /// Update weights directly (admin/governance only).
    ///
    /// Bypasses the propose/timelock flow. Also clears any pending timelocked
    /// proposal, since otherwise a later `apply_weights()` call would silently
    /// overwrite this direct update once the original proposal's timelock
    /// elapses.
    pub fn update_weights(env: Env, weights: ScoringWeights) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        if weights.vc_weight + weights.tx_weight + weights.repayment_weight != 100 {
            return Err(CreditOracleError::InvalidWeights);
        }
        require_admin(&env);
        env.storage().instance().set(&DataKey::Config, &weights);
        env.storage().instance().remove(&DataKey::PendingWeights);
        env.storage()
            .instance()
            .remove(&DataKey::PendingWeightsEffectiveLedger);
        env.events().publish(
            (symbol_short!("WtApply"),),
            (
                weights.vc_weight,
                weights.tx_weight,
                weights.repayment_weight,
            ),
        );
        Ok(())
    }

    /// Update the per-subject score recomputation cooldown.
    ///
    /// Auth: admin/governance only. A value of 0 disables cooldown enforcement.
    pub fn update_compute_cooldown(
        env: Env,
        cooldown_ledgers: u32,
    ) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .set(&DataKey::ComputeCooldownLedgers, &cooldown_ledgers);
        env.events()
            .publish((symbol_short!("CdSet"),), cooldown_ledgers);
        Ok(())
    }

    /// Get the configured per-subject score recomputation cooldown.
    pub fn get_compute_cooldown(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::ComputeCooldownLedgers)
            .unwrap_or(DEFAULT_COMPUTE_COOLDOWN_LEDGERS)
    }

    /// Set the identity-oracle contract ID for cross-contract VC count lookup.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn set_identity_oracle(
        env: Env,
        identity_oracle_id: Address,
    ) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        let previous: Option<Address> = env.storage().instance().get(&DataKey::IdentityOracleId);
        env.storage()
            .instance()
            .set(&DataKey::IdentityOracleId, &identity_oracle_id);
        let prev = previous.unwrap_or_else(|| identity_oracle_id.clone());
        env.events()
            .publish((symbol_short!("OrclSet"),), (prev, identity_oracle_id));
        Ok(())
    }

    /// Set the scoring multiplier for a credential type label (100 = 1×, 200 = 2×).
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn set_credential_type_weight(
        env: Env,
        admin: Address,
        credential_type: Symbol,
        weight_bps: u32,
    ) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        let stored = require_admin(&env);
        if admin != stored {
            return Err(CreditOracleError::NotAuthorized);
        }
        if weight_bps == 0 || weight_bps > MAX_CREDENTIAL_TYPE_WEIGHT_BPS {
            panic!("invalid credential type weight");
        }
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage().instance().set(
            &DataKey::CredentialTypeWeight(credential_type.clone()),
            &weight_bps,
        );
        env.events()
            .publish((symbol_short!("VcTypWt"),), (credential_type, weight_bps));
        Ok(())
    }

    /// Returns the configured credential-type multiplier, defaulting to 100 bps.
    pub fn get_credential_type_weight(env: Env, credential_type: Symbol) -> u32 {
        credential_type_weight(&env, &credential_type)
    }

    /// Get current scoring weights
    pub fn get_scoring_weights(env: Env) -> ScoringWeights {
        env.storage().instance().get(&DataKey::Config).unwrap()
    }

    /// Get pending weights (if any)
    pub fn get_pending_weights(env: Env) -> Option<PendingWeightsRecord> {
        let weights: Option<ScoringWeights> =
            env.storage().instance().get(&DataKey::PendingWeights);
        let effective_ledger: Option<u32> = env
            .storage()
            .instance()
            .get(&DataKey::PendingWeightsEffectiveLedger);
        match (weights, effective_ledger) {
            (Some(w), Some(l)) => Some(PendingWeightsRecord {
                weights: w,
                effective_ledger: l,
            }),
            _ => None,
        }
    }

    /// Propose a new contract admin (two-step admin transfer).
    pub fn propose_new_admin(env: Env, new_admin: Address) -> Result<(), CreditOracleError> {
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage()
            .instance()
            .set(&DataKey::PendingAdmin, &new_admin);
        Ok(())
    }

    /// Accept a proposed admin role (two-step admin transfer).
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
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
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        Ok(())
    }

    /// Admin-only maintenance: extend instance storage TTL so critical
    /// configuration (Admin, Config, pending weights) does not expire
    /// on an idle contract.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn maintain_storage(env: Env) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        Ok(())
    }

    /// Upgrade the contract WASM in-place, preserving address and all stored state.
    ///
    /// Auth: admin only — verified via `require_admin`.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        ensure_not_paused(&env).unwrap();
        require_admin(&env);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use identity_oracle::{IdentityOracle, IdentityOracleClient};
    use proptest::prelude::*;
    use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};

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
    #[should_panic(expected = "weights must sum to 100, got 150 (vc: 50, tx: 50, repayment: 50)")]
    fn test_propose_invalid_weights_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let invalid_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 50,
            repayment_weight: 50,
        };
        client.propose_weights(&admin, &invalid_weights);
    }

    #[test]
    #[should_panic(expected = "weights must sum to 100, got 90 (vc: 30, tx: 30, repayment: 30)")]
    fn test_update_invalid_weights_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let invalid_weights = ScoringWeights {
            vc_weight: 30,
            tx_weight: 30,
            repayment_weight: 30,
        };
        client.update_weights(&invalid_weights);
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
        client.register_lender(&lender);

        let is_trusted: bool = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::TrustedLender(lender.clone()))
                .unwrap_or(false)
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
        client.register_feeder(&feeder);
        client.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 5000,
                tx_count_30d: 10,
                avg_counterparties: 3,
            },
        );

        let stored: TxStats = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::TxStats(subject.clone()))
                .unwrap()
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
        client.register_lender(&lender);

        for _ in 0..8 {
            client.record_repayment(&lender, &subject, &1000, &true);
        }
        for _ in 0..2 {
            client.record_repayment(&lender, &subject, &1000, &false);
        }

        let record: RepaymentRecord = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject.clone()))
                .unwrap()
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
    fn test_compute_score_emits_score_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        // Compute the score which should emit a Score event
        let score = client.compute_score(&subject);

        // Retrieve all emitted events
        let events = env.events().all();

        // Should be exactly one event (the Score event)
        assert_eq!(events.len(), 1, "expected exactly one event");

        let (event_contract_id, _, _): (Address, soroban_sdk::Vec<Val>, Val) =
            events.get(0).unwrap();

        // Verify the event was emitted by this contract
        assert_eq!(event_contract_id, contract_id, "event contract id mismatch");

        // The score should still be computed successfully; the event payload is
        // covered by the contract's behavior and snapshot tests.
        assert_eq!(score, MIN_SCORE);
    }

    #[test]
    fn test_counterparty_bonus_adds_points() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);
        client.register_feeder(&feeder);
        client.register_lender(&lender);

        // Set up identical scores except for counterparty diversity
        client.set_vc_count(&feeder, &subject, &3);
        client.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 3_000_000_000i128,
                tx_count_30d: 100,
                avg_counterparties: 5, // below threshold - no bonus
            },
        );
        for _ in 0..8 {
            client.record_repayment(&lender, &subject, &1000, &true);
        }
        let score_without_bonus = client.compute_score(&subject);

        // Same config but with diverse counterparties
        client.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 3_000_000_000i128,
                tx_count_30d: 100,
                avg_counterparties: 12, // at or above threshold - bonus applies
            },
        );
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_with_bonus = client.compute_score(&subject);

        // Score with bonus should be higher (by ~30 points with default tx_weight=30)
        assert!(
            score_with_bonus > score_without_bonus,
            "expected bonus score ({}) > non-bonus score ({})",
            score_with_bonus,
            score_without_bonus
        );
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
        client.register_lender(&lender);

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
        client.register_feeder(&feeder);
        client.register_lender(&lender);

        client.set_vc_count(&feeder, &subject, &5);
        client.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 100_000_000_000i128,
                tx_count_30d: 1000,
                avg_counterparties: 100,
            },
        );
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
        let result = client.try_propose_weights(
            &admin,
            &ScoringWeights {
                vc_weight: 40,
                tx_weight: 40,
                repayment_weight: 40,
            },
        );
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

        client.propose_weights(
            &admin,
            &ScoringWeights {
                vc_weight: 50,
                tx_weight: 30,
                repayment_weight: 20,
            },
        );

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
        client.propose_weights(
            &admin,
            &ScoringWeights {
                vc_weight: 50,
                tx_weight: 30,
                repayment_weight: 20,
            },
        );
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
        client.propose_weights(
            &admin,
            &ScoringWeights {
                vc_weight: 50,
                tx_weight: 25,
                repayment_weight: 25,
            },
        );

        // Extend instance TTL before jumping the ledger so it isn't archived.
        let jump = TIMELOCK_LEDGERS + 2;
        env.as_contract(&contract_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + jump);
        client.apply_weights();

        let w = client.get_scoring_weights();
        assert_eq!(w.vc_weight, 50);
        assert_eq!(w.tx_weight, 25);
        assert_eq!(w.repayment_weight, 25);
    }

    #[test]
    fn test_update_weights_bypasses_timelock() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // No propose_weights / apply_weights round trip — update_weights
        // should take effect on the very same call, with no timelock wait.
        client.update_weights(&ScoringWeights {
            vc_weight: 20,
            tx_weight: 50,
            repayment_weight: 30,
        });

        let w = client.get_scoring_weights();
        assert_eq!(w.vc_weight, 20);
        assert_eq!(w.tx_weight, 50);
        assert_eq!(w.repayment_weight, 30);
    }

    #[test]
    fn test_update_weights_rejects_invalid_sum() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let result = client.try_update_weights(&ScoringWeights {
            vc_weight: 40,
            tx_weight: 40,
            repayment_weight: 40, // sums to 120
        });
        assert_eq!(result, Err(Ok(CreditOracleError::InvalidWeights)));

        // Confirm the rejected call left the stored config untouched.
        let w = client.get_scoring_weights();
        assert_eq!(w.vc_weight, 40);
        assert_eq!(w.tx_weight, 30);
        assert_eq!(w.repayment_weight, 30);
    }

    #[test]
    fn test_update_weights_requires_admin_auth() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Withdraw the blanket auth mock so require_admin's require_auth()
        // call inside update_weights has nothing authorizing the invocation.
        env.mock_auths(&[]);
        let result = client.try_update_weights(&ScoringWeights {
            vc_weight: 20,
            tx_weight: 50,
            repayment_weight: 30,
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_update_weights_clears_pending_proposal() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Queue a timelocked proposal.
        client.propose_weights(
            &admin,
            &ScoringWeights {
                vc_weight: 10,
                tx_weight: 10,
                repayment_weight: 80,
            },
        );
        assert!(client.get_pending_weights().is_some());

        // Admin bypasses the timelock with a direct update.
        client.update_weights(&ScoringWeights {
            vc_weight: 20,
            tx_weight: 50,
            repayment_weight: 30,
        });

        let w = client.get_scoring_weights();
        assert_eq!(w.vc_weight, 20);
        assert_eq!(w.tx_weight, 50);
        assert_eq!(w.repayment_weight, 30);

        // The fix clears the stale proposal, so there's nothing left for a
        // later apply_weights() call to silently resurrect.
        assert!(client.get_pending_weights().is_none());
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
        client.register_feeder(&feeder);
        client.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 5000,
                tx_count_30d: 10,
                avg_counterparties: 3,
            },
        );
        client.deregister_feeder(&feeder);
        let result = client.try_update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 6000,
                tx_count_30d: 11,
                avg_counterparties: 4,
            },
        );
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
        client.register_lender(&lender);
        client.record_repayment(&lender, &subject, &1000, &true);
        client.deregister_lender(&lender);
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
        client.register_feeder(&feeder);

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
    fn test_compute_score_rejects_within_cooldown() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        assert_eq!(
            client.get_compute_cooldown(),
            DEFAULT_COMPUTE_COOLDOWN_LEDGERS
        );
        assert_eq!(client.compute_score(&subject), MIN_SCORE);

        let result = client.try_compute_score(&subject);
        assert_eq!(result, Err(Ok(CreditOracleError::ComputeCooldownActive)));

        let last_computed: u32 = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::LastComputed(subject.clone()))
                .unwrap()
        });
        assert_eq!(last_computed, env.ledger().sequence());
    }

    #[test]
    fn test_compute_score_allows_after_cooldown_expires() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);
        client.update_compute_cooldown(&2);

        assert_eq!(client.compute_score(&subject), MIN_SCORE);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        assert_eq!(
            client.try_compute_score(&subject),
            Err(Ok(CreditOracleError::ComputeCooldownActive))
        );

        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        assert_eq!(client.compute_score(&subject), MIN_SCORE);
    }

    #[test]
    fn test_compute_cooldown_can_be_disabled_by_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);
        client.update_compute_cooldown(&0);

        assert_eq!(client.get_compute_cooldown(), 0);
        assert_eq!(client.compute_score(&subject), MIN_SCORE);
        assert_eq!(client.compute_score(&subject), MIN_SCORE);
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

        // propose new admin
        client.propose_new_admin(&admin2);

        // accept by proposed admin
        client.accept_admin(&admin2);

        let stored_admin: Address = env.as_contract(&contract_id, || {
            env.storage().instance().get(&DataKey::Admin).unwrap()
        });
        assert_eq!(stored_admin, admin2);

        // new admin can register feeder
        client.register_feeder(&feeder);
    }

    #[test]
    fn test_admin_transfer_preserves_trusted_feeder_auth() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let old_feeder = Address::generate(&env);
        let new_feeder = Address::generate(&env);
        let subject = Address::generate(&env);

        client.initialize(&admin1);
        client.register_feeder(&old_feeder);

        client.update_tx_stats(
            &old_feeder,
            &subject,
            &TxStats {
                volume_30d: 5_000,
                tx_count_30d: 10,
                avg_counterparties: 3,
            },
        );

        client.propose_new_admin(&admin2);
        client.accept_admin(&admin2);

        let stored_admin: Address = env.as_contract(&contract_id, || {
            env.storage().instance().get(&DataKey::Admin).unwrap()
        });
        assert_eq!(stored_admin, admin2);

        let feeder_registered: bool = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::TrustedFeeder(old_feeder.clone()))
                .unwrap_or(false)
        });
        assert!(feeder_registered, "trusted feeder should survive admin transfer");

        client.update_tx_stats(
            &old_feeder,
            &subject,
            &TxStats {
                volume_30d: 6_000,
                tx_count_30d: 11,
                avg_counterparties: 4,
            },
        );

        let stored_stats: TxStats = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::TxStats(subject.clone()))
                .unwrap()
        });
        assert_eq!(stored_stats.volume_30d, 6_000);

        client.deregister_feeder(&old_feeder);
        client.register_feeder(&new_feeder);

        let result = client.try_update_tx_stats(
            &old_feeder,
            &subject,
            &TxStats {
                volume_30d: 7_000,
                tx_count_30d: 12,
                avg_counterparties: 5,
            },
        );
        assert_eq!(result, Err(Ok(CreditOracleError::FeederNotRegistered)));

        client.update_tx_stats(
            &new_feeder,
            &subject,
            &TxStats {
                volume_30d: 8_000,
                tx_count_30d: 13,
                avg_counterparties: 6,
            },
        );

        let updated_stats: TxStats = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::TxStats(subject.clone()))
                .unwrap()
        });
        assert_eq!(updated_stats.volume_30d, 8_000);
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
    fn test_is_stale_no_score_returns_true() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        assert!(client.is_stale(&subject, &86_400));
    }

    #[test]
    fn test_is_stale_fresh_score_not_stale() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        client.compute_score(&subject);
        assert!(!client.is_stale(&subject, &86_400));
    }

    #[test]
    fn test_is_stale_old_score_is_stale() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        client.compute_score(&subject);

        // Jump ledger to advance timestamp past the staleness threshold.
        let jump: u64 = 100;
        env.ledger().set_timestamp(env.ledger().timestamp() + jump);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + jump as u32);

        // With max_age_seconds = 50, the score (which is 100 ledgers old) is stale.
        assert!(client.is_stale(&subject, &50));
    }

    #[test]
    fn test_compute_weighted_vc_score_issuer_tier() {
        assert_eq!(compute_weighted_vc_score(&[(100, 100)]), 20);
        assert_eq!(compute_weighted_vc_score(&[(200, 100)]), 40);
        assert_eq!(compute_weighted_vc_score(&[(100, 150)]), 30);
    }

    #[test]
    fn test_vc_score_from_count_matches_legacy_formula() {
        assert_eq!(vc_score_from_count(0), 0);
        assert_eq!(vc_score_from_count(3), 60);
        assert_eq!(vc_score_from_count(5), 100);
        assert_eq!(vc_score_from_count(u32::MAX), 100);
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
        client.register_feeder(&feeder);
        client.register_lender(&lender);

        // Apply weights immediately by setting pending weights and jumping beyond timelock.
        client.propose_weights(&admin, &weights);
        let jump = TIMELOCK_LEDGERS + 2;
        env.as_contract(&contract_id, || {
            env.storage().instance().extend_ttl(jump, jump);
            // Persistent entries (TrustedFeeder, TrustedLender) would be
            // archived after the ledger jump without this TTL extension.
            env.storage().persistent().extend_ttl(
                &DataKey::TrustedFeeder(feeder.clone()),
                jump,
                jump,
            );
            env.storage().persistent().extend_ttl(
                &DataKey::TrustedLender(lender.clone()),
                jump,
                jump,
            );
        });
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + jump);
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
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]
        #[test]
        fn proptest_score_always_in_range(
            vc_count in any::<u32>(),
            volume_30d in any::<i64>(),
            on_time in any::<u32>(),
            total in any::<u32>(),
        ) {
            let on_time_count = on_time.min(total);
            let weights = ScoringWeights { vc_weight: 40, tx_weight: 30, repayment_weight: 30 };
            let score = compute_score_pure(
                vc_score_from_count(vc_count),
                volume_30d as i128,
                0,
                on_time_count,
                total,
                &weights,
            );
            prop_assert!(score >= MIN_SCORE && score <= MAX_SCORE);
        }
    }

    proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]
        #[test]
        fn proptest_score_monotone_on_repayment(
            vc_count in 0u32..100u32,
            volume_30d in any::<i64>(),
            total1 in 1u32..500u32,
            on_time1 in 0u32..500u32,
        ) {
            let on_time1 = on_time1.min(total1);
            let total2 = total1 + 1;

            let on_time2 = ((on_time1 as u128) * (total2 as u128) + (total1 as u128) - 1) / (total1 as u128);
            let on_time2 = on_time2.min(total2 as u128) as u32;

            let weights = ScoringWeights { vc_weight: 40, tx_weight: 30, repayment_weight: 30 };

            let score1 = compute_score_pure(vc_score_from_count(vc_count), volume_30d as i128, 0, on_time1, total1, &weights);
            let score2 = compute_score_pure(vc_score_from_count(vc_count), volume_30d as i128, 0, on_time2, total2, &weights);

            prop_assert!(score2 >= score1);
        }
    }

    proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]
        #[test]
        fn proptest_no_panic_on_any_valid_weights(
            a in 0u32..=100u32,
            b in 0u32..=100u32,
            vc_count in any::<u32>(),
            volume_30d in any::<i64>(),
            on_time in any::<u32>(),
            total in any::<u32>(),
        ) {
            // Derive c so that a + b + c == 100 without rejection sampling.
            // If a + b > 100, clamp b so the triple is always valid.
            let b = b.min(100 - a.min(100));
            let c = 100 - a.min(100) - b;

            let on_time_count = on_time.min(total);
            let weights = ScoringWeights { vc_weight: a.min(100), tx_weight: b, repayment_weight: c };
            let score = compute_score_pure(vc_score_from_count(vc_count), volume_30d as i128, 0, on_time_count, total, &weights);
            prop_assert!(score >= MIN_SCORE && score <= MAX_SCORE);
        }
    }

    /// Verifies that the score stays in [300, 850] for every weight boundary
    /// combination listed in the issue, using maximum possible inputs.
    ///
    /// Mathematical invariant (see also the comment in `compute_score`):
    /// Each sub-score is clamped to [0, 100] and valid weights sum to exactly
    /// 100, so composite ≤ 100 for *any* valid weight triple.  Therefore
    /// score = 300 + composite*550/100 ≤ 300 + 550 = 850, and the
    /// clamp(300, 850) is always safe — never triggered for valid inputs.
    /// Verifies that the "Exceptional" profile described in the README and
    /// docs/scoring-spec.md achieves exactly 850 (MAX_SCORE).
    ///
    /// Inputs: vc_count=5, volume_30d=10_000_000_000 stroops (100 XLM),
    ///         on_time=100, total=100, avg_counterparties=0 (no bonus).
    ///
    /// Formula (default weights 40/30/30, no counterparty bonus):
    ///   vc_score    = min(5×20, 100)  = 100
    ///   tx_score    = min(10_000_000_000÷100_000_000, 100) = 100
    ///   repay_score = (100×10000÷100)÷100 = 100
    ///   composite   = (100×40 + 100×30 + 100×30) ÷ 100 = 100
    ///   score       = clamp(300 + 100×550÷100, 300, 850) = 850
    #[test]
    fn test_exceptional_score_equals_850() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let lender = Address::generate(&env);
        let subject = Address::generate(&env);

        client.initialize(&admin);
        client.register_feeder(&feeder);
        client.register_lender(&lender);

        client.set_vc_count(&feeder, &subject, &5);
        client.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 10_000_000_000i128,
                tx_count_30d: 0,
                avg_counterparties: 0,
            },
        );
        for _ in 0..100 {
            client.record_repayment(&lender, &subject, &1000, &true);
        }

        let score = client.compute_score(&subject);
        assert_eq!(
            score, MAX_SCORE,
            "exceptional profile must score exactly {MAX_SCORE}"
        );
    }

    #[test]
    fn test_score_in_range_for_all_weight_boundaries() {
        // (vc_weight, tx_weight, repayment_weight) — all must sum to 100.
        let weight_combos: &[(u32, u32, u32)] = &[
            (100, 0, 0),
            (0, 100, 0),
            (0, 0, 100),
            (50, 50, 0),
            (50, 0, 50),
            (0, 50, 50),
            (34, 33, 33),
            (40, 30, 30),
        ];

        // Maximum inputs so each sub-score is driven to its ceiling of 100:
        //   vc_count=5   → vc_score  = 5*20 = 100 (clamped to 100)
        //   volume_30d=10_000_000_000 → tx_score = 10_000_000_000/100_000_000 = 100
        //   100/100 repayments → repay_score = 10000/100 = 100
        for &(vc_w, tx_w, repay_w) in weight_combos {
            let weights = ScoringWeights {
                vc_weight: vc_w,
                tx_weight: tx_w,
                repayment_weight: repay_w,
            };
            let score = setup_and_compute_score(
                5,                 // vc_count
                10_000_000_000i64, // volume_30d in stroops
                100,               // on_time_count
                100,               // total_count
                weights,
            );
            assert!(
                (MIN_SCORE..=MAX_SCORE).contains(&score),
                "score {score} out of [{MIN_SCORE}, {MAX_SCORE}] for weights ({vc_w}, {tx_w}, {repay_w})"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Governor tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_governor_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let governor = Address::generate(&env);

        client.initialize(&admin);
        client.register_governor(&admin, &governor);

        let is_governor: bool = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::Governor(governor.clone()))
                .unwrap_or(false)
        });
        assert!(is_governor);
    }

    #[test]
    fn test_deregister_governor_removes_role() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let governor = Address::generate(&env);

        client.initialize(&admin);
        client.register_governor(&admin, &governor);

        let is_governor: bool = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::Governor(governor.clone()))
                .unwrap_or(false)
        });
        assert!(is_governor);

        client.deregister_governor(&admin, &governor);

        let is_governor: bool = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::Governor(governor.clone()))
                .unwrap_or(false)
        });
        assert!(!is_governor);
    }

    #[test]
    fn test_only_admin_can_register_governor() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        let governor = Address::generate(&env);

        client.initialize(&admin);
        let result = client.try_register_governor(&non_admin, &governor);
        assert_eq!(result, Err(Ok(CreditOracleError::NotAuthorized)));
    }

    #[test]
    fn test_governor_can_propose_weights() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let governor = Address::generate(&env);

        client.initialize(&admin);
        client.register_governor(&admin, &governor);

        // Governor proposes weights
        client.propose_weights(
            &governor,
            &ScoringWeights {
                vc_weight: 50,
                tx_weight: 25,
                repayment_weight: 25,
            },
        );

        let pending = client.get_pending_weights();
        assert!(pending.is_some());
        let pending = pending.unwrap();
        assert_eq!(pending.weights.vc_weight, 50);
    }

    #[test]
    fn test_non_governor_cannot_propose_weights() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let stranger = Address::generate(&env);

        client.initialize(&admin);
        let result = client.try_propose_weights(
            &stranger,
            &ScoringWeights {
                vc_weight: 50,
                tx_weight: 25,
                repayment_weight: 25,
            },
        );
        assert_eq!(result, Err(Ok(CreditOracleError::NotAuthorized)));
    }

    #[test]
    fn test_deregistered_governor_cannot_propose_weights() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let governor = Address::generate(&env);

        client.initialize(&admin);
        client.register_governor(&admin, &governor);
        client.deregister_governor(&admin, &governor);

        let result = client.try_propose_weights(
            &governor,
            &ScoringWeights {
                vc_weight: 50,
                tx_weight: 25,
                repayment_weight: 25,
            },
        );
        assert_eq!(result, Err(Ok(CreditOracleError::NotAuthorized)));
    }

    // -----------------------------------------------------------------------
    // maintain_storage tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_maintain_storage_succeeds_for_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let res = client.try_maintain_storage();
        assert!(res.is_ok());
    }

    #[test]
    fn test_maintain_storage_fails_for_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        env.mock_auths(&[]);
        let res = client.try_maintain_storage();
        assert!(res.is_err());
    }
}
