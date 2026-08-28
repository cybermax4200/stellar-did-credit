# Governance Contract

## 1. Contract Overview & Purpose

The governance contract provides on-chain proposal creation, weighted voting, and multi-step execution for updating the credit-oracle's scoring weights. A successful proposal does not change weights immediately — it queues them through the credit-oracle's own 24-hour timelock, giving the community two separate reaction windows before any weight change takes effect.

**Important note on naming:** No separate `governor` role exists in the credit-oracle contract today. Older test snapshot filenames in the repository and sections of `WEIGHTED_VOTING_DESIGN.md` reference a governor-registration flow that was never merged into the deployed credit-oracle code. The actual mechanism by which governance controls the credit-oracle is that the governance contract *itself becomes the stored admin address* of the credit-oracle via the two-step admin-transfer flow (`propose_new_admin` + `accept_admin`). There is no whitelist of "governors" on the credit-oracle side — only a single admin, which after setup is the governance contract address. This distinction matters for security analysis: compromising the governance contract's admin is equivalent to compromising the credit-oracle's admin.

**Current design decisions (not a tokenized DAO):**

- Voting power is assigned by the governance contract's admin via `register_voter`. This is a permissioned model suitable for an initial trusted set of stewards while the protocol stabilizes on testnet.
- Token-based voting (e.g., SEP-41 token balance snapshot at proposal creation) is explicitly out of scope for the current implementation. See `WEIGHTED_VOTING_DESIGN.md` in the repository root for the original design rationale and out-of-scope items.
- Each voter may cast multiple partial votes on a single proposal up to their registered weight, and weights are tracked per-proposal so the same voter can vote fully on concurrent proposals without conflict.

Related reading:

- [Architecture Guide](architecture.md) — ADR-003 (governance respects credit-oracle timelock) and the admin-transfer two-step flow.
- [Epoch Model](EPOCH_MODEL.md) — Instance TTL requirements, weight timelock semantics, and the relationship between admin-gated calls and contract liveness.
- [Scoring Specification](scoring-spec.md) — What the three scoring weights (vc, tx, repayment) actually control in the credit score formula.

---

## 2. Core Concepts

### 2.1 `GovernanceProposal`

Every proposal is a persisted struct with these fields (see `DataKey::Proposal(u64)` in the contract):

| Field | Type | Meaning |
|---|---|---|
| `id` | `u64` | Monotonically assigned at creation, starting from 1. |
| `proposed_weights` | `ScoringWeights` | The weights that will be queued if the proposal passes. Must sum to exactly 100 and each component weight must be at least 10 (`MIN_COMPONENT_WEIGHT`). |
| `votes_for` | `i128` | Accumulated registered voting weight cast in favor. Saturating add. |
| `votes_against` | `i128` | Accumulated registered voting weight cast against. Saturating add. |
| `expiry_ledger` | `u32` | Ledger sequence after which `vote` is rejected (`ProposalExpired`). Set to `current_sequence + voting_period_ledgers` at creation. |
| `execution_delay_ledgers` | `u32` | Extra ledgers *after* `expiry_ledger` that must pass before `execute` is allowed. Gives a reaction window independently of the credit-oracle timelock. |
| `executed` | `bool` | Set to `true` the first time `execute` is called successfully; subsequent calls return `ProposalAlreadyExecuted`. |
| `quorum_required` | `i128` | Snapshot of `QuorumRequired` at proposal creation. A later `set_quorum` by the admin never changes the quorum of a proposal that is already up for a vote. Threshold is `votes_for + votes_against >= quorum_required`, checked at `execute`. |

### 2.2 The Double Timelock Model

Weight changes from a passing proposal are gated by **two sequential waiting periods**, enforced in **different contracts**, and require **two separate transactions** to complete. This is the Issue #8 behavior described as it actually works in the current code — several internal unit-test assertions assume the second timelock does not exist and are inconsistent with the real contract behavior (see §2.3).

| # | Timelock | Where enforced | Duration | Gates |
|---|---|---|---|---|
| 1 | Governance execution delay | Governance proposal field | Per-proposal, caller-chosen at `create_proposal` time via `execution_delay_ledgers`. Can be zero. | `governance.execute(proposal_id)` |
| 2 | Credit-oracle weight timelock | `TIMELOCK_LEDGERS = 17_280` constant inside credit-oracle | Fixed at approximately 24 hours (17,280 ledgers at 5-second cadence). | `credit-oracle.apply_weights()`, which is called via `governance.apply_weights()` |

Full end-to-end timeline from proposal creation to active weights:

```
create_proposal(id=1, weights=W2, voting_period=VP, execution_delay=ED)
  │
  ├─ current_sequence ─────────────────► proposal.expiry_ledger = seq + VP
  │                                                        │
  │  Voting window: vote() accepted during this range     │
  │                                                        ▼
  │                                                   seq == expiry + 1  ──► vote() now returns ProposalExpired
  │                                                        │
  │  Governance execution-delay window                     ▼
  │                                                   seq == expiry + ED + 1 ──► execute() now allowed
  │                                                        │
  └─ execute(id=1)                                         │
         └─ calls credit_oracle.propose_weights(W2)        ▼
                sets pending_weights.effective_ledger = seq + 17_280
                                                           │
                                                           ▼
                                                 seq >= effective_ledger
                                                           │
                                                           ▼
                                    apply_weights()  ──►  W2 now active in get_scoring_weights()
```

Minimum total wall-clock time from proposal creation to active weights (with `VP=0` and `ED=0` and a 5-second ledger cadence):

```
17_280 ledgers × 5 seconds = 86_400 seconds = 24 hours
```

Any non-zero `voting_period_ledgers` or `execution_delay_ledgers` adds on top of that baseline.

### 2.3 Known Test Inconsistency (Issue #8)

The unit tests `test_execution_timelock_delays_after_voting_ends` inside `contracts/governance/src/lib.rs` and `test_governance_execution_timelock_integration` inside `contracts/tests/src/integration_test.rs` call `execute()` and then immediately read `get_scoring_weights()` to assert the new values, **without** waiting the additional 17,280 ledgers or calling `apply_weights()`. These assertions are inconsistent with the real contract behavior described in ADR-003 and implemented in both `governance.execute` (which calls `propose_weights`, not `apply_weights`) and `credit-oracle.apply_weights` (which rejects with `TimelockNotExpired` before `effective_ledger`).

The accurate working test is `test_governance_proposal_creation_voting_and_execution` in the governance contract's own tests, which correctly:

1. Calls `execute`, checks `get_pending_weights().is_some()` on the credit-oracle, checks active weights are **still default**.
2. Advances 17,282 ledgers, extends instance TTL on both contracts.
3. Calls `governance.apply_weights()`.
4. Only then checks `get_scoring_weights()` equals the proposal's values.

When reading tests, prefer the pattern in that test as the canonical flow. The inconsistent tests are a known documentation debt and should not be cited as evidence that weights activate immediately after `execute`.

### 2.4 Voter-Weight Accounting

- A voter has one global registered weight set by the admin (`DataKey::VoterWeight(Address)`).
- A separate per-proposal-per-voter counter tracks how much of that weight has been used (`DataKey::VoteWeightUsed(u64, Address)`).
- The available weight for a given voter on a given proposal is `total_weight - used_weight[proposal][voter]`.
- `vote_weight <= 0` returns `InvalidVoteWeight`.
- `vote_weight > available_weight` returns `InsufficientVoteWeight`.
- Multiple partial votes across different `vote` transactions are allowed, as long as the sum for a voter on a proposal does not exceed their registered total.
- Weights are NOT locked across proposals: the same registered total is available independently per proposal (because `VoteWeightUsed` is keyed by `(proposal_id, Address)`). A voter with weight 100 can vote 100 FOR on proposal 1 and 100 AGAINST on proposal 2 concurrently.

### 2.5 The `cancel()` Function Is a Stub

The public `cancel(env, canceller, proposal_id, reason)` entrypoint:

- Requires `canceller.require_auth()` (any signer is accepted — the contract does **not** check that canceller is admin, proposer, or any specific role).
- Emits a `PropCanc` event with the canceller address and optional reason string.
- Does **not** set any flag on the `GovernanceProposal` struct.
- Does **not** prevent subsequent `vote` calls.
- Does **not** prevent subsequent `execute` calls.
- Does **not** refund or reset used vote weights.

It is currently an off-chain signaling hook only. Operators should not rely on it for on-chain cancellation guarantees.

---

## 3. Function Reference with Worked Examples

All examples use the Soroban SDK test-style `Address::generate` and client pattern shown in the contract's own tests. On-chain calls via CLI or the TypeScript SDK are structurally identical: same function names, same argument order, same auth requirements.

Common setup shared by all examples below:

```
admin_addr        = GADMIN...  (initial governance admin)
credit_oracle_id  = CCREDIT... (deployed credit-oracle, already initialized with admin_addr)
gov_id            = CGOV...    (deployed governance, not yet initialized)
proposer_addr     = GPROP...
voter_A_addr      = GVA...     (to be registered with weight 1000)
voter_B_addr      = GVB...     (to be registered with weight 400)
```

### 3.1 `initialize`

**Signature:**
```rust
pub fn initialize(
    env: Env,
    admin: Address,
    credit_oracle: Address,
    quorum_required: i128,
) -> Result<(), GovernanceError>
```

**Auth:** `admin.require_auth()`.

**Errors:**
- `AlreadyInitialized` if called more than once.
- `InvalidQuorum` if `quorum_required <= 0`.

**Worked example — complete setup including two-step oracle admin transfer:**

```rust
// 1. credit-oracle was already deployed and initialized:
credit_oracle_client.initialize(&admin_addr);
//    Stores DataKey::Admin = admin_addr in credit-oracle instance storage.

// 2. Initialize governance, pointing at the credit-oracle and setting
//    a default quorum of 1000 total vote-weight across all voters.
gov_client.initialize(&admin_addr, &credit_oracle_id, &1000);
//    - Stores DataKey::Admin = admin_addr
//    - Stores DataKey::CreditOracle = credit_oracle_id
//    - Stores DataKey::NextProposalId = 1
//    - Stores DataKey::QuorumRequired = 1000

// 3. Hand the credit-oracle's admin role to the governance contract address.
//    Step A — the current credit-oracle admin (admin_addr) proposes governance:
credit_oracle_client.propose_new_admin(&gov_id);
//    Stores DataKey::PendingAdmin = gov_id (credit-oracle side).
//    admin_addr still retains full admin authority until step B runs.

// 4. Step B — governance admin triggers accept_oracle_admin:
gov_client.accept_oracle_admin();
//    Internally calls credit_oracle.accept_admin(&gov_id).
//    credit-oracle side:
//      - reads PendingAdmin, checks it equals gov_id
//      - writes DataKey::Admin = gov_id
//      - removes DataKey::PendingAdmin
//
//    After this call: the governance contract address is the sole admin of the
//    credit-oracle. Only calls that originate from governance (cross-contract
//    with caller = gov_id) will pass the require_auth() checks inside
//    credit_oracle.propose_weights / register_feeder / upgrade / etc.
```

**Post-state sanity checks:**
- `credit_oracle.get_admin()`-equivalent behavior: `propose_weights` requires the caller (or cross-contract caller) to equal the stored admin; a direct call from `admin_addr` now fails with `NotAuthorized` because admin is now `gov_id`.
- `governance.get_quorum()` returns `1000`.

### 3.2 `create_proposal`

**Signature:**
```rust
pub fn create_proposal(
    env: Env,
    proposer: Address,
    weights: ScoringWeights,
    voting_period_ledgers: u32,
    execution_delay_ledgers: u32,
) -> Result<u64, GovernanceError>
```

**Auth:** `proposer.require_auth()`. The proposer does not need to be a registered voter; any signer can create a proposal.

**Errors:**
- `InvalidWeights` if `vc_weight + tx_weight + repayment_weight != 100`.

**Worked example — create a proposal shifting weights from tx to VC:**

Default credit-oracle weights after initialization are `(vc=40, tx=30, repayment=30)`. Propose `(50, 20, 30)`, with a voting period of 14,400 ledgers (≈20 hours) and an execution delay of 1,000 ledgers (≈1.4 hours).

```rust
let new_weights = ScoringWeights {
    vc_weight:        50,
    tx_weight:        20,
    repayment_weight: 30,
};

let proposal_id: u64 = gov_client.create_proposal(
    &proposer_addr,
    &new_weights,
    &14_400,    // voting_period_ledgers — ~20h at 5s/ledger
    &1_000,     // execution_delay_ledgers — extra reaction window after voting
);
// assert_eq!(proposal_id, 1);   // NextProposalId started at 1

let p = gov_client.get_proposal(&proposal_id).unwrap();
assert_eq!(p.id, 1);
assert_eq!(p.proposed_weights.vc_weight, 50);
assert_eq!(p.votes_for, 0);
assert_eq!(p.votes_against, 0);
assert_eq!(p.execution_delay_ledgers, 1_000);
assert_eq!(p.quorum_required, 1_000);   // snapshotted from default at creation
assert!(!p.executed);
// p.expiry_ledger == env.ledger().sequence() + 14_400
```

Events: `PropCreat(u64 id)` with data `(proposer, expiry_ledger)`.

### 3.3 `vote`

**Signature:**
```rust
pub fn vote(
    env: Env,
    voter: Address,
    proposal_id: u64,
    vote_for: bool,
    vote_weight: i128,
) -> Result<(), GovernanceError>
```

**Auth:** `voter.require_auth()`.

**Errors:**
- `InvalidVoteWeight` if `vote_weight <= 0`.
- `VoterNotRegistered` if `DataKey::VoterWeight(voter)` is missing.
- `InsufficientVoteWeight` if `vote_weight > (total_weight - already_used_on_this_proposal)`.
- `ProposalNotFound` if `DataKey::Proposal(proposal_id)` is missing.
- `ProposalExpired` if `env.ledger().sequence() > proposal.expiry_ledger`.
- `ProposalAlreadyExecuted` if `proposal.executed == true`.

**Worked example — two voters, split vote, quorum exactly met:**

```rust
// Admin registers voters before the vote window ends.
// (Registration can happen at any time — even mid-vote or after a proposal
//  is created. The check is performed inside vote(), not create_proposal().)
gov_client.register_voter(&admin_addr, &voter_A_addr, &1000);
gov_client.register_voter(&admin_addr, &voter_B_addr, &400);

// Voter A votes FOR with all 1000 weight in a single call.
gov_client.vote(&voter_A_addr, &proposal_id, &true,  &1000);

// Voter B votes AGAINST, splitting into two partial votes (60 + 340).
gov_client.vote(&voter_B_addr, &proposal_id, &false, &60);
// used_weight[(proposal_id, voter_B)] = 60; available = 400 - 60 = 340
gov_client.vote(&voter_B_addr, &proposal_id, &false, &340);
// used_weight[(proposal_id, voter_B)] = 400; available = 0

// Third vote by voter B would fail:
//   gov_client.try_vote(&voter_B_addr, &proposal_id, &false, &1)
//   → Err(Ok(GovernanceError::InsufficientVoteWeight))

let p = gov_client.get_proposal(&proposal_id).unwrap();
assert_eq!(p.votes_for,     1000);
assert_eq!(p.votes_against, 400);
// votes_for + votes_against = 1400 >= quorum_required (1000) ✓
// votes_for (1000) > votes_against (400)                  ✓ → will pass
```

Events: `Voted(u64 proposal_id)` with data `(voter, vote_for, vote_weight)` — one event per `vote` call, even for partial votes on the same proposal.

### 3.4 `execute`

**Signature:**
```rust
pub fn execute(env: Env, proposal_id: u64) -> Result<(), GovernanceError>
```

**Auth:** none — permissionless. Any caller can trigger execute after the gates pass. The effect (changing weights) is gated by the conditions below, not by signer.

**Errors (checked in order):**
- `ProposalNotFound`
- `ProposalNotExpired` if `sequence <= proposal.expiry_ledger`. (Voting period still open.)
- `TimelockNotExpired` if `sequence <= proposal.expiry_ledger + execution_delay_ledgers`. (Governance's own reaction window.)
- `ProposalAlreadyExecuted`
- `QuorumNotMet` if `proposal.votes_for + proposal.votes_against < proposal.quorum_required`.

**Worked example — step 1 of 2 toward active weights:**

Continuing the proposal above. `expiry_ledger = seq_at_creation + 14_400`; `execution_delay = 1_000`.

```rust
// Attempt 1 — during voting period (before expiry_ledger):
let res = gov_client.try_execute(&proposal_id);
assert_eq!(res, Err(Ok(GovernanceError::ProposalNotExpired)));

// Advance the ledger past voting AND past the governance execution delay:
env.ledger().with_mut(|l| l.sequence_number += 14_400 + 1_000 + 1);
// Now sequence > expiry_ledger AND sequence > expiry_ledger + execution_delay

// Attempt 2 — valid execute:
gov_client.execute(&proposal_id);
//
// Inside execute:
//   1. quorum check passes: 1400 >= 1000
//   2. votes_for (1000) > votes_against (400) → branch into apply path
//   3. loads credit_oracle_id from instance storage
//   4. CreditOracleClient::new(&env, &credit_oracle_id)
//   5. ⚠️  calls .propose_weights(&proposal.proposed_weights)
//        NOT apply_weights.
//        This STARTS the credit-oracle 17_280-ledger timelock.
//        It does NOT change get_scoring_weights() return value.
//   6. proposal.executed = true, stored.

let p = gov_client.get_proposal(&proposal_id).unwrap();
assert!(p.executed);

// CRITICAL: active weights on credit-oracle are STILL the old values.
let still_active = credit_oracle_client.get_scoring_weights();
assert_eq!(still_active.vc_weight,        40);   // unchanged default
assert_eq!(still_active.tx_weight,        30);
assert_eq!(still_active.repayment_weight, 30);

// The proposal values are now visible via get_pending_weights on the oracle:
let pending = credit_oracle_client.get_pending_weights().unwrap();
assert_eq!(pending.weights.vc_weight,        50);
assert_eq!(pending.weights.tx_weight,        20);
assert_eq!(pending.weights.repayment_weight, 30);
// pending.effective_ledger = sequence_now + 17_280
```

**What happens when a proposal fails the vote but quorum was met?** If `votes_for <= votes_against`, `execute` skips the `propose_weights` call but still sets `executed = true` and emits the event. The proposal is finalized as rejected; no weights change, no credit-oracle timelock is started.

**What happens if quorum is not met?** `execute` returns `QuorumNotMet`, does NOT set `executed = true`. The proposal remains open for (re-)execution in the future — but there is no way to add votes, since `vote` rejects after `expiry_ledger`. In effect, a proposal that fails quorum is permanently stuck unexecuted. The `cancel()` stub cannot help here (see §2.5).

Events: `PropExec(u64 id)` with data `(votes_for, votes_against)`.

### 3.5 `apply_weights`

**Signature:**
```rust
pub fn apply_weights(env: Env) -> Result<(), GovernanceError>
```

**Auth:** none — permissionless. Any caller can finalize the credit-oracle's pending weights after its timelock expires. There is no `proposal_id` argument: there can be at most one set of pending weights in the credit-oracle at any given time, because `credit-oracle.propose_weights` overwrites `PendingWeights` and `PendingWeightsEffectiveLedger` each call.

**Errors:**
- `TimelockNotExpired` if the credit-oracle's 17,280-ledger timelock has not yet elapsed. The relay maps the credit-oracle's typed `CreditOracleError::TimelockNotExpired` to its own `GovernanceError::TimelockNotExpired`.
- `NoPendingWeights` if nothing was proposed (`PendingWeightsEffectiveLedger` is unset in the credit-oracle), or if the credit-oracle invocation failed for an unexpected reason.
- `ContractPaused` if the credit-oracle is paused and refuses the write.
- `NotAuthorized` if the `CreditOracle` storage key is missing — returned only when governance was never initialized with a credit-oracle address.

Previously these situations surfaced as raw panics inside the credit-oracle (`panic!("timelock not expired")`, `.expect("no pending weights")`), which governance tooling could not distinguish or handle. `apply_weights` now propagates typed errors only.

**Worked example — step 2 of 2, weights finally go active:**

```rust
// 17,280 ledgers ≈ 24 hours. Add a small safety margin of 2.
let jump = 17_280 + 2;

// ⚠️ Instance-TTL housekeeping — before jumping the ledger far ahead,
// extend instance storage TTL on BOTH contracts so the storage entries
// (PendingWeightsEffectiveLedger in credit-oracle, CreditOracle key in
// governance) don't get archived during the jump.
env.as_contract(&credit_oracle_id, || {
    env.storage().instance().extend_ttl(jump, jump);
});
env.as_contract(&gov_id, || {
    env.storage().instance().extend_ttl(jump, jump);
});

env.ledger().with_mut(|l| l.sequence_number += jump);

// Now anyone can call apply_weights:
gov_client.apply_weights();

let now_active = credit_oracle_client.get_scoring_weights();
assert_eq!(now_active.vc_weight,        50);
assert_eq!(now_active.tx_weight,        20);
assert_eq!(now_active.repayment_weight, 30);

// Pending state in credit-oracle has been cleared:
assert!(credit_oracle_client.get_pending_weights().is_none());
```

Events: `WtApplied` with data `(env.ledger().sequence())`.

### 3.6 Admin-only Config Functions

All three require `admin.require_auth()` AND verify the passed `admin` argument equals the stored `DataKey::Admin` (i.e., you can't pass an arbitrary authorized address — it has to be the actual stored admin).

| Function | Signature | Errors |
|---|---|---|
| `set_quorum` | `(env, admin, quorum_required: i128) -> Result<()>` | `InvalidQuorum` if <= 0; `NotAuthorized` if caller mismatch. |
| `register_voter` | `(env, admin, voter: Address, weight: i128) -> Result<()>` | `InvalidVoteWeight` if weight <= 0; `NotAuthorized`. |
| `update_voter_weight` | `(env, admin, voter: Address, weight: i128) -> Result<()>` | `InvalidVoteWeight` if weight < 0; `NotAuthorized`. Passing weight = 0 deletes the VoterWeight key (same as deregister). |
| `deregister_voter` | `(env, admin, voter: Address) -> Result<()>` | `NotAuthorized`. |
| `accept_oracle_admin` | `(env) -> Result<()>` | `NotAuthorized` if gov admin missing or credit-oracle missing. Requires admin signer. |

Events:
- `VoterReg(voter_addr)` with data `weight`
- `VoterUpd(voter_addr)` with data `new_weight`
- `VoterDer(voter_addr)` with unit data

**Note on updating voter weights mid-proposal:** If the admin calls `update_voter_weight` for a voter who has already partially voted on a proposal, the registered-total check in the next `vote` call uses the *new* total, but the used-weight counter is already persisted. This means:

- Lowering a voter's weight from 1000 to 200 after they already voted 600 on proposal 1 → subsequent `vote` calls by that voter on that proposal will compute `available = 200 - 600 = negative (saturates to 0 via i128 subtraction semantics that underflow in release — in practice, any further `vote` returns `InsufficientVoteWeight` because `vote_weight > available`). Already-cast votes (600) remain counted.
- Raising a voter's weight after a partial vote grants them new available headroom for the same proposal.

Operators are advised against adjusting voter weights while proposals they have voted on are still open.

### 3.7 Read-only Queries

No auth, no side effects.

| Function | Signature | Returns |
|---|---|---|
| `get_quorum` | `(env) -> i128` | Default quorum for future proposals. Returns 0 if never set (shouldn't happen post-initialize). |
| `get_proposal` | `(env, proposal_id: u64) -> Option<GovernanceProposal>` | Full `GovernanceProposal` struct copy, or `None`. |
| `get_voter_weight` | `(env, voter: Address) -> Option<i128>` | Registered total weight, or `None` if not registered. |
| `get_vote_weight_used` | `(env, proposal_id: u64, voter: Address) -> i128` | Used weight on a specific proposal. 0 if never voted. |
| `get_available_vote_weight` | `(env, proposal_id: u64, voter: Address) -> i128` | `total_weight - used_weight`. 0 if not registered (total defaults to 0). |
| `list_proposals` | `(env, from_id: u64, limit: u32, include_inactive: bool) -> Vec<GovernanceProposal>` | Enumerates up to `limit` (max 20) proposals starting from `from_id`. Skips non-existent proposals and filters inactive (cancelled/executed) proposals unless `include_inactive` is true. Returns empty vector if `from_id >= NextProposalId`. |

---

## 4. Integration Guide

### 4.1 Full Deployment Sequence

Order is strict because governance initialization needs the credit-oracle's address, and the admin-transfer cannot run before both are initialized.

```
Step  Contract              Caller         Action
────  ────────────────────  ─────────────  ─────────────────────────────────────────────────────────
 1    credit-oracle         deployer       deploy WASM, get contract id CC...
 2    credit-oracle         deployer       initialize(initial_admin_addr)
 3    governance            deployer       deploy WASM, get contract id CG...
 4    governance            initial_admin  initialize(initial_admin, CC, quorum_default)
 5    credit-oracle         initial_admin  propose_new_admin(CG)        # admin transfer step A
 6    governance            initial_admin  accept_oracle_admin()         # admin transfer step B
 7    governance            initial_admin  register_voter(admin, v1, w1)
 8    governance            initial_admin  register_voter(admin, v2, w2)
 9    governance            initial_admin  ...
```

At the end of step 6, confirm the transfer by attempting a credit-oracle admin-gated call from outside governance (it should fail with `NotAuthorized`) and from inside governance (it should succeed, as demonstrated by the `accept_oracle_admin` unit test calling `propose_weights` inside `execute`).

### 4.2 Lifecycle Checklist for a Proposal

For operators running off-chain tooling or an indexer:

1. **Create.** `create_proposal` → record the returned `proposal_id` and the `expiry_ledger` from the `PropCreat` event.
2. **Register voters** (before expiry). Any time is fine, but after expiry `vote` fails regardless of registration status.
3. **Monitor voting.** Call `get_proposal(id)` and the `Voted` events. Track `votes_for`, `votes_against`, and whether `(for + against) >= quorum_required`.
4. **Wait two gates.** Do not attempt `execute` until `sequence > expiry_ledger + execution_delay_ledgers`. Use `ProposalNotExpired` / `TimelockNotExpired` as machine-checkable error codes for retry logic.
5. **Call `execute`.** Anyone can call it. After success, check `credit_oracle.get_pending_weights()` — if the vote passed, the record should now exist; if failed or quorum-not-met, nothing is pending.
6. **Wait the credit-oracle timelock.** Roughly 24 hours. `credit_oracle.get_pending_weights().unwrap().effective_ledger` gives the exact ledger number.
7. **Call `apply_weights`.** Anyone can call it. After success, the new weights are visible via `credit_oracle.get_scoring_weights()`.
8. **Housekeeping.** Periodically call any admin-gated governance function (e.g., a no-op-like `set_quorum` with the same value, or registering a voter that's already registered — the latter writes storage and bumps instance TTL) to ensure the governance contract's instance storage TTL stays healthy. Instance TTL is not extended by `vote`, `execute`, or `apply_weights` in the current code because those functions do not write to instance storage; they write to persistent storage. Persistent storage has its own Soroban-managed default TTL.

### 4.3 Event Topics for Indexing

All events use Soroban's `(topics_tuple, data)` shape with the first topic element being a short symbol:

| Topic 1 (symbol) | Extra topic | Data tuple | Emitted by |
|---|---|---|---|
| `PropCreat` | `proposal_id: u64` | `(proposer: Address, expiry_ledger: u32)` | `create_proposal` |
| `Voted` | `proposal_id: u64` | `(voter: Address, vote_for: bool, vote_weight: i128)` | `vote` |
| `PropExec` | `proposal_id: u64` | `(votes_for: i128, votes_against: i128)` | `execute` |
| `WtApplied` | — | `(ledger_sequence_when_applied: u32)` | `apply_weights` |
| `VoterReg` | `voter: Address` | `weight: i128` | `register_voter` |
| `VoterUpd` | `voter: Address` | `weight: i128` | `update_voter_weight` |
| `VoterDer` | `voter: Address` | `()` (unit) | `deregister_voter` |
| `PropCanc` | `proposal_id: u64` | `(canceller: Address, reason: Option<String>)` | `cancel` (stub) |

Also subscribe to credit-oracle events for the downstream weight-change steps:

- Credit-oracle `WtProp` with data `(vc, tx, repayment, effective_ledger)` emitted by `propose_weights` (called from governance `execute`).
- Credit-oracle `WtApply` with data `(vc, tx, repayment)` emitted by `apply_weights`.

### 4.4 TypeScript SDK

The SDK exposes `GovernanceClient` as `sdk.governance` when
`ProtocolConfig.governanceId` is configured. It provides signed helpers for
`createProposal`, `vote`, `execute`, and `applyWeights`, plus read-only
`getProposal` and `listProposals` helpers. See
[`packages/sdk/README.md`](../packages/sdk/README.md#governance) for the
TypeScript API and examples.

`listProposals` scans the contract's monotonically increasing proposal IDs
because the governance contract exposes `get_proposal` but does not expose a
bulk list entrypoint. Its `fromId` and `limit` arguments therefore bound the
number of read-only simulations performed by the client.

---

## 5. Security Considerations

### 5.1 Admin Is a Single Point of Centralization

- The governance `admin` registers/deregisters voters, sets voter weights, sets the default quorum, and triggers `accept_oracle_admin`.
- After `accept_oracle_admin` runs, the governance **contract address** is itself the credit-oracle admin. This means governance admin = credit-oracle admin (transitive).
- A malicious or compromised governance admin can:
  - Register themselves (or sybil addresses) with arbitrarily high vote weight and push through any quorum.
  - Deregister all opposing voters.
  - Change the default quorum to 1 for future proposals.
  - Call `credit_oracle.upgrade(admin, new_wasm_hash)` via a cross-contract call from their admin position (they would need to add that call path — the current governance contract does not expose an `upgrade` passthrough, but the admin of the governance contract can deploy an upgrade TO governance itself through the normal Soroban upgrade flow if they have the wasm hash, and governance-as-credit-oracle-admin can thereafter call `credit_oracle.upgrade`).
- This model is appropriate for testnet / initial trusted-steward deployment. A production DAO model requires token-weighted voting and an admin multisig or timelock on the governance admin itself.

### 5.2 No On-Chain Cancellation

`cancel()` emits an event but does not actually prevent `vote` or `execute` from running. Operators relying on a cancellation signal today must enforce it in the off-chain layer (e.g., by refusing to call `execute`). An attacker who can front-run the canceler can still vote and execute before any off-chain coordination completes.

### 5.3 Double-Timelock and Reaction Windows

- Reaction window #1 (governance `execution_delay_ledgers`) starts when voting closes. Operators should set this to a non-zero value for mainnet proposals so the community can review the outcome of a contentious vote before `execute` queues the weights.
- Reaction window #2 (credit-oracle 17,280 ledgers) is fixed and unavoidable even with `execution_delay_ledgers = 0`. Users and integrators can inspect `credit-oracle.get_pending_weights()` during this window and, if disagreeing, exit dependent positions.
- There is NO `update_weights` bypass in the current credit-oracle code. Earlier documentation (`docs/EPOCH_MODEL.md` §5.2 and §8 table) mentions an `update_weights` admin bypass that was never merged into `credit-oracle/src/lib.rs`. The code only exposes `propose_weights` → `apply_weights`. All weight changes, even admin-initiated, must wait the 17,280-ledger timelock. (The admin of the credit-oracle — i.e., the governance contract — can always deploy a new credit-oracle WASM via `upgrade`, which changes behavior without going through the weight timelock. This is an orthogonal escape hatch.)

### 5.4 Instance TTL and Lost Pending Proposals

- If no admin-gated call is made on governance for longer than the instance TTL window (~30 days with the default pattern used in the other three contracts — see `EPOCH_MODEL.md`), the governance contract's instance storage could be archived. If that happens before `apply_weights` is called, the `CreditOracle` instance key is lost. The pending weights inside the credit-oracle are **still there** (they live in credit-oracle's instance storage, managed by credit-oracle admin-gated calls), so an operator can call `credit-oracle.apply_weights()` directly without going through governance. The governance contract being archived does not trap pending weights — but it does mean future proposals cannot be created until governance is redeployed and re-accepted as credit-oracle admin.
- Moral: ensure at least one admin-gated call runs on governance every ~30 days. A cronned `set_quorum(admin, current_quorum_value)` (no-op write) suffices.

### 5.5 Proposal ID and Creation Order

Proposal IDs are intentionally **1-based**. `NextProposalId` is initialized to
`1`, so the first call to `create_proposal` returns ID `1`; each subsequent call
increments the ID by `1`. ID `0` is unused and should not be queried or used as
the starting point for off-chain iteration.

The counter is stored in instance storage (not persistent), but the actual
proposals are stored in persistent storage keyed by ID. Proposals can be
enumerated on-chain using `list_proposals(from_id, limit, include_inactive)`;
callers that want all proposals should start with `from_id = 1`. Off-chain
indexers can also track `PropCreat` events, whose proposal ID is the canonical
identifier.

This convention is part of the deployed contract interface. Changing the first
ID to `0` would require coordinating a migration for existing deployments and
updating every indexer, SDK, and integration that consumes proposal IDs.

### 5.6 No Proposal Expiry Cleanup

Proposals whose quorum fails at `execute` time are left in state with `executed = false`. They will never become executable (since votes can't be added after `expiry_ledger`), but they also aren't pruned. Storage for failed proposals persists for the persistent-storage TTL lifetime.

### 5.7 `saturating_add` on Vote Totals

Vote totals (`votes_for`, `votes_against`) use `saturating_add` when accumulating. If a voter's weight is large enough that multiple votes reach `i128::MAX`, further votes silently stop adding to the counter. With admin-registered voter weights (not token supply based), reaching `i128::MAX` is practically impossible; the saturation is a defense-in-depth against overflow panics in release builds with overflow-checks off. (Note: the workspace Cargo.toml sets `overflow-checks = true` for release profiles, so the saturation is belt-and-suspenders.)

### 5.8 Two-Step Oracle Admin Transfer Liveness

If the deployer calls `credit_oracle.propose_new_admin(gov_id)` in step 5A of the deployment sequence but step 6 (`accept_oracle_admin`) is never run:

- The credit-oracle's stored admin is still the original deployer address (unchanged until `accept_admin` runs).
- `PendingAdmin = gov_id` is left dangling.
- The original admin can overwrite it by calling `propose_new_admin(some_other_addr)` again (overwrites `PendingAdmin` in place).
- Governance's `execute` would fail inside `propose_weights` because the governance contract is not yet the stored admin.

Test before mainnet: after step 6, try `credit_oracle.register_feeder(initial_admin, feeder)`. It should return `NotAuthorized` (authority was transferred away). If it succeeds, the transfer didn't happen.

---

## 6. Governance ↔ Credit-Oracle Admin Relationship

Repeating the key point from §1 for visibility: **there is no separate governor role or register_governor function in the credit-oracle today**, despite older test snapshot filenames and the `WEIGHTED_VOTING_DESIGN.md` document using that language.

The actual control path is:

```
   [Stored Admin: Address]   ← single admin slot in credit-oracle instance storage
              │
              │ after setup: equals the governance contract address
              ▼
   credit_oracle.propose_weights()
   credit_oracle.register_feeder()
   credit_oracle.register_lender()
   credit_oracle.set_identity_oracle()
   credit_oracle.propose_new_admin()
   credit_oracle.upgrade()
     → all require stored_admin.require_auth()
```

How governance satisfies `stored_admin.require_auth()` in cross-contract calls: Soroban's auth model considers the calling contract address to be the authenticated principal when a contract invokes another contract (as opposed to the top-level EOA signer). When `governance.execute` does `client.propose_weights(...)`, the credit-oracle sees the caller = `gov_id` — which IS the stored admin after the two-step transfer runs. This is why `accept_oracle_admin` is necessary: without it, the credit-oracle's stored admin is still the original EOA, and governance-initiated calls fail auth.

Why governance's `execute` calls `propose_weights` rather than applying weights immediately (ADR-003 from `docs/architecture.md`):

> Early designs called `update_weights` (which no longer exists) directly, bypassing the credit-oracle's built-in 24-hour reaction window. `propose_weights` + a separate `apply_weights` gives the community an auditable, non-bypassable pending-weight state during that window. Anyone watching the credit-oracle can see the exact weights that are about to activate, well in advance.

**Issue #8 timelock interaction (accurate current behavior summary):**

1. Governance has its own post-vote execution delay (`execution_delay_ledgers`). This is ONE wait.
2. After that delay clears, `governance.execute()` queues weights in credit-oracle via `propose_weights()`, which sets a fixed 17,280-ledger effective ledger. This is a SECOND wait.
3. Only after both waits and TWO function calls (`execute`, then `apply_weights`) do weights become active.
4. Operators, indexers, and downstream consumers should treat `execute` success as "weights queued, pending timelock", not as "weights applied". The tests that conflate the two are the inconsistency documented in §2.3.
