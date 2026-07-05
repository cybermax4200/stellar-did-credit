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
IDENTITY_ORACLE_ID=CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX \
CREDIT_ORACLE_ID=CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX \
REVOCATION_REG_ID=CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX \
npm run issue -- \
  --subject GSUBJECTADDRESSXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX \
  --kyc-level basic \
  --country NG
```

### Environment variables

| Variable             | Required | Default                           | Description |
| -------------------- | -------- | --------------------------------- | ----------- |
| `ISSUER_SECRET`      | Yes      | —                                 | Secret key of the registered issuer |
| `IDENTITY_ORACLE_ID` | Yes      | Placeholder C...                  | identity-oracle contract address |
| `CREDIT_ORACLE_ID`   | Yes      | Placeholder C...                  | credit-oracle contract address |
| `REVOCATION_REG_ID`  | Yes      | Placeholder C...                  | revocation-registry contract address |
| `NETWORK_PASSPHRASE` | No       | Stellar testnet passphrase        | Set to mainnet passphrase for mainnet |
| `RPC_URL`            | No       | `https://soroban-testnet.stellar.org` | Soroban RPC endpoint |
| `SIM_ACCOUNT`        | No       | Well-known funded testnet address | Fee source for read-only simulations |

Contract addresses for the current testnet deployment are in [deployments.testnet.json](../../deployments.testnet.json).

## What the script does

1. Builds a `KYCCredential` JSON-LD document with the subject DID, KYC level, and country.
2. Canonicalizes it using [RFC 8785 JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785).
3. Computes the SHA-256 hash of the canonical bytes.
4. Calls `sdk.issueVC(issuerKeypair, subjectAddress, vcHash)`, which submits a Soroban transaction invoking `anchor_vc` on the identity-oracle contract.
5. Calls `sdk.verifyVC` to confirm the anchor is readable on-chain.
6. Prints the off-chain record (VC + hash + transaction hash) that you should persist in your database.

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
