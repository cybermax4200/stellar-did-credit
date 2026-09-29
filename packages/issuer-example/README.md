# issuer-example

Minimal working example of the stellar-did-credit issuer flow. It builds a KYC Verifiable Credential as JSON-LD, hashes it with SHA-256, and anchors the hash on the Stellar testnet using the `@stellar-did-credit/sdk`.

For a thorough explanation of each step, see [docs/issuer-guide.md](../../docs/issuer-guide.md).

## Prerequisites

- Node.js 18+
- pnpm
- A funded Stellar testnet keypair that has been registered as a trusted issuer on the identity-oracle contract

## Setup

```bash
# From the repo root
pnpm install

# Or from this directory
npm install
```

## Usage

```bash
ISSUER_SECRET=YOUR_ISSUER_SECRET_KEY \
IDENTITY_ORACLE_ID=C... \
CREDIT_ORACLE_ID=C... \
REVOCATION_REG_ID=C... \
npm run issue -- \
  --subject GSUBJECT... \
  --kyc-level basic \
  --country NG \
  --revoke
```

### Environment variables

| Variable             | Required | Default                               | Description |
| -------------------- | -------- | ------------------------------------- | ----------- |
| `ISSUER_SECRET`      | Yes      | —                                     | Secret key of the registered issuer |
| `IDENTITY_ORACLE_ID` | Yes      | —                                     | identity-oracle contract address |
| `CREDIT_ORACLE_ID`   | Yes      | —                                     | credit-oracle contract address |
| `REVOCATION_REG_ID`  | Yes      | —                                     | revocation-registry contract address |
| `NETWORK_PASSPHRASE` | No       | Stellar testnet passphrase            | Set to mainnet passphrase for mainnet |
| `RPC_URL`            | No       | `https://soroban-testnet.stellar.org` | Soroban RPC endpoint |
| `SIM_ACCOUNT`        | No       | Well-known funded testnet address     | Fee source for read-only simulations |

The script exits immediately with a clear error message if any required variable is missing. Contract addresses for the current testnet deployment are in [deployments.testnet.json](../../deployments.testnet.json).

## What the script does

1. Builds a `KYCCredential` JSON-LD document from the subject address, KYC level, and country (see [docs/issuer-guide.md](../../docs/issuer-guide.md) for the canonical format).
2. Canonicalizes it using [RFC 8785 JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785).
3. Computes the SHA-256 hash of the canonical bytes (`crypto.createHash('sha256').update(canonicalJson).digest()`).
4. Calls `sdk.verifyVC(subjectAddress, vcHash)` — if the hash is already anchored, prints `VC already anchored, skipping.` and exits without submitting a duplicate.
5. Otherwise calls `sdk.issueVC(issuerKeypair, subjectAddress, vcHash)`, which submits a Soroban transaction invoking `anchor_vc` on the identity-oracle contract.
6. Calls `sdk.verifyVC` again to confirm the anchor is readable on-chain.
7. When `--revoke` is set, calls `sdk.revokeVC(issuerKeypair, vcHash)` and waits for final transaction confirmation.
8. Prints the off-chain record (VC + hash + transaction hashes) that you should persist in your database.

## Idempotent hashing

The script is **idempotent**: running it twice with the same VC content produces the same hash, and the second run skips anchoring if the hash is already on-chain.

Timestamps are **not** generated at runtime. Instead, `--issuance-date` and `--verified-at` default to the stable example values from the issuer guide (`2026-06-28T12:00:00Z` and `2026-06-28T10:00:00Z`). Override them only when you intentionally issue a new credential with different metadata:

```bash
npm run issue -- \
  --subject GSUBJECT... \
  --kyc-level basic \
  --country NG \
  --issuance-date 2026-06-28T12:00:00Z \
  --verified-at 2026-06-28T10:00:00Z
```

The hash function lives in `src/hash.ts` and matches the issuer guide:

```typescript
const canonical = canonicalize(vc);
const vcHash = createHash("sha256")
  .update(Buffer.from(canonical, "utf8"))
  .digest();
```

## Storing the off-chain record

The on-chain entry is just a hash. You must store the plaintext VC alongside it so that:
- The subject can present the credential to lenders.
- Lenders can reproduce the hash and confirm it matches the on-chain anchor.

A minimal off-chain record looks like:

```json
{
  "vcHash": "a3f9...",
  "txHash": "abc123...",
  "subject": "GSUBJECT...",
  "issuer": "GISSUER...",
  "anchoredAt": "2026-06-28T12:00:00.000Z",
  "vc": { ... }
}
```
