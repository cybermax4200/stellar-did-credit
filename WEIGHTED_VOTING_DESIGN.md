# Governance Weighted Voting Design

## Problem Statement

The current governance contract's `vote` function accepts a `vote_weight: i128` parameter that's caller-supplied without verification. There's no on-chain verification that the voter actually has that weight, meaning anyone can claim any weight and pass proposals unilaterally.

## Solution: Admin-Registered Voting Power

We will implement an admin-registered voting power system where the governance admin can register voters with specific voting weights. This provides proper Sybil resistance while being simple to implement and manage.

### Design Decisions

1. **Admin-Registered Weights**: The governance admin can register voters with specific voting weights
2. **Weight Verification**: The `vote` function will verify the caller's weight against their registered amount
3. **Vote Tracking**: Track how much weight each voter has already used in each proposal
4. **Error Handling**: Clear error messages for unauthorized voters and insufficient weight

### Data Structures

#### New Storage Keys
```rust
/// Registered voting weight for an address
VoterWeight(Address),
/// Amount of weight already used by voter in a specific proposal  
VoteWeightUsed(u64, Address),
```

#### New Error Types
```rust
/// Voter is not registered or has no voting weight
VoterNotRegistered = 13,
/// Vote weight exceeds voter's available balance
InsufficientVoteWeight = 14,
```

### New Functions

#### Admin Functions
```rust
/// Register a voter with specific voting weight
pub fn register_voter(env: Env, admin: Address, voter: Address, weight: i128) -> Result<(), GovernanceError>

/// Update a voter's weight (can increase or decrease)
pub fn update_voter_weight(env: Env, admin: Address, voter: Address, weight: i128) -> Result<(), GovernanceError>

/// Remove a voter's registration
pub fn deregister_voter(env: Env, admin: Address, voter: Address) -> Result<(), GovernanceError>
```

#### Query Functions
```rust
/// Get a voter's total registered weight
pub fn get_voter_weight(env: Env, voter: Address) -> Option<i128>

/// Get how much weight a voter has used in a specific proposal
pub fn get_vote_weight_used(env: Env, proposal_id: u64, voter: Address) -> i128

/// Get a voter's available weight for a proposal (total - used)
pub fn get_available_vote_weight(env: Env, proposal_id: u64, voter: Address) -> i128
```

### Modified Behavior

#### Vote Function Changes
1. **Weight Verification**: Check that voter is registered and has sufficient available weight
2. **Weight Deduction**: Track used weight per proposal to prevent over-voting
3. **Partial Voting**: Allow voters to cast multiple votes up to their total weight limit

```rust
pub fn vote(
    env: Env,
    voter: Address,
    proposal_id: u64,
    vote_for: bool,
    vote_weight: i128,
) -> Result<(), GovernanceError> {
    voter.require_auth();

    // Existing validations...
    
    // NEW: Verify voter is registered
    let total_weight = get_voter_weight(env.clone(), voter.clone())
        .ok_or(GovernanceError::VoterNotRegistered)?;
    
    // NEW: Check available weight
    let used_weight = get_vote_weight_used(env.clone(), proposal_id, voter.clone());
    let available_weight = total_weight - used_weight;
    
    if vote_weight > available_weight {
        return Err(GovernanceError::InsufficientVoteWeight);
    }
    
    // Record vote and update used weight...
}
```

## Implementation Plan

1. **Add new storage keys and error types**
2. **Implement voter registration functions**  
3. **Modify vote function to verify weights**
4. **Add query functions for weight tracking**
5. **Update tests to cover new functionality**
6. **Ensure CI passes with comprehensive test coverage**

## Test Cases

### Positive Cases
- Admin registers voter with weight 100, voter votes with weight 100 → success
- Voter with weight 100 votes with weight 60, then votes again with weight 40 → both succeed
- Multiple voters with different weights vote on same proposal → weights properly accumulated

### Negative Cases  
- Unregistered voter tries to vote → `VoterNotRegistered` error
- Voter tries to vote with weight exceeding their balance → `InsufficientVoteWeight` error
- Voter tries to vote with combined weight exceeding balance → `InsufficientVoteWeight` error
- Non-admin tries to register voter → `NotAuthorized` error

## Out of Scope
- Token contract integration (SEP-41)
- Delegation mechanisms  
- Vote locking or time-weighted voting
- Weight transfers between voters