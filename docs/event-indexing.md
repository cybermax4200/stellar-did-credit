# Event Indexing Guide

This guide describes the on-chain events emitted by the `stellar-did-credit` contracts and how off-chain data feeders/indexers can subscribe to and process these events to maintain synchronized off-chain states.

## Event Catalog

Soroban events are structured as a topic vector and a data payload. By convention, the first topic is a symbol representing the event name.

### 1. Identity Oracle Events

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

#### IssReg / IssDeReg
* **Topic:** `[Symbol("IssReg")]` or `[Symbol("IssDeReg")]`
* **Data:** `issuer: Address`
* **Emitted When:** An issuer is registered or deregistered by the admin.

---

### 2. Revocation Registry Events

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
