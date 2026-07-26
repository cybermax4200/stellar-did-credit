#![no_std]
use credit_oracle::{CreditOracleClient, ScoringWeights};
use identity_oracle::{IdentityOracleClient, IssuerTier};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum GovernanceError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    ProposalNotFound = 3,
    ProposalExpired = 4,
    ProposalNotExpired = 5,
    ProposalAlreadyExecuted = 6,
    AlreadyVoted = 7,
    InvalidWeights = 8,
    InvalidQuorum = 9,
    InvalidVoteWeight = 10,
    QuorumNotMet = 11,
    /// Execution timelock has not yet expired after the voting period.
    TimelockNotExpired = 12,
}

#[contracttype]
pub enum DataKey {
    Admin,
    CreditOracle,
    /// Optional identity-oracle contract ID, used by governance to adjust
    /// issuer tiers cross-contract when exercising governance-based tier
    /// management.
    IdentityOracle,
    NextProposalId,
    QuorumRequired,
    Proposal(u64),
    Voted(u64, Address),
}

#[contracttype]
#[derive(Clone)]
pub struct GovernanceProposal {
    pub id: u64,
    pub proposed_weights: ScoringWeights,
    pub votes_for: i128,
    pub votes_against: i128,
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

#[contract]
pub struct Governance;

/// Pure, on-chain recommendation for an issuer tier based on issuance
/// and revocation history. Governance is NOT required to follow this
/// recommendation — it exists purely to make tier decisions auditable
/// and transparent when voters / the DAO want to apply a consistent,
/// metrics-based rule.
///
/// The rule intentionally avoids coupling reputation to downstream
/// subject scores, preventing circular dependence (reputation ↔ score).
/// Only revocation ratio and minimum issuance sample size are inputs.
///
/// Thresholds (revoked / issued ratio, with a minimum of 5 VCs issued
/// before any demotion can occur so early issuers aren't punished):
///   ratio == 0                  → Tier3 (gold / no issues)
///   ratio  > 0   and ≤ 0.10     → Tier2 (silver / light penalty)
///   ratio  > 0.10 and ≤ 0.33    → Tier1 (bronze / heavy penalty)
///   ratio  > 0.33               → Tier0 (suspended / no weight)
///
/// Ratio is computed in integer basis points (10_000 = 1.00) so no
/// floating point is required inside WASM.
pub fn recommend_tier_from_metrics(vcs_issued: u32, vcs_revoked: u32) -> IssuerTier {
    if vcs_issued < 5 {
        return IssuerTier::Tier3;
    }
    // revoked_bps = (revoked * 10_000) / issued — basis points of revocations
    let revoked_bps = vcs_revoked as u64 * 10_000u64 / (vcs_issued as u64).max(1);
    if revoked_bps > 3_333 {
        IssuerTier::Tier0
    } else if revoked_bps > 1_000 {
        IssuerTier::Tier1
    } else if revoked_bps > 0 {
        IssuerTier::Tier2
    } else {
        IssuerTier::Tier3
    }
}

#[contractimpl]
impl Governance {
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

    /// Configure the identity-oracle contract ID used by
    /// `adjust_issuer_tier` to forward tier changes.
    ///
    /// Auth: admin only.
    pub fn set_identity_oracle(
        env: Env,
        admin: Address,
        identity_oracle: Address,
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
            .instance()
            .set(&DataKey::IdentityOracle, &identity_oracle);
        Ok(())
    }

    /// Returns the configured identity-oracle address (Option).
    pub fn get_identity_oracle(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::IdentityOracle)
    }

    /// Pure, on-chain recommendation for an issuer tier based on issuance
    /// and revocation history. Governance is NOT required to follow this
    /// recommendation — it exists purely to make tier decisions auditable
    /// and transparent when voters / the DAO want to apply a consistent,
    /// metrics-based rule.
    ///
    /// The rule intentionally avoids coupling reputation to downstream
    /// subject scores, preventing circular dependence (reputation ↔ score).
    /// Only revocation ratio and minimum issuance sample size are inputs.
    ///
    /// Thresholds (revoked / issued ratio, with a minimum of 5 VCs issued
    /// before any demotion can occur so early issuers aren't punished):
    ///   ratio == 0                  → Tier3 (gold / no issues)
    ///   ratio  > 0   and ≤ 0.10     → Tier2 (silver / light penalty)
    ///   ratio  > 0.10 and ≤ 0.33    → Tier1 (bronze / heavy penalty)
    ///   ratio  > 0.33               → Tier0 (suspended / no weight)
    ///
    /// Ratio is computed in integer basis points (10_000 = 1.00) so no
    /// floating point is required inside WASM.
    pub fn recommend_tier_from_metrics(vcs_issued: u32, vcs_revoked: u32) -> IssuerTier {
        if vcs_issued < 5 {
            return IssuerTier::Tier3;
        }
        // revoked_bps = (revoked * 10_000) / issued — basis points of revocations
        let revoked_bps = vcs_revoked as u64 * 10_000u64 / (vcs_issued as u64).max(1);
        if revoked_bps > 3_333 {
            IssuerTier::Tier0
        } else if revoked_bps > 1_000 {
            IssuerTier::Tier1
        } else if revoked_bps > 0 {
            IssuerTier::Tier2
        } else {
            IssuerTier::Tier3
        }
    }

    /// Governance-side wrapper around `identity-oracle.set_issuer_tier`.
    ///
    /// Adjusts an issuer's reputation tier directly (e.g. after a DAO vote
    /// passes that applies `recommend_tier_from_metrics` or another
    /// community-defined rule). The call forwards through to
    /// `IdentityOracleClient.set_issuer_tier` after validating governance
    /// admin auth.
    ///
    /// Auth: governance admin only.
    ///
    /// Panics if `set_identity_oracle` has not yet been called to configure
    /// the identity-oracle address.
    pub fn adjust_issuer_tier(
        env: Env,
        admin: Address,
        issuer: Address,
        tier: IssuerTier,
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

        let identity_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::IdentityOracle)
            .expect("identity oracle not configured");
        let ido_client = IdentityOracleClient::new(&env, &identity_addr);
        ido_client.set_issuer_tier(&issuer, &tier);
        Ok(())
    }

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

        let voted_key = DataKey::Voted(proposal_id, voter.clone());
        if env.storage().persistent().has(&voted_key) {
            return Err(GovernanceError::AlreadyVoted);
        }

        if vote_for {
            proposal.votes_for = proposal.votes_for.saturating_add(vote_weight);
        } else {
            proposal.votes_against = proposal.votes_against.saturating_add(vote_weight);
        }

        env.storage().persistent().set(&proposal_key, &proposal);
        env.storage().persistent().set(&voted_key, &true);

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
    /// are applied to the credit-oracle. Otherwise the proposal is marked executed
    /// without changing the weights.
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

    pub fn get_proposal(env: Env, proposal_id: u64) -> Option<GovernanceProposal> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
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
        gov_client.vote(&voter1, &proposal_id, &true, &1000);
        gov_client.vote(&voter2, &proposal_id, &false, &400);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 1000);
        assert_eq!(proposal.votes_against, 400);

        // Try to execute before expiry (should fail)
        let res = gov_client.try_execute(&proposal_id);
        assert_eq!(res, Err(Ok(GovernanceError::ProposalNotExpired)));

        // Advance ledger
        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });

        // Execute proposal
        gov_client.execute(&proposal_id);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.executed);

        // Advance ledger to pass the timelock
        let jump = 3_000_000;
        env.as_contract(&credit_oracle_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger().with_mut(|l| {
            l.sequence_number += jump;
        });

        // Apply proposed weights in credit-oracle
        credit_oracle_client.apply_weights();

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
        gov_client.vote(&voter1, &proposal_id, &true, &300);
        gov_client.vote(&voter2, &proposal_id, &false, &200);

        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });

        gov_client.execute(&proposal_id);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.executed);

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
        let res = gov_client.try_vote(&voter, &proposal_id, &true, &0);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVoteWeight)));

        let res = gov_client.try_vote(&voter, &proposal_id, &true, &-10);
        assert_eq!(res, Err(Ok(GovernanceError::InvalidVoteWeight)));
    }
}
