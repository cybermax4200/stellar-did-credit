# Issuer Integration Guide

This guide shows how a credential issuer — a KYC provider, payroll platform, or microfinance institution — formats, hashes, and anchors a Verifiable Credential (VC) using the stellar-did-credit protocol.

The protocol stores only a 32-byte SHA-256 hash of the credential on-chain. The full JSON-LD document stays off-chain (on your servers or IPFS), preserving user privacy while making the credential verifiable by any third party who receives a copy.

## Table of contents

- [Prerequisites](#prerequisites)
- [VC JSON-LD format](#vc-json-ld-format)
- [Hashing the credential](#hashing-the-credential)
- [Anchoring on-chain via the SDK](#anchoring-on-chain-via-the-sdk)
- [Key management best practices](#key-management-best-practices)
- [Revoking a credential](#revoking-a-credential)
- [Full working example](#full-working-example)

---

## Prerequisites

Before you can anchor credentials, the protocol admin must register your Stellar address as a trusted issuer by calling `register_issuer(admin, issuer_address)` on the identity-oracle contract. Until that transaction is confirmed, `anchor_vc` will reject your calls with `IssuerNotAuthorized`.

You will also need:

- A funded Stellar testnet account (to pay transaction fees). Fund it via [Friendbot](https://friendbot.stellar.org/?addr=YOUR_ADDRESS).
- Node.js 18+
- The `@stellar-did-credit/sdk` package

```bash
npm install @stellar-did-credit/sdk @stellar/stellar-sdk
```

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

The on-chain `anchor_vc` function takes a `BytesN<32>`, which is the raw SHA-256 digest of the **canonicalized** credential JSON. Canonicalization ensures the same logical document always produces the same hash regardless of key ordering or whitespace differences.

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

Once you have the 32-byte hash, call `issueVC` from the SDK. This submits a Soroban transaction that invokes `anchor_vc(issuer, subject, vc_hash)` on the identity-oracle contract.

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

const txHash = await sdk.issueVC(issuerKeypair, subjectAddress, vcHash);
console.log("Anchored. Transaction:", txHash);
```

After the transaction is confirmed (typically 5–10 seconds on Stellar), any caller can verify the credential with:

```typescript
const valid = await sdk.verifyVC(subjectAddress, vcHash);
console.log("Is valid:", valid); // true
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

- Subscribe to Stellar Horizon event streams for your issuer address and alert on any `anchor_vc` invocation you did not initiate.
- Maintain an internal audit log mapping every anchored `txHash` to the subject, credential type, and the off-chain copy of the VC.

---

## Revoking a credential

If a credential is no longer valid (the user's KYC has lapsed, a document expired, or there was a data error), you must revoke it. Two paths exist:

**Via the identity-oracle** (marks the hash as revoked in the VC record):

```typescript
// Not yet in the SDK — call via Soroban directly, or wait for revokeVC in SDK v0.2
```

**Via the revocation-registry** (independent on-chain registry that any verifier can check):

```typescript
// anchor_vc does not automatically consult the revocation-registry;
// the registry is intended for verifiers and lenders to query independently.
```

Until `revokeVC` is added to the SDK, you can invoke `mark_vc_revoked(issuer, subject, vc_hash)` on the identity-oracle contract directly using `stellar-cli`:

```bash
stellar contract invoke \
  --id CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX \
  --source-account ISSUER_KEY_NAME \
  --network testnet \
  -- mark_vc_revoked \
  --issuer YOUR_G_ADDRESS \
  --subject SUBJECT_G_ADDRESS \
  --vc_hash HEX_ENCODED_HASH
```

---

## Full working example

A complete, runnable issuer script is in [`packages/issuer-example/`](../packages/issuer-example/). It takes a subject address and credential claims from the command line, hashes the VC, and anchors it in one step.

```bash
cd packages/issuer-example
npm install
ISSUER_SECRET=YOUR_STELLAR_SECRET_KEY npm run issue -- --subject GSUBJECT... --kyc-level basic --country NG
```

See [`packages/issuer-example/README.md`](../packages/issuer-example/README.md) for full setup instructions.

---

## See also

- [DID method specification](did-spec.md) — DID document format and anchoring
- [Architecture overview](architecture.md) — how the three contracts interact
- [W3C Verifiable Credentials Data Model](https://www.w3.org/TR/vc-data-model/)
- [RFC 8785 — JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785)
