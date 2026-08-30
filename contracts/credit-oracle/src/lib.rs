#![no_std]
pub use credit_oracle_types::{CreditOracleError, PendingWeightsRecord, ScoringWeights};
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, Address, BytesN, Env, IntoVal, String,
    Symbol, TryFromVal, Val, Vec,
};

pub const MIN_SCORE: u32 = 300;
pub const MAX_SCORE: u32 = 850;

/// Aggregate protocol-level counters stored in instance storage.
///
/// Updated on every write operation to provide on-chain operational metrics
/// without requiring an external indexer.
#[contracttype]
#[derive(Clone, Default)]
pub struct ProtocolStats {
    /// Total number of unique subjects that have had a credit score computed.
    pub total_subjects_scored: u64,
    /// Total number of repayment events recorded across all subjects.
    pub total_repayments_recorded: u64,
}

/// Storage keys for the credit oracle contract
#[contracttype]
pub enum DataKey {
    /// Contract administrator address
    Admin,
    /// Pending contract admin address for two-step transfer
    PendingAdmin,
    /// Storage layout version.
    StorageVersion,
    /// Global configuration
    Config,
    /// Registered weight for a VC credential type (default 100 when unset)
    VcWeight(Symbol),
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
    /// Stored VC list per user with type tags
    VcList(Address),
    /// Pending weights awaiting timelock
    PendingWeights,
    /// Ledger number when pending weights become effective
    PendingWeightsEffectiveLedger,
    /// Identity oracle contract ID for cross-contract lookups
    IdentityOracleId,
    /// Number of ledgers to wait before recomputing score
    ComputeCooldownLedgers,
    /// Aggregate protocol-level counters
    ProtocolStats,
    /// Index of all registered feeders
    FeedersIndex,
    /// Index of all registered lenders
    LendersIndex,
    /// Dispute record for a (subject, input_key) pair
    Dispute(Address, Symbol),
    /// Index of all disputed input keys for a subject
    DisputeIndex(Address),
    /// Whether the contract is currently paused for writes.
    Paused,
    /// Issue #530: whether VC recency decay is applied inside `compute_score`.
    /// Absent or `false` reproduces the pre-#530 scoring behavior exactly.
    RecencyDecayEnabled,
    /// Issue #530: basis points of recency multiplier lost per day of VC age.
    DecayBpsPerDay,
    /// Issue #530: lower bound (bps) the recency multiplier may decay to.
    MinRecencyBps,
}

/// Basis-point denominator: 10_000 bps == 1.00x multiplier.
pub const BPS_DENOMINATOR: u32 = 10_000;

/// Seconds in one day. `VCRecord.anchored_at` is a ledger timestamp in Unix
/// seconds, so credential age must be divided by this to obtain whole days.
pub const SECONDS_PER_DAY: u64 = 86_400;

/// Default decay rate from docs/vc-weighting-design.md: 5 bps/day (0.05%).
pub const DEFAULT_DECAY_BPS_PER_DAY: u32 = 5;

/// Default recency floor from docs/vc-weighting-design.md: 5_000 bps (50%).
pub const DEFAULT_MIN_RECENCY_BPS: u32 = 5_000;

/// Recency decay configuration for the VC component of the score (issue #530).
///
/// Returned by `get_recency_decay` and written by `set_recency_decay`. When
/// `enabled` is `false` the other two fields are ignored and `compute_score`
/// behaves exactly as it did before recency decay existed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecencyDecayConfig {
    /// Whether `compute_score` applies the recency multiplier.
    pub enabled: bool,
    /// Basis points of multiplier lost per whole day of credential age.
    pub decay_bps_per_day: u32,
    /// Floor (bps) below which the multiplier never falls, e.g. 5_000 = 50%.
    pub min_recency_bps: u32,
}

/// Recency multiplier in basis points for a credential anchored at `anchored_at`.
///
/// Implements the model documented in docs/vc-weighting-design.md:
///
/// ```text
/// recency_multiplier = max(min_recency_bps, 10_000 - age_days * decay_bps_per_day)
/// ```
///
/// `anchored_at` and `now` are ledger timestamps in **Unix seconds**; the age is
/// converted to whole days by integer division by `SECONDS_PER_DAY`, so a
/// credential keeps full weight for its first 24 hours.
///
/// Saturating arithmetic throughout: a credential anchored in the future (clock
/// skew, or a replayed ledger) yields age 0 rather than underflowing, and an
/// arbitrarily large `decay_bps_per_day` clamps to the floor instead of
/// panicking under `overflow-checks`. `min_recency_bps` is also clamped to
/// `BPS_DENOMINATOR` so a mis-set floor can never *increase* a VC contribution.
///
/// The result is monotonically non-decreasing in `anchored_at`: a younger
/// credential always yields a multiplier greater than or equal to an older one.
pub fn recency_multiplier_bps(
    anchored_at: u64,
    now: u64,
    decay_bps_per_day: u32,
    min_recency_bps: u32,
) -> u32 {
    let age_days = now.saturating_sub(anchored_at) / SECONDS_PER_DAY;
    let decay = age_days.saturating_mul(decay_bps_per_day as u64);
    let multiplier = (BPS_DENOMINATOR as u64).saturating_sub(decay);
    let floor = (min_recency_bps as u64).min(BPS_DENOMINATOR as u64);
    multiplier.max(floor) as u32
}

/// Scale `points` by a basis-point multiplier, rounding down.
///
/// Widened to `u64` before multiplying: `points` is already type-weighted and
/// `recency_bps` can be up to 10_000, so the product overflows `u32` for any
/// non-trivial input and would panic under `overflow-checks`.
pub fn apply_recency_bps(points: u32, recency_bps: u32) -> u32 {
    ((points as u64).saturating_mul(recency_bps as u64) / BPS_DENOMINATOR as u64) as u32
}

/// Pure scoring function that computes a credit score from input parameters.
///
/// This function contains no Soroban environment dependencies, making it
/// suitable for fuzz testing and property-based testing.
/// Score is always clamped to [MIN_SCORE, MAX_SCORE] range.
#[allow(clippy::too_many_arguments)]
pub fn compute_score_pure(
    vc_points: u32,
    volume_30d: i128,
    avg_counterparties: u32,
    on_time_count: u32,
    total_count: u32,
    total_repaid: i128,
    vc_weight: u32,
    tx_weight: u32,
    repayment_weight: u32,
) -> u32 {
    let vc_score = (vc_points).min(100) as u128;
    let volume_score = ((volume_30d / 100_000_000i128).max(0) as u128).min(80);
    let counterparty_bonus = (avg_counterparties / 5).min(20) as u128;
    let tx_score = (volume_score + counterparty_bonus).min(100);
    let repayment_rate_score = (on_time_count as u128)
        .saturating_mul(10_000)
        .checked_div(total_count as u128)
        .map(|r| r / 100)
        .unwrap_or(0);
    let repayment_volume_score = ((total_repaid / 100_000_000i128).max(0) as u128).min(100);
    let repay_score = (repayment_rate_score + repayment_volume_score) / 2;
    let composite = (vc_score * vc_weight as u128
        + tx_score * tx_weight as u128
        + repay_score * repayment_weight as u128)
        / 100;
    let score = MIN_SCORE as u128 + composite.saturating_mul(550) / 100;
    score.min(MAX_SCORE as u128).max(MIN_SCORE as u128) as u32
}

/// Number of ledgers after which a score is considered stale.
/// Roughly 30 days at 5-second ledgers (~86,400 ledgers).
const STALE_LEDGER_AGE: u32 = 86_400;

fn validate_contract_function<T>(env: &Env, contract_id: &Address, func: Symbol, args: Vec<Val>) -> bool
where
    T: TryFromVal<Env, Val>,
{
    matches!(
        env.try_invoke_contract::<T, soroban_sdk::InvokeError>(contract_id, &func, args),
        Ok(Ok(_))
    )
}

fn validate_identity_oracle_ref(env: &Env, identity_oracle_id: &Address) -> bool {
    let dummy_subject = env.current_contract_address();
    validate_contract_function::<u32>(
        env,
        identity_oracle_id,
        Symbol::new(env, "get_active_vc_count"),
        soroban_sdk::vec![env, dummy_subject.clone().into_val(env)],
    )
}

/// Credit score record with metadata
#[contracttype]
#[derive(Clone)]
pub struct ScoreRecord {
    /// Credit score value
    pub score: u32,
    /// Timestamp of last update (Unix seconds)
    pub last_updated: u64,
    /// Number of verified credentials
    pub vc_count: u32,
    /// Repayment rate in basis points (0-10000)
    pub repayment_rate: u32,
    /// Transaction volume in last 30 days
    pub tx_volume_30d: i128,
    /// Previous credit score, if one exists
    pub previous_score: Option<u32>,
    /// Ledger sequence number when this score was last computed.
    /// Consumers can compare this against the current ledger sequence
    /// to determine freshness without relying solely on wall-clock time.
    pub computed_at_ledger: u32,
    /// Whether the stored score is considered stale based on
    /// `STALE_LEDGER_AGE`. Computed at read time in `get_score` by
    /// comparing `computed_at_ledger` against the current ledger
    /// sequence. Always `false` for a freshly computed score.
    pub stale: bool,
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

/// Internal repayment counters for a subject
#[contracttype]
#[derive(Clone)]
pub struct RepaymentRecord {
    pub on_time_count: u32,
    pub total_count: u32,
    /// Cumulative amount repaid across all recorded repayments.
    pub total_repaid: i128,
}

/// Legacy repayment counters stored by V1 of the contract.
///
/// Preserved as a distinct type so the `migrate` function can deserialise
/// pre-upgrade storage entries and convert them to [`RepaymentRecord`].
#[contracttype]
#[derive(Clone)]
pub struct RepaymentRecordV1 {
    /// Number of repayments made on time.
    pub on_time_count: u32,
    /// Total number of repayments recorded.
    pub total_count: u32,
}
/// A verifiable credential entry stored per-user with an optional type tag.
#[contracttype]
#[derive(Clone)]
pub struct VcEntry {
    /// VC hash (e.g., SHA-256 of the off-chain VC document).
    pub vc_id: BytesN<32>,
    /// Optional credential type label (e.g., "kyc", "employment").
    /// `None` defaults to weight 100.
    pub vc_type: Option<Symbol>,
}

/// An on-chain anchor record for a verifiable credential in the identity oracle.
#[contracttype]
#[derive(Clone)]
pub struct IdentityVCRecord {
    /// SHA-256 hash of the off-chain verifiable credential JSON.
    pub vc_hash: BytesN<32>,
    /// Address of the issuer who anchored this credential.
    pub issuer: Address,
    /// Ledger timestamp (Unix seconds) when this credential was anchored.
    pub anchored_at: u64,
    /// Whether this credential has been revoked by the issuer.
    pub revoked: bool,
}

/// Status of an on-chain score input dispute.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisputeStatus {
    /// Dispute filed; awaiting admin review.
    Pending,
    /// Admin accepted the dispute; feeder re-sync requested.
    Resolved,
    /// Admin rejected the dispute; input deemed correct.
    Rejected,
}

/// A subject's on-chain record disputing a specific score input.
///
/// Filed via `flag_score_input`; resolved by the admin via `resolve_dispute`.
/// The `input_key` is one of `tx_stats`, `repayment`, or `vc_count`.
#[contracttype]
#[derive(Clone)]
pub struct DisputeRecord {
    /// The subject who filed the dispute.
    pub subject: Address,
    /// Which input is disputed: `tx_stats`, `repayment`, or `vc_count`.
    pub input_key: Symbol,
    /// Free-text reason provided by the subject.
    pub reason: String,
    /// Ledger sequence number when the dispute was filed.
    pub filed_at_ledger: u32,
    /// Current resolution status.
    pub status: DisputeStatus,
}

const TIMELOCK_LEDGERS: u32 = 17_280; // approximately 24 hours
const DEFAULT_COMPUTE_COOLDOWN_LEDGERS: u32 = 1;
/// Persistent-entry TTL threshold (≈ 7 days at 5 s/ledger).
const PERS_TTL_THRESHOLD: u32 = 120_960;
/// Persistent-entry TTL extension (≈ 30 days at 5 s/ledger).
const PERS_TTL_EXTEND: u32 = 518_400;

#[contract]
pub struct CreditOracle;

fn load_protocol_stats(env: &Env) -> ProtocolStats {
    env.storage()
        .instance()
        .get(&DataKey::ProtocolStats)
        .unwrap_or_default()
}

fn save_protocol_stats(env: &Env, stats: &ProtocolStats) {
    env.storage().instance().set(&DataKey::ProtocolStats, stats);
}

/// Read the recency decay configuration from instance storage (issue #530).
///
/// Every key is optional: a contract deployed before #530 (or one whose admin
/// never called `set_recency_decay`) reads back `enabled: false` plus the
/// documented defaults, which keeps `compute_score` backward compatible with
/// no storage migration.
fn load_recency_config(env: &Env) -> RecencyDecayConfig {
    RecencyDecayConfig {
        enabled: env
            .storage()
            .instance()
            .get(&DataKey::RecencyDecayEnabled)
            .unwrap_or(false),
        decay_bps_per_day: env
            .storage()
            .instance()
            .get(&DataKey::DecayBpsPerDay)
            .unwrap_or(DEFAULT_DECAY_BPS_PER_DAY),
        min_recency_bps: env
            .storage()
            .instance()
            .get(&DataKey::MinRecencyBps)
            .unwrap_or(DEFAULT_MIN_RECENCY_BPS),
    }
}

fn ensure_not_paused(env: &Env) -> Result<(), CreditOracleError> {
    if env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
    {
        Err(CreditOracleError::ContractPaused)
    } else {
        Ok(())
    }
}

fn increment_subjects_scored(env: &Env) {
    let mut stats = load_protocol_stats(env);
    stats.total_subjects_scored = stats
        .total_subjects_scored
        .checked_add(1)
        .expect("total_subjects_scored overflow");
    save_protocol_stats(env, &stats);
}

fn increment_repayments_recorded(env: &Env) {
    let mut stats = load_protocol_stats(env);
    stats.total_repayments_recorded = stats
        .total_repayments_recorded
        .checked_add(1)
        .expect("total_repayments_recorded overflow");
    save_protocol_stats(env, &stats);
}

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
        env.storage()
            .instance()
            .set(&DataKey::Config, &default_weights);
        env.storage().instance().set(
            &DataKey::ComputeCooldownLedgers,
            &DEFAULT_COMPUTE_COOLDOWN_LEDGERS,
        );
        // New deployments start at storage layout V2 — no migration needed.
        env.storage()
            .instance()
            .set(&DataKey::StorageVersion, &2u32);
        env.events()
            .publish((symbol_short!("Init"),), admin.clone());
        Ok(())
    }

    /// Register a trusted feeder address
    pub fn register_feeder(
        env: Env,
        admin: Address,
        feeder: Address,
    ) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();

        let feeder_key = DataKey::TrustedFeeder(feeder.clone());
        if !env.storage().persistent().has(&feeder_key) {
            let mut feeders: Vec<Address> = env
                .storage()
                .persistent()
                .get(&DataKey::FeedersIndex)
                .unwrap_or(Vec::new(&env));
            feeders.push_back(feeder.clone());
            let index_key = DataKey::FeedersIndex;
            env.storage()
                .persistent()
                .set(&index_key, &feeders);
            env.storage()
                .persistent()
                .extend_ttl(&index_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        }

        env.storage().persistent().set(&feeder_key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&feeder_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        env.events().publish((symbol_short!("FdrReg"),), feeder);
        Ok(())
    }

    /// Deregister a trusted feeder address
    pub fn deregister_feeder(
        env: Env,
        admin: Address,
        feeder: Address,
    ) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage()
            .persistent()
            .remove(&DataKey::TrustedFeeder(feeder.clone()));

        let ever_registered: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::FeedersIndex)
            .unwrap_or(Vec::new(&env));

        let mut compacted = Vec::new(&env);
        for i in 0..ever_registered.len() {
            let addr: Address = ever_registered.get(i).unwrap();
            if env.storage().persistent().has(&DataKey::TrustedFeeder(addr.clone())) {
                compacted.push_back(addr);
            }
        }
        env.storage().persistent().set(&DataKey::FeedersIndex, &compacted);

        env.events().publish((symbol_short!("FdrDeReg"),), feeder);
        Ok(())
    }

    /// Register a trusted lender address
    pub fn register_lender(
        env: Env,
        admin: Address,
        lender: Address,
    ) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();

        let lender_key = DataKey::TrustedLender(lender.clone());
        if !env.storage().persistent().has(&lender_key) {
            let mut lenders: Vec<Address> = env
                .storage()
                .persistent()
                .get(&DataKey::LendersIndex)
                .unwrap_or(Vec::new(&env));
            lenders.push_back(lender.clone());
            let index_key = DataKey::LendersIndex;
            env.storage()
                .persistent()
                .set(&index_key, &lenders);
            env.storage()
                .persistent()
                .extend_ttl(&index_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        }

        env.storage().persistent().set(&lender_key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&lender_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        env.events().publish((symbol_short!("LndReg"),), lender);
        Ok(())
    }

    /// Deregister a trusted lender address
    pub fn deregister_lender(
        env: Env,
        admin: Address,
        lender: Address,
    ) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage()
            .persistent()
            .remove(&DataKey::TrustedLender(lender.clone()));

        let ever_registered: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::LendersIndex)
            .unwrap_or(Vec::new(&env));

        let mut compacted = Vec::new(&env);
        for i in 0..ever_registered.len() {
            let addr: Address = ever_registered.get(i).unwrap();
            if env.storage().persistent().has(&DataKey::TrustedLender(addr.clone())) {
                compacted.push_back(addr);
            }
        }
        env.storage().persistent().set(&DataKey::LendersIndex, &compacted);

        env.events().publish((symbol_short!("LndDeReg"),), lender);
        Ok(())
    }

    /// Update transaction statistics for a user
    /// Pause all writes on the contract.
    ///
    /// Auth: admin only.
    pub fn pause(env: Env, admin: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(CreditOracleError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish((symbol_short!("Paused"),), ());
        Ok(())
    }

    /// Resume the contract and allow writes again.
    ///
    /// Auth: admin only.
    pub fn unpause(env: Env, admin: Address) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(CreditOracleError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish((symbol_short!("Unpaused"),), ());
        Ok(())
    }

    /// Returns true if the contract is paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

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

    /// Record a repayment event for a user
    pub fn record_repayment(
        env: Env,
        lender: Address,
        subject: Address,
        amount: i128,
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
        let current_version: u32 = env
            .storage()
            .instance()
            .get(&DataKey::StorageVersion)
            .unwrap_or(1);

        if current_version < 2 {
            // Storage is still V1 layout — read and write back as RepaymentRecordV1
            // so we don't corrupt existing data before migrate() is called.
            let mut record: RepaymentRecordV1 = env
                .storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject.clone()))
                .unwrap_or(RepaymentRecordV1 {
                    on_time_count: 0,
                    total_count: 0,
                });
            if on_time {
                record.on_time_count = record.on_time_count.saturating_add(1);
            }
            record.total_count = record.total_count.saturating_add(1);
            env.storage()
                .persistent()
                .set(&DataKey::RepaymentRecord(subject), &record);
        } else {
            let mut record: RepaymentRecord = env
                .storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject.clone()))
                .unwrap_or(RepaymentRecord {
                    on_time_count: 0,
                    total_count: 0,
                    total_repaid: 0,
                });
            if on_time {
                record.on_time_count = record.on_time_count.saturating_add(1);
            }
            record.total_count = record.total_count.saturating_add(1);
            record.total_repaid = record.total_repaid.saturating_add(amount);
            env.storage()
                .persistent()
                .set(&DataKey::RepaymentRecord(subject), &record);
        }
        increment_repayments_recorded(&env);
        Ok(())
    }

    /// Migrate stored repayment records from V1 (2-field) to V2 (3-field) layout.
    ///
    /// Call this once after upgrading the contract WASM. Pass every subject
    /// whose repayment history must be preserved. After all subjects are
    /// converted the contract bumps `StorageVersion` to `2` so future reads
    /// and writes use the new layout automatically.
    ///
    /// **Authentication:** only the current admin may call this function.
    pub fn migrate(env: Env, subjects: soroban_sdk::Vec<Address>) -> Result<(), CreditOracleError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(CreditOracleError::NotInitialized)?;
        admin.require_auth();

        for i in 0..subjects.len() {
            let subject = subjects.get(i).unwrap();
            let key = DataKey::RepaymentRecord(subject.clone());
            if let Some(v1) = env
                .storage()
                .persistent()
                .get::<DataKey, RepaymentRecordV1>(&key)
            {
                let v2 = RepaymentRecord {
                    on_time_count: v1.on_time_count,
                    total_count: v1.total_count,
                    total_repaid: 0,
                };
                env.storage().persistent().set(&key, &v2);
                env.storage()
                    .persistent()
                    .extend_ttl(&key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::StorageVersion, &2u32);
        Ok(())
    }

    /// Cache VC count for a subject (feeder-only)
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

        // Emit deprecation warning if identity oracle is configured
        if env.storage().instance().has(&DataKey::IdentityOracleId) {
            env.events().publish(
                (symbol_short!("VcCntDep"),),
                (subject.clone(), count),
            );
        }

        env.storage()
            .persistent()
            .set(&DataKey::VcCount(subject), &count);
        Ok(())
    }

    /// Register a weight for a VC credential type.
    ///
    /// When `compute_score` processes a VC, its type weight (in basis points)
    /// determines how many score points that VC contributes:
    ///   `points = 20 × weight / 100`
    /// Default weight for unregistered types (or VCs with no type) is 100 (1×).
    ///
    /// Auth: admin only.
    pub fn set_vc_type_weight(
        env: Env,
        admin: Address,
        type_name: Symbol,
        weight: u32,
    ) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::VcWeight(type_name.clone()), &weight);
        env.events()
            .publish((symbol_short!("VcWtSet"),), (type_name, weight));
        Ok(())
    }

    /// Return the registered weight for a credential type (default 100).
    pub fn get_vc_type_weight(env: Env, type_name: Symbol) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::VcWeight(type_name))
            .unwrap_or(100u32)
    }

    /// Configure VC recency decay for the score's VC component (issue #530).
    ///
    /// When `enabled`, `compute_score` multiplies each credential's weighted
    /// points by
    ///   `max(min_recency_bps, 10_000 - age_days * decay_bps_per_day) / 10_000`
    /// where `age_days` derives from `VCRecord.anchored_at`. Decay therefore
    /// only applies on the cross-contract path, which requires
    /// `set_identity_oracle` to have been called - `anchored_at` is only
    /// available from `get_vc_details`.
    ///
    /// Decay is **opt-in**: the feature ships disabled, so existing deployments
    /// score identically until an admin turns it on. Disabling it again
    /// restores the previous behavior on the next `compute_score`.
    ///
    /// Suggested values from docs/vc-weighting-design.md: `decay_bps_per_day =
    /// 5` (0.05%/day) and `min_recency_bps = 5_000` (50% floor).
    ///
    /// Auth: admin only.
    ///
    /// Errors with `InvalidRecencyConfig` when `min_recency_bps` exceeds
    /// `BPS_DENOMINATOR`, which would otherwise define a floor above full
    /// weight. `decay_bps_per_day` needs no bound: the floor already caps how
    /// far any rate can push the multiplier down.
    pub fn set_recency_decay(
        env: Env,
        admin: Address,
        enabled: bool,
        decay_bps_per_day: u32,
        min_recency_bps: u32,
    ) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        if min_recency_bps > BPS_DENOMINATOR {
            return Err(CreditOracleError::InvalidRecencyConfig);
        }

        env.storage()
            .instance()
            .set(&DataKey::RecencyDecayEnabled, &enabled);
        env.storage()
            .instance()
            .set(&DataKey::DecayBpsPerDay, &decay_bps_per_day);
        env.storage()
            .instance()
            .set(&DataKey::MinRecencyBps, &min_recency_bps);

        env.events().publish(
            (symbol_short!("RecDecay"),),
            (enabled, decay_bps_per_day, min_recency_bps),
        );
        Ok(())
    }

    /// Return the current recency decay configuration (issue #530).
    ///
    /// Defaults to `enabled: false` with the documented suggested parameters
    /// when the admin has never called `set_recency_decay`.
    pub fn get_recency_decay(env: Env) -> RecencyDecayConfig {
        load_recency_config(&env)
    }

    /// Anchor a verifiable credential for a user with an optional type tag.
    ///
    /// The `vc_type` can later be looked up by `compute_score` to apply
    /// per-type weights.  Duplicate `vc_id`s are silently ignored.
    ///
    /// Auth: registered feeder.
    pub fn anchor_vc(
        env: Env,
        feeder: Address,
        user: Address,
        vc_id: BytesN<32>,
        vc_type: Option<Symbol>,
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
        let list_key = DataKey::VcList(user.clone());
        let mut list: Vec<VcEntry> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or(Vec::new(&env));
        for entry in list.iter() {
            if entry.vc_id == vc_id {
                return Ok(());
            }
        }
        list.push_back(VcEntry {
            vc_id,
            vc_type: vc_type.clone(),
        });
        env.storage().persistent().set(&list_key, &list);
        env.storage()
            .persistent()
            .extend_ttl(&list_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        env.events()
            .publish((symbol_short!("VcAnchCr"),), (user, vc_type));
        Ok(())
    }

    /// Return the stored VC list for a user.
    pub fn get_vc_list(env: Env, user: Address) -> Vec<VcEntry> {
        env.storage()
            .persistent()
            .get(&DataKey::VcList(user))
            .unwrap_or(Vec::new(&env))
    }

    /// Compute and store credit score for a user
    pub fn compute_score(env: Env, subject: Address) -> Result<u32, CreditOracleError> {
        ensure_not_paused(&env)?;
        // Reject if last computation was within the cooldown window
        Self::check_compute_cooldown(&env, &subject)?;

        let tx_stats: TxStats = env
            .storage()
            .persistent()
            .get(&DataKey::TxStats(subject.clone()))
            .unwrap_or(TxStats {
                volume_30d: 0,
                tx_count_30d: 0,
                avg_counterparties: 0,
            });

        let storage_version: u32 = env
            .storage()
            .instance()
            .get(&DataKey::StorageVersion)
            .unwrap_or(1);

        let repayment: RepaymentRecord = if storage_version < 2 {
            // V1 layout: two-field struct. Convert to V2 with total_repaid = 0.
            let v1: RepaymentRecordV1 = env
                .storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject.clone()))
                .unwrap_or(RepaymentRecordV1 {
                    on_time_count: 0,
                    total_count: 0,
                });
            RepaymentRecord {
                on_time_count: v1.on_time_count,
                total_count: v1.total_count,
                total_repaid: 0,
            }
        } else {
            env.storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject.clone()))
                .unwrap_or(RepaymentRecord {
                    on_time_count: 0,
                    total_count: 0,
                    total_repaid: 0,
                })
        };

        let mut vc_points: u32 = 0;
        let mut vc_count: u32 = 0;

        // Issue #530: recency decay is opt-in. `None` here means the multiplier
        // is never computed, so the per-VC arithmetic below is identical to the
        // pre-#530 code path.
        let recency_config = load_recency_config(&env);
        let recency = if recency_config.enabled {
            Some(recency_config)
        } else {
            None
        };
        let now = env.ledger().timestamp();

        // Check if identity-oracle is configured
        if let Some(identity_oracle_id) = env
            .storage()
            .instance()
            .get::<_, Address>(&DataKey::IdentityOracleId)
        {
            let is_deactivated: bool = env.invoke_contract(
                &identity_oracle_id,
                &Symbol::new(&env, "is_deactivated"),
                soroban_sdk::vec![&env, subject.clone().into_val(&env)],
            );
            if is_deactivated {
                return Ok(MIN_SCORE);
            }

            let identity_vcs = env.invoke_contract::<Vec<IdentityVCRecord>>(
                &identity_oracle_id,
                &Symbol::new(&env, "get_vc_details"),
                soroban_sdk::vec![&env, subject.clone().into_val(&env)],
            );

            for record in identity_vcs.iter() {
                let is_verified = env.invoke_contract::<bool>(
                    &identity_oracle_id,
                    &Symbol::new(&env, "verify_vc"),
                    soroban_sdk::vec![&env, subject.clone().into_val(&env), record.vc_hash.clone().into_val(&env)],
                );
                if !is_verified {
                    continue;
                }
                vc_count += 1;
                let vc_type = env.invoke_contract::<Symbol>(
                    &identity_oracle_id,
                    &Symbol::new(&env, "get_vc_credential_type"),
                    soroban_sdk::vec![&env, subject.clone().into_val(&env), record.vc_hash.clone().into_val(&env)],
                );
                let weight = env
                    .storage()
                    .instance()
                    .get(&DataKey::VcWeight(vc_type.clone()))
                    .unwrap_or(100u32);
                let mut points = 20u32.saturating_mul(weight) / 100;
                // Issue #530: decay this credential by its age. `anchored_at` is
                // only reachable on this cross-contract path, which is why decay
                // requires a configured identity-oracle. Computed per VC inside
                // the loop, alongside the existing per-VC type-weight lookup.
                if let Some(ref cfg) = recency {
                    let recency_bps = recency_multiplier_bps(
                        record.anchored_at,
                        now,
                        cfg.decay_bps_per_day,
                        cfg.min_recency_bps,
                    );
                    points = apply_recency_bps(points, recency_bps);
                }
                vc_points = vc_points.saturating_add(points);
            }
        } else {
            // Compute weighted VC points from stored VC list
            let vc_list_key = DataKey::VcList(subject.clone());
            let vc_list: Vec<VcEntry> = env
                .storage()
                .persistent()
                .get(&vc_list_key)
                .unwrap_or(Vec::new(&env));
            
            if !vc_list.is_empty() {
                vc_count = vc_list.len();
                for entry in vc_list.iter() {
                    let weight = match entry.vc_type {
                        Some(ref t) => env
                            .storage()
                            .instance()
                            .get(&DataKey::VcWeight(t.clone()))
                            .unwrap_or(100u32),
                        None => 100u32,
                    };
                    vc_points = vc_points.saturating_add(20u32.saturating_mul(weight) / 100);
                }
            } else {
                // Fall back to cached VcCount
                vc_count = env
                    .storage()
                    .persistent()
                    .get(&DataKey::VcCount(subject.clone()))
                    .unwrap_or(0u32);
                vc_points = vc_count.saturating_mul(20).min(100);
            }
        }
        let vc_points = vc_points.min(100);

        let weights: ScoringWeights = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(CreditOracleError::NotInitialized)?;

        let score = compute_score_pure(
            vc_points,
            tx_stats.volume_30d,
            tx_stats.avg_counterparties,
            repayment.on_time_count,
            repayment.total_count,
            repayment.total_repaid,
            weights.vc_weight,
            weights.tx_weight,
            weights.repayment_weight,
        );

        // Compute the repayment rate in basis points. The counters are u32, so
        // the product must be widened to u64: `on_time_count * 10000` overflows
        // u32 once a subject passes ~429k on-time repayments, which would panic
        // (overflow-checks are enabled) and permanently brick compute_score.
        let repayment_rate = (repayment.on_time_count as u64 * 10_000)
            .checked_div(repayment.total_count as u64)
            .unwrap_or(0) as u32;

        let mut previous_score: Option<u32> = None;
        let mut needs_write = true;
        let mut is_first_computation = true;
        if let Some(prev) = env
            .storage()
            .persistent()
            .get::<_, ScoreRecord>(&DataKey::Score(subject.clone()))
        {
            if prev.score == score
                && prev.vc_count == vc_count
                && prev.repayment_rate == repayment_rate
                && prev.tx_volume_30d == tx_stats.volume_30d
            {
                needs_write = false;
            }
            previous_score = Some(prev.score);
            is_first_computation = false;
        }

        let _is_first = !env
            .storage()
            .persistent()
            .has(&DataKey::Score(subject.clone()));

        if needs_write {
            if is_first_computation {
                increment_subjects_scored(&env);
            }
            let score_key = DataKey::Score(subject.clone());
            env.storage().persistent().set(
                &score_key,
                &ScoreRecord {
                    score,
                    last_updated: env.ledger().timestamp(),
                    vc_count,
                    repayment_rate,
                    tx_volume_30d: tx_stats.volume_30d,
                    previous_score,
                    computed_at_ledger: env.ledger().sequence(),
                    stale: false,
                },
            );
            env.storage()
                .persistent()
                .extend_ttl(&score_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        }

        env.events()
            .publish((symbol_short!("Score"),), (subject.clone(), score));

        Ok(score)
    }

    /// Check cooldown — reject if the last computation was within the cooldown window.
    fn check_compute_cooldown(env: &Env, subject: &Address) -> Result<(), CreditOracleError> {
        let cooldown: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ComputeCooldownLedgers)
            .unwrap_or(0);

        if cooldown > 0 {
            if let Some(record) = env
                .storage()
                .persistent()
                .get::<_, ScoreRecord>(&DataKey::Score(subject.clone()))
            {
                let current_ledger = env.ledger().sequence();
                if current_ledger.saturating_sub(record.computed_at_ledger) < cooldown {
                    return Err(CreditOracleError::ComputeCooldownActive);
                }
            }
        }
        Ok(())
    }

    /// Get credit score for a user; returns None if score has not been computed yet.
    ///
    /// The returned `ScoreRecord` includes a `stale` flag computed
    /// at read time by comparing `computed_at_ledger` against the
    /// current ledger sequence. A score is considered stale when the
    /// ledger delta exceeds `STALE_LEDGER_AGE` (~30 days).
    ///
    /// When an identity-oracle is configured, the record is additionally
    /// marked stale if the subject's identity state changed *after* the score
    /// was computed (`computed_at_ledger` is older than the identity-oracle's
    /// `get_last_state_change_ledger`), so a cached score never outlives the
    /// on-chain identity data it was derived from.
    pub fn get_score(env: Env, subject: Address) -> Option<ScoreRecord> {
        let current_ledger = env.ledger().sequence();
        env.storage()
            .persistent()
            .get::<_, ScoreRecord>(&DataKey::Score(subject.clone()))
            .map(|mut record| {
                record.stale =
                    current_ledger.saturating_sub(record.computed_at_ledger) > STALE_LEDGER_AGE;
                if !record.stale {
                    if let Some(identity_oracle_id) = env
                        .storage()
                        .instance()
                        .get::<_, Address>(&DataKey::IdentityOracleId)
                    {
                        let last_state_change: Option<u32> = env
                            .try_invoke_contract::<Option<u32>, soroban_sdk::InvokeError>(
                                &identity_oracle_id,
                                &Symbol::new(&env, "get_last_state_change_ledger"),
                                soroban_sdk::vec![&env, subject.clone().into_val(&env)],
                            )
                            .ok()
                            .and_then(|res| res.ok())
                            .flatten();
                        if let Some(change_ledger) = last_state_change {
                            if record.computed_at_ledger < change_ledger {
                                record.stale = true;
                            }
                        }
                    }
                }
                record
            })
    }

    /// Propose new scoring weights with timelock
    /// Propose new scoring weights with timelock
    pub fn propose_weights(env: Env, weights: ScoringWeights) -> Result<(), CreditOracleError> {
        if !weights.is_valid() {
            return Err(CreditOracleError::InvalidWeights);
        }
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        stored_admin.require_auth();

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
    ///
    /// Returns a typed error instead of panicking so callers (governance
    /// tooling, scripts, cross-contract relays) can distinguish calling too
    /// early (`TimelockNotExpired`) from nothing being queued
    /// (`NoPendingWeights`).
    pub fn apply_weights(env: Env) -> Result<(), CreditOracleError> {
        ensure_not_paused(&env)?;
        let effective_ledger: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PendingWeightsEffectiveLedger)
            .ok_or(CreditOracleError::NoPendingWeights)?;

        if env.ledger().sequence() < effective_ledger {
            return Err(CreditOracleError::TimelockNotExpired);
        }

        let weights: ScoringWeights = env
            .storage()
            .instance()
            .get(&DataKey::PendingWeights)
            .ok_or(CreditOracleError::NoPendingWeights)?;

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

    /// Get current scoring weights
    pub fn get_scoring_weights(env: Env) -> Result<ScoringWeights, CreditOracleError> {
        env.storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(CreditOracleError::NotInitialized)
    }

    /// Set the identity-oracle contract ID for cross-contract VC count lookups.
    ///
    /// When configured, `compute_score` will call `get_active_vc_count` on the
    /// identity-oracle instead of reading the cached `VcCount` storage key.
    /// This enables live VC count resolution that automatically excludes revoked VCs.
    ///
    /// Auth: admin only.
    pub fn set_identity_oracle(
        env: Env,
        admin: Address,
        identity_oracle_id: Address,
    ) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        if !validate_identity_oracle_ref(&env, &identity_oracle_id) {
            return Err(CreditOracleError::InvalidIdentityOracle);
        }
        env.storage()
            .instance()
            .set(&DataKey::IdentityOracleId, &identity_oracle_id);
        env.events()
            .publish((symbol_short!("IdOracle"),), identity_oracle_id);
        Ok(())
    }

    /// Returns the configured identity-oracle contract ID, if any.
    ///
    /// Returns `None` if cross-contract VC count lookup is not configured.
    pub fn get_identity_oracle(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::IdentityOracleId)
    }

    /// Get pending weights (if any)
    pub fn get_pending_weights(env: Env) -> Option<PendingWeightsRecord> {
        let weights: Option<ScoringWeights> =
            env.storage().instance().get(&DataKey::PendingWeights);
        weights.map(|w| {
            let effective_ledger: u32 = env
                .storage()
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
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        stored_admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::PendingAdmin, &new_admin);
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
        env.events()
            .publish((symbol_short!("AdmAccept"),), new_admin);
        Ok(())
    }

    /// Upgrade the contract WASM in-place, preserving address and all stored state.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        if admin != stored_admin {
            return Err(CreditOracleError::NotAuthorized);
        }
        admin.require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    /// Returns aggregate protocol-level counters.
    ///
    /// These counters are updated on every write operation and provide
    /// on-chain operational metrics without requiring an external indexer.
    pub fn get_protocol_stats(env: Env) -> ProtocolStats {
        load_protocol_stats(&env)
    }

    /// Returns all currently registered feeder addresses.
    pub fn list_feeders(env: Env) -> Vec<Address> {
        let feeders: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::FeedersIndex)
            .unwrap_or(Vec::new(&env));

        let mut active = Vec::new(&env);
        for i in 0..feeders.len() {
            let feeder: Address = feeders.get(i).unwrap();
            if env.storage().persistent().has(&DataKey::TrustedFeeder(feeder.clone())) {
                active.push_back(feeder);
            }
        }
        active
    }

    /// Flag a specific score input as potentially incorrect.
    ///
    /// The subject authenticates themselves and names which of the three score
    /// inputs they believe is wrong (`tx_stats`, `repayment`, or `vc_count`),
    /// providing a free-text reason.
    ///
    /// Anti-griefing: only one `Pending` dispute per `(subject, input_key)` is
    /// allowed at a time.  Filing a second dispute for the same key while the
    /// first is still pending returns `DisputeAlreadyPending`.
    ///
    /// Emits a `DsptFild` event that off-chain feeders and admins can index
    /// to trigger a review workflow.
    pub fn flag_score_input(
        env: Env,
        subject: Address,
        input_key: Symbol,
        reason: String,
    ) -> Result<(), CreditOracleError> {
        subject.require_auth();

        // Validate input_key is one of the three recognised score inputs.
        let key_tx_stats = Symbol::new(&env, "tx_stats");
        let key_repayment = Symbol::new(&env, "repayment");
        let key_vc_count = Symbol::new(&env, "vc_count");
        if input_key != key_tx_stats && input_key != key_repayment && input_key != key_vc_count {
            return Err(CreditOracleError::InvalidInputKey);
        }

        let dispute_key = DataKey::Dispute(subject.clone(), input_key.clone());

        // Reject if a Pending dispute already exists for this (subject, input_key).
        if let Some(existing) = env
            .storage()
            .persistent()
            .get::<_, DisputeRecord>(&dispute_key)
        {
            if existing.status == DisputeStatus::Pending {
                return Err(CreditOracleError::DisputeAlreadyPending);
            }
        }

        let record = DisputeRecord {
            subject: subject.clone(),
            input_key: input_key.clone(),
            reason,
            filed_at_ledger: env.ledger().sequence(),
            status: DisputeStatus::Pending,
        };

        env.storage().persistent().set(&dispute_key, &record);
        env.storage()
            .persistent()
            .extend_ttl(&dispute_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);

        // Maintain a per-subject index of disputed keys for `list_disputes`.
        let index_key = DataKey::DisputeIndex(subject.clone());
        let mut disputed_keys: Vec<Symbol> = env
            .storage()
            .persistent()
            .get(&index_key)
            .unwrap_or(Vec::new(&env));

        let mut already_indexed = false;
        for i in 0..disputed_keys.len() {
            if disputed_keys.get(i).unwrap() == input_key {
                already_indexed = true;
                break;
            }
        }
        if !already_indexed {
            disputed_keys.push_back(input_key.clone());
            env.storage().persistent().set(&index_key, &disputed_keys);
            env.storage()
                .persistent()
                .extend_ttl(&index_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
        }

        env.events()
            .publish((symbol_short!("DsptFild"),), (subject, input_key));
        Ok(())
    }

    /// Resolve a pending dispute as admin.
    ///
    /// Pass `accepted = true` to mark the dispute `Resolved` — signalling to
    /// the off-chain feeder that the flagged input should be re-fetched and
    /// corrected.  Pass `accepted = false` to mark it `Rejected` (the input
    /// is deemed correct).  After resolution the subject may file a new
    /// dispute for the same key.
    ///
    /// Auth: admin only.
    pub fn resolve_dispute(
        env: Env,
        subject: Address,
        input_key: Symbol,
        accepted: bool,
    ) -> Result<(), CreditOracleError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        stored_admin.require_auth();

        let dispute_key = DataKey::Dispute(subject.clone(), input_key.clone());
        let mut record: DisputeRecord = env
            .storage()
            .persistent()
            .get(&dispute_key)
            .ok_or(CreditOracleError::DisputeNotFound)?;

        if record.status != DisputeStatus::Pending {
            return Err(CreditOracleError::DisputeNotFound);
        }

        record.status = if accepted {
            DisputeStatus::Resolved
        } else {
            DisputeStatus::Rejected
        };

        env.storage().persistent().set(&dispute_key, &record);
        env.storage()
            .persistent()
            .extend_ttl(&dispute_key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);

        if accepted {
            // DsptRslv signals feeders to re-fetch and correct the flagged input.
            env.events()
                .publish((symbol_short!("DsptRslv"),), (subject, input_key));
        } else {
            env.events()
                .publish((symbol_short!("DsptRjct"),), (subject, input_key));
        }
        Ok(())
    }

    /// Get the dispute record for a `(subject, input_key)` pair.
    ///
    /// Returns `None` if no dispute has ever been filed for this pair.
    pub fn get_dispute(env: Env, subject: Address, input_key: Symbol) -> Option<DisputeRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::Dispute(subject, input_key))
    }

    /// List all dispute records for a subject (one per input key, latest status).
    ///
    /// Returns an empty vec if the subject has never filed a dispute.
    pub fn list_disputes(env: Env, subject: Address) -> Vec<DisputeRecord> {
        let index_key = DataKey::DisputeIndex(subject.clone());
        let disputed_keys: Vec<Symbol> = env
            .storage()
            .persistent()
            .get(&index_key)
            .unwrap_or(Vec::new(&env));

        let mut records = Vec::new(&env);
        for i in 0..disputed_keys.len() {
            let key: Symbol = disputed_keys.get(i).unwrap();
            if let Some(record) = env
                .storage()
                .persistent()
                .get::<_, DisputeRecord>(&DataKey::Dispute(subject.clone(), key))
            {
                records.push_back(record);
            }
        }
        records
    }

    /// Returns all currently registered lender addresses.
    pub fn list_lenders(env: Env) -> Vec<Address> {
        let lenders: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::LendersIndex)
            .unwrap_or(Vec::new(&env));

        let mut active = Vec::new(&env);
        for i in 0..lenders.len() {
            let lender: Address = lenders.get(i).unwrap();
            if env.storage().persistent().has(&DataKey::TrustedLender(lender.clone())) {
                active.push_back(lender);
            }
        }
        active
    }
}

#[cfg(test)]
mod prop_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use identity_oracle::{IdentityOracle, IdentityOracleClient};
    use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
    use soroban_sdk::TryIntoVal;

    #[test]
    fn test_scoring_spec_example_1_new_user_scores_300() {
        let score = compute_score_pure(0, 0, 0, 0, 0, 0, 40, 30, 30);

        assert_eq!(score, 300);
    }

    #[test]
    fn test_scoring_spec_example_2_moderate_scores_569() {
        let score = compute_score_pure(
            60,
            3_000_000_000,
            0,
            8,
            10,
            3_000_000_000,
            40,
            30,
            30,
        );

        assert_eq!(score, 569);
    }

    #[test]
    fn test_scoring_spec_example_3_strong_scores_817() {
        let score = compute_score_pure(
            100,
            8_000_000_000,
            0,
            20,
            20,
            10_000_000_000,
            40,
            30,
            30,
        );

        assert_eq!(score, 817);
    }

    #[test]
    fn test_scoring_spec_perfect_scores_850() {
        let score = compute_score_pure(
            100,
            10_000_000_000,
            100,
            20,
            20,
            10_000_000_000,
            40,
            30,
            30,
        );

        assert_eq!(score, 850);
    }

    #[test]
    fn test_default_weights_sum_to_100() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let w = client.get_scoring_weights().unwrap();
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
        client.register_feeder(&admin, &feeder);
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
        client.register_lender(&admin, &lender);

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
    fn test_compute_score_repayment_rate_does_not_overflow() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        // Seed a subject with more on-time repayments than `u32::MAX / 10000`.
        // Without the u64 widening in `compute_score`, `on_time_count * 10000`
        // overflows u32 and panics (overflow-checks are enabled), permanently
        // reverting every future `compute_score` call for this subject.
        let on_time_count: u32 = 1_000_000;
        env.as_contract(&contract_id, || {
            env.storage().instance().set(&DataKey::StorageVersion, &2u32);
            env.storage().persistent().set(
                &DataKey::RepaymentRecord(subject.clone()),
                &RepaymentRecord {
                    on_time_count,
                    total_count: on_time_count,
                    total_repaid: 0,
                },
            );
        });

        let score = client.compute_score(&subject);
        assert!(score >= MIN_SCORE && score <= MAX_SCORE);

        let record = client
            .get_score(&subject)
            .expect("score should have been computed");
        assert_eq!(record.repayment_rate, 10_000);
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

        let (event_contract_id, topics, data) = events.get(0).unwrap();

        // Verify the event was emitted by this contract
        assert_eq!(event_contract_id, contract_id, "event contract id mismatch");

        // Verify the topic is Symbol("Score") — decode Val back to Symbol for comparison
        assert_eq!(topics.len(), 1, "expected 1 topic element");
        let topic_val = topics.get(0).unwrap();
        let topic_sym: Symbol = topic_val
            .try_into_val(&env)
            .expect("topic should be a Symbol");
        assert_eq!(topic_sym, symbol_short!("Score"), "expected Score topic");

        // Verify the data payload is (subject, score) — decode Val back to typed tuple
        let (event_subject, event_score): (Address, u32) = data
            .try_into_val(&env)
            .expect("data should be (Address, u32)");
        assert_eq!(event_subject, subject, "event subject mismatch");
        assert_eq!(event_score, score, "event score mismatch");
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
        client.register_feeder(&admin, &feeder);
        client.register_lender(&admin, &lender);

        // Set up identical scores except for counterparty diversity
        for i in 0u8..3 {
            let vc_id = BytesN::from_array(&env, &[i; 32]);
            client.anchor_vc(&feeder, &subject, &vc_id, &None);
        }
        client.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 3_000_000_000i128,
                tx_count_30d: 100,
                avg_counterparties: 0, // no bonus
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
                avg_counterparties: 35, // bonus applies
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
        client.register_lender(&admin, &lender);

        for _ in 0..10 {
            client.record_repayment(&lender, &subject, &1000, &true);
        }

        let score = client.compute_score(&subject);
        assert!(score > MIN_SCORE);
    }

    #[test]
    fn test_repayment_amount_affects_score() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let lender = Address::generate(&env);
        let small_subject = Address::generate(&env);
        let large_subject = Address::generate(&env);
        client.initialize(&admin);
        client.register_lender(&admin, &lender);

        client.record_repayment(&lender, &small_subject, &1, &true);
        client.record_repayment(&lender, &large_subject, &10_000_000_000, &true);

        let small_score = client.compute_score(&small_subject);
        let large_score = client.compute_score(&large_subject);

        assert!(
            large_score > small_score,
            "expected larger repayment score ({}) > tiny repayment score ({})",
            large_score,
            small_score
        );
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

        for i in 0u8..5 {
            let vc_id = BytesN::from_array(&env, &[i; 32]);
            client.anchor_vc(&feeder, &subject, &vc_id, &None);
        }
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
        // Invalid weights (sum != 100) — should return error via try_
        let result = client.try_propose_weights(&ScoringWeights {
            vc_weight: 40,
            tx_weight: 40,
            repayment_weight: 40,
        });
        assert_eq!(result, Err(Ok(CreditOracleError::InvalidWeights)));

        // Component below MIN_COMPONENT_WEIGHT (10) — should return error
        let res_vc_low = client.try_propose_weights(&ScoringWeights {
            vc_weight: 9,
            tx_weight: 45,
            repayment_weight: 46,
        });
        assert_eq!(res_vc_low, Err(Ok(CreditOracleError::InvalidWeights)));

        let res_tx_zero = client.try_propose_weights(&ScoringWeights {
            vc_weight: 55,
            tx_weight: 0,
            repayment_weight: 45,
        });
        assert_eq!(res_tx_zero, Err(Ok(CreditOracleError::InvalidWeights)));

        let res_repayment_low = client.try_propose_weights(&ScoringWeights {
            vc_weight: 45,
            tx_weight: 46,
            repayment_weight: 9,
        });
        assert_eq!(res_repayment_low, Err(Ok(CreditOracleError::InvalidWeights)));

        // Valid weights with exact MIN_COMPONENT_WEIGHT (10) — should succeed
        let res_valid_min = client.try_propose_weights(&ScoringWeights {
            vc_weight: 50,
            tx_weight: 10,
            repayment_weight: 40,
        });
        assert!(res_valid_min.is_ok());
    }

    #[test]
    fn test_propose_weights_unchanged_until_applied() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let original_weights = client.get_scoring_weights().unwrap();
        assert_eq!(original_weights.vc_weight, 40);

        client.propose_weights(&ScoringWeights {
            vc_weight: 50,
            tx_weight: 30,
            repayment_weight: 20,
        });

        let current_weights = client.get_scoring_weights();
        assert_eq!(current_weights.vc_weight, 40);
    }

    #[test]
    fn test_get_pending_weights_returns_correct_record() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let propose_ledger = env.ledger().sequence();
        client.propose_weights(&ScoringWeights {
            vc_weight: 50,
            tx_weight: 30,
            repayment_weight: 20,
        });

        let pending = client.get_pending_weights();
        assert!(pending.is_some(), "get_pending_weights should return Some after proposal");
        let record = pending.unwrap();
        assert_eq!(record.weights.vc_weight, 50);
        assert_eq!(record.weights.tx_weight, 30);
        assert_eq!(record.weights.repayment_weight, 20);
        assert_eq!(record.effective_ledger, propose_ledger + TIMELOCK_LEDGERS);
    }

    #[test]
    fn test_apply_weights_before_timelock_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);
        client.propose_weights(&ScoringWeights {
            vc_weight: 50,
            tx_weight: 30,
            repayment_weight: 20,
        });
        let result = client.try_apply_weights();
        assert_eq!(result, Err(Ok(CreditOracleError::TimelockNotExpired)));
    }

    #[test]
    fn test_apply_weights_with_no_pending_weights_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);
        let result = client.try_apply_weights();
        assert_eq!(result, Err(Ok(CreditOracleError::NoPendingWeights)));
    }

    #[test]
    fn test_apply_weights_after_timelock_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);
        client.propose_weights(&ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        });

        // Extend instance TTL before jumping the ledger so it isn't archived.
        let jump = TIMELOCK_LEDGERS + 2;
        env.as_contract(&contract_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + jump);
        client.apply_weights();

        let w = client.get_scoring_weights().unwrap();
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
        client.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 5000,
                tx_count_30d: 10,
                avg_counterparties: 3,
            },
        );
        client.deregister_feeder(&admin, &feeder);
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
    fn test_compute_score_not_initialized() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);

        // Only set admin, not config (simulating a botched upgrade)
        env.as_contract(&contract_id, || {
            env.storage().instance().set(&DataKey::Admin, &admin);
        });

        // compute_score should return NotInitialized error
        let result = client.try_compute_score(&subject);
        assert_eq!(result, Err(Ok(CreditOracleError::NotInitialized)));
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
    fn test_upgrade_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        client.initialize(&admin);
        let result = client.try_upgrade(&non_admin, &BytesN::from_array(&env, &[0u8; 32]));
        assert_eq!(result, Err(Ok(CreditOracleError::NotAuthorized)));
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

        // Proposing tx_weight = 0 to contract is rejected as InvalidWeights
        let res_zero = client.try_propose_weights(&ScoringWeights {
            vc_weight: 60,
            tx_weight: 0,
            repayment_weight: 40,
        });
        assert_eq!(res_zero, Err(Ok(CreditOracleError::InvalidWeights)));

        // Pure calculation check: when tx_weight = 0, counterparty bonus has no effect
        let score_with = compute_score_pure(0, 0, 100, 0, 0, 0, 60, 0, 40);
        let score_without = compute_score_pure(0, 0, 0, 0, 0, 0, 60, 0, 40);
        assert_eq!(
            score_with, score_without,
            "counterparty bonus should have no effect when tx_weight is 0"
        );
    }

    /// When tx_weight is high (e.g., 80), the counterparty bonus is applied and
    /// a subject with 100+ counterparties scores higher than one with none.
    #[test]
    fn test_counterparty_bonus_applied_when_tx_weight_is_high() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        client.initialize(&admin);

        // Propose and apply valid weights with tx_weight = 80
        client.propose_weights(&ScoringWeights {
            vc_weight: 10,
            tx_weight: 80,
            repayment_weight: 10,
        });
        let jump = TIMELOCK_LEDGERS + 2;
        env.as_contract(&contract_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + jump);
        client.apply_weights();

        client.register_feeder(&admin, &feeder);

        let subject_with = Address::generate(&env);
        let subject_without = Address::generate(&env);

        // Same volume, but subject_with has 100 counterparties (max bonus = 20 pts)
        client.update_tx_stats(
            &feeder,
            &subject_with,
            &TxStats {
                volume_30d: 0,
                tx_count_30d: 0,
                avg_counterparties: 100,
            },
        );
        client.update_tx_stats(
            &feeder,
            &subject_without,
            &TxStats {
                volume_30d: 0,
                tx_count_30d: 0,
                avg_counterparties: 0,
            },
        );

        let score_with = client.compute_score(&subject_with);
        let score_without = client.compute_score(&subject_without);

        assert!(
            score_with > score_without,
            "subject with 100 counterparties should score higher when tx_weight=100"
        );
    }

    #[test]
    fn test_get_identity_oracle_returns_none_when_not_configured() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Before configuration, get_identity_oracle returns None
        assert!(client.get_identity_oracle().is_none());
    }

    #[test]
    fn test_set_and_get_identity_oracle() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let identity_oracle_id = env.register_contract(None, identity_oracle::IdentityOracle);

        let admin = Address::generate(&env);

        client.initialize(&admin);

        // Set the identity oracle
        client.set_identity_oracle(&admin, &identity_oracle_id);

        // Verify get_identity_oracle returns the configured address
        let result = client.get_identity_oracle();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), identity_oracle_id);
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
    #[should_panic(expected = "Error(Contract, #2)")]
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

        // Advance ledger to bypass cooldown
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        env.ledger().set_timestamp(env.ledger().timestamp() + 100);

        // Second computation with identical inputs — write is skipped
        client.compute_score(&subject);
        let record2 = client.get_score(&subject).unwrap();

        // Timestamp shouldn't change because write was skipped
        assert_eq!(record1.last_updated, record2.last_updated);

        // Change an input (anchor a VC)
        let feeder = Address::generate(&env);
        client.register_feeder(&admin, &feeder);
        let vc_id = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&feeder, &subject, &vc_id, &None);

        // Advance ledger again
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        env.ledger().set_timestamp(env.ledger().timestamp() + 100);

        // Third computation with changed input
        client.compute_score(&subject);
        let record3 = client.get_score(&subject).unwrap();

        // Write occurred, so timestamp is updated
        assert!(record3.last_updated > record2.last_updated);
        assert_eq!(record3.vc_count, 1);
    }

    #[test]
    fn test_protocol_stats_default_zero() {
        let env = Env::default();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_subjects_scored, 0);
        assert_eq!(stats.total_repayments_recorded, 0);
    }

    #[test]
    fn test_protocol_stats_increments_on_record_repayment() {
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
        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_repayments_recorded, 1);

        client.record_repayment(&lender, &subject, &1000, &false);
        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_repayments_recorded, 2);
    }

    #[test]
    fn test_protocol_stats_increments_subjects_scored() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let subject1 = Address::generate(&env);
        let subject2 = Address::generate(&env);

        client.compute_score(&subject1);
        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_subjects_scored, 1);

        client.compute_score(&subject2);
        let stats = client.get_protocol_stats();
        assert_eq!(stats.total_subjects_scored, 2);
    }

    #[test]
    fn test_protocol_stats_no_double_count_recompute() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let subject = Address::generate(&env);

        // First computation — subjects_scored should increment
        client.compute_score(&subject);
        let stats1 = client.get_protocol_stats();
        assert_eq!(stats1.total_subjects_scored, 1);

        // Advance ledger to bypass cooldown before recomputing
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);

        // Second computation with identical inputs — should NOT double count
        client.compute_score(&subject);
        let stats2 = client.get_protocol_stats();
        assert_eq!(stats2.total_subjects_scored, 1);
    }

    // ── Dispute mechanism tests ───────────────────────────────────────────

    #[test]
    fn test_flag_score_input_stores_pending_dispute() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "tx_stats");
        let reason = soroban_sdk::String::from_str(&env, "Volume looks inflated");
        client.flag_score_input(&subject, &input_key, &reason);

        let record = client.get_dispute(&subject, &input_key).unwrap();
        assert_eq!(record.subject, subject);
        assert_eq!(record.input_key, input_key);
        assert_eq!(record.status, DisputeStatus::Pending);
        assert_eq!(record.filed_at_ledger, env.ledger().sequence());
    }

    #[test]
    fn test_flag_score_input_rejects_invalid_key() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let bad_key = soroban_sdk::Symbol::new(&env, "bad_input");
        let reason = soroban_sdk::String::from_str(&env, "test");
        let result = client.try_flag_score_input(&subject, &bad_key, &reason);
        assert_eq!(result, Err(Ok(CreditOracleError::InvalidInputKey)));
    }

    #[test]
    fn test_flag_score_input_rejects_duplicate_pending() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "repayment");
        let reason = soroban_sdk::String::from_str(&env, "Repayment not recorded");
        client.flag_score_input(&subject, &input_key, &reason);

        // Second filing with pending status should fail
        let result = client.try_flag_score_input(&subject, &input_key, &reason);
        assert_eq!(result, Err(Ok(CreditOracleError::DisputeAlreadyPending)));
    }

    #[test]
    fn test_resolve_dispute_accepted() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "vc_count");
        let reason = soroban_sdk::String::from_str(&env, "VC count is wrong");
        client.flag_score_input(&subject, &input_key, &reason);

        client.resolve_dispute(&subject, &input_key, &true);

        let record = client.get_dispute(&subject, &input_key).unwrap();
        assert_eq!(record.status, DisputeStatus::Resolved);
    }

    #[test]
    fn test_resolve_dispute_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "tx_stats");
        let reason = soroban_sdk::String::from_str(&env, "Tx volume wrong");
        client.flag_score_input(&subject, &input_key, &reason);

        client.resolve_dispute(&subject, &input_key, &false);

        let record = client.get_dispute(&subject, &input_key).unwrap();
        assert_eq!(record.status, DisputeStatus::Rejected);
    }

    #[test]
    fn test_resolve_nonexistent_dispute_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "repayment");
        let result = client.try_resolve_dispute(&subject, &input_key, &true);
        assert_eq!(result, Err(Ok(CreditOracleError::DisputeNotFound)));
    }

    #[test]
    fn test_resolve_already_resolved_dispute_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "repayment");
        let reason = soroban_sdk::String::from_str(&env, "test");
        client.flag_score_input(&subject, &input_key, &reason);
        client.resolve_dispute(&subject, &input_key, &true);

        // Resolving again should fail
        let result = client.try_resolve_dispute(&subject, &input_key, &true);
        assert_eq!(result, Err(Ok(CreditOracleError::DisputeNotFound)));
    }

    #[test]
    fn test_new_dispute_allowed_after_resolution() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "tx_stats");
        let reason = soroban_sdk::String::from_str(&env, "First dispute");
        client.flag_score_input(&subject, &input_key, &reason);
        client.resolve_dispute(&subject, &input_key, &false); // rejected

        // After rejection, subject can re-file for the same key
        let reason2 = soroban_sdk::String::from_str(&env, "Still wrong after review");
        client.flag_score_input(&subject, &input_key, &reason2);
        let record = client.get_dispute(&subject, &input_key).unwrap();
        assert_eq!(record.status, DisputeStatus::Pending);
    }

    #[test]
    fn test_list_disputes_returns_all_filed() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let r = soroban_sdk::String::from_str(&env, "reason");
        client.flag_score_input(&subject, &soroban_sdk::Symbol::new(&env, "tx_stats"), &r);
        client.flag_score_input(&subject, &soroban_sdk::Symbol::new(&env, "repayment"), &r);

        let disputes = client.list_disputes(&subject);
        assert_eq!(disputes.len(), 2);
    }

    #[test]
    fn test_list_disputes_empty_for_new_subject() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let disputes = client.list_disputes(&subject);
        assert_eq!(disputes.len(), 0);
    }

    #[test]
    fn test_get_dispute_returns_none_when_no_dispute() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "vc_count");
        assert!(client.get_dispute(&subject, &input_key).is_none());
    }

    #[test]
    fn test_flag_score_input_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "tx_stats");
        let reason = soroban_sdk::String::from_str(&env, "inflated volume");
        client.flag_score_input(&subject, &input_key, &reason);

        let events = env.events().all();
        let mut dispute_count = 0u32;
        for i in 0..events.len() {
            let (id, topics, _) = events.get(i).unwrap();
            if id == contract_id && topics.len() == 1 {
                let topic: soroban_sdk::Symbol = topics
                    .get(0)
                    .unwrap()
                    .try_into_val(&env)
                    .unwrap();
                if topic == symbol_short!("DsptFild") {
                    dispute_count += 1;
                }
            }
        }
        assert_eq!(dispute_count, 1, "expected 1 DsptFild event");
    }

    #[test]
    fn test_vc_type_weight_scoring() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let user = Address::generate(&env);

        client.initialize(&admin);
        client.register_feeder(&admin, &feeder);

        // Register type weights
        let premium = Symbol::new(&env, "premium");
        let basic = Symbol::new(&env, "basic");
        client.set_vc_type_weight(&admin, &premium, &200);
        client.set_vc_type_weight(&admin, &basic, &50);

        // Expected VC points per credential:
        //   default_weight * 20 / 100 = 100 * 20 / 100 = 20
        //   premium * 20 / 100        = 200 * 20 / 100 = 40
        //   basic * 20 / 100          =  50 * 20 / 100 = 10
        //   unregistered type         = 100 * 20 / 100 = 20
        //   Total = 90, capped at 100
        let vc1 = BytesN::from_array(&env, &[0u8; 32]);
        client.anchor_vc(&feeder, &user, &vc1, &None);

        let vc2 = BytesN::from_array(&env, &[1u8; 32]);
        client.anchor_vc(&feeder, &user, &vc2, &Some(premium));

        let vc3 = BytesN::from_array(&env, &[2u8; 32]);
        client.anchor_vc(&feeder, &user, &vc3, &Some(basic));

        let unreg = Symbol::new(&env, "unknown");
        let vc4 = BytesN::from_array(&env, &[3u8; 32]);
        client.anchor_vc(&feeder, &user, &vc4, &Some(unreg));

        let score = client.compute_score(&user);
        let expected = compute_score_pure(90, 0, 0, 0, 0, 0, 40, 30, 30);
        assert_eq!(score, expected);

        let record = client.get_score(&user).unwrap();
        assert_eq!(record.vc_count, 4);
    }

    #[test]
    fn test_set_vc_count_emits_deprecation_event_when_oracle_configured() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let identity_oracle_id = env.register_contract(None, identity_oracle::IdentityOracle);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let subject = Address::generate(&env);

        client.initialize(&admin);
        client.register_feeder(&admin, &feeder);
        
        // Configure identity oracle
        client.set_identity_oracle(&admin, &identity_oracle_id);

        // Clear any existing events from setup
        env.events().all().len(); // Just to consume them
        
        // Call set_vc_count - should emit deprecation warning
        client.set_vc_count(&feeder, &subject, &5);

        // Check for VcCntDep event
        let events = env.events().all();
        let mut found_deprecation_event = false;
        for i in 0..events.len() {
            let (event_contract_id, topics, data) = events.get(i).unwrap();
            if event_contract_id == contract_id && topics.len() == 1 {
                let topic: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
                if topic == symbol_short!("VcCntDep") {
                    let (event_subject, event_count): (Address, u32) =
                        data.try_into_val(&env).unwrap();
                    assert_eq!(event_subject, subject);
                    assert_eq!(event_count, 5);
                    found_deprecation_event = true;
                    break;
                }
            }
        }
        assert!(
            found_deprecation_event,
            "VcCntDep event should be emitted when identity oracle is configured"
        );
    }

    // -----------------------------------------------------------------
    // Issue #530: VC recency decay
    // -----------------------------------------------------------------

    #[test]
    fn test_recency_multiplier_full_weight_within_first_day() {
        // age_days uses integer division, so a VC keeps full weight for 24h.
        assert_eq!(recency_multiplier_bps(0, 0, 5, 5_000), 10_000);
        assert_eq!(
            recency_multiplier_bps(0, SECONDS_PER_DAY - 1, 5, 5_000),
            10_000
        );
        // Exactly one day old: the first decay step applies.
        assert_eq!(recency_multiplier_bps(0, SECONDS_PER_DAY, 5, 5_000), 9_995);
    }

    #[test]
    fn test_recency_multiplier_decays_linearly_per_day() {
        // 100 days x 5 bps = 500 bps lost.
        assert_eq!(
            recency_multiplier_bps(0, 100 * SECONDS_PER_DAY, 5, 5_000),
            9_500
        );
        // 365 days x 5 bps = 1_825 bps lost.
        assert_eq!(
            recency_multiplier_bps(0, 365 * SECONDS_PER_DAY, 5, 5_000),
            8_175
        );
    }

    #[test]
    fn test_recency_multiplier_clamps_to_floor() {
        // 5_000 days x 5 bps = 25_000 bps, far past zero: the floor holds.
        let now = 5_000 * SECONDS_PER_DAY;
        assert_eq!(recency_multiplier_bps(0, now, 5, 5_000), 5_000);
        // A zero floor decays all the way down without underflowing.
        assert_eq!(recency_multiplier_bps(0, now, 5, 0), 0);
    }

    #[test]
    fn test_recency_multiplier_saturates_on_extreme_inputs() {
        // Anchored "in the future" (clock skew): age saturates to 0, not underflow.
        assert_eq!(
            recency_multiplier_bps(1_000 * SECONDS_PER_DAY, 0, 5, 5_000),
            10_000
        );
        // Absurd age and decay rate saturate to the floor instead of panicking.
        assert_eq!(recency_multiplier_bps(0, u64::MAX, u32::MAX, 5_000), 5_000);
        // A floor above 10_000 is clamped: decay can never *raise* a VC's value.
        assert_eq!(recency_multiplier_bps(0, 0, 5, u32::MAX), 10_000);
    }

    #[test]
    fn test_apply_recency_bps_scales_points() {
        assert_eq!(apply_recency_bps(20, 10_000), 20);
        assert_eq!(apply_recency_bps(20, 5_000), 10);
        assert_eq!(apply_recency_bps(20, 9_500), 19);
        assert_eq!(apply_recency_bps(20, 0), 0);
        // Widening to u64 keeps the largest input from overflowing.
        assert_eq!(apply_recency_bps(u32::MAX, 10_000), u32::MAX);
    }

    #[test]
    fn test_get_recency_decay_defaults_to_disabled() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let cfg = client.get_recency_decay();
        assert!(!cfg.enabled, "decay must be opt-in");
        assert_eq!(cfg.decay_bps_per_day, DEFAULT_DECAY_BPS_PER_DAY);
        assert_eq!(cfg.min_recency_bps, DEFAULT_MIN_RECENCY_BPS);
    }

    #[test]
    fn test_set_recency_decay_stores_config() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        client.set_recency_decay(&admin, &true, &10, &2_500);

        let cfg = client.get_recency_decay();
        assert!(cfg.enabled);
        assert_eq!(cfg.decay_bps_per_day, 10);
        assert_eq!(cfg.min_recency_bps, 2_500);
    }

    #[test]
    fn test_set_recency_decay_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        client.initialize(&admin);

        let result = client.try_set_recency_decay(&non_admin, &true, &5, &5_000);
        assert_eq!(result, Err(Ok(CreditOracleError::NotAuthorized)));
    }

    #[test]
    fn test_set_recency_decay_rejects_floor_above_full_weight() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin);

        let result = client.try_set_recency_decay(&admin, &true, &5, &10_001);
        assert_eq!(result, Err(Ok(CreditOracleError::InvalidRecencyConfig)));

        // The rejected call must not have mutated stored config.
        assert!(!client.get_recency_decay().enabled);
    }

    #[test]
    fn test_recency_decay_ignored_without_identity_oracle() {
        // The local VcList path has no `anchored_at`, so enabling decay must
        // leave the score untouched (backward compatibility).
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, CreditOracle);
        let client = CreditOracleClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let user = Address::generate(&env);
        client.initialize(&admin);
        client.register_feeder(&admin, &feeder);

        env.ledger().set_timestamp(1_700_000_000);
        for i in 0..3u8 {
            client.anchor_vc(&feeder, &user, &BytesN::from_array(&env, &[i; 32]), &None);
        }

        let before = client.compute_score(&user);

        client.set_recency_decay(&admin, &true, &5, &5_000);
        // Age the ledger by 5 years and recompute.
        env.ledger().set_timestamp(1_700_000_000 + 1_825 * 86_400);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let after = client.compute_score(&user);

        assert_eq!(
            before, after,
            "decay must be a no-op when anchored_at is unavailable"
        );
    }

    proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        /// Issue #530 acceptance criterion: monotonically younger credentials
        /// produce a higher or equal score under the same decay configuration.
        #[test]
        fn proptest_younger_credentials_score_at_least_as_high(
            younger_age_days in 0u64..3_650,
            extra_age_days in 0u64..3_650,
            decay_bps_per_day in 0u32..100,
            min_recency_bps in 0u32..=10_000,
            vc_count in 1u32..=5,
            type_weight in 0u32..=300,
        ) {
            let now = 10_000u64 * SECONDS_PER_DAY;
            let younger_anchor = now - younger_age_days * SECONDS_PER_DAY;
            let older_anchor = now - (younger_age_days + extra_age_days) * SECONDS_PER_DAY;
            prop_assert!(older_anchor <= younger_anchor);

            // Mirrors the per-VC arithmetic in `compute_score`.
            let base_points = 20u32.saturating_mul(type_weight) / 100;
            let vc_points_at = |anchored_at: u64| -> u32 {
                let bps = recency_multiplier_bps(
                    anchored_at,
                    now,
                    decay_bps_per_day,
                    min_recency_bps,
                );
                apply_recency_bps(base_points, bps)
                    .saturating_mul(vc_count)
                    .min(100)
            };
            let score_at = |anchored_at: u64| -> u32 {
                compute_score_pure(
                    vc_points_at(anchored_at),
                    3_000_000_000,
                    7,
                    8,
                    10,
                    3_000_000_000,
                    40,
                    30,
                    30,
                )
            };

            // The multiplier is monotonic in `anchored_at` ...
            prop_assert!(
                recency_multiplier_bps(younger_anchor, now, decay_bps_per_day, min_recency_bps)
                    >= recency_multiplier_bps(older_anchor, now, decay_bps_per_day, min_recency_bps)
            );
            // ... and so is the resulting score.
            prop_assert!(score_at(younger_anchor) >= score_at(older_anchor));
        }
    }
}
