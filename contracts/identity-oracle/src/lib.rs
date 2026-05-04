#![no_std]
use soroban_sdk::{contract, contractimpl, Env};

#[contract]
pub struct IdentityOracle;

#[contractimpl]
impl IdentityOracle {}

#[cfg(test)]
mod tests {}
