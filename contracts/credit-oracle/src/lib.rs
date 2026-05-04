#![no_std]
use soroban_sdk::{contract, contractimpl, Env};

#[contract]
pub struct CreditOracle;

#[contractimpl]
impl CreditOracle {}

#[cfg(test)]
mod tests {}
