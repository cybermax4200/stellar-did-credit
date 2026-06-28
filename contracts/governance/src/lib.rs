#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror, symbol_short, Address, Env
};
use credit_oracle::{ScoringWeights, CreditOracleClient};

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
}

#[contracttype]
pub enum DataKey {
    Admin,
    CreditOracle,
    NextProposalId,
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
    pub executed: bool,
}

#[contract]
pub struct Governance;

#[contractimpl]
impl Governance {
    pub fn initialize(env: Env, admin: Address, credit_oracle: Address) -> Result<(), GovernanceError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(GovernanceError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::CreditOracle, &credit_oracle);
        env.storage().instance().set(&DataKey::NextProposalId, &1u64);
        Ok(())
    }

    pub fn create_proposal(
        env: Env,
        proposer: Address,
        weights: ScoringWeights,
        voting_period_ledgers: u32,
    ) -> Result<u64, GovernanceError> {
        proposer.require_auth();
        if weights.vc_weight + weights.tx_weight + weights.repayment_weight != 100 {
            return Err(GovernanceError::InvalidWeights);
        }

        let id: u64 = env.storage().instance().get(&DataKey::NextProposalId).unwrap_or(1);
        let expiry_ledger = env.ledger().sequence() + voting_period_ledgers;

        let proposal = GovernanceProposal {
            id,
            proposed_weights: weights,
            votes_for: 0,
            votes_against: 0,
            expiry_ledger,
            executed: false,
        };

        env.storage().persistent().set(&DataKey::Proposal(id), &proposal);
        env.storage().instance().set(&DataKey::NextProposalId, &(id + 1));

        env.events().publish(
            (symbol_short!("PropCreat"), id),
            (proposer, expiry_ledger),
        );

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

        if proposal.executed {
            return Err(GovernanceError::ProposalAlreadyExecuted);
        }

        if proposal.votes_for > proposal.votes_against {
            let credit_oracle_addr: Address = env
                .storage()
                .instance()
                .get(&DataKey::CreditOracle)
                .expect("no credit oracle");

            let client = CreditOracleClient::new(&env, &credit_oracle_addr);
            client.update_weights(&proposal.proposed_weights);
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
        env.storage().persistent().get(&DataKey::Proposal(proposal_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::{Address as _, Ledger}, Env};
    use credit_oracle::{CreditOracle, CreditOracleClient};

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
        gov_client.initialize(&admin, &credit_oracle_id);

        // Propose governance contract as new admin of credit oracle
        credit_oracle_client.propose_new_admin(&admin, &gov_id);

        // Accept oracle admin from governance
        gov_client.accept_oracle_admin();

        // Create a proposal
        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };

        let proposer = Address::generate(&env);
        let proposal_id = gov_client.create_proposal(&proposer, &proposed_weights, &100);
        assert_eq!(proposal_id, 1);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.expiry_ledger, env.ledger().sequence() + 100);
        assert_eq!(proposal.executed, false);

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
            l.sequence_number = l.sequence_number + 101;
        });

        // Execute proposal
        gov_client.execute(&proposal_id);

        let proposal = gov_client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.executed, true);

        // Verify credit oracle weights updated
        let active_weights = credit_oracle_client.get_scoring_weights();
        assert_eq!(active_weights.vc_weight, 50);
        assert_eq!(active_weights.tx_weight, 20);
        assert_eq!(active_weights.repayment_weight, 30);
    }
}
