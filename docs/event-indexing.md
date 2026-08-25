# Event Indexing Guide

This guide describes the on-chain events emitted by the `stellar-did-credit` contracts and how off-chain data feeders/indexers can subscribe to and process these events to maintain synchronized off-chain states.

## Event Catalog

Soroban events are structured as a topic vector and a data payload. By convention, the first topic is a symbol representing the event name.

### 0. Common Events

#### Initialized

The `identity-oracle`, `credit-oracle`, and `revocation-registry` contracts emit an `Initialized` event during their `initialize` function. The `governance` contract also emits one — see the Governance section below. In all cases the event is emitted exactly once per contract, immediately after the admin address and target wiring is stored.

* **Topic:** `[Symbol("Initialized")]`
* **Data:** `admin: Address` (governance uses `(admin: Address, credit_oracle: Address)` — see below)
* **Emitted When:** The contract is initialized with an administrator address.
* **feeder Action:** None (metadata tracking).

---

### 1. Identity Oracle Events

#### Initialized
* **Topic:** `[Symbol("Initialized")]`
* **Data:** `admin: Address`
* **Emitted When:** The contract is initialized with an admin address.
* **Note:** Emitted exactly once — the `AlreadyInitialized` error prevents re-initialization.

#### DIDAnch
* **Topic:** `[Symbol("DIDAnch")]`
* **Data:** `(subject: Address, did_doc_cid: String)`
* **Emitted When:** A subject anchors or updates their DID document CID.
* **feeder Action:** None (metadata tracking).

#### VCAnch
* **Topic:** `[Symbol("VCAnch")]`
* **Data:** `(issuer: Address, subject: Address, vc_hash: BytesN<32>)`
* **Emitted When:** A trusted issuer anchors a new Verifiable Credential for a subject.
* **feeder Action:** Trigger sync for `subject` (fetch new VC count, submit `set_vc_count`).

#### RevocationRegistryUpdated
* **Topic:** `[Symbol("RegSet")]`
* **Data:** `(previous_registry: Address, new_registry: Address)`
* **Emitted When:** The admin updates the revocation registry contract ID on the identity oracle.
* **feeder Action:** None (configuration tracking). Update local cache of the revocation registry address.

#### IssReg / IssDeReg
* **Topic:** `[Symbol("IssReg")]` or `[Symbol("IssDeReg")]`
* **Data:** `issuer: Address`
* **Emitted When:** An issuer is registered or deregistered by the admin.

---

### 2. Revocation Registry Events

#### Initialized
* **Topic:** `[Symbol("Initialized")]`
* **Data:** `admin: Address`
* **Emitted When:** The contract is initialized with an admin address.
* **Note:** Emitted exactly once — the `AlreadyInitialized` error prevents re-initialization.

#### Revoked
* **Topic:** `[Symbol("Revoked")]`
* **Data:** `(issuer: Address, vc_hash: BytesN<32>)`
* **Emitted When:** An issuer revokes a single VC hash.
* **feeder Action:** Map the `vc_hash` to the subject, decrement their VC count, and submit `set_vc_count` to the credit oracle.

#### BatchRev
* **Topic:** `[Symbol("BatchRev")]`
* **Data:** `(issuer: Address, count: u32)`
* **Emitted When:** An issuer revokes a batch of VC hashes.

---

### 3. Credit Oracle Events

#### IdentityOracleUpdated
* **Topic:** `[Symbol("OrclSet")]`
* **Data:** `(previous_oracle: Address, new_oracle: Address)`
* **Emitted When:** The admin updates the identity-oracle contract ID on the credit oracle.
* **feeder Action:** None (configuration tracking). Update local cache of the identity-oracle address.

#### Score
* **Topic:** `[Symbol("Score")]`
* **Data:** `(subject: Address, score: u32)`
* **Emitted When:** A subject's credit score is recomputed and updated.

#### FdrReg / FdrDeReg
* **Topic:** `[Symbol("FdrReg")]` / `[Symbol("FdrDeReg")]`
* **Data:** `feeder: Address`
* **Emitted When:** A feeder is registered or deregistered.

#### LndReg / LndDeReg
* **Topic:** `[Symbol("LndReg")]` / `[Symbol("LndDeReg")]`
* **Data:** `lender: Address`
* **Emitted When:** A lender is registered or deregistered.

#### WtProp
* **Topic:** `[Symbol("WtProp")]`
* **Data:** `(vc_weight: u32, tx_weight: u32, repayment_weight: u32, effective_ledger: u32)`
* **Emitted When:** New scoring weights are proposed.

#### WtApply
* **Topic:** `[Symbol("WtApply")]`
* **Data:** `(vc_weight: u32, tx_weight: u32, repayment_weight: u32)`
* **Emitted When:** Pending or direct weights are applied.

#### CdSet
* **Topic:** `[Symbol("CdSet")]`
* **Data:** `(ledgers: u32, admin: Address)`
* **Emitted When:** The compute cooldown ledgers value is updated by the admin.

---

### 4. Governance Events

#### Initialized
* **Topic:** `[Symbol("Initialized")]`
* **Data:** `(admin: Address, credit_oracle: Address)`
* **Emitted When:** The governance contract is initialized with an admin address and the credit-oracle it will govern. The admin address must be passed in by the caller (matches the `initialize` parameter); `credit_oracle` is also passed in at init time and must match the address stored under `DataKey::CreditOracle`.
* **Note:** The data format differs from the other contracts because governance's `initialize` signature includes the credit-oracle target. The identity-oracle address is not currently stored by governance (a specific follow-up to issue #39 would make this consistent — for now governance is the only contract that emits more than just the admin on init). Emitted exactly once — the `AlreadyInitialized` error prevents re-initialization.

#### ProposalCreated
* **Topic:** `[Symbol("PropCreat"), proposal_id: u64]`
* **Data:** `(proposer: Address, expiry_ledger: u32)`
* **Emitted When:** A new governance proposal is created.

#### ProposalExecuted
* **Topic:** `[Symbol("PropExec"), proposal_id: u64]`
* **Data:** `(votes_for: i128, votes_against: i128)`
* **Emitted When:** An expired governance proposal is executed.

#### ProposalCancelled
* **Topic:** `[Symbol("PropCanc"), proposal_id: u64]`
* **Data:** `(canceller: Address, reason: Option<String>)`
* **Emitted When:** A governance proposal is cancelled.

---

## Subscribing to Events with the SDK

The TypeScript SDK provides polling helpers for the three events most useful to
off-chain lenders, feeders, and analytics services. Each helper polls
Soroban RPC's `getEvents` method and returns an unsubscribe function. Configure
the polling interval with `pollIntervalMs`; no WebSocket connection is
required.

```typescript
import StellarDIDCreditSDK from "@stellar-did-credit/sdk";

const sdkConfig = {
  identityOracleId: "C...",       // Identity Oracle contract ID
  creditOracleId: "C...",         // Credit Oracle contract ID
  revocationRegistryId: "C...",   // Revocation Registry contract ID
  networkPassphrase: "Test SDF Network ; September 2015",
  rpcUrl: "https://soroban-testnet.stellar.org",
  simAccount: "G...",             // Account used for read-only simulations
  pollIntervalMs: 5_000,
};
const sdk = new StellarDIDCreditSDK(sdkConfig);

const stopAnchored = sdk.onVCAnchored(
  sdkConfig.identityOracleId,
  (issuer, subject, vcHash) => {
    console.log("VC anchored", { issuer, subject, vcHash });
    // Trigger your feeder sync logic here.
  },
);

const stopScored = sdk.onScoreComputed(
  sdkConfig.creditOracleId,
  (subject, score) => {
    console.log("Score computed", { subject, score });
  },
);

const stopRevoked = sdk.onVCRevoked(
  sdkConfig.revocationRegistryId,
  (issuer, vcHash) => {
    console.log("VC revoked", { issuer, vcHash });
  },
);

// Call the returned functions during shutdown.
void stopAnchored;
void stopScored;
void stopRevoked;
```

The first poll begins at the latest ledger available from the RPC server.
After each successful response, the SDK advances the subscription cursor to the
next ledger after the latest ledger seen, so events are not delivered again by
later polls.

---

## Feeder Event-Driven Sync Algorithm

To maintain a real-time credit score, the off-chain feeder performs the following event-driven loops:

### Scenario A: VC Anchored
1. Subscribe to `VCAnch` events on `identity-oracle`.
2. Extract the `subject` address from the event payload.
3. Call `get_active_vc_count(subject)` on `identity-oracle` via read-only RPC simulation to get the latest count.
4. Call `set_vc_count(feeder, subject, count)` on `credit-oracle`.

### Scenario B: VC Revoked
1. Subscribe to `Revoked` events on `revocation-registry`.
2. Extract the `vc_hash`.
3. Resolve the `subject` address associated with that `vc_hash` (e.g. from local indexing database).
4. Call `get_active_vc_count(subject)` on `identity-oracle` via read-only RPC simulation to get the decremented count.
5. Call `set_vc_count(feeder, subject, count)` on `credit-oracle`.
