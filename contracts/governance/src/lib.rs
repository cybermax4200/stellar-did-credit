#![no_std]
//! Governance contract for the Stellar DID Credit protocol.
//!
//! Provides on-chain proposal creation, voting, and execution that can
//! update the credit-oracle's scoring weights through a community vote.
use credit_oracle_types::{CreditOracleClient, CreditOracleError, ScoringWeights};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, String,
    Symbol, Vec,
};

/// Error types for the governance contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum GovernanceError {
    /// Contract is already initialized.
    AlreadyInitialized = 1,
    /// Caller is not authorized to perform this action.
    NotAuthorized = 2,
    /// Proposal with the given ID does not exist.
    ProposalNotFound = 3,
    /// Proposal voting period has already expired.
    ProposalExpired = 4,
    /// Proposal voting period has not yet expired; cannot execute.
    ProposalNotExpired = 5,
    /// Proposal has already been executed.
    ProposalAlreadyExecuted = 6,
    /// Proposed scoring weights do not sum to 100 or a component is below MIN_COMPONENT_WEIGHT (10).
    InvalidWeights = 7,
    /// Quorum value must be positive.
    InvalidQuorum = 8,
    /// Vote weight must be positive.
    InvalidVoteWeight = 9,
    /// Total votes cast did not meet the required quorum.
    QuorumNotMet = 10,
    /// Execution timelock has not yet expired after the voting period.
    TimelockNotExpired = 11,
    /// Voter is not registered or has no voting weight.
    VoterNotRegistered = 12,
    /// Vote weight exceeds voter's available balance.
    InsufficientVoteWeight = 13,
    /// Proposal has already been cancelled and cannot be executed or cancelled again.
    ProposalAlreadyCancelled = 14,
    /// The credit-oracle has no pending weights to apply.
    NoPendingWeights = 15,
    /// The credit-oracle is paused and cannot apply weights.
    ContractPaused = 16,
    /// The vote would overflow the proposal's tally.
    VoteTallyOverflow = 17,
    /// Proposal did not receive more for-votes than against-votes.
    ProposalRejected = 18,
    /// Voting period cannot be zero.
    InvalidVotingPeriod = 19,
    /// The credit-oracle already has weights pending under an active timelock.
    TimelockAlreadyPending = 20,
    /// A voter batch exceeds the maximum number of entries.
    VoterBatchTooLarge = 21,
    /// Proposal metadata CID is empty or exceeds the maximum length (128 bytes).
    InvalidMetadataCid = 22,
}

/// Maximum allowed length in bytes for a proposal metadata CID/URI.
pub const MAX_METADATA_CID_LEN: u32 = 128;

/// Maximum number of entries accepted by either voter batch operation.
pub const MAX_VOTER_BATCH: u32 = 50;

/// A voter and the voting weight to assign.
#[contracttype]
#[derive(Clone)]
pub struct VoterWeightEntry {
    pub voter: Address,
    pub weight: i128,
}

/// Storage keys for the governance contract.
#[contracttype]
pub enum DataKey {
    /// Contract administrator address.
    Admin,
    /// Optional emergency guardian authorized to veto proposals.
    Guardian,
    /// Address of the credit-oracle contract this governance controls.
    CreditOracle,
    /// Monotonically increasing counter used to assign proposal IDs.
    /// IDs start at 1; ID 0 is intentionally unused.
    NextProposalId,
    /// Default quorum (minimum total votes) required for proposal execution.
    QuorumRequired,
    /// Optional percentage of total registered voting weight required for new proposals.
    QuorumPercent,
    /// Proposal data stored by proposal ID.
    Proposal(u64),
    /// Original proposer address for a given proposal ID.
    Proposer(u64),
    /// Human-readable reason recorded when the guardian vetoes a proposal.
    VetoReason(u64),
    /// Registered voting weight for an address.
    VoterWeight(Address),
    /// Amount of weight already used by voter in a specific proposal.
    VoteWeightUsed(u64, Address),
    TotalRegisteredWeight,
}

/// Instance-storage TTL bump constants.
///
/// Instance entries are bumped to ~30 days on initialize so the
/// critical configuration (`Admin`, `CreditOracle`,
/// `QuorumRequired`, `NextProposalId`) does not expire on an idle
/// governance contract.
///
/// Threshold (5,000 ledgers) and extend amount (500,000 ledgers)
/// match `identity-oracle`'s `INSTANCE_BUMP_*` constants â€” borrowed
/// pattern from the security TTL-survival fix (PR #456) so storage
/// survives the same default Soroban instance TTL even when no
/// proposals are being created/voted on.
const INSTANCE_BUMP_THRESHOLD: u32 = 5_000;
const INSTANCE_BUMP_AMOUNT: u32 = 500_000;

const FIRST_PROPOSAL_ID: u64 = 1;

// Persistent voter entries must survive long voting periods.
const PERS_TTL_THRESHOLD: u32 = 120_960;
const PERS_TTL_EXTEND: u32 = 518_400;

#[contracttype]
#[derive(Clone)]
/// An on-chain governance proposal for updating credit-oracle scoring weights.
pub struct GovernanceProposal {
    /// Unique proposal identifier, assigned at creation.
    pub id: u64,
    /// Address of the account that created this proposal.
    pub proposer: Address,
    /// Scoring weights to apply to the credit-oracle if the proposal passes.
    pub proposed_weights: ScoringWeights,
    /// Accumulated weight of votes cast in favor.
    pub votes_for: i128,
    /// Accumulated weight of votes cast against.
    pub votes_against: i128,
    /// Ledger sequence number after which voting ends.
    pub expiry_ledger: u32,
    /// Number of ledgers after `expiry_ledger` that must pass before `execute`
    /// may be called. This gives the community a reaction window between a vote
    /// passing and its effects taking hold.
    pub execution_delay_ledgers: u32,
    /// Whether this proposal has been executed (weights applied or vote failed).
    pub executed: bool,
    /// Whether this proposal has been cancelled. Cancelled proposals cannot be
    /// executed. Only the original proposer or the contract admin may cancel.
    pub cancelled: bool,
    /// Minimum `votes_for + votes_against` required for `execute` to apply
    /// this proposal's weights, snapshotted from the contract-wide default
    /// at proposal-creation time so later `set_quorum` calls never change
    /// the rules for a proposal already up for a vote.
    pub quorum_required: i128,
    /// IPFS CID or URI explaining the rationale / discussion behind the proposal.
    pub metadata_cid: String,
}

/// On-chain governance contract.
#[contract]
pub struct Governance;

// Applies the same storage, TTL and event behavior as the single-voter calls.
fn set_voter_weight(env: &Env, voter: Address, weight: i128, registration: bool) {
    let key = DataKey::VoterWeight(voter.clone());
    let old_weight: i128 = env.storage().persistent().get(&key).unwrap_or(0);
    if weight == 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &weight);
        env.storage()
            .persistent()
            .extend_ttl(&key, PERS_TTL_THRESHOLD, PERS_TTL_EXTEND);
    }
    let total: i128 = env
        .storage()
        .instance()
        .get(&DataKey::TotalRegisteredWeight)
        .unwrap_or(0);
    env.storage().instance().set(
        &DataKey::TotalRegisteredWeight,
        &(total - old_weight + weight),
    );
    let topic = if registration {
        symbol_short!("VoterReg")
    } else {
        symbol_short!("VoterUpd")
    };
    env.events().publish((topic, voter), weight);
}

#[contractimpl]
impl Governance {
    /// Initialize the governance contract.
    ///
    /// Sets the administrator, credit-oracle address, and default quorum required
    /// for proposals. `quorum_required` must be greater than zero.
    ///
    /// Emits an `Initialized` event with the admin and the credit-oracle
    /// target address â€” indexers use this to detect deployments before the
    /// first admin action. The event is documented in
    /// `docs/event-indexing.md`.
    ///
    /// **Note on payload scope:** Issue #302 originally listed both
    /// `credit-oracle` and `identity-oracle` as "target contracts" to include.
    /// Identity-oracle is **not** currently stored by governance (its wiring
    /// is a follow-up to issue #39), so only `credit_oracle` is emitted in the
    /// event payload today. Once governance learns about identity-oracle (via
    /// a dedicated setter, mirroring `set_quorum`), the payload should be
    /// extended to include it â€” see `docs/event-indexing.md` for the
    /// documented schema.
    ///
    /// Auth: `admin` must sign the transaction.
    pub fn initialize(
        env: Env,
        admin: Address,
        credit_oracle: Address,
        quorum_required: i128,
    ) -> Result<(), GovernanceError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(GovernanceError::AlreadyInitialized);
        }
        if quorum_required <= 0 {
            return Err(GovernanceError::InvalidQuorum);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::CreditOracle, &credit_oracle);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &FIRST_PROPOSAL_ID);
        env.storage()
            .instance()
            .set(&DataKey::QuorumRequired, &quorum_required);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        // Issue #665: emit an Initialized event so off-chain indexers can
        // observe governance deployment before the first admin action.
        // Data tuple includes the admin and the credit-oracle target so
        // indexers can record the contract's wiring.
        env.events()
            .publish((Symbol::new(&env, "Initialized"),), (admin, credit_oracle));

        Ok(())
    }

    /// Updates the contract-wide default quorum applied to proposals created
    /// from this point on. Admin only. Does not affect proposals that
    /// already exist â€” each snapshots its own `quorum_required` at creation.
    pub fn set_quorum(
        env: Env,
        admin: Address,
        quorum_required: i128,
    ) -> Result<(), GovernanceError> {
        if quorum_required <= 0 {
            return Err(GovernanceError::InvalidQuorum);
        }
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(GovernanceError::NotAuthorized);
        }
        admin.require_auth();

        let total_weight: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalRegisteredWeight)
            .unwrap_or(0);
        if quorum_required > total_weight {
            return Err(GovernanceError::InvalidQuorum);
        }

        env.storage()
            .instance()
            .set(&DataKey::QuorumRequired, &quorum_required);
        env.storage().instance().remove(&DataKey::QuorumPercent);
        Ok(())
    }

    /// Configure the quorum for future proposals as a percentage of currently
    /// registered voting weight. The computed quorum is snapshotted at proposal creation.
    pub fn set_quorum_percent(
        env: Env,
        admin: Address,
        percent: u32,
    ) -> Result<(), GovernanceError> {
        if percent == 0 || percent > 100 {
            return Err(GovernanceError::InvalidQuorum);
        }
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(GovernanceError::NotAuthorized);
        }
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::QuorumPercent, &percent);
        Ok(())
    }

    /// Designate the emergency guardian address.
    ///
    /// The guardian is the only address allowed to call [`Governance::emergency_veto`]
    /// to cancel a proposal that has passed voting but is still inside its
    /// execution-delay timelock. Replaces any previously configured guardian.
    ///
    /// Auth: `admin` must equal the stored admin and sign the transaction.
    pub fn set_guardian(
        env: Env,
        admin: Address,
        guardian: Address,
    ) -> Result<(), GovernanceError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(GovernanceError::NotAuthorized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Guardian, &guardian);
        Ok(())
    }

    /// Return the currently configured emergency guardian, if any.
    pub fn get_guardian(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Guardian)
    }

    /// Return the percentage-based quorum configuration, if enabled.
    pub fn get_quorum_percent(env: Env) -> Option<u32> {
        env.storage().instance().get(&DataKey::QuorumPercent)
    }

    /// Returns the contract-wide default quorum applied to newly created proposals.
    pub fn get_quorum(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::QuorumRequired)
            .unwrap_or(0)
    }

    /// Returns the total registered voting weight across all voters.
    pub fn get_total_registered_weight(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TotalRegisteredWeight)
            .unwrap_or(0)
    }

    /// Create a new governance proposal to update the credit-oracle's scoring weights.
    ///
    /// `weights` must sum to 100. The voting period runs for `voting_period_ledgers`
    /// ledgers from the current sequence. After voting ends, execution is further
    /// delayed by `execution_delay_ledgers` ledgers to give the community a reaction
    /// window. Returns the new proposal ID. The first proposal has ID 1 and each
    /// subsequent proposal increments the ID by 1.
    ///
    /// Auth: `proposer` must sign the transaction.
    pub fn create_proposal(
        env: Env,
        proposer: Address,
        weights: ScoringWeights,
        voting_period_ledgers: u32,
        execution_delay_ledgers: u32,
        metadata_cid: String,
    ) -> Result<u64, GovernanceError> {
        proposer.require_auth();
        if !weights.is_valid() {
            return Err(GovernanceError::InvalidWeights);
        }
        if voting_period_ledgers == 0 {
            return Err(GovernanceError::InvalidVotingPeriod);
        }
        if metadata_cid.is_empty() || metadata_cid.len() > MAX_METADATA_CID_LEN {
            return Err(GovernanceError::InvalidMetadataCid);
        }

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(FIRST_PROPOSAL_ID);
        let expiry_ledger = env.ledger().sequence() + voting_period_ledgers;
        let quorum_required = match env
            .storage()
            .instance()
            .get::<_, u32>(&DataKey::QuorumPercent)
        {
            Some(percent) => {
                let total: i128 = env
                    .storage()
                    .instance()
                    .get(&DataKey::TotalRegisteredWeight)
                    .unwrap_or(0);
                total
                    .checked_mul(i128::from(percent))
                    .ok_or(GovernanceError::InvalidQuorum)?
                    / 100
            }
            None => env
                .storage()
                .instance()
                .get(&DataKey::QuorumRequired)
                .unwrap_or(0),
        };

        let proposal = GovernanceProposal {
            id,
            proposer: proposer.clone(),
            proposed_weights: weights,
            votes_for: 0,
            votes_against: 0,
            expiry_ledger,
            execution_delay_ledgers,
            executed: false,
            cancelled: false,
            quorum_required,
            metadata_cid: metadata_cid.clone(),
        };

        env.storage()
            .persistent()
            .set(&DataKey::Proposal(id), &proposal);
        env.storage()
            .persistent()
            .set(&DataKey::Proposer(id), &proposer);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &(id + 1));

        env.events()
            .publish((symbol_short!("PropCreat"), id), (proposer, expiry_ledger, metadata_cid));

        Ok(id)
    }

    /// Cast a vote on an open proposal.
    ///
    /// `vote_weight` must be positive and cannot exceed the voter's available
    /// weight for this proposal. Each voter can cast multiple votes up to their
    /// total registered weight. Returns an error if the proposal has expired,
    /// been executed, been cancelled, or if the voter lacks sufficient weight.
    ///
    /// Auth: `voter` must sign the transaction.
    pub fn vote(
        env: Env,
        voter: Address,
        proposal_id: u64,
        vote_for: bool,
        vote_weight: i128,
    ) -> Result<(), GovernanceError> {
        voter.require_auth();

        if vote_weight <= 0 {
            return Err(GovernanceError::InvalidVoteWeight);
        }

        // Verify voter is registered and has sufficient weight
        let total_weight: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::VoterWeight(voter.clone()))
            .ok_or(GovernanceError::VoterNotRegistered)?;
        env.storage().persistent().extend_ttl(
            &DataKey::VoterWeight(voter.clone()),
            PERS_TTL_THRESHOLD,
            PERS_TTL_EXTEND,
        );

        // Check how much weight this voter has already used for this proposal
        let used_weight: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::VoteWeightUsed(proposal_id, voter.clone()))
            .unwrap_or(0);
        if used_weight > 0 {
            env.storage().persistent().extend_ttl(
                &DataKey::VoteWeightUsed(proposal_id, voter.clone()),
                PERS_TTL_THRESHOLD,
                PERS_TTL_EXTEND,
            );
        }

        let available_weight = total_weight - used_weight;
        if vote_weight > available_weight {
            return Err(GovernanceError::InsufficientVoteWeight);
        }

        let proposal_key = DataKey::Proposal(proposal_id);
        let mut proposal: GovernanceProposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .ok_or(GovernanceError::ProposalNotFound)?;

        if env.ledger().sequence() > proposal.expiry_ledger {
            return Err(GovernanceError::ProposalExpired);
        }

        if proposal.executed {
            return Err(GovernanceError::ProposalAlreadyExecuted);
        }

        // A cancelled proposal is dead: no further votes may be cast, even if
        // the voting period has not yet expired.
        if proposal.cancelled {
            return Err(GovernanceError::ProposalAlreadyCancelled);
        }

        // Reject votes that would overflow a proposal tally instead of
        // silently saturating and potentially bypassing quorum.
        if vote_for {
            proposal.votes_for = proposal
                .votes_for
                .checked_add(vote_weight)
                .ok_or(GovernanceError::VoteTallyOverflow)?;
        } else {
            proposal.votes_against = proposal
                .votes_against
                .checked_add(vote_weight)
                .ok_or(GovernanceError::VoteTallyOverflow)?;
        }

        // Update used weight for this voter on this proposal
        let new_used_weight = used_weight + vote_weight;
        env.storage().persistent().set(
            &DataKey::VoteWeightUsed(proposal_id, voter.clone()),
            &new_used_weight,
        );
        env.storage().persistent().extend_ttl(
            &DataKey::VoteWeightUsed(proposal_id, voter.clone()),
            PERS_TTL_THRESHOLD,
            PERS_TTL_EXTEND,
        );

        // Store updated proposal
        env.storage().persistent().set(&proposal_key, &proposal);

        env.events().publish(
            (symbol_short!("Voted"), proposal_id),
            (voter, vote_for, vote_weight),
        );

        Ok(())
    }

    /// Execute an expired proposal.
    ///
    /// Two conditions must both be true before execution is allowed:
    /// 1. The voting period has ended (`sequence > expiry_ledger`).
    /// 2. The execution timelock has expired (`sequence > expiry_ledger + execution_delay_ledgers`).
    ///
    /// If `votes_for > votes_against` and the quorum is met, the proposed weights
    /// are queued in the credit-oracle via `propose_weights` (starting the timelock).
    /// Otherwise the proposal is marked executed without changing the weights.
    /// Can only be called after `expiry_ledger`.
    ///
    /// After calling this function, `apply_weights` must be called once the
    /// credit-oracle's timelock expires (approximately 24 hours / 17,280 ledgers)
    /// to finalize the weight change.
    pub fn execute(env: Env, proposal_id: u64) -> Result<(), GovernanceError> {
        let proposal_key = DataKey::Proposal(proposal_id);
        let mut proposal: GovernanceProposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .ok_or(GovernanceError::ProposalNotFound)?;

        // Terminal states are checked before the timing gates so a cancelled
        // (including guardian-vetoed) proposal is reported as
        // `ProposalAlreadyCancelled` immediately, regardless of whether the
        // voting period or execution timelock has elapsed.
        if proposal.executed {
            return Err(GovernanceError::ProposalAlreadyExecuted);
        }

        if proposal.cancelled {
            return Err(GovernanceError::ProposalAlreadyCancelled);
        }

        if env.ledger().sequence() <= proposal.expiry_ledger {
            return Err(GovernanceError::ProposalNotExpired);
        }

        // Check execution timelock: must wait execution_delay_ledgers after voting ends.
        let executable_at = proposal
            .expiry_ledger
            .saturating_add(proposal.execution_delay_ledgers);
        if env.ledger().sequence() <= executable_at {
            return Err(GovernanceError::TimelockNotExpired);
        }

        if proposal.votes_for <= proposal.votes_against {
            return Err(GovernanceError::ProposalRejected);
        }

        if proposal.votes_for + proposal.votes_against < proposal.quorum_required {
            return Err(GovernanceError::QuorumNotMet);
        }

        let credit_oracle_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::CreditOracle)
            .expect("no credit oracle");

        if let Some(pending) = CreditOracleClient::get_pending_weights(&env, &credit_oracle_addr) {
            if env.ledger().sequence() < pending.effective_ledger {
                return Err(GovernanceError::TimelockAlreadyPending);
            }
        }

        // Use propose_weights to start the timelock, not update_weights which bypasses it
        CreditOracleClient::propose_weights(&env, &credit_oracle_addr, &proposal.proposed_weights);

        proposal.executed = true;
        env.storage().persistent().set(&proposal_key, &proposal);

        env.events().publish(
            (symbol_short!("PropExec"), proposal_id),
            (proposal.votes_for, proposal.votes_against),
        );

        Ok(())
    }

    /// Cleanup VoteWeightUsed entries for a completed proposal.
    ///
    /// Only the contract admin can call this function. The proposal must be
    /// executed (`proposal.executed == true`) before cleanup is allowed.
    /// This removes all VoteWeightUsed entries for the specified voters on
    /// the completed proposal.
    ///
    /// If the proposal is not executed, this is a no-op (not an error).
    ///
    /// Auth: `admin` must sign the transaction.
    pub fn cleanup_proposal_votes(
        env: Env,
        admin: Address,
        proposal_id: u64,
        voters: Vec<Address>,
    ) -> Result<(), GovernanceError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(GovernanceError::NotAuthorized);
        }
        admin.require_auth();

        let proposal: GovernanceProposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .ok_or(GovernanceError::ProposalNotFound)?;

        // No-op if proposal not executed
        if !proposal.executed {
            return Ok(());
        }

        // Remove VoteWeightUsed entries for each voter
        for voter in voters.iter() {
            env.storage()
                .persistent()
                .remove(&DataKey::VoteWeightUsed(proposal_id, voter.clone()));
        }

        env.events()
            .publish((symbol_short!("VCln"), proposal_id), voters.len());

        Ok(())
    }

    /// Apply pending weights to the credit-oracle after the timelock expires.
    ///
    /// This function should be called after `execute` has successfully queued
    /// new weights and the credit-oracle's timelock (17,280 ledgers / ~24 hours)
    /// has elapsed. Anyone can call this function.
    ///
    /// Errors from the credit-oracle's `apply_weights` are propagated with the
    /// equivalent governance error so callers can distinguish calling too early
    /// (`TimelockNotExpired`) from nothing being queued (`NoPendingWeights`).
    pub fn apply_weights(env: Env) -> Result<(), GovernanceError> {
        let credit_oracle_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::CreditOracle)
            .ok_or(GovernanceError::NotAuthorized)?;

        match CreditOracleClient::apply_weights(&env, &credit_oracle_addr) {
            Ok(()) => {
                env.events()
                    .publish((symbol_short!("WtApplied"),), env.ledger().sequence());
                Ok(())
            }
            Err(CreditOracleError::TimelockNotExpired) => Err(GovernanceError::TimelockNotExpired),
            Err(CreditOracleError::NoPendingWeights) => Err(GovernanceError::NoPendingWeights),
            Err(CreditOracleError::ContractPaused) => Err(GovernanceError::ContractPaused),
            Err(_) => Err(GovernanceError::NotAuthorized),
        }
    }

    /// Accept the admin role of the credit-oracle on behalf of this contract.
    ///
    /// Must be called after the current oracle admin proposes this contract as
    /// the new admin via `propose_new_admin`. Admin auth is required.
    pub fn accept_oracle_admin(env: Env) -> Result<(), GovernanceError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        admin.require_auth();

        let credit_oracle_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::CreditOracle)
            .ok_or(GovernanceError::NotAuthorized)?;

        CreditOracleClient::accept_admin(
            &env,
            &credit_oracle_addr,
            &env.current_contract_address(),
        );
        Ok(())
    }

    /// Register or replace up to 50 voter weights. Duplicate addresses are
    /// processed in order, so the last entry determines the final weight.
    pub fn batch_register_voters(
        env: Env,
        admin: Address,
        entries: Vec<VoterWeightEntry>,
    ) -> Result<(), GovernanceError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(GovernanceError::NotAuthorized);
        }
        admin.require_auth();
        if entries.len() > MAX_VOTER_BATCH {
            return Err(GovernanceError::VoterBatchTooLarge);
        }
        for entry in entries.iter() {
            if entry.weight <= 0 {
                return Err(GovernanceError::InvalidVoteWeight);
            }
        }
        for entry in entries.iter() {
            set_voter_weight(&env, entry.voter, entry.weight, true);
        }
        Ok(())
    }

    /// Update up to 50 voter weights in order; zero removes a voter.
    /// For duplicate addresses, the last entry determines the final weight.
    pub fn batch_update_voter_weights(
        env: Env,
        admin: Address,
        entries: Vec<VoterWeightEntry>,
    ) -> Result<(), GovernanceError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(GovernanceError::NotAuthorized);
        }
        admin.require_auth();
        if entries.len() > MAX_VOTER_BATCH {
            return Err(GovernanceError::VoterBatchTooLarge);
        }
        for entry in entries.iter() {
            if entry.weight < 0 {
                return Err(GovernanceError::InvalidVoteWeight);
            }
        }
        for entry in entries.iter() {
            set_voter_weight(&env, entry.voter, entry.weight, false);
        }
        Ok(())
    }

    /// Register a voter with specific voting weight.
    ///
    /// Only the contract admin can register voters. The weight must be positive.
    ///
    /// Auth: `admin` must sign the transaction.
    pub fn register_voter(
        env: Env,
        admin: Address,
        voter: Address,
        weight: i128,
    ) -> Result<(), GovernanceError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(GovernanceError::NotAuthorized);
        }
        if weight <= 0 {
            return Err(GovernanceError::InvalidVoteWeight);
        }
        admin.require_auth();

        // `register_voter` is also the upsert entry point for voter weights.
        // Replace (rather than add to) any existing weight so the aggregate
        // remains an accurate basis for percentage-based quorum calculations.
        let old_weight: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::VoterWeight(voter.clone()))
            .unwrap_or(0);

        env.storage()
            .persistent()
            .set(&DataKey::VoterWeight(voter.clone()), &weight);
        env.storage().persistent().extend_ttl(
            &DataKey::VoterWeight(voter.clone()),
            PERS_TTL_THRESHOLD,
            PERS_TTL_EXTEND,
        );

        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalRegisteredWeight)
            .unwrap_or(0);
        env.storage().instance().set(
            &DataKey::TotalRegisteredWeight,
            &(total - old_weight + weight),
        );

        env.events()
            .publish((symbol_short!("VoterReg"), voter.clone()), weight);

        Ok(())
    }

    /// Update a voter's weight.
    ///
    /// Only the contract admin can update voter weights. The weight must be positive.
    /// Setting weight to 0 effectively deregisters the voter.
    ///
    /// Auth: `admin` must sign the transaction.
    pub fn update_voter_weight(
        env: Env,
        admin: Address,
        voter: Address,
        weight: i128,
    ) -> Result<(), GovernanceError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(GovernanceError::NotAuthorized);
        }
        if weight < 0 {
            return Err(GovernanceError::InvalidVoteWeight);
        }
        admin.require_auth();

        let old_weight: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::VoterWeight(voter.clone()))
            .unwrap_or(0);

        if weight == 0 {
            env.storage()
                .persistent()
                .remove(&DataKey::VoterWeight(voter.clone()));
        } else {
            env.storage()
                .persistent()
                .set(&DataKey::VoterWeight(voter.clone()), &weight);
            env.storage().persistent().extend_ttl(
                &DataKey::VoterWeight(voter.clone()),
                PERS_TTL_THRESHOLD,
                PERS_TTL_EXTEND,
            );
        }

        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalRegisteredWeight)
            .unwrap_or(0);
        env.storage().instance().set(
            &DataKey::TotalRegisteredWeight,
            &(total - old_weight + weight),
        );

        env.events()
            .publish((symbol_short!("VoterUpd"), voter.clone()), weight);

        Ok(())
    }

    /// Remove a voter's registration.
    ///
    /// Only the contract admin can deregister voters. This removes all voting
    /// weight from the voter.
    ///
    /// Auth: `admin` must sign the transaction.
    pub fn deregister_voter(
        env: Env,
        admin: Address,
        voter: Address,
    ) -> Result<(), GovernanceError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        if admin != stored_admin {
            return Err(GovernanceError::NotAuthorized);
        }
        admin.require_auth();

        let old_weight: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::VoterWeight(voter.clone()))
            .unwrap_or(0);

        env.storage()
            .persistent()
            .remove(&DataKey::VoterWeight(voter.clone()));

        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalRegisteredWeight)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalRegisteredWeight, &(total - old_weight));

        env.events()
            .publish((symbol_short!("VoterDer"), voter.clone()), ());

        Ok(())
    }

    /// Fetch a proposal by its ID, or `None` if it does not exist.
    pub fn get_proposal(env: Env, proposal_id: u64) -> Option<GovernanceProposal> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
    }

    /// Get a voter's total registered weight.
    ///
    /// Returns `None` if the voter is not registered.
    pub fn get_voter_weight(env: Env, voter: Address) -> Option<i128> {
        env.storage().persistent().get(&DataKey::VoterWeight(voter))
    }

    /// Get how much weight a voter has used in a specific proposal.
    ///
    /// Returns 0 if the voter has not voted on this proposal.
    pub fn get_vote_weight_used(env: Env, proposal_id: u64, voter: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::VoteWeightUsed(proposal_id, voter))
            .unwrap_or(0)
    }

    /// Get a voter's available weight for a proposal (total - used).
    ///
    /// Returns 0 if the voter is not registered.
    pub fn get_available_vote_weight(env: Env, proposal_id: u64, voter: Address) -> i128 {
        let total_weight = env
            .storage()
            .persistent()
            .get(&DataKey::VoterWeight(voter.clone()))
            .unwrap_or(0);

        let used_weight = env
            .storage()
            .persistent()
            .get(&DataKey::VoteWeightUsed(proposal_id, voter))
            .unwrap_or(0);

        total_weight - used_weight
    }

    /// List governance proposals starting from `from_id` up to `limit`.
    ///
    /// Iterates proposal IDs in `[from_id, from_id + min(limit, 20))`.
    /// `limit` is capped at 20 to prevent storage read budget exhaustion.
    /// Non-existent proposals (deleted or skipped) are omitted.
    /// If `include_inactive` is `false`, cancelled and executed proposals are skipped.
    /// Returns an empty vector (not an error) if `from_id` is beyond `NextProposalId` or `limit` is 0.
    pub fn list_proposals(
        env: Env,
        from_id: u64,
        limit: u32,
        include_inactive: bool,
    ) -> Vec<GovernanceProposal> {
        let mut result = Vec::new(&env);
        let cap = limit.min(20);
        if cap == 0 {
            return result;
        }

        let next_proposal_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(FIRST_PROPOSAL_ID);

        if from_id >= next_proposal_id {
            return result;
        }

        let end_id = from_id.saturating_add(cap as u64).min(next_proposal_id);

        for id in from_id..end_id {
            if let Some(proposal) = env
                .storage()
                .persistent()
                .get::<DataKey, GovernanceProposal>(&DataKey::Proposal(id))
            {
                if include_inactive || (!proposal.executed && !proposal.cancelled) {
                    result.push_back(proposal);
                }
            }
        }

        result
    }

    /// Cancel a governance proposal.
    ///
    /// Only the original proposer or the contract admin may cancel a proposal.
    /// A proposal that has already been executed or cancelled cannot be cancelled again.
    /// Cancellation is immediate and permanent â€” a cancelled proposal can never be
    /// executed, regardless of how many votes it accumulated.
    ///
    /// Votes already cast are preserved in storage but have no effect on a cancelled
    /// proposal. The votes are not refunded because registered voting weight is not
    /// consumed globally â€” each voter retains their full weight for other proposals.
    ///
    /// Emits a `PropCanc` event with `(proposal_id)` as topics and
    /// `(canceller)` as data so off-chain indexers can track cancellations.
    ///
    /// Auth: `canceller` must sign the transaction and must be either the
    /// original proposer or the contract admin.
    pub fn cancel_proposal(
        env: Env,
        canceller: Address,
        proposal_id: u64,
    ) -> Result<(), GovernanceError> {
        canceller.require_auth();

        let proposal_key = DataKey::Proposal(proposal_id);
        let mut proposal: GovernanceProposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .ok_or(GovernanceError::ProposalNotFound)?;

        if proposal.executed {
            return Err(GovernanceError::ProposalAlreadyExecuted);
        }

        if proposal.cancelled {
            return Err(GovernanceError::ProposalAlreadyCancelled);
        }

        // Only the original proposer or the admin may cancel.
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotAuthorized)?;
        let stored_proposer: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Proposer(proposal_id))
            .ok_or(GovernanceError::NotAuthorized)?;

        if canceller != stored_admin && canceller != stored_proposer {
            return Err(GovernanceError::NotAuthorized);
        }

        proposal.cancelled = true;
        env.storage().persistent().set(&proposal_key, &proposal);

        env.events()
            .publish((symbol_short!("PropCanc"), proposal_id), canceller);

        Ok(())
    }

    /// Emergency veto of a proposal by the designated guardian.
    ///
    /// Cancels the proposal permanently, even after voting has concluded and
    /// while the proposal is inside its execution-delay timelock. This is the
    /// safety valve for a malicious proposal that passed because of compromised
    /// voting keys: the guardian can block it before `execute` queues the new
    /// weights in the credit-oracle.
    ///
    /// The veto reason is recorded under `DataKey::VetoReason(proposal_id)` and
    /// emitted in the `ProposalVetoed` event.
    ///
    /// Errors:
    /// - `NotAuthorized` if no guardian is configured or `guardian` is not it.
    /// - `ProposalNotFound` if the proposal does not exist.
    /// - `ProposalAlreadyExecuted` if the proposal has already been executed.
    /// - `ProposalAlreadyCancelled` if it was already cancelled or vetoed.
    ///
    /// Auth: `guardian` must sign the transaction.
    pub fn emergency_veto(
        env: Env,
        guardian: Address,
        proposal_id: u64,
        reason: String,
    ) -> Result<(), GovernanceError> {
        guardian.require_auth();

        let stored_guardian: Address = env
            .storage()
            .instance()
            .get(&DataKey::Guardian)
            .ok_or(GovernanceError::NotAuthorized)?;
        if guardian != stored_guardian {
            return Err(GovernanceError::NotAuthorized);
        }

        let proposal_key = DataKey::Proposal(proposal_id);
        let mut proposal: GovernanceProposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .ok_or(GovernanceError::ProposalNotFound)?;

        if proposal.executed {
            return Err(GovernanceError::ProposalAlreadyExecuted);
        }
        if proposal.cancelled {
            return Err(GovernanceError::ProposalAlreadyCancelled);
        }

        proposal.cancelled = true;
        env.storage().persistent().set(&proposal_key, &proposal);

        // Stored separately so the GovernanceProposal layout stays stable for
        // proposals written before this feature was deployed.
        env.storage()
            .persistent()
            .set(&DataKey::VetoReason(proposal_id), &reason);
        env.storage().persistent().extend_ttl(
            &DataKey::VetoReason(proposal_id),
            PERS_TTL_THRESHOLD,
            PERS_TTL_EXTEND,
        );

        env.events().publish(
            (Symbol::new(&env, "ProposalVetoed"), proposal_id),
            (guardian, reason),
        );

        Ok(())
    }

    /// Return the recorded veto reason for a proposal, if it was vetoed.
    pub fn get_veto_reason(env: Env, proposal_id: u64) -> Option<String> {
        env.storage()
            .persistent()
            .get(&DataKey::VetoReason(proposal_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use credit_oracle::{CreditOracle, CreditOracleClient};
    use soroban_sdk::{
        testutils::{Address as _, Events, Ledger},
        Env, TryIntoVal,
    };

    fn test_cid(env: &Env) -> String {
        String::from_str(env, "ipfs://QmDefaultMetadataCID123456789")
    }

    #[test]
    fn test_voter_batches() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let other = Address::generate(&env);
        let gov_id = env.register(Governance, ());
        let client = GovernanceClient::new(&env, &gov_id);
        client.initialize(&admin, &Address::generate(&env), &1);

        let mut entries = Vec::new(&env);
        for _ in 0..25 {
            entries.push_back(VoterWeightEntry {
                voter: Address::generate(&env),
                weight: 10,
            });
        }
        assert_eq!(
            client.try_batch_register_voters(&other, &entries),
            Err(Ok(GovernanceError::NotAuthorized))
        );
        assert_eq!(
            client.try_batch_update_voter_weights(&other, &entries),
            Err(Ok(GovernanceError::NotAuthorized))
        );
        client.batch_register_voters(&admin, &entries);
        for entry in entries.iter() {
            assert_eq!(client.get_voter_weight(&entry.voter), Some(10));
        }
        assert_eq!(client.get_total_registered_weight(), 250);

        let voter = entries.get(0).unwrap().voter;
        let mut updates = Vec::new(&env);
        updates.push_back(VoterWeightEntry {
            voter: voter.clone(),
            weight: 0,
        });
        updates.push_back(VoterWeightEntry {
            voter: voter.clone(),
            weight: 30,
        });
        client.batch_update_voter_weights(&admin, &updates);
        assert_eq!(client.get_voter_weight(&voter), Some(30));
        assert_eq!(client.get_total_registered_weight(), 270);
        let mut duplicates = Vec::new(&env);
        duplicates.push_back(VoterWeightEntry {
            voter: voter.clone(),
            weight: 20,
        });
        duplicates.push_back(VoterWeightEntry {
            voter: voter.clone(),
            weight: 30,
        });
        client.batch_register_voters(&admin, &duplicates);
        assert_eq!(client.get_total_registered_weight(), 270);

        let mut invalid = Vec::new(&env);
        invalid.push_back(VoterWeightEntry {
            voter: voter.clone(),
            weight: 50,
        });
        invalid.push_back(VoterWeightEntry {
            voter: other.clone(),
            weight: -1,
        });
        assert_eq!(
            client.try_batch_register_voters(&admin, &invalid),
            Err(Ok(GovernanceError::InvalidVoteWeight))
        );
        assert_eq!(
            client.try_batch_update_voter_weights(&admin, &invalid),
            Err(Ok(GovernanceError::InvalidVoteWeight))
        );
        assert_eq!(client.get_voter_weight(&voter), Some(30));
        assert_eq!(client.get_total_registered_weight(), 270);

        let mut oversized = Vec::new(&env);
        for _ in 0..51 {
            oversized.push_back(VoterWeightEntry {
                voter: other.clone(),
                weight: 1,
            });
        }
        assert_eq!(
            client.try_batch_register_voters(&admin, &oversized),
            Err(Ok(GovernanceError::VoterBatchTooLarge))
        );
        assert_eq!(
            client.try_batch_update_voter_weights(&admin, &oversized),
            Err(Ok(GovernanceError::VoterBatchTooLarge))
        );
        assert_eq!(client.get_voter_weight(&other), None);
    }

    #[test]
    fn test_governance_proposal_creation_voting_and_execution() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        let credit_oracle_client = CreditOracleClient::new(&env, &credit_oracle_id);
        credit_oracle_client.initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &1000);

        // Propose governance contract as new admin of credit oracle
        credit_oracle_client.propose_new_admin(&gov_id);

        // Accept oracle admin from governance
        gov_client.accept_oracle_admin();

        // Create a proposal
        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };

        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));
        assert_eq!(proposal_id, 1);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.expiry_ledger, env.ledger().sequence() + 100);
        assert!(!proposal.executed);

        // Vote
        let voter1 = Address::generate(&env);
        let voter2 = Address::generate(&env);

        // Register voters with appropriate weights
        gov_client.register_voter(&admin, &voter1, &1000);
        gov_client.register_voter(&admin, &voter2, &400);

        gov_client.vote(&voter1, &proposal_id, &true, &1000);
        gov_client.vote(&voter2, &proposal_id, &false, &400);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 1000);
        assert_eq!(proposal.votes_against, 400);

        // Try to execute before expiry (should fail)
        let res = gov_client.try_execute(&proposal_id);
        assert_eq!(res, Err(Ok(GovernanceError::ProposalNotExpired)));

        // Advance ledger past voting period
        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });

        // Execute proposal - this now calls propose_weights (starts timelock)
        gov_client.execute(&proposal_id);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.executed);

        // Verify weights are NOT changed immediately (timelock in effect)
        let weights_after_execute = credit_oracle_client.get_scoring_weights();
        assert_eq!(weights_after_execute.vc_weight, 40); // Still default
        assert_eq!(weights_after_execute.tx_weight, 30);
        assert_eq!(weights_after_execute.repayment_weight, 30);

        // Verify pending weights exist
        let pending = credit_oracle_client.get_pending_weights();
        assert!(pending.is_some());
        let pending_record = pending.unwrap();
        assert_eq!(pending_record.weights.vc_weight, 50);
        assert_eq!(pending_record.weights.tx_weight, 20);
        assert_eq!(pending_record.weights.repayment_weight, 30);

        // Advance ledger past timelock (~24 hours = 17,280 ledgers)
        let jump = 17_280 + 2;
        env.as_contract(&credit_oracle_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.as_contract(&gov_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger().with_mut(|l| {
            l.sequence_number += jump;
        });

        // Apply weights after timelock
        gov_client.apply_weights();

        // Verify credit oracle weights updated
        let active_weights = credit_oracle_client.get_scoring_weights();
        assert_eq!(active_weights.vc_weight, 50);
        assert_eq!(active_weights.tx_weight, 20);
        assert_eq!(active_weights.repayment_weight, 30);
    }

    /// Issue #665: `initialize` must emit exactly one `Initialized` event
    /// carrying the admin and credit-oracle addresses so off-chain indexers
    /// can observe governance deployments before the first admin action, and
    /// a second initialization must fail without emitting a duplicate event.
    #[test]
    fn test_initialize_emits_initialized_event() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &500);

        // A second initialize must fail and must not emit another event.
        let events = env.events().all();
        let mut gov_events = 0;
        let mut found_initialized = false;
        for (contract_id, topics, data) in events.iter() {
            if contract_id != gov_id {
                continue;
            }
            gov_events += 1;
            if topics.len() == 1 {
                let evt_topic: soroban_sdk::Symbol = topics
                    .get(0)
                    .unwrap()
                    .try_into_val(&env)
                    .unwrap_or(soroban_sdk::symbol_short!("invalid"));
                if evt_topic == Symbol::new(&env, "Initialized") {
                    found_initialized = true;
                    let payload: (Address, Address) = data.try_into_val(&env).unwrap();
                    assert_eq!(
                        payload.0, admin,
                        "governance Initialized event admin mismatch"
                    );
                    assert_eq!(
                        payload.1, credit_oracle_id,
                        "governance Initialized event credit_oracle mismatch"
                    );
                }
            }
        }

        assert_eq!(
            gov_events, 1,
            "governance should emit exactly one event on initialize"
        );
        assert!(
            found_initialized,
            "governance Initialized event should be emitted"
        );

        let res = gov_client.try_initialize(&admin, &credit_oracle_id, &500);
        assert_eq!(res, Err(Ok(GovernanceError::AlreadyInitialized)));
    }

    #[test]
    fn test_proposal_with_exactly_quorum_votes_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        let credit_oracle_client = CreditOracleClient::new(&env, &credit_oracle_id);
        credit_oracle_client.initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &500);

        credit_oracle_client.propose_new_admin(&gov_id);
        gov_client.accept_oracle_admin();

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.quorum_required, 500);

        // votes_for + votes_against == quorum_required exactly, and for > against.
        let voter1 = Address::generate(&env);
        let voter2 = Address::generate(&env);

        // Register voters with appropriate weights
        gov_client.register_voter(&admin, &voter1, &300);
        gov_client.register_voter(&admin, &voter2, &200);

        gov_client.vote(&voter1, &proposal_id, &true, &300);
        gov_client.vote(&voter2, &proposal_id, &false, &200);

        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });

        // Execute queues weights (starts timelock)
        gov_client.execute(&proposal_id);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.executed);

        // Weights NOT changed immediately
        let weights_after_execute = credit_oracle_client.get_scoring_weights();
        assert_eq!(weights_after_execute.vc_weight, 40); // Default values match proposal, so no change visible

        // Advance past timelock and apply
        let jump = 17_280 + 2;
        env.as_contract(&credit_oracle_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.as_contract(&gov_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger().with_mut(|l| {
            l.sequence_number += jump;
        });

        gov_client.apply_weights();

        let active_weights = credit_oracle_client.get_scoring_weights();
        assert_eq!(active_weights.vc_weight, 40);
    }

    #[test]
    fn test_vote_rejects_non_positive_weight() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &500);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        let voter = Address::generate(&env);

        // Register voter with sufficient weight for this test
        gov_client.register_voter(&admin, &voter, &100);

        let res = gov_client.try_vote(&voter, &proposal_id, &true, &0);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVoteWeight)));

        let res = gov_client.try_vote(&voter, &proposal_id, &true, &-10);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVoteWeight)));
    }

    #[test]
    fn test_vote_rejects_tally_overflow() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &500);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));
        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &1);

        env.as_contract(&gov_id, || {
            let proposal_key = DataKey::Proposal(proposal_id);
            let mut proposal: GovernanceProposal =
                env.storage().persistent().get(&proposal_key).unwrap();
            proposal.votes_for = i128::MAX;
            env.storage().persistent().set(&proposal_key, &proposal);
        });

        let result = gov_client.try_vote(&voter, &proposal_id, &true, &1);
        assert_eq!(result, Err(Ok(GovernanceError::VoteTallyOverflow)));
    }

    #[test]
    fn test_cancel_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &500);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        // The proposer cancels their own proposal.
        gov_client.cancel_proposal(&proposer, &proposal_id);

        let events = env.events().all();
        let mut found_event = false;

        for (contract_id, topics, data) in events.iter() {
            if contract_id == gov_id {
                if topics.len() == 2 {
                    let symbol: soroban_sdk::Symbol = topics
                        .get(0)
                        .unwrap()
                        .try_into_val(&env)
                        .unwrap_or(soroban_sdk::symbol_short!("invalid"));
                    if symbol == soroban_sdk::symbol_short!("PropCanc") {
                        found_event = true;
                        let id: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
                        assert_eq!(id, proposal_id);

                        let event_canceller: Address = data.try_into_val(&env).unwrap();
                        assert_eq!(event_canceller, proposer);
                    }
                }
            }
        }

        assert!(found_event, "ProposalCancelled event should be emitted");

        // Verify the proposal is now marked cancelled on-chain.
        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(
            proposal.cancelled,
            "proposal.cancelled must be true after cancel_proposal"
        );
        assert!(!proposal.executed, "proposal.executed must remain false");
    }

    /// A cancelled proposal must reject all further vote calls with
    /// `ProposalAlreadyCancelled`, even while the voting period is still
    /// open, and must remain unexecutable (`execute` already guards against
    /// cancelled proposals). Votes cast before cancellation are preserved.
    #[test]
    fn test_vote_rejected_after_cancellation() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        // Vote before cancellation succeeds.
        let voter1 = Address::generate(&env);
        let voter2 = Address::generate(&env);
        gov_client.register_voter(&admin, &voter1, &500);
        gov_client.register_voter(&admin, &voter2, &500);
        gov_client.vote(&voter1, &proposal_id, &true, &300);

        // The proposer cancels their own proposal.
        gov_client.cancel_proposal(&proposer, &proposal_id);

        // Voting after cancellation must fail even though the voting period
        // has not expired.
        let res = gov_client.try_vote(&voter2, &proposal_id, &true, &100);
        assert_eq!(res, Err(Ok(GovernanceError::ProposalAlreadyCancelled)));

        // A voter who already voted cannot add more weight either.
        let res = gov_client.try_vote(&voter1, &proposal_id, &true, &100);
        assert_eq!(res, Err(Ok(GovernanceError::ProposalAlreadyCancelled)));

        // Votes cast before cancellation are preserved for audit.
        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.cancelled);
        assert_eq!(proposal.votes_for, 300);

        // Execution of a cancelled proposal remains rejected.
        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });
        let res = gov_client.try_execute(&proposal_id);
        assert_eq!(res, Err(Ok(GovernanceError::ProposalAlreadyCancelled)));
    }

    /// The admin can cancel someone else's proposal, and voting on that
    /// proposal is then rejected just as if the proposer had cancelled it.
    #[test]
    fn test_admin_cancel_blocks_further_votes() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        // Admin (not the proposer) cancels the proposal.
        gov_client.cancel_proposal(&admin, &proposal_id);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.cancelled, "admin cancel must persist cancelled");

        // Voting on an admin-cancelled proposal is rejected.
        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &100);
        let res = gov_client.try_vote(&voter, &proposal_id, &true, &100);
        assert_eq!(res, Err(Ok(GovernanceError::ProposalAlreadyCancelled)));
    }

    #[test]
    fn test_set_guardian_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let guardian = Address::generate(&env);
        let gov_id = env.register_contract(None, Governance);
        let client = GovernanceClient::new(&env, &gov_id);
        client.initialize(&admin, &Address::generate(&env), &1);

        assert_eq!(client.get_guardian(), None);

        let non_admin = Address::generate(&env);
        assert_eq!(
            client.try_set_guardian(&non_admin, &guardian),
            Err(Ok(GovernanceError::NotAuthorized))
        );

        client.set_guardian(&admin, &guardian);
        assert_eq!(client.get_guardian(), Some(guardian));
    }

    #[test]
    fn test_emergency_veto_requires_guardian() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let gov_id = env.register_contract(None, Governance);
        let client = GovernanceClient::new(&env, &gov_id);
        client.initialize(&admin, &Address::generate(&env), &1);

        let proposer = Address::generate(&env);
        let weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposal_id = client.create_proposal(&proposer, &weights, &100, &0);

        let guardian = Address::generate(&env);
        let attacker = Address::generate(&env);
        let reason = String::from_str(&env, "attempted veto");

        // No guardian configured yet.
        assert_eq!(
            client.try_emergency_veto(&attacker, &proposal_id, &reason),
            Err(Ok(GovernanceError::NotAuthorized))
        );

        client.set_guardian(&admin, &guardian);

        // The admin is not automatically the guardian.
        assert_eq!(
            client.try_emergency_veto(&admin, &proposal_id, &reason),
            Err(Ok(GovernanceError::NotAuthorized))
        );
        assert_eq!(
            client.try_emergency_veto(&attacker, &proposal_id, &reason),
            Err(Ok(GovernanceError::NotAuthorized))
        );
    }

    #[test]
    fn test_emergency_veto_during_timelock_blocks_execution() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        let credit_oracle_client = CreditOracleClient::new(&env, &credit_oracle_id);
        credit_oracle_client.initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        credit_oracle_client.propose_new_admin(&gov_id);
        gov_client.accept_oracle_admin();

        let guardian = Address::generate(&env);
        gov_client.set_guardian(&admin, &guardian);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        // voting_period = 100 ledgers, execution_delay = 50 ledgers.
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &50);

        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &200);
        gov_client.vote(&voter, &proposal_id, &true, &200);

        // Voting has concluded but the execution timelock is still running.
        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });
        assert_eq!(
            gov_client.try_execute(&proposal_id),
            Err(Ok(GovernanceError::TimelockNotExpired))
        );

        let reason = String::from_str(&env, "compromised proposer key");
        gov_client.emergency_veto(&guardian, &proposal_id, &reason);

        // Drain events before any further client call clears the buffer.
        let events = env.events().all();
        let mut veto_event_count = 0u32;
        for (contract_id, topics, data) in events.iter() {
            if contract_id != gov_id || topics.len() != 2 {
                continue;
            }
            let topic: Symbol = topics
                .get(0)
                .unwrap()
                .try_into_val(&env)
                .expect("topic should be a Symbol");
            if topic != Symbol::new(&env, "ProposalVetoed") {
                continue;
            }
            let event_id: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
            assert_eq!(event_id, proposal_id);
            let (event_guardian, event_reason): (Address, String) =
                data.try_into_val(&env).unwrap();
            assert_eq!(event_guardian, guardian);
            assert_eq!(event_reason, reason);
            veto_event_count += 1;
        }
        assert_eq!(veto_event_count, 1, "expected one ProposalVetoed event");

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.cancelled, "veto must mark the proposal cancelled");
        assert!(
            !proposal.executed,
            "veto must not mark the proposal executed"
        );
        assert_eq!(gov_client.get_veto_reason(&proposal_id), Some(reason));

        // Effective immediately, while still inside the timelock.
        assert_eq!(
            gov_client.try_execute(&proposal_id),
            Err(Ok(GovernanceError::ProposalAlreadyCancelled))
        );

        // ...and permanently, after the timelock would have expired.
        env.ledger().with_mut(|l| {
            l.sequence_number += 50;
        });
        assert_eq!(
            gov_client.try_execute(&proposal_id),
            Err(Ok(GovernanceError::ProposalAlreadyCancelled))
        );

        // A proposal can only be vetoed once.
        let again = String::from_str(&env, "second veto");
        assert_eq!(
            gov_client.try_emergency_veto(&guardian, &proposal_id, &again),
            Err(Ok(GovernanceError::ProposalAlreadyCancelled))
        );
    }

    #[test]
    fn test_emergency_veto_rejects_executed_proposal() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let gov_id = env.register_contract(None, Governance);
        let client = GovernanceClient::new(&env, &gov_id);
        client.initialize(&admin, &Address::generate(&env), &1);

        let guardian = Address::generate(&env);
        client.set_guardian(&admin, &guardian);

        let proposer = Address::generate(&env);
        let weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposal_id = client.create_proposal(&proposer, &weights, &100, &0);

        // Simulate the proposal having already been executed.
        env.as_contract(&gov_id, || {
            let key = DataKey::Proposal(proposal_id);
            let mut proposal: GovernanceProposal = env.storage().persistent().get(&key).unwrap();
            proposal.executed = true;
            env.storage().persistent().set(&key, &proposal);
        });

        let reason = String::from_str(&env, "too late");
        assert_eq!(
            client.try_emergency_veto(&guardian, &proposal_id, &reason),
            Err(Ok(GovernanceError::ProposalAlreadyExecuted))
        );
    }

    /// Verifies the full execution timelock flow:
    /// vote passes â†’ advance past voting â†’ execution rejected (timelock) â†’
    /// advance past delay â†’ execution succeeds.
    ///
    /// Per the double-timelock model in docs/governance.md Â§2.2, `execute()`
    /// only queues weights (via `propose_weights`), so the active weights must
    /// remain unchanged and a `PendingWeights` record must be visible until
    /// `apply_weights()` runs after the credit-oracle's 17,280-ledger timelock.
    #[test]
    fn test_execution_timelock_delays_after_voting_ends() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        let credit_oracle_client = CreditOracleClient::new(&env, &credit_oracle_id);
        credit_oracle_client.initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        credit_oracle_client.propose_new_admin(&gov_id);
        gov_client.accept_oracle_admin();

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };

        let proposer = Address::generate(&env);
        // voting_period = 100 ledgers, execution_delay = 50 ledgers
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &50, &test_cid(&env));

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.execution_delay_ledgers, 50);

        // Cast enough votes for the proposal to pass
        let voter = Address::generate(&env);

        // Register voter with sufficient weight
        gov_client.register_voter(&admin, &voter, &200);

        gov_client.vote(&voter, &proposal_id, &true, &200);

        // Advance just past voting period but NOT past the execution timelock
        // sequence = expiry_ledger + 1  (voting done, delay not done)
        env.ledger().with_mut(|l| {
            l.sequence_number += 101; // past expiry, but only 1 ledger into the delay
        });

        // Execution must be rejected â€” timelock not yet expired
        let res = gov_client.try_execute(&proposal_id);
        assert_eq!(res, Err(Ok(GovernanceError::TimelockNotExpired)));

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(!proposal.executed);

        // Advance past the execution timelock (50 more ledgers)
        env.ledger().with_mut(|l| {
            l.sequence_number += 50;
        });

        // Execution must now succeed
        gov_client.execute(&proposal_id);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.executed);

        // Verify weights are pending (execute queues them, does not apply immediately)
        let active_weights = credit_oracle_client.get_scoring_weights();
        assert_eq!(
            active_weights.vc_weight, 40,
            "weights should not change until apply_weights after timelock"
        );

        // Verify pending weights exist
        let pending = credit_oracle_client.get_pending_weights();
        assert!(pending.is_some());
        let pending_record = pending.unwrap();
        assert_eq!(pending_record.weights.vc_weight, 50);
        assert_eq!(pending_record.weights.tx_weight, 20);
        assert_eq!(pending_record.weights.repayment_weight, 30);
    }

    #[test]
    fn test_execute_rejected_proposal() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        let voter_for = Address::generate(&env);
        let voter_against = Address::generate(&env);
        gov_client.register_voter(&admin, &voter_for, &40);
        gov_client.register_voter(&admin, &voter_against, &60);
        gov_client.vote(&voter_for, &proposal_id, &true, &40);
        gov_client.vote(&voter_against, &proposal_id, &false, &60);

        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });

        let res = gov_client.try_execute(&proposal_id);
        assert_eq!(res, Err(Ok(GovernanceError::ProposalRejected)));
    }

    #[test]
    fn test_apply_weights_relay_propagates_timelock_error() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        let credit_oracle_client = CreditOracleClient::new(&env, &credit_oracle_id);
        credit_oracle_client.initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        credit_oracle_client.propose_new_admin(&gov_id);
        gov_client.accept_oracle_admin();

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &200);
        gov_client.vote(&voter, &proposal_id, &true, &200);

        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });

        // Execute queues weights and starts the credit-oracle timelock.
        gov_client.execute(&proposal_id);

        // Calling before the credit-oracle timelock elapses must surface the
        // typed error instead of a panic.
        let res = gov_client.try_apply_weights();
        assert_eq!(res, Err(Ok(GovernanceError::TimelockNotExpired)));
    }

    #[test]
    fn test_apply_weights_relay_propagates_no_pending_weights_error() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        let credit_oracle_client = CreditOracleClient::new(&env, &credit_oracle_id);
        credit_oracle_client.initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &500);

        credit_oracle_client.propose_new_admin(&gov_id);
        gov_client.accept_oracle_admin();

        // No proposal has been executed, so the credit-oracle has no pending
        // weights to apply.
        let res = gov_client.try_apply_weights();
        assert_eq!(res, Err(Ok(GovernanceError::NoPendingWeights)));
    }

    #[test]
    fn test_voter_registration_and_weight_management() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let voter = Address::generate(&env);

        // Initially voter has no weight
        assert_eq!(gov_client.get_voter_weight(&voter), None);

        // Admin can register voter with weight
        gov_client.register_voter(&admin, &voter, &500);
        assert_eq!(gov_client.get_voter_weight(&voter), Some(500));

        // Admin can update voter weight
        gov_client.update_voter_weight(&admin, &voter, &750);
        assert_eq!(gov_client.get_voter_weight(&voter), Some(750));

        // Admin can deregister voter
        gov_client.deregister_voter(&admin, &voter);
        assert_eq!(gov_client.get_voter_weight(&voter), None);

        // Setting weight to 0 also deregisters
        gov_client.register_voter(&admin, &voter, &100);
        gov_client.update_voter_weight(&admin, &voter, &0);
        assert_eq!(gov_client.get_voter_weight(&voter), None);
    }

    #[test]
    fn test_quorum_percent_uses_total_registered_weight_for_new_proposals() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);
        let gov_id = env.register(Governance, ());
        let client = GovernanceClient::new(&env, &gov_id);
        client.initialize(&admin, &credit_oracle_id, &1);
        client.register_voter(&admin, &Address::generate(&env), &101);
        client.set_quorum_percent(&admin, &50);
        assert_eq!(client.get_quorum_percent(), Some(50));
        let proposal_id = client.create_proposal(
            &Address::generate(&env),
            &ScoringWeights {
                vc_weight: 40,
                tx_weight: 30,
                repayment_weight: 30,
            },
            &10,
            &0,
            &test_cid(&env),
        );
        assert_eq!(
            client.get_proposal(&proposal_id).unwrap().quorum_required,
            50
        );
    }

    #[test]
    fn test_reregistering_voter_keeps_relative_quorum_total_accurate() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let voter = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);
        let gov_id = env.register(Governance, ());
        let client = GovernanceClient::new(&env, &gov_id);
        client.initialize(&admin, &credit_oracle_id, &1);

        client.register_voter(&admin, &voter, &100);
        client.register_voter(&admin, &voter, &40);
        assert_eq!(client.get_total_registered_weight(), 40);

        client.set_quorum_percent(&admin, &50);
        let proposal_id = client.create_proposal(
            &Address::generate(&env),
            &ScoringWeights {
                vc_weight: 40,
                tx_weight: 30,
                repayment_weight: 30,
            },
            &10,
            &0,
            &test_cid(&env),
        );
        assert_eq!(
            client.get_proposal(&proposal_id).unwrap().quorum_required,
            20
        );
    }

    #[test]
    fn test_unregistered_voter_cannot_vote() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        let voter = Address::generate(&env);
        let res = gov_client.try_vote(&voter, &proposal_id, &true, &100);
        assert_eq!(res, Err(Ok(GovernanceError::VoterNotRegistered)));
    }

    #[test]
    fn test_voter_cannot_exceed_weight_limit() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &100);

        // Voter tries to vote with more weight than they have
        let res = gov_client.try_vote(&voter, &proposal_id, &true, &150);
        assert_eq!(res, Err(Ok(GovernanceError::InsufficientVoteWeight)));
    }

    #[test]
    fn test_multiple_votes_within_weight_limit() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));

        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &100);

        // Voter casts partial votes
        gov_client.vote(&voter, &proposal_id, &true, &60);
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id, &voter), 60);
        assert_eq!(
            gov_client.get_available_vote_weight(&proposal_id, &voter),
            40
        );

        gov_client.vote(&voter, &proposal_id, &false, &40);
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id, &voter), 100);
        assert_eq!(
            gov_client.get_available_vote_weight(&proposal_id, &voter),
            0
        );

        // Trying to vote more should fail
        let res = gov_client.try_vote(&voter, &proposal_id, &true, &1);
        assert_eq!(res, Err(Ok(GovernanceError::InsufficientVoteWeight)));

        // Check final vote totals
        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 60);
        assert_eq!(proposal.votes_against, 40);
    }

    #[test]
    fn test_vote_weight_used_survives_long_voting_period() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &1_000_000, &0, &test_cid(&env));

        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &100);
        gov_client.vote(&voter, &proposal_id, &true, &60);

        // Keep the proposal alive so this test isolates voter-entry TTLs.
        env.as_contract(&gov_id, || {
            env.storage().instance().extend_ttl(500_001, 500_001);
            env.storage().persistent().extend_ttl(
                &DataKey::Proposal(proposal_id),
                PERS_TTL_THRESHOLD,
                PERS_TTL_EXTEND,
            );
        });
        env.ledger().with_mut(|ledger| {
            ledger.sequence_number += 500_001;
        });

        gov_client.vote(&voter, &proposal_id, &false, &40);

        assert_eq!(gov_client.get_vote_weight_used(&proposal_id, &voter), 100);
        assert_eq!(
            gov_client.get_available_vote_weight(&proposal_id, &voter),
            0
        );
    }

    #[test]
    fn test_weight_tracking_per_proposal() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        };
        let proposer = Address::generate(&env);

        // Create two proposals
        let proposal_id_1 = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));
        let proposal_id_2 = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0, &test_cid(&env));
        assert_eq!(proposal_id_1, 1);
        assert_eq!(proposal_id_2, 2);

        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &100);

        // Vote on first proposal
        gov_client.vote(&voter, &proposal_id_1, &true, &80);
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id_1, &voter), 80);
        assert_eq!(
            gov_client.get_available_vote_weight(&proposal_id_1, &voter),
            20
        );

        // Weight usage is tracked separately for second proposal
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id_2, &voter), 0);
        assert_eq!(
            gov_client.get_available_vote_weight(&proposal_id_2, &voter),
            100
        );

        // Can vote full weight on second proposal
        gov_client.vote(&voter, &proposal_id_2, &false, &100);
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id_2, &voter), 100);
        assert_eq!(
            gov_client.get_available_vote_weight(&proposal_id_2, &voter),
            0
        );

        // First proposal usage unchanged
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id_1, &voter), 80);
        assert_eq!(
            gov_client.get_available_vote_weight(&proposal_id_1, &voter),
            20
        );
    }

    #[test]
    fn test_non_admin_cannot_register_voters() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let voter = Address::generate(&env);

        // Non-admin cannot register voter
        let res = gov_client.try_register_voter(&non_admin, &voter, &100);
        assert_eq!(res, Err(Ok(GovernanceError::NotAuthorized)));

        // Non-admin cannot update voter weight
        let res = gov_client.try_update_voter_weight(&non_admin, &voter, &200);
        assert_eq!(res, Err(Ok(GovernanceError::NotAuthorized)));

        // Non-admin cannot deregister voter
        let res = gov_client.try_deregister_voter(&non_admin, &voter);
        assert_eq!(res, Err(Ok(GovernanceError::NotAuthorized)));
    }

    #[test]
    fn test_invalid_weight_registration_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let voter = Address::generate(&env);

        // Cannot register with zero weight
        let res = gov_client.try_register_voter(&admin, &voter, &0);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVoteWeight)));

        // Cannot register with negative weight
        let res = gov_client.try_register_voter(&admin, &voter, &-50);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVoteWeight)));

        // Cannot update to negative weight (but 0 is allowed for deregistration)
        gov_client.register_voter(&admin, &voter, &100);
        let res = gov_client.try_update_voter_weight(&admin, &voter, &-10);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVoteWeight)));
    }

    #[test]
    fn test_create_proposal_weight_component_bounds() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposer = Address::generate(&env);

        // Proposal with tx_weight = 0 fails with InvalidWeights
        let res_tx_zero = gov_client.try_create_proposal(
            &proposer,
            &ScoringWeights {
                vc_weight: 60,
                tx_weight: 0,
                repayment_weight: 40,
            },
            &100,
            &10,
            &test_cid(&env),
        );
        assert_eq!(res_tx_zero, Err(Ok(GovernanceError::InvalidWeights)));

        // Proposal with tx_weight = 9 (< MIN_COMPONENT_WEIGHT) fails with InvalidWeights
        let res_tx_low = gov_client.try_create_proposal(
            &proposer,
            &ScoringWeights {
                vc_weight: 51,
                tx_weight: 9,
                repayment_weight: 40,
            },
            &100,
            &10,
            &test_cid(&env),
        );
        assert_eq!(res_tx_low, Err(Ok(GovernanceError::InvalidWeights)));

        // Proposal with vc_weight = 9 fails with InvalidWeights
        let res_vc_low = gov_client.try_create_proposal(
            &proposer,
            &ScoringWeights {
                vc_weight: 9,
                tx_weight: 45,
                repayment_weight: 46,
            },
            &100,
            &10,
            &test_cid(&env),
        );
        assert_eq!(res_vc_low, Err(Ok(GovernanceError::InvalidWeights)));

        // Proposal with repayment_weight = 9 fails with InvalidWeights
        let res_repayment_low = gov_client.try_create_proposal(
            &proposer,
            &ScoringWeights {
                vc_weight: 45,
                tx_weight: 46,
                repayment_weight: 9,
            },
            &100,
            &10,
            &test_cid(&env),
        );
        assert_eq!(res_repayment_low, Err(Ok(GovernanceError::InvalidWeights)));

        // Proposal with tx_weight = 10 (exact MIN_COMPONENT_WEIGHT bound) passes
        let prop_id = gov_client.create_proposal(
            &proposer,
            &ScoringWeights {
                vc_weight: 50,
                tx_weight: 10,
                repayment_weight: 40,
            },
            &100,
            &10,
            &test_cid(&env),
        );
        assert_eq!(prop_id, 1);
    }

    #[test]
    fn test_create_proposal_zero_period() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposer = Address::generate(&env);

        let res = gov_client.try_create_proposal(
            &proposer,
            &ScoringWeights {
                vc_weight: 40,
                tx_weight: 30,
                repayment_weight: 30,
            },
            &0,
            &10,
            &test_cid(&env),
        );
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVotingPeriod)));
    }

    /// Issue #763: Proposals where `votes_against > votes_for` must fail execution
    /// with `ProposalRejected`, even when total votes meet the required quorum (100).
    #[test]
    fn test_governance_majority_check() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let voting_period = 100;
        let execution_delay = 10;
        let proposal_id = gov_client.create_proposal(
            &proposer,
            &proposed_weights,
            &voting_period,
            &execution_delay,
            &test_cid(&env),
        );

        let voter_for = Address::generate(&env);
        let voter_against = Address::generate(&env);
        gov_client.register_voter(&admin, &voter_for, &40);
        gov_client.register_voter(&admin, &voter_against, &60);
        gov_client.vote(&voter_for, &proposal_id, &true, &40);
        gov_client.vote(&voter_against, &proposal_id, &false, &60);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 40);
        assert_eq!(proposal.votes_against, 60);
        assert_eq!(proposal.quorum_required, 100);
        assert_eq!(proposal.votes_for + proposal.votes_against, 100);
        assert!(!proposal.executed);

        // Advance ledger past voting expiry + execution delay
        env.ledger().with_mut(|l| {
            l.sequence_number += voting_period + execution_delay + 1;
        });

        // Attempt execution: must be rejected because votes_against > votes_for
        let res = gov_client.try_execute(&proposal_id);
        assert_eq!(res, Err(Ok(GovernanceError::ProposalRejected)));

        // Verify proposal state was not marked as executed
        let proposal_after = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(
            !proposal_after.executed,
            "rejected proposal must not be marked as executed"
        );
    }

    /// Issue #763: Proposals where `votes_for == votes_against` (tie) must fail
    /// execution with `ProposalRejected`, even when total votes meet the required quorum (100).
    #[test]
    fn test_governance_tie_is_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register(CreditOracle, ());
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register(Governance, ());
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let voting_period = 100;
        let execution_delay = 10;
        let proposal_id = gov_client.create_proposal(
            &proposer,
            &proposed_weights,
            &voting_period,
            &execution_delay,
            &test_cid(&env),
        );

        let voter_for = Address::generate(&env);
        let voter_against = Address::generate(&env);
        gov_client.register_voter(&admin, &voter_for, &50);
        gov_client.register_voter(&admin, &voter_against, &50);
        gov_client.vote(&voter_for, &proposal_id, &true, &50);
        gov_client.vote(&voter_against, &proposal_id, &false, &50);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 50);
        assert_eq!(proposal.votes_against, 50);
        assert_eq!(proposal.quorum_required, 100);
        assert_eq!(proposal.votes_for + proposal.votes_against, 100);
        assert!(!proposal.executed);

        // Advance ledger past voting expiry + execution delay
        env.ledger().with_mut(|l| {
            l.sequence_number += voting_period + execution_delay + 1;
        });

        // Attempt execution: must be rejected because votes_for == votes_against
        let res = gov_client.try_execute(&proposal_id);
        assert_eq!(res, Err(Ok(GovernanceError::ProposalRejected)));

        // Verify proposal state was not marked as executed
        let proposal_after = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(
            !proposal_after.executed,
            "tied proposal must not be marked as executed"
        );
    }

    #[test]
    fn test_create_proposal_stores_and_returns_metadata_cid() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposer = Address::generate(&env);
        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };
        let cid = String::from_str(&env, "ipfs://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &10, &cid);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.metadata_cid, cid);

        let proposals = gov_client.list_proposals(&proposal_id, &1, &true);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals.get(0).unwrap().metadata_cid, cid);
    }

    #[test]
    fn test_create_proposal_rejects_empty_metadata_cid() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposer = Address::generate(&env);
        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };
        let empty_cid = String::from_str(&env, "");
        let res = gov_client.try_create_proposal(&proposer, &proposed_weights, &100, &10, &empty_cid);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidMetadataCid)));
    }

    #[test]
    fn test_create_proposal_rejects_oversized_metadata_cid() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposer = Address::generate(&env);
        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };

        // 129 characters (MAX_METADATA_CID_LEN is 128)
        let oversized_str = "123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789";
        let oversized_cid = String::from_str(&env, oversized_str);
        let res = gov_client.try_create_proposal(&proposer, &proposed_weights, &100, &10, &oversized_cid);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidMetadataCid)));
    }

    #[test]
    fn test_create_proposal_accepts_boundary_length_metadata_cid() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposer = Address::generate(&env);
        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };

        // Boundary 1: minimum valid length (1 character)
        let min_cid = String::from_str(&env, "x");
        let id1 = gov_client.create_proposal(&proposer, &proposed_weights, &100, &10, &min_cid);
        assert_eq!(gov_client.get_proposal(&id1).unwrap().metadata_cid, min_cid);

        // Boundary 2: maximum allowed length (128 characters)
        let max_str = "12345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678";
        let max_cid = String::from_str(&env, max_str);
        let id2 = gov_client.create_proposal(&proposer, &proposed_weights, &100, &10, &max_cid);
        assert_eq!(gov_client.get_proposal(&id2).unwrap().metadata_cid, max_cid);
    }

    #[test]
    fn test_create_proposal_emits_metadata_cid_in_event() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposer = Address::generate(&env);
        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };
        let cid = String::from_str(&env, "ipfs://QmTestProposalEventCID");
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &10, &cid);

        let events = env.events().all();
        let mut found_event = false;

        for (contract_id, topics, data) in events.iter() {
            if contract_id == gov_id && topics.len() == 2 {
                let symbol: soroban_sdk::Symbol = topics
                    .get(0)
                    .unwrap()
                    .try_into_val(&env)
                    .unwrap_or(soroban_sdk::symbol_short!("invalid"));
                if symbol == soroban_sdk::symbol_short!("PropCreat") {
                    let id: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
                    if id == proposal_id {
                        found_event = true;
                        let (event_proposer, expiry_ledger, event_cid): (Address, u32, String) =
                            data.try_into_val(&env).unwrap();
                        assert_eq!(event_proposer, proposer);
                        assert_eq!(expiry_ledger, env.ledger().sequence() + 100);
                        assert_eq!(event_cid, cid);
                    }
                }
            }
        }

        assert!(found_event, "ProposalCreated event with metadata_cid must be emitted");
    }
}
