# Troubleshooting Guide

This guide helps integrators diagnose and resolve common errors when working with the Stellar DID Credit protocol contracts and SDK.

## Error Catalog

### Identity Oracle Contract Errors

| Error Code | Error Name | Description |
|------------|------------|-------------|
| 1 | `AlreadyInitialized` | Contract has already been initialized |
| 2 | `NotAuthorized` | Caller lacks authorization for the operation |
| 3 | `IssuerNotRegistered` | Issuer is not registered as a trusted issuer |
| 4 | `InvalidCID` | Provided CID format is invalid |
| 5 | `NoPendingAdmin` | No pending admin proposal exists |
| 6 | `DuplicateVC` | VC with the same hash already anchored for this subject |
| 7 | `VCNotFound` | No matching VC record found for the given hash/issuer |

### Credit Oracle Contract Errors

| Error Code | Error Name | Description |
|------------|------------|-------------|
| 1 | `AlreadyInitialized` | Contract has already been initialized |
| 2 | `NotAuthorized` | Caller lacks authorization for the operation |
| 3 | `FeederNotRegistered` | Feeder is not registered as a trusted feeder |
| 4 | `LenderNotRegistered` | Lender is not registered as a trusted lender |
| 5 | `InvalidWeights` | Proposed weights do not sum to 100 |
| 6 | `NoPendingAdmin` | No pending admin proposal exists |
| 7 | `ComputeCooldownActive` | Score was computed too recently for this subject |

### Revocation Registry Contract Errors

| Error Code | Error Name | Description |
|------------|------------|-------------|
| 1 | `AlreadyInitialized` | Contract has already been initialized |
| 2 | `NotAuthorized` | Caller lacks authorization for the operation |
| 3 | `IssuerMismatch` | VC hash was revoked/registered for a different issuer |
| 4 | `NoPendingAdmin` | No pending admin proposal exists |
| 5 | `BatchTooLarge` | Batch size exceeds maximum allowed (100) |

## Common Troubleshooting Scenarios

### 1. Contract Already Initialized Error

**Error:** `IdentityOracleError::AlreadyInitialized`, `CreditOracleError::AlreadyInitialized`, or `RevocationRegistryError::AlreadyInitialized`

**Cause:** Attempting to initialize a contract that has already been initialized.

**Solution:**
- Verify the contract has not been deployed previously
- Check if you're targeting the correct contract address
- If re-initialization is needed, deploy a new contract instance instead

**Contract Reference:**
- Identity Oracle: `contracts/identity-oracle/src/lib.rs:140-148`
- Credit Oracle: `contracts/credit-oracle/src/lib.rs:209-231`
- Revocation Registry: `contracts/revocation-registry/src/lib.rs:82-90`

---

### 2. Issuer Not Registered Error

**Error:** `IdentityOracleError::IssuerNotRegistered`

**Cause:** Attempting to anchor a VC using an issuer address that is not registered as a trusted issuer.

**Solution:**
- Contact the contract administrator to register the issuer
- Use `register_issuer` admin function to add the issuer to the trusted list
- Verify the issuer address is correct and matches the registered address

**Contract Reference:** `contracts/identity-oracle/src/lib.rs:266-274`

**SDK Example:**
```typescript
// First check if issuer is registered
const issuers = await sdk.getRegisteredIssuers();
if (!issuers.includes(issuerAddress)) {
  throw new Error("Issuer not registered. Contact admin.");
}
```

---

### 3. Invalid CID Format Error

**Error:** `IdentityOracleError::InvalidCID`

**Cause:** The provided IPFS CID does not match the required format.

**Solution:**
- Ensure CID starts with one of the valid prefixes: `ipfs://`, `bafy`, or `Qm`
- CID must be at least 7 characters long
- Verify the CID is a valid IPFS content identifier

**Valid Examples:**
- `ipfs://QmYwAPJzagoJzrKSTTkG8w6zWZSNxrCYhpDkxQottEwHym`
- `bafy2bzacedw4hc6k2vxtcmfmr3jtcl6yvqohqmvtqj7lhyzuejcxgxvl6yv4`
- `QmVocdeKSNbd9jkc3pDjq9FdAVLpiHrfQFwcJMgB7aXZi3`

**Invalid Examples:**
- Empty string
- Single space
- `invalid-cid-data`

**Contract Reference:** `contracts/identity-oracle/src/lib.rs:233-249`

---

### 4. Not Authorized Error

**Error:** `NotAuthorized` (any contract)

**Cause:** The caller does not have permission to perform the requested operation.

**Solution:**
- Verify the caller is the contract administrator for admin operations
- For issuer operations, ensure the caller is a registered trusted issuer
- For governor operations in Credit Oracle, verify the caller is a registered governor
- Check that the correct keypair is being used for signing

**Contract Reference:**
- Identity Oracle: `contracts/identity-oracle/src/lib.rs:24-32`
- Credit Oracle: `contracts/credit-oracle/src/lib.rs:23-31` (admin), `38-57` (admin/governor)
- Revocation Registry: `contracts/revocation-registry/src/lib.rs:26-34`

---

### 5. Duplicate VC Error

**Error:** `IdentityOracleError::DuplicateVC`

**Cause:** Attempting to anchor a VC with a hash that already exists for the same subject.

**Solution:**
- Verify the VC hash is unique for this subject
- Check if the VC was already anchored using `verify_vc`
- If re-anchoring is needed, use a different VC hash (e.g., updated credential)

**Contract Reference:** `contracts/identity-oracle/src/lib.rs:283-288`

**SDK Example:**
```typescript
// Check if VC already exists before anchoring
const exists = await sdk.verifyVC(subjectAddress, vcHash);
if (exists) {
  throw new Error("VC already anchored for this subject");
}
```

---

### 6. VC Not Found Error

**Error:** `IdentityOracleError::VCNotFound`

**Cause:** Attempting to revoke or mark as revoked a VC that doesn't exist for the given subject/issuer combination.

**Solution:**
- Verify the VC hash is correct
- Ensure the issuer is the one who originally anchored the VC
- Check that the subject address is correct
- Use `verify_vc` to confirm the VC exists before revocation

**Contract Reference:** `contracts/identity-oracle/src/lib.rs:330-332`

---

### 7. Feeder Not Registered Error

**Error:** `CreditOracleError::FeederNotRegistered`

**Cause:** Attempting to update transaction statistics using a feeder that is not registered.

**Solution:**
- Contact the contract administrator to register the feeder
- Use `register_feeder` admin function to add the feeder
- Verify the feeder address is correct

**Contract Reference:** `contracts/credit-oracle/src/lib.rs:342-349`

---

### 8. Lender Not Registered Error

**Error:** `CreditOracleError::LenderNotRegistered`

**Cause:** Attempting to record a repayment using a lender that is not registered.

**Solution:**
- Contact the contract administrator to register the lender
- Use `register_lender` admin function to add the lender
- Verify the lender address is correct

**Contract Reference:** `contracts/credit-oracle/src/lib.rs:364-371`

---

### 9. Invalid Weights Error

**Error:** `CreditOracleError::InvalidWeights`

**Cause:** Proposed scoring weights do not sum to 100.

**Solution:**
- Ensure `vc_weight + tx_weight + repayment_weight = 100`
- Each weight must be between 0 and 100
- Verify the weight values before submitting the proposal

**Contract Reference:** `contracts/credit-oracle/src/lib.rs:595-597`, `664-666`

**Valid Example:**
```typescript
const weights = {
  vcWeight: 40,
  txWeight: 30,
  repaymentWeight: 30
}; // Sum = 100 ✓
```

**Invalid Example:**
```typescript
const weights = {
  vcWeight: 50,
  txWeight: 30,
  repaymentWeight: 30
}; // Sum = 110 ✗
```

---

### 10. Compute Cooldown Active Error

**Error:** `CreditOracleError::ComputeCooldownActive`

**Cause:** Attempting to compute a score for a subject too soon after the previous computation.

**Solution:**
- Wait for the cooldown period to expire (default: 1 ledger)
- Check the current cooldown setting using `get_compute_cooldown`
- If immediate recomputation is needed, admin can set cooldown to 0 using `update_compute_cooldown`

**Contract Reference:** `contracts/credit-oracle/src/lib.rs:481-491`

**SDK Example:**
```typescript
// Check cooldown before computing
const cooldown = await sdk.getComputeCooldown();
// Wait appropriate time or handle cooldown error
```

---

### 11. No Pending Admin Error

**Error:** `NoPendingAdmin` (any contract)

**Cause:** Attempting to accept an admin transfer when no proposal exists.

**Solution:**
- Ensure `propose_new_admin` was called first
- Verify the pending admin address matches the caller
- Check that the proposal hasn't expired or been cancelled

**Contract Reference:**
- Identity Oracle: `contracts/identity-oracle/src/lib.rs:442-451`
- Credit Oracle: `contracts/credit-oracle/src/lib.rs:759-767`
- Revocation Registry: `contracts/revocation-registry/src/lib.rs:109-117`

---

### 12. Issuer Mismatch Error

**Error:** `RevocationRegistryError::IssuerMismatch`

**Cause:** Attempting to revoke a VC hash that was previously revoked by a different issuer.

**Solution:**
- Only the first issuer to revoke a VC hash can continue revoking it
- Verify the issuer address matches the original revoker
- If multiple issuers need revocation rights, coordinate through admin

**Contract Reference:** `contracts/revocation-registry/src/lib.rs:144-148`

---

### 13. Batch Too Large Error

**Error:** `RevocationRegistryError::BatchTooLarge`

**Cause:** Attempting to revoke more than 100 VCs in a single batch operation.

**Solution:**
- Split large revocation batches into smaller chunks (≤ 100 VCs each)
- Process batches sequentially
- Consider using individual `revoke` calls for small numbers of VCs

**Contract Reference:** `contracts/revocation-registry/src/lib.rs:196-198`

**SDK Example:**
```typescript
const MAX_BATCH_SIZE = 100;
for (let i = 0; i < vcHashes.length; i += MAX_BATCH_SIZE) {
  const batch = vcHashes.slice(i, i + MAX_BATCH_SIZE);
  await sdk.batchRevoke(issuerKeypair, batch);
}
```

---

### 14. Score Not Computed Error

**Error:** `ScoreNotComputedError` (SDK)

**Cause:** Attempting to retrieve a score that has never been computed for the subject.

**Solution:**
- Call `computeScore` first to calculate and store the score
- Verify the subject address is correct
- Check if the score computation transaction succeeded

**SDK Reference:** `packages/sdk/src/index.ts:742-752`

**SDK Example:**
```typescript
try {
  const score = await sdk.getScore(subjectAddress);
  if (!score) {
    // Score not computed, compute it first
    score = await sdk.computeScore(payerKeypair, subjectAddress);
  }
} catch (error) {
  if (error instanceof ScoreNotComputedError) {
    // Handle case where score needs to be computed
  }
}
```

---

### 15. Simulation Failed Error

**Error:** `Simulation failed: [error message]` (SDK)

**Cause:** Transaction simulation failed due to contract-level errors or RPC issues.

**Solution:**
- Check the specific error message for contract error details
- Verify all input parameters are valid
- Ensure the RPC endpoint is accessible
- The SDK automatically retries transient failures up to 3 times with exponential backoff

**SDK Reference:** `packages/sdk/src/index.ts:145-167`

**SDK Retry Logic:**
```typescript
// Automatic retry with exponential backoff:
// Attempt 1: immediate
// Attempt 2: 500ms delay
// Attempt 3: 1000ms delay
// Attempt 4: 2000ms delay
```

---

### 16. Transaction Submission Failed Error

**Error:** `Transaction submission failed: [error result]` (SDK)

**Cause:** Transaction was not accepted by the network after simulation succeeded.

**Solution:**
- Check the account has sufficient XLM balance for fees
- Verify the transaction fee is adequate (configurable via `baseFee`)
- Ensure the account sequence number is current
- Check network connectivity and RPC status

**SDK Reference:** `packages/sdk/src/index.ts:221-227`, `283-289`, `355-361`, `408-412`

---

### 17. Stale Simulation Results

**Error:** Simulation succeeds but transaction fails due to stale state.

**Cause:** Time delay between simulation and transaction submission causes state changes.

**Solution:**
- Minimize delay between simulation and submission
- The SDK's `simulateWithRetry` helps handle transient failures
- Consider implementing a fresh simulation before submission if delays are expected

**SDK Reference:** `packages/sdk/src/index.ts:145-167`

---

### 18. VC Hash Length Validation Error

**Error:** `vcHash must be exactly 32 bytes` (SDK)

**Cause:** Provided VC hash is not exactly 32 bytes (SHA-256 length).

**Solution:**
- Ensure the VC hash is a proper SHA-256 hash (32 bytes)
- Validate hash length before passing to SDK methods
- Use proper cryptographic hashing functions

**SDK Reference:** `packages/sdk/src/index.ts:309-311`, `570-572`

**SDK Example:**
```typescript
import crypto from 'crypto';

function hashVC(vcJson: string): Buffer {
  return crypto.createHash('sha256').update(vcJson).digest();
}

const vcHash = hashVC(vcJson);
if (vcHash.length !== 32) {
  throw new Error("Invalid hash length");
}
```

---

### 19. Transaction Timeout Error

**Error:** Transaction confirmation timeout

**Cause:** Transaction not confirmed within the expected time window.

**Solution:**
- Increase `timeoutSeconds` in SDK configuration (default: 30)
- Check network congestion and ledger times
- Verify the transaction hash and check status manually
- The SDK waits up to 20 seconds with 1-second intervals by default

**SDK Reference:** `packages/sdk/src/index.ts:792-824`

---

### 20. Network Configuration Error

**Error:** Various RPC or network-related errors

**Cause:** Incorrect network passphrase or RPC URL configuration.

**Solution:**
- Verify `networkPassphrase` matches the target network (testnet vs mainnet)
- Ensure `rpcUrl` is accessible and valid
- Check that contract IDs are correct for the target network
- Use the correct `simAccount` for the network

**SDK Reference:** `packages/sdk/src/index.ts:98-121`

**Configuration Example:**
```typescript
const config: ProtocolConfig = {
  identityOracleId: "CC...",
  creditOracleId: "CC...",
  revocationRegistryId: "CC...",
  networkPassphrase: "Test SDF Network ; September 2015", // or mainnet
  rpcUrl: "https://rpc.testnet.stellar.org",
  simAccount: "G...",
  timeoutSeconds: 30,
  maxRetries: 3,
  baseFee: "100"
};
```

---

## SDK Error Handling Patterns

### Custom Error Classes

The SDK provides custom error classes for specific failure modes:

```typescript
export class ScoreNotComputedError extends Error {
  constructor(address?: string) {
    super(
      address
        ? `No score computed for address: ${address}`
        : "Score has not been computed",
    );
    this.name = "ScoreNotComputedError";
  }
}
```

### Simulation Retry Logic

The SDK implements automatic retry with exponential backoff for transient RPC failures:

- **Maximum retries:** 3 (configurable via `maxRetries`)
- **Backoff pattern:** 500ms, 1s, 2s
- **Contract errors:** Surface immediately without retry
- **Transient failures:** Retry automatically

### Error Detection in SDK

```typescript
// Check for simulation errors
if (SorobanRpc.Api.isSimulationError(sim)) {
  throw new Error(`Simulation failed: ${sim.error}`);
}

// Check for successful simulation
if (!SorobanRpc.Api.isSimulationSuccess(sim)) {
  throw new Error("Simulation returned unexpected response");
}
```

### Transaction Status Handling

```typescript
if (response.status !== "PENDING") {
  throw new Error(`Transaction submission failed: ${String(response.errorResult)}`);
}
```

### Validation Errors

The SDK performs input validation before contract calls:

```typescript
// VC hash length validation
if (vcHash.length !== 32) {
  throw new Error("vcHash must be exactly 32 bytes");
}
```

---

## Debugging Tips

### 1. Enable Detailed Logging

Add logging to track SDK operations:

```typescript
console.log("Anchoring DID for subject:", publicKey);
console.log("CID:", didDocCid);
const txHash = await sdk.anchorDID(subjectKeypair, didDocCid);
console.log("Transaction hash:", txHash);
```

### 2. Check Contract State

Use read-only methods to verify contract state before write operations:

```typescript
// Check if issuer is registered
const issuers = await sdk.getRegisteredIssuers();
console.log("Registered issuers:", issuers);

// Check if VC exists
const exists = await sdk.verifyVC(subjectAddress, vcHash);
console.log("VC exists:", exists);

// Check current score
const score = await sdk.getScore(subjectAddress);
console.log("Current score:", score);
```

### 3. Verify Network Configuration

Ensure all contract IDs and network settings match:

```typescript
console.log("Network:", config.networkPassphrase);
console.log("RPC:", config.rpcUrl);
console.log("Identity Oracle:", config.identityOracleId);
console.log("Credit Oracle:", config.creditOracleId);
console.log("Revocation Registry:", config.revocationRegistryId);
```

### 4. Test with Small Operations

Start with simple operations before complex workflows:

```typescript
// Test basic read operations
const issuers = await sdk.getRegisteredIssuers();
const score = await sdk.getScore(testAddress);

// Then test write operations
const txHash = await sdk.anchorDID(testKeypair, testCID);
```

### 5. Use Explorer for Transaction Verification

After submission, verify transactions on Stellar Explorer:
- Testnet: https://stellar.expert/testnet
- Mainnet: https://stellar.expert

Search by transaction hash to see detailed status and error messages.

---

## Getting Help

If you encounter an error not covered in this guide:

1. **Check the contract source code** for detailed error conditions
2. **Review SDK error messages** for specific failure details
3. **Verify network configuration** matches your target environment
4. **Test with a known-good configuration** to isolate the issue
5. **Check Stellar RPC status** for network issues
6. **Review transaction details** on Stellar Explorer

### Contract Source References

- Identity Oracle: `contracts/identity-oracle/src/lib.rs`
- Credit Oracle: `contracts/credit-oracle/src/lib.rs`
- Revocation Registry: `contracts/revocation-registry/src/lib.rs`
- SDK: `packages/sdk/src/index.ts`

### Related Documentation

- [Scoring Specification](./scoring-spec.md)
- [Contract API Documentation](./api.md)
- [SDK Documentation](../packages/sdk/README.md)
