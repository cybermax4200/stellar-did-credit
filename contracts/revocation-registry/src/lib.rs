#![no_std]
use soroban_sdk::{contract, contractimpl, Env};

#[contract]
pub struct RevocationRegistry;

#[contractimpl]
impl RevocationRegistry {}

#[cfg(test)]
mod tests {}
