# Issuer Integration Guide

This guide shows how a credential issuer — a KYC provider, payroll platform, or microfinance institution — formats, hashes, and anchors a Verifiable Credential (VC) using the stellar-did-credit protocol.

The protocol stores only a 32-byte SHA-256 hash of the credential on-chain. The full JSON-LD document stays off-chain (on your servers or IPFS), preserving user privacy while making the credential verifiable by any third party who receives a copy.

## Prerequisites

Before you can anchor credentials, the protocol admin must register your Stellar address as a trusted issuer by calling `register_issuer(admin, issuer_address)` on the identity-oracle contract. For the admin-side setup flow, see the [admin setup](architecture.md#admin-setup) section in the architecture guide. Until that transaction is confirmed, `anchor_vc` and `anchor_vc_typed` will reject your calls with `IssuerNotAuthorized`.

You will also need:

- A funded Stellar testnet account (to pay transaction fees). Fund it via [Friendbot](https://friendbot.stellar.org/?addr=YOUR_ADDRESS).
- Node.js 18+
- The `@stellar-did-credit/sdk` package

```bash
npm install @stellar-did-credit/sdk @stellar/stellar-sdk
```

## Table of contents

- [Prerequisites](#prerequisites)
- [VC JSON-LD format](#vc-json-ld-format)
- [Hashing the credential](#hashing-the-credential)
- [Anchoring on-chain via the SDK](#anchoring-on-chain-via-the-sdk)
- [Typed credential anchoring](#typed-credential-anchoring)
- [Issuer tiers and scoring impact](#issuer-tiers-and-scoring-impact)
- [Key management best practices](#key-management-best-practices)
- [Revoking a credential](#revoking-a-credential)
- [VC anchor limit](#vc-anchor-limit)
- [Full working example](#full-working-example)
- [See also](#see-also)

---

## VC JSON-LD format

Credentials MUST be valid [W3C Verifiable Credentials](https://www.w3.org/TR/vc-data-model/) serialized as JSON-LD. The following fields are required:

| Field                | Required | Description |
| -------------------- | -------- | ----------- |
| `@context`           | Yes      | JSON-LD context array. Must include the W3C VC base context. |
| `type`               | Yes      | Array containing `"VerifiableCredential"` and at least one domain type. |
| `issuer`             | Yes      | The issuer DID (`did:stellar:testnet:G...`) or an object with an `id` field. |
| `issuanceDate`       | Yes      | ISO 8601 timestamp when the credential was issued. |
| `credentialSubject`  | Yes      | Object describing the claims. Must include `id` (the subject DID). |

### Minimal example

```json
{
  "@context": [
    "https://www.w3.org/2018/credentials/v1"
  ],
  "type": ["VerifiableCredential", "KYCCredential"],
  "issuer": "did:stellar:testnet:GISSUER11111111111111111111111111111111111111111111111111",
  "issuanceDate": "2026-06-28T12:00:00Z",
  "credentialSubject": {
    "id": "did:stellar:testnet:GSUBJECT1111111111111111111111111111111111111111111111111",
    "kycLevel": "basic",
    "verifiedAt": "2026-06-28T10:00:00Z",
    "country": "NG"
  }
}
```

### Extended KYC example

A real KYC credential typically attests to identity verification level and the jurisdiction in which it was performed. More claims can be added inside `credentialSubject` without affecting the on-chain hash format — the hash always covers the full document.

```json
{
  "@context": [
    "https://www.w3.org/2018/credentials/v1",
    "https://schema.org/"
  ],
  "type": ["VerifiableCredential", "IdentityVerificationCredential"],
  "issuer": {
    "id": "did:stellar:testnet:GISSUER11111111111111111111111111111111111111111111111111",
    "name": "Acme KYC Ltd"
  },
  "issuanceDate": "2026-06-28T12:00:00Z",
  "expirationDate": "2027-06-28T12:00:00Z",
  "credentialSubject": {
    "id": "did:stellar:testnet:GSUBJECT1111111111111111111111111111111111111111111111111",
    "kycLevel": "enhanced",
    "legalName": "Jane Doe",
    "dateOfBirth": "1990-03-15",
    "nationality": "NG",
    "documentType": "passport",
    "verifiedAt": "2026-06-28T10:00:00Z"
  }
}
```

> **Privacy note:** The full JSON is never stored on-chain — only its SHA-256 hash. You control who receives the plaintext credential. Share it only with the subject and lenders they authorize.

---

## Hashing the credential

The on-chain anchoring functions take a `BytesN<32>`, which is the raw SHA-256 digest of the **canonicalized** credential JSON. Canonicalization ensures the same logical document always produces the same hash regardless of key ordering or whitespace differences.

### Step 1 — Canonicalize the JSON

Use [RFC 8785 JSON Canonicalization Scheme (JCS)](https://www.rfc-editor.org/rfc/rfc8785) to produce a deterministic byte sequence from the credential object. JCS recursively sorts all object keys and removes insignificant whitespace.

```bash
npm install canonicalize
```

```typescript
import canonicalize from "canonicalize";

const vc = {
  "@context": ["https://www.w3.org/2018/credentials/v1"],
  "type": ["VerifiableCredential", "KYCCredential"],
  "issuer": "did:stellar:testnet:GISSUER...",
  "issuanceDate": "2026-06-28T12:00:00Z",
  "credentialSubject": {
    "id": "did:stellar:testnet:GSUBJECT...",
    "kycLevel": "basic",
    "country": "NG"
  }
};

const canonical: string = canonicalize(vc)!;
// → '{"@context":["https://www.w3.org/2018/credentials/v1"],"credentialSubject":{...},...}'
```

> **Consistency is critical.** Every party who needs to verify the credential — including the subject presenting it to a lender — must produce the same bytes. Use the same JCS library and the same JSON structure you stored off-chain. If you later add or reorder fields, the hash will change and the on-chain anchor will no longer match.

### Step 2 — SHA-256 hash the canonical bytes

```typescript
import { createHash } from "crypto";

const vcHash: Buffer = createHash("sha256")
  .update(Buffer.from(canonical, "utf8"))
  .digest(); // 32 bytes
```

### Step 3 — Verify the length

The contract expects exactly 32 bytes (`BytesN<32>`). SHA-256 always produces 32 bytes, but validate before submitting:

```typescript
if (vcHash.length !== 32) {
  throw new Error(`Expected 32 bytes, got ${vcHash.length}`);
}
```

### Putting the hash function together

```typescript
import canonicalize from "canonicalize";
import { createHash } from "crypto";

function hashVC(vc: object): Buffer {
  const canonical = canonicalize(vc);
  if (!canonical) throw new Error("canonicalize returned undefined");
  return createHash("sha256").update(Buffer.from(canonical, "utf8")).digest();
}
```

---

## Anchoring on-chain via the SDK

Once you have the 32-byte hash, call `issueVC` from the SDK. This submits a Soroban transaction to the `identity-oracle` contract.

```typescript
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";
import { Keypair } from "@stellar/stellar-sdk";

const sdk = new StellarDIDCreditSDK({
  identityOracleId: "CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
  creditOracleId:   "CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
  revocationRegistryId: "CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
  networkPassphrase: "Test SDF Network ; September 2015",
  rpcUrl: "https://soroban-testnet.stellar.org",
  simAccount: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
});

// Load your issuer keypair from an environment variable — never hardcode the secret
const issuerKeypair = Keypair.fromSecret(process.env.ISSUER_SECRET!);

const subjectAddress = "GSUBJECT1111111111111111111111111111111111111111111111111";

// Untyped anchor (defaults to 'generic' type on-chain via anchor_vc)
const txHash = await sdk.issueVC(issuerKeypair, subjectAddress, vcHash);
console.log("Anchored. Transaction:", txHash);
```

After the transaction is confirmed (typically 5–10 seconds on Stellar), any caller can verify the credential with:

```typescript
const valid = await sdk.verifyVC(subjectAddress, vcHash);
console.log("Is valid:", valid); // true
```

> **Recommendation:** While untyped anchoring via `anchor_vc` is supported for backward compatibility, all new integrations should use **typed credential anchoring** (`anchor_vc_typed`) so credentials receive appropriate credit score weighting.

---

## Typed Credential Anchoring

### Why use typed anchoring?

In the stellar-did-credit protocol, the `credit-oracle` contract calculates credit scores using a weighted formula rather than treating all credentials identically. 

When you anchor a credential without a type (via `anchor_vc`), the protocol records it with the default `generic` type, receiving baseline weighting (100 bps / 1.0×). By calling `anchor_vc_typed`, you attach an explicit type label (`Symbol` in Soroban) to the on-chain anchor in `identity-oracle`. This allows the `credit-oracle` scoring algorithm to look up the credential's type via `get_vc_credential_type` and apply type-specific score multipliers (`type_weight_bps`).

### Recognized credential type symbols

The protocol defines several standard credential type symbols:

| Credential Type Symbol | Description | Typical Multiplier (`type_weight_bps`) | Use Case |
| ---------------------- | ----------- | -------------------------------------- | -------- |
| `kyc` | Identity verification | 150 bps (1.50×) | Passport, national ID, or biometric verification by an accredited KYC provider. |
| `employment` | Income / payroll verification | 120–150 bps | Proof of active employment, salary history, or steady payroll deposits. |
| `email` | Contact verification | 100 bps (1.00×) | Verified email or electronic communication endpoint. |
| `generic` | Default / unclassified | 100 bps (1.00×) | Untyped or general-purpose credentials. |

> **Custom types:** Issuers can pass any valid Soroban `Symbol` (up to 32 characters, e.g. `license`, `academic`, `business`). Unconfigured custom symbols default to 100 bps (1.00×) on the `credit-oracle` unless the protocol admin explicitly configures a custom weight for that symbol via `set_credential_type_weight`.

### Node.js SDK usage

The `@stellar-did-credit/sdk` provides first-class support for typed anchoring via the optional 4th parameter of `issueVC`:

```typescript
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";
import { Keypair } from "@stellar/stellar-sdk";

const sdk = new StellarDIDCreditSDK({ ... });
const issuerKeypair = Keypair.fromSecret(process.env.ISSUER_SECRET!);
const subjectAddress = "GSUBJECT1111111111111111111111111111111111111111111111111";

// 1. Single typed credential anchor
const txHash = await sdk.issueVC(
  issuerKeypair,
  subjectAddress,
  vcHash,
  "kyc" // Optional credential type symbol ('kyc', 'employment', 'email', etc.)
);

console.log("Anchored typed VC. Transaction:", txHash);
```

### Batch typed anchoring

To anchor credentials for multiple subjects or different credential types efficiently in chunks of up to 10 operations per transaction:

```typescript
const batchResult = await sdk.batchAnchorVCs(issuerKeypair, [
  { subject: "GSUBJECT_1...", vcHash: vcHash1, type: "kyc" },
  { subject: "GSUBJECT_2...", vcHash: vcHash2, type: "employment" },
  { subject: "GSUBJECT_3...", vcHash: vcHash3, type: "generic" },
]);

if (batchResult.success) {
  console.log("Batch anchoring complete. Transactions:", batchResult.transactionHashes);
} else {
  console.error(`Batch completed with ${batchResult.failedChunks} failed chunk(s).`);
}
```

### Direct Soroban contract invocation

If you are calling the `identity-oracle` contract directly from Rust or a custom Soroban client:

```rust
// identity-oracle interface
pub fn anchor_vc_typed(
    env: Env,
    issuer: Address,
    subject: Address,
    vc_hash: BytesN<32>,
    credential_type: Symbol,
) -> Result<(), IdentityOracleError>;
```

### CLI usage

You can also anchor typed credentials using the `@stellar-did-credit/cli`:

```bash
did-credit vc anchor \
  --secret $ISSUER_SECRET \
  --subject GSUBJECT1111111111111111111111111111111111111111111111111 \
  --hash $VC_HASH \
  --type kyc
```

---

## Issuer Tiers and Scoring Impact

### How issuer tiers work

The protocol incorporates an admin-configurable trust multiplier for each registered issuer: `issuer_tier_bps`. This multiplier is stored on-chain in `identity-oracle` and directly scales the credit score contribution of all active credentials issued by that address.

- **Storage:** Persisted per-issuer on `identity-oracle` under `DataKey::IssuerTier(issuer)`.
- **Default value:** Unset issuer tiers default to **100 bps (1.00×)**, ensuring existing and newly registered issuers start at standard parity.
- **Maximum value:** Tiers are capped at **300 bps (3.00×)** (`MAX_ISSUER_TIER_BPS`).

### Scoring formula

When a subject's credit score is calculated on `credit-oracle`, each active credential contributes points according to both the issuer's tier and the credential type:

$$\text{credential\_points}(\text{vc}) = \text{base\_points} \times \text{issuer\_tier\_bps} \times \text{type\_weight\_bps} \div 10,000$$

$$\text{vc\_score} = \min\left( \sum \text{credential\_points}(\text{vc}), 100 \right)$$

*Note: `base_points` is fixed at 20 points per credential.*

### Multiplier matrix and examples

The table below illustrates how issuer tiers and credential types combine to determine score points per anchored credential:

| Issuer Tier | Issuer Multiplier (`issuer_tier_bps`) | Credential Type | Type Multiplier (`type_weight_bps`) | Points Contributed | Active VCs Needed for Max (100 pts) |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Tier 1 (Standard / Default)** | 100 bps (1.00×) | `generic` | 100 bps (1.00×) | **20 pts** | 5 VCs |
| **Tier 1 (Standard / Default)** | 100 bps (1.00×) | `kyc` | 150 bps (1.50×) | **30 pts** | 4 VCs (capped at 100) |
| **Tier 2 (Accredited Partner)** | 150 bps (1.50×) | `generic` | 100 bps (1.00×) | **30 pts** | 4 VCs (capped at 100) |
| **Tier 2 (Accredited Partner)** | 150 bps (1.50×) | `kyc` | 150 bps (1.50×) | **45 pts** | 3 VCs (capped at 100) |
| **Tier 3 (Regulated Institution)** | 200 bps (2.00×) | `kyc` | 150 bps (1.50×) | **60 pts** | 2 VCs (capped at 100) |

> **Retroactive scoring:** When an issuer's tier is upgraded on-chain, all previously anchored, non-revoked credentials issued by that address automatically receive the upgraded multiplier on the next `compute_score` call. There is no need to re-anchor existing credentials.

### How to request a tier upgrade

All newly registered issuers begin at **Tier 1 (100 bps)**. Issuers with formal regulatory status, accredited KYC certification, or audited track records can request a tier upgrade:

1. **Prepare verification documentation:** Assemble evidence of institutional accreditation, financial/banking licenses, SOC 2/ISO certifications, or third-party KYC audit reports.
2. **Submit an upgrade proposal:** Submit a governance proposal or contact protocol administrators with your issuer Stellar address and documentation.
3. **Admin configuration:** Following review and governance approval, a protocol admin executes:
   ```rust
   identity_oracle.set_issuer_tier(admin, issuer_address, weight_bps);
   ```
4. **Verify your active tier:** Query your current tier using the SDK or contract reader:
   ```typescript
   const tierBps = await sdk.getIssuerTier(issuerAddress);
   console.log(`Current issuer tier: ${tierBps} bps (${tierBps / 100}x)`);
   ```

---

## Key management best practices

Your issuer keypair is a signing key that directly controls which credential hashes appear on-chain under your identity. Compromising it lets an attacker anchor fraudulent credentials as if they came from you.

### Storage

- **Never store the secret key in source code, environment files committed to version control, or logs.**
- In production, use a Hardware Security Module (HSM) or a cloud KMS (AWS KMS, GCP Cloud HSM, Azure Key Vault). The Stellar SDK supports external signers that never expose the raw secret.
- In development and CI environments, store the secret in a secrets manager (GitHub Secrets, HashiCorp Vault) and inject it at runtime via an environment variable.

### Key rotation

- Generate a new keypair when rotating. Register the new address with the protocol admin (`register_issuer`) before decommissioning the old one to avoid a gap in your ability to issue.
- Deregister the old keypair (`deregister_issuer`) once all in-flight operations are complete. Existing anchored VCs from the old key remain valid.

### Separation of duties

- Use a dedicated issuer keypair that is not used for any other purpose (payments, DID anchoring, etc.).
- The account only needs enough XLM to cover transaction fees (a few stroops per transaction). Keep the balance minimal.
- If your platform has multiple credential types or divisions, consider a separate issuer keypair per product line. Each must be registered independently.

### Monitoring

- Subscribe to Stellar Horizon event streams for your issuer address and alert on any `anchor_vc` or `anchor_vc_typed` invocation you did not initiate.
- Maintain an internal audit log mapping every anchored `txHash` to the subject, credential type, and the off-chain copy of the VC.

---

## Revoking a credential

If a credential is no longer valid (the user's KYC has lapsed, a document expired, or there was a data error), you must revoke it.

You can revoke a credential using the SDK's `revokeVC` method, which atomically marks the credential hash as revoked in both the `revocation-registry` and the `identity-oracle`:

```typescript
try {
  const txHash = await sdk.revokeVC(
    issuerKeypair,
    vcHash
  );
  console.log(`Credential revoked successfully. TX: ${txHash}`);
} catch (error) {
  console.error("Failed to revoke credential:", error);
  // Handle specific errors, e.g., signature failure, authorization failure, or invalid VC hash
}
```

The SDK submits one Soroban operation to `revocation-registry.revoke`; the registry then calls `identity-oracle.mark_vc_revoked` within the same invocation. Soroban transactions execute atomically, so a contract error discards all state changes. `revokeVC` waits for final transaction confirmation and throws a descriptive error if either contract fails. See Stellar's [transaction simulation and atomicity documentation](https://developers.stellar.org/docs/learn/fundamentals/contract-development/contract-interactions/transaction-simulation).

---

## VC Anchor Limit

Each subject can have a maximum of **100 active (non-revoked) VCs** anchored at any time.

### What counts toward the limit
- Any non-revoked VC anchored via `anchor_vc` or `anchor_vc_typed`

### What does NOT count toward the limit
- Revoked VCs (marked via `mark_vc_revoked`)
- Duplicate (issuer, vc_hash) pairs (these are no-ops)

### Recovery flow when the cap is reached
1. The subject (or issuer) calls `mark_vc_revoked` on old/unused VCs
2. Once active VC count drops below 100, new VCs can be anchored

### Error
If a new anchor would exceed the cap, the contract returns:
`IdentityOracleError::VCLimitReached` (error code 10)

---

## Full working example

A complete, runnable issuer script is in [`packages/issuer-example/`](../packages/issuer-example/). It takes a subject address and credential claims from the command line, hashes the VC, and anchors it as a typed credential in one step.

```bash
cd packages/issuer-example
npm install
ISSUER_SECRET=YOUR_STELLAR_SECRET_KEY npm run issue -- \
  --subject GSUBJECT1111111111111111111111111111111111111111111111111 \
  --kyc-level basic \
  --country NG \
  --type kyc
```

### Complete programmatic example (TypeScript)

```typescript
import { StellarDIDCreditSDK } from "@stellar-did-credit/sdk";
import { Keypair } from "@stellar/stellar-sdk";
import canonicalize from "canonicalize";
import { createHash } from "crypto";

function hashVC(vc: object): Buffer {
  const canonical = canonicalize(vc);
  if (!canonical) throw new Error("Canonicalization failed");
  return createHash("sha256").update(Buffer.from(canonical, "utf8")).digest();
}

async function main() {
  const sdk = new StellarDIDCreditSDK({
    identityOracleId: process.env.IDENTITY_ORACLE_ID!,
    creditOracleId: process.env.CREDIT_ORACLE_ID!,
    revocationRegistryId: process.env.REVOCATION_REGISTRY_ID!,
    networkPassphrase: "Test SDF Network ; September 2015",
    rpcUrl: "https://soroban-testnet.stellar.org",
  });

  const issuerKeypair = Keypair.fromSecret(process.env.ISSUER_SECRET!);
  const subjectAddress = "GSUBJECT1111111111111111111111111111111111111111111111111";

  // 1. Build W3C JSON-LD credential
  const vc = {
    "@context": ["https://www.w3.org/2018/credentials/v1"],
    "type": ["VerifiableCredential", "KYCCredential"],
    "issuer": `did:stellar:testnet:${issuerKeypair.publicKey()}`,
    "issuanceDate": new Date().toISOString(),
    "credentialSubject": {
      "id": `did:stellar:testnet:${subjectAddress}`,
      "kycLevel": "enhanced",
      "country": "NG",
    },
  };

  // 2. Hash canonical JSON
  const vcHash = hashVC(vc);

  // 3. Anchor as a typed 'kyc' credential on-chain
  const txHash = await sdk.issueVC(
    issuerKeypair,
    subjectAddress,
    vcHash,
    "kyc"
  );
  console.log("VC successfully anchored! TX:", txHash);

  // 4. Verify the credential anchor
  const isValid = await sdk.verifyVC(subjectAddress, vcHash);
  console.log("On-chain verification status:", isValid);
}

main().catch(console.error);
```

See [`packages/issuer-example/README.md`](../packages/issuer-example/README.md) for full setup instructions.

---

## See also

- [VC weighting design](vc-weighting-design.md) — scoring formula, credential type weights, and issuer tier multipliers
- [DID method specification](did-spec.md) — DID document format and anchoring
- [Architecture overview](architecture.md) — how the three contracts interact
- [W3C Verifiable Credentials Data Model](https://www.w3.org/TR/vc-data-model/)
- [RFC 8785 — JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785)
