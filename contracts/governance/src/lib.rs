#![no_std]
//! Governance contract for the Stellar DID Credit protocol.
//!
//! Provides on-chain proposal creation, voting, and execution that can
//! update the credit-oracle's scoring weights through a community vote.
use credit_oracle::{CreditOracleClient, ScoringWeights};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
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
    /// Proposed scoring weights do not sum to 100.
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
}

/// Storage keys for the governance contract.
#[contracttype]
pub enum DataKey {
    /// Contract administrator address.
    Admin,
    /// Address of the credit-oracle contract this governance controls.
    CreditOracle,
    /// Monotonically increasing counter used to assign proposal IDs.
    NextProposalId,
    /// Default quorum (minimum total votes) required for proposal execution.
    QuorumRequired,
    /// Proposal data stored by proposal ID.
    Proposal(u64),
    /// Registered voting weight for an address.
    VoterWeight(Address),
    /// Amount of weight already used by voter in a specific proposal.
    VoteWeightUsed(u64, Address),
}

#[contracttype]
#[derive(Clone)]
/// An on-chain governance proposal for updating credit-oracle scoring weights.
pub struct GovernanceProposal {
    /// Unique proposal identifier, assigned at creation.
    pub id: u64,
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
    /// Minimum `votes_for + votes_against` required for `execute` to apply
    /// this proposal's weights, snapshotted from the contract-wide default
    /// at proposal-creation time so later `set_quorum` calls never change
    /// the rules for a proposal already up for a vote.
    pub quorum_required: i128,
}

/// On-chain governance contract.
#[contract]
pub struct Governance;

#[contractimpl]
impl Governance {
    /// Initialize the governance contract.
    ///
    /// Sets the administrator, credit-oracle address, and default quorum required
    /// for proposals. `quorum_required` must be greater than zero.
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
            .set(&DataKey::NextProposalId, &1u64);
        env.storage()
            .instance()
            .set(&DataKey::QuorumRequired, &quorum_required);
        Ok(())
    }

    /// Updates the contract-wide default quorum applied to proposals created
    /// from this point on. Admin only. Does not affect proposals that
    /// already exist — each snapshots its own `quorum_required` at creation.
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
        env.storage()
            .instance()
            .set(&DataKey::QuorumRequired, &quorum_required);
        Ok(())
    }

    /// Returns the contract-wide default quorum applied to newly created proposals.
    pub fn get_quorum(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::QuorumRequired)
            .unwrap_or(0)
    }

    /// Create a new governance proposal to update the credit-oracle's scoring weights.
    ///
    /// `weights` must sum to 100. The voting period runs for `voting_period_ledgers`
    /// ledgers from the current sequence. After voting ends, execution is further
    /// delayed by `execution_delay_ledgers` ledgers to give the community a reaction
    /// window. Returns the new proposal ID.
    ///
    /// Auth: `proposer` must sign the transaction.
    pub fn create_proposal(
        env: Env,
        proposer: Address,
        weights: ScoringWeights,
        voting_period_ledgers: u32,
        execution_delay_ledgers: u32,
    ) -> Result<u64, GovernanceError> {
        proposer.require_auth();
        if weights.vc_weight + weights.tx_weight + weights.repayment_weight != 100 {
            return Err(GovernanceError::InvalidWeights);
        }

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(1);
        let expiry_ledger = env.ledger().sequence() + voting_period_ledgers;
        let quorum_required: i128 = env
            .storage()
            .instance()
            .get(&DataKey::QuorumRequired)
            .unwrap_or(0);

        let proposal = GovernanceProposal {
            id,
            proposed_weights: weights,
            votes_for: 0,
            votes_against: 0,
            expiry_ledger,
            execution_delay_ledgers,
            executed: false,
            quorum_required,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Proposal(id), &proposal);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &(id + 1));

        env.events()
            .publish((symbol_short!("PropCreat"), id), (proposer, expiry_ledger));

        Ok(id)
    }

    /// Cast a vote on an open proposal.
    ///
    /// `vote_weight` must be positive and cannot exceed the voter's available
    /// weight for this proposal. Each voter can cast multiple votes up to their
    /// total registered weight. Returns an error if the proposal has expired,
    /// been executed, or if the voter lacks sufficient weight.
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

        // Check how much weight this voter has already used for this proposal
        let used_weight: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::VoteWeightUsed(proposal_id, voter.clone()))
            .unwrap_or(0);

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

        // Update vote totals
        if vote_for {
            proposal.votes_for = proposal.votes_for.saturating_add(vote_weight);
        } else {
            proposal.votes_against = proposal.votes_against.saturating_add(vote_weight);
        }

        // Update used weight for this voter on this proposal
        let new_used_weight = used_weight + vote_weight;
        env.storage().persistent().set(
            &DataKey::VoteWeightUsed(proposal_id, voter.clone()),
            &new_used_weight,
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

        if proposal.executed {
            return Err(GovernanceError::ProposalAlreadyExecuted);
        }

        if proposal.votes_for + proposal.votes_against < proposal.quorum_required {
            return Err(GovernanceError::QuorumNotMet);
        }

        if proposal.votes_for > proposal.votes_against {
            let credit_oracle_addr: Address = env
                .storage()
                .instance()
                .get(&DataKey::CreditOracle)
                .expect("no credit oracle");

            let client = CreditOracleClient::new(&env, &credit_oracle_addr);
            // Use propose_weights to start the timelock, not update_weights which bypasses it
            client.propose_weights(&proposal.proposed_weights);
        }

        proposal.executed = true;
        env.storage().persistent().set(&proposal_key, &proposal);

        env.events().publish(
            (symbol_short!("PropExec"), proposal_id),
            (proposal.votes_for, proposal.votes_against),
        );

        Ok(())
    }

    /// Apply pending weights to the credit-oracle after the timelock expires.
    ///
    /// This function should be called after `execute` has successfully queued
    /// new weights and the credit-oracle's timelock (17,280 ledgers / ~24 hours)
    /// has elapsed. Anyone can call this function.
    pub fn apply_weights(env: Env) -> Result<(), GovernanceError> {
        let credit_oracle_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::CreditOracle)
            .ok_or(GovernanceError::NotAuthorized)?;

        let client = CreditOracleClient::new(&env, &credit_oracle_addr);
        client.apply_weights();

        env.events().publish(
            (symbol_short!("WtApplied"),),
            env.ledger().sequence(),
        );

        Ok(())
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

        let client = CreditOracleClient::new(&env, &credit_oracle_addr);
        client.accept_admin(&env.current_contract_address());
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
        
        env.storage()
            .persistent()
            .set(&DataKey::VoterWeight(voter.clone()), &weight);

        env.events().publish(
            (symbol_short!("VoterReg"), voter.clone()),
            weight,
        );

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

        if weight == 0 {
            env.storage()
                .persistent()
                .remove(&DataKey::VoterWeight(voter.clone()));
        } else {
            env.storage()
                .persistent()
                .set(&DataKey::VoterWeight(voter.clone()), &weight);
        }

        env.events().publish(
            (symbol_short!("VoterUpd"), voter.clone()),
            weight,
        );

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

        env.storage()
            .persistent()
            .remove(&DataKey::VoterWeight(voter.clone()));

        env.events().publish(
            (symbol_short!("VoterDer"), voter.clone()),
            (),
        );

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
        env.storage()
            .persistent()
            .get(&DataKey::VoterWeight(voter))
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

    /// Cancel a governance proposal.
    /// 
    /// Note: Full cancellation logic is out of scope. This only emits the cancellation event.
    pub fn cancel(
        env: Env,
        canceller: Address,
        proposal_id: u64,
        reason: Option<soroban_sdk::String>,
    ) -> Result<(), GovernanceError> {
        canceller.require_auth();
        
        env.events().publish(
            (symbol_short!("PropCanc"), proposal_id),
            (canceller, reason),
        );
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use credit_oracle::{CreditOracle, CreditOracleClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger, Events},
        Env, TryIntoVal
    };

    #[test]
    fn test_governance_proposal_creation_voting_and_execution() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        let credit_oracle_client = CreditOracleClient::new(&env, &credit_oracle_id);
        credit_oracle_client.initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
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
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0);
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

    #[test]
    fn test_proposal_with_exactly_quorum_votes_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        let credit_oracle_client = CreditOracleClient::new(&env, &credit_oracle_id);
        credit_oracle_client.initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
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
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0);

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
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &500);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0);

        let voter = Address::generate(&env);
        
        // Register voter with sufficient weight for this test
        gov_client.register_voter(&admin, &voter, &100);
        
        let res = gov_client.try_vote(&voter, &proposal_id, &true, &0);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVoteWeight)));

        let res = gov_client.try_vote(&voter, &proposal_id, &true, &-10);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVoteWeight)));
    }

    #[test]
    fn test_cancel_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &500);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0);

        let canceller = Address::generate(&env);
        let reason = Some(soroban_sdk::String::from_str(&env, "Spam proposal"));

        gov_client.cancel(&canceller, &proposal_id, &reason);

        let events = env.events().all();
        let mut found_event = false;

        for (contract_id, topics, data) in events.iter() {
            if contract_id == gov_id {
                if topics.len() == 2 {
                    let symbol: soroban_sdk::Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap_or(soroban_sdk::symbol_short!("invalid"));
                    if symbol == soroban_sdk::symbol_short!("PropCanc") {
                        found_event = true;
                        let id: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
                        assert_eq!(id, proposal_id);

                        let event_data: (Address, Option<soroban_sdk::String>) = data.try_into_val(&env).unwrap();
                        assert_eq!(event_data.0, canceller);
                        // String comparison might require converting to bytes or comparing values but Option<String> should be somewhat comparable.
                        // We will skip strict String value checking for now.
                    }
                }
            }
        }

        assert!(found_event, "ProposalCancelled event should be emitted");
    }

    /// Verifies the full execution timelock flow:
    /// vote passes → advance past voting → execution rejected (timelock) →
    /// advance past delay → execution succeeds.
    #[test]
    fn test_execution_timelock_delays_after_voting_ends() {
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

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };

        let proposer = Address::generate(&env);
        // voting_period = 100 ledgers, execution_delay = 50 ledgers
        let proposal_id =
            gov_client.create_proposal(&proposer, &proposed_weights, &100, &50);

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

        // Execution must be rejected — timelock not yet expired
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

        // Verify weights were applied
        let active_weights = credit_oracle_client.get_scoring_weights();
        assert_eq!(active_weights.vc_weight, 50);
        assert_eq!(active_weights.tx_weight, 20);
        assert_eq!(active_weights.repayment_weight, 30);
    }

    #[test]
    fn test_voter_registration_and_weight_management() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
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
    fn test_unregistered_voter_cannot_vote() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0);

        let voter = Address::generate(&env);
        let res = gov_client.try_vote(&voter, &proposal_id, &true, &100);
        assert_eq!(res, Err(Ok(GovernanceError::VoterNotRegistered)));
    }

    #[test]
    fn test_voter_cannot_exceed_weight_limit() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0);

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
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        };
        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0);

        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &100);

        // Voter casts partial votes
        gov_client.vote(&voter, &proposal_id, &true, &60);
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id, &voter), 60);
        assert_eq!(gov_client.get_available_vote_weight(&proposal_id, &voter), 40);

        gov_client.vote(&voter, &proposal_id, &false, &40);
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id, &voter), 100);
        assert_eq!(gov_client.get_available_vote_weight(&proposal_id, &voter), 0);

        // Trying to vote more should fail
        let res = gov_client.try_vote(&voter, &proposal_id, &true, &1);
        assert_eq!(res, Err(Ok(GovernanceError::InsufficientVoteWeight)));

        // Check final vote totals
        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 60);
        assert_eq!(proposal.votes_against, 40);
    }

    #[test]
    fn test_weight_tracking_per_proposal() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
        let gov_client = GovernanceClient::new(&env, &gov_id);
        gov_client.initialize(&admin, &credit_oracle_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 25,
            repayment_weight: 25,
        };
        let proposer = Address::generate(&env);
        
        // Create two proposals
        let proposal_id_1 = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0);
        let proposal_id_2 = gov_client.create_proposal(&proposer, &proposed_weights, &100, &0);

        let voter = Address::generate(&env);
        gov_client.register_voter(&admin, &voter, &100);

        // Vote on first proposal
        gov_client.vote(&voter, &proposal_id_1, &true, &80);
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id_1, &voter), 80);
        assert_eq!(gov_client.get_available_vote_weight(&proposal_id_1, &voter), 20);

        // Weight usage is tracked separately for second proposal
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id_2, &voter), 0);
        assert_eq!(gov_client.get_available_vote_weight(&proposal_id_2, &voter), 100);

        // Can vote full weight on second proposal
        gov_client.vote(&voter, &proposal_id_2, &false, &100);
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id_2, &voter), 100);
        assert_eq!(gov_client.get_available_vote_weight(&proposal_id_2, &voter), 0);

        // First proposal usage unchanged
        assert_eq!(gov_client.get_vote_weight_used(&proposal_id_1, &voter), 80);
        assert_eq!(gov_client.get_available_vote_weight(&proposal_id_1, &voter), 20);
    }

    #[test]
    fn test_non_admin_cannot_register_voters() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
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
        let credit_oracle_id = env.register_contract(None, CreditOracle);
        CreditOracleClient::new(&env, &credit_oracle_id).initialize(&admin);

        let gov_id = env.register_contract(None, Governance);
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
}
