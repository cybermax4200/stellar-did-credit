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
* **Note:** Always emitted by `compute_score`, regardless of the `verbose_events` flag.

#### ScoreDtl _(verbose only)_
* **Topic:** `[Symbol("ScoreDtl")]`
* **Data:** `ScoreDetail { subject: Address, vc_score: u32, tx_score: u32, repay_score: u32, composite: u32, score: u32, weights: ScoringWeights }`
* **Emitted When:** `compute_score` is called **and** the admin has enabled verbose events via `set_verbose_events(admin, true)`.
* **Purpose:** Provides the full intermediate scoring breakdown so analytics platforms can build issuer and feeder quality dashboards without re-running the scoring formula. The `weights` field captures the active `ScoringWeights` at compute time, which is critical because weights can be updated via the timelock mechanism.
* **feeder Action:** Index the component scores alongside the `Score` event to populate per-subject scoring timelines. The `composite` field can be used to detect which component is dragging a subject's score.
* **Default state:** Disabled (`false`). No `ScoreDtl` events are emitted until an admin call enables them.

#### VbsEvt
* **Topic:** `[Symbol("VbsEvt")]`
* **Data:** `enabled: bool`
* **Emitted When:** The admin calls `set_verbose_events`.
* **feeder Action:** Update your local flag so you know whether to expect `ScoreDtl` events alongside each `Score` event.

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

---

## Verbose Scoring Events

### Overview

By default `compute_score` emits only the `Score` event (subject + final score). When verbose events are enabled the contract additionally emits a `ScoreDtl` event containing every intermediate component and the weights that were active at compute time.

| Flag state | Events emitted by `compute_score` |
|---|---|
| `verbose_events = false` (default) | `Score` |
| `verbose_events = true` | `Score` + `ScoreDtl` |

### Enabling verbose events

Only the contract admin can toggle this flag:

```bash
stellar contract invoke \
  --id <CREDIT_ORACLE_CONTRACT_ID> \
  --source <ADMIN_SECRET_KEY> \
  -- set_verbose_events \
  --admin <ADMIN_ADDRESS> \
  --enabled true
```

To disable again, pass `--enabled false`.

### ScoreDetail fields

| Field | Type | Description |
|---|---|---|
| `subject` | `Address` | The subject whose score was computed. |
| `vc_score` | `u32` | VC component score (0–100) before weighting. |
| `tx_score` | `u32` | Transaction component score (0–100) before weighting. |
| `repay_score` | `u32` | Repayment component score (0–100) before weighting. |
| `composite` | `u32` | Weighted composite (0–100): `(vc_score × vc_weight + tx_score × tx_weight + repay_score × repayment_weight) / 100`. |
| `score` | `u32` | Final clamped score ([300, 850]). Matches the value in the `Score` event. |
| `weights` | `ScoringWeights` | Active weights at compute time (`vc_weight`, `tx_weight`, `repayment_weight`). |

### Indexer guidance

When `VbsEvt` fires with `enabled = true`, start correlating `ScoreDtl` events with `Score` events (same ledger, same contract, same `subject`).

**Dashboard use-cases enabled by `ScoreDtl`:**

* **Feeder quality:** Track `tx_score` trends per feeder to detect stale or missing `update_tx_stats` calls.
* **Issuer quality:** Track `vc_score` to see whether new VC issuances are lifting subject scores.
* **Repayment health:** Monitor `repay_score` across a lender's portfolio without re-running the formula.
* **Weight-change impact analysis:** Because `weights` is captured at compute time you can retroactively compare scores computed under different weight regimes.

### Node.js example — consuming ScoreDtl

```typescript
import { SorobanRpc, xdr, scValToNative } from "@stellar/stellar-sdk";

const rpcUrl = "https://soroban-testnet.stellar.org";
const server = new SorobanRpc.Server(rpcUrl);
const contractId = "<CREDIT_ORACLE_CONTRACT_ID>";

async function indexScoreDetails(startLedger: number) {
  const response = await server.getEvents({
    startLedger,
    filters: [
      {
        type: "contract",
        contractIds: [contractId],
        topics: [[xdr.ScVal.scvSymbol("ScoreDtl").toXDR("base64")]],
      },
    ],
    limit: 100,
  });

  for (const event of response.events) {
    const detail = scValToNative(event.value);
    // detail is a ScoreDetail struct — fields match the order in the contract type
    const { subject, vc_score, tx_score, repay_score, composite, score, weights } = detail;

    console.log(
      `[ScoreDtl] subject=${subject} ` +
      `vc=${vc_score} tx=${tx_score} repay=${repay_score} ` +
      `composite=${composite} final=${score} ` +
      `weights=${weights.vc_weight}/${weights.tx_weight}/${weights.repayment_weight}`
    );

    // Store in your analytics database:
    // await db.upsertScoreBreakdown({ subject, vc_score, tx_score, repay_score, composite, score, weights, ledger: event.ledger });
  }
}
```

---

## Subscribing to Events (Node.js Example)

Here is a Node.js example using the `@stellar/stellar-sdk` to subscribe to `VCAnch` events on the Identity Oracle contract.

```typescript
import { SorobanRpc, xdr, scValToNative } from "@stellar/stellar-sdk";

const rpcUrl = "https://soroban-testnet.stellar.org";
const server = new SorobanRpc.Server(rpcUrl);
const contractId = "CATORJPJ..."; // Replace with Identity Oracle contract ID

async function pollEvents() {
  const currentLedger = await server.getLatestLedger();
  const startLedger = currentLedger.sequence - 100; // Start polling from 100 ledgers ago

  console.log(`Polling events starting from ledger ${startLedger}...`);

  const response = await server.getEvents({
    startLedger,
    filters: [
      {
        type: "contract",
        contractIds: [contractId],
        topics: [
          [
            xdr.ScVal.scvSymbol("VCAnch").toXDR("base64")
          ]
        ]
      }
    ],
    limit: 50
  });

  for (const event of response.events) {
    const value = scValToNative(event.value);
    // VCAnch value is a tuple/array: [issuer, subject, vc_hash]
    const [issuer, subject, vcHash] = value;
    console.log(`[VCAnch] Issuer: ${issuer}, Subject: ${subject}, Hash: ${vcHash}`);
    
    // Trigger your feeder sync logic here:
    // await syncSubjectVCs(subject);
  }
}

pollEvents().catch(console.error);
```

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
