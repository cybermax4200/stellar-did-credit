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
  networkPassphrase: "Test SDF Network ; September 2015",
  rpcUrl: "https://soroban-testnet.stellar.org",
});

const score = await sdk.getScore("G...");
console.log(score.score); // e.g. 612
```

## API

### `getScore(subjectAddress: string): Promise<ScoreRecord>`

Fetches the on-chain credit score for a subject address. Uses a read-only simulation — no signing or fees required.

```typescript
interface ScoreRecord {
  score: number; // 300–850
  lastUpdated: number; // ledger timestamp
  vcCount: number; // number of verified credentials
  repaymentRate: number; // basis points (0–10000)
  txVolume30d: bigint; // 30-day transaction volume in stroops
}
```

### `verifyVC(subjectAddress: string, vcHash: Buffer): Promise<boolean>`

Checks whether a specific 32-byte credential hash is valid for a subject. Uses a read-only simulation against the identity-oracle contract.

```typescript
const isValid = await sdk.verifyVC("G...", vcHash);
```

## SDK status

| Method                           | Status         |
| -------------------------------- | -------------- |
| `getScore(address)`              | ✅ Implemented |
| `verifyVC(subject, hash)`        | ✅ Implemented |
| `isVerified(address)`            | 🚧 Open        |
| `anchorDID(keypair, cid)`        | 🚧 Open        |
| `issueVC(issuer, subject, hash)` | 🚧 Open        |
| `revokeVC(issuer, hash)`         | 📋 Planned     |

### Other methods (coming soon)

- `anchorDID(subjectKeypair, didDocCid)` — anchor a DID document CID on-chain
- `issueVC(issuerKeypair, subjectAddress, vcHash)` — anchor a verifiable credential
- `isVerified(subjectAddress)` — check if a subject has any active VC

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
