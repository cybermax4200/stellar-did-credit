# @stellar-did-credit/sdk

TypeScript SDK for the Stellar DID Credit Protocol.

## Installation

```bash
npm install @stellar-did-credit/sdk
```

## Usage

```typescript
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";

const sdk = new StellarDIDCreditSDK({
  identityOracleId: "C...",
  creditOracleId: "C...",
  revocationRegistryId: "C...",
  governanceId: "C...",
  networkPassphrase: "Test SDF Network ; September 2015",
  rpcUrl: "https://soroban-testnet.stellar.org",
  simAccount: "G...",
});

const score = await sdk.getScore("G...");
if (score) {
  console.log(score.score); // e.g. 612
} else {
  console.log("No score computed yet");
}
```

## Transaction reliability

`anchorDID`, `issueVC`, `revokeVC`, and `computeScore` retry transient
submission failures with exponential backoff starting at one second. The
default is three retries after the initial submission attempt. Set
`maxRetries` to `0` to disable submission retries.

All four methods wait for a final on-chain transaction status before
returning. `timeoutSeconds` sets the total confirmation deadline and defaults
to 30 seconds. A transaction that remains pending or receives no RPC response
before the deadline throws `SDKError` with code `TRANSACTION_TIMEOUT`.

## API

### `getScore(subjectAddress: string): Promise<ScoreRecord | null>`

Fetches the on-chain credit score for a subject address. Uses a read-only simulation — no signing or fees required. Returns `null` if no score has been computed for this address.

```typescript
interface ScoreRecord {
  score: number; // 300–850
  lastUpdated: number; // ledger timestamp
  vcCount: number; // number of verified credentials
  repaymentRate: number; // basis points (0–10000)
  txVolume30d: bigint; // 30-day transaction volume in stroops
  computedAtLedger: number; // ledger sequence when score was computed
  stale: boolean; // true if score is older than STALE_LEDGER_AGE (~30 days)
}
```

### `computeScore(payerKeypair: Keypair, subjectAddress: string): Promise<ScoreRecord>`

Computes and persists a subject's credit score on-chain. This method submits a transaction to call `compute_score`, waits for confirmation, and then fetches the updated score.

**Important: Cooldown Interaction**
The `compute_score` contract method is protected by a configurable cooldown period (`ComputeCooldownLedgers`) to prevent spam and reduce computational load.
- If you call `computeScore` while the cooldown is active, the contract will reject the transaction.
- **Fresh Deployments**: Depending on the contract's configuration, the cooldown might apply immediately upon initialization. If your first `computeScore` call fails right after a fresh deployment, you may need to wait for the initial cooldown period (e.g., 1 ledger) to pass.

#### Recommended Cooldown Settings

The cooldown can be configured by the contract admin using `update_compute_cooldown`. The ideal setting depends on the environment:

| Environment | Recommended Cooldown (Ledgers) | Rationale |
|-------------|--------------------------------|-----------|
| **Development / Local** | `1` | Allows rapid testing and immediate score recomputation. |
| **Testnet** | `100` (~8 minutes) | Balances testing convenience with realistic network conditions. |
| **Mainnet** | `17280` (~24 hours) | Prevents spam, reduces fees, and aligns with typical score update frequencies. |

### `revokeVC(issuerKeypair: KeypairLike, vcHash: Buffer): Promise<string>`

Submits a signed transaction to `revocation_registry.revoke(issuer, vc_hash)`
and waits for final confirmation. The hash must be exactly 32 bytes.

```typescript
const txHash = await sdk.revokeVC(issuerKeypair, vcHash);
```

Use `confirmationTimeoutMs` and `pollIntervalMs` in `ProtocolConfig` to tune
confirmation polling. Invalid hashes throw `SDKError` with code
`INVALID_VC_HASH`; issuer mismatch failures use `NOT_REGISTERED_ISSUER`.

### SDK Method Status Table

| Method | Status | Description |
|--------|--------|-------------|
| `getScore` | ✅ Implemented | Read persisted score record from credit-oracle |
| `computeScore` | ✅ Implemented | Compute and persist subject credit score on-chain |
| `anchorDID` | ✅ Implemented | Anchor a DID document IPFS CID on-chain |
| `issueVC` | ✅ Implemented | Anchor a verifiable credential for a subject |
| `revokeVC` | ✅ Implemented | Revoke a verifiable credential by hash |
| `getDIDDocument` | ✅ Implemented | Fetch anchored DID document CID for a subject |
| `isVerified` | ✅ Implemented | Check if a subject has active (non-revoked) VCs |
| `verifyVC` | ✅ Implemented | Verify whether a subject has a specific active VC hash |
| `getVCCount` | ✅ Implemented | Fetch count of active VCs for a subject |
| `getWeights` | ✅ Implemented | Fetch contract scoring weight configuration |
| `getRegisteredIssuers` | ✅ Implemented | List all registered trusted credential issuers |

### Governance

Set `governanceId` in `ProtocolConfig` to use the exported `GovernanceClient`
through `sdk.governance`. The client wraps proposal creation, weighted voting,
execution, application of scoring weights, and read-only proposal queries.

```typescript
const proposalId = await sdk.governance.createProposal(
  proposerKeypair,
  { vcWeight: 50, txWeight: 25, repaymentWeight: 25 },
  17_280,
  17_280,
);

await sdk.governance.vote(voterKeypair, proposalId, true, 100n);
const proposal = await sdk.governance.getProposal(proposalId);
const recentProposals = await sdk.governance.listProposals(1n, 10);

// After voting closes and the proposal execution delay expires:
await sdk.governance.execute(payerKeypair, proposalId);

// After the credit-oracle's additional approximately 24-hour timelock:
await sdk.governance.applyWeights(payerKeypair);
```

Governance uses a double timelock. Voting closes at the proposal's
`expiryLedger`, and `execute` is available only after that plus the proposal's
`executionDelayLedgers`. A successful `execute` queues the weights in the
credit-oracle; it does not activate them. Wait approximately 24 hours, or
until the credit-oracle pending record's `effective_ledger`, before calling
`applyWeights`.

Proposal IDs are 1-based. The first proposal has ID `1`; ID `0` is unused.
Start `listProposals` with `1n` when scanning from the beginning.

`GovernanceProposal` mirrors the Rust contract struct. Its `id`, vote tallies,
and `quorumRequired` fields are `bigint`, preserving Soroban `u64` and `i128`
values without JavaScript precision loss.

## Error handling

`getScore()` can throw several types of errors. Applications should catch and distinguish between them:

```typescript
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";

const sdk = new StellarDIDCreditSDK({...});

try {
  const score = await sdk.getScore("G...");
  console.log(`Score: ${score.score}`);
} catch (error) {
  if (error instanceof SimulationError) {
    // Contract rejected the call (e.g., invalid subject address)
    console.error(`Contract error: ${error.message}`);
  } else if (error instanceof NetworkError) {
    // RPC endpoint unreachable or timeout
    console.error(`Network issue: ${error.message}`);
  } else {
    // Other errors (parsing, connection, etc.)
    console.error(`Unexpected error: ${error.message}`);
  }
}
```

### Error types and handling

| Error Type | Cause | Message Pattern | Recommended Action |
|-----------|-------|-----------------|-------------------|
| `SimulationError` | Contract call failed | `Simulation failed: ...` | Validate subject address format; check contract state |
| `SimulationError` | Missing return value | `No return value in simulation result` | Verify RPC endpoint is compatible; check contract deployment |
| `NetworkError` | RPC endpoint unreachable | `Failed to connect to RPC` | Retry with backoff; fallback to alternate RPC endpoint |
| `NetworkError` | Request timeout | `Request timeout` | Increase timeout; check network connectivity |
| Generic `Error` | Invalid subject address | `Invalid Stellar address` | Verify address starts with 'G' and is 56 chars |
| Generic `Error` | Parsing failures | `Failed to parse response` | Log full response; file an issue if RPC format changed |

### Common error scenarios

**Invalid subject address:**
```typescript
try {
  const score = await sdk.getScore("invalid");
} catch (error) {
  console.error("Subject address must be a valid Stellar address (56 chars, starts with G)");
}
```

**Subject not registered in identity-oracle:**
```typescript
try {
  const score = await sdk.getScore("GXXXXXX...");
  // If score is valid but all fields are 0, subject may not be registered
  if (score.score === 0 && score.vcCount === 0) {
    console.log("Subject has no verified credentials");
  }
} catch (error) {
  console.error("Failed to fetch score:", error.message);
}
```

**Network connectivity issues:**
```typescript
async function getScoreWithRetry(address: string, maxRetries = 3) {
  for (let attempt = 0; attempt < maxRetries; attempt++) {
    try {
      return await sdk.getScore(address);
    } catch (error) {
      if (attempt === maxRetries - 1) throw error;
      // Exponential backoff: 1s, 2s, 4s
      await new Promise(resolve => 
        setTimeout(resolve, Math.pow(2, attempt) * 1000)
      );
    }
  }
}
```

## Testnet contract addresses

See [`deployments.testnet.json`](../../deployments.testnet.json) at the repo root.
