#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol};

#[derive(Clone)]
#[contracttype]
pub struct ScoreRecord {
    pub score: u32,
    pub vc_count: u32,
    pub computed_at_ledger: u32,
    pub stale: bool,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Admin,
    IdentityOracle,
    Score(Address),
}

#[contract]
pub struct CreditOracleContract;

#[contractimpl]
impl CreditOracleContract {
    pub fn initialize(env: Env, admin: Address, identity_oracle: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::IdentityOracle, &identity_oracle);
    }

    pub fn compute_score(env: Env, subject: Address) -> ScoreRecord {
        let current_ledger = env.ledger().sequence();
        
        let record = ScoreRecord {
            score: 750,
            vc_count: 2,
            computed_at_ledger: current_ledger,
            stale: false,
        };

        env.storage().persistent().set(&DataKey::Score(subject.clone()), &record);
        record
    }

    pub fn get_score(env: Env, subject: Address) -> Option<ScoreRecord> {
        let mut record: ScoreRecord = env.storage().persistent().get(&DataKey::Score(subject.clone()))?;
        
        if let Some(identity_oracle_id) = env.storage().instance().get::<_, Address>(&DataKey::IdentityOracle) {
            let client = IdentityOracleClient::new(&env, &identity_oracle_id);
            let last_state_change = client.get_last_state_change_ledger(&subject);
            
            if record.computed_at_ledger < last_state_change {
                record.stale = true;
            }
        }

        Some(record)
    }
}

// Client stub for identity-oracle cross-contract calls
#[soroban_sdk::contractclient(contract_id = "identity_oracle")]
pub trait IdentityOracle {
    fn get_last_state_change_ledger(env: Env, subject: Address) -> u32;
}
