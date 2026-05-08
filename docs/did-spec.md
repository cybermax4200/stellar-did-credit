# Stellar DID Method Specification

This document defines the `did:stellar` method, which utilizes the Stellar network and IPFS to provide decentralized identity.

## 1. DID Method Format

The DID method name is `stellar`. The method-specific identifier is composed of the network identifier and the Stellar account address (G-address).

**Format:** `did:stellar:<network>:<account_address>`

- **network**: Either `mainnet` or `testnet`.
- **account_address**: A standard Stellar public key (G-address).

**Example:**
`did:stellar:testnet:GABC123...`

## 2. DID Document Structure

DID documents are JSON-LD files stored on IPFS. They MUST comply with the W3C DID Core specification.

### 2.1 Required Fields

- `id`: The DID itself.
- `verificationMethod`: An array of public keys used for authentication and verification.
- `authentication`: References to verification methods allowed for authentication.

### 2.2 Example Document

```json
{
  "@context": [
    "https://www.w3.org/ns/did/v1",
    "https://w3id.org/security/suites/ed25519-2020/v1"
  ],
  "id": "did:stellar:testnet:GABC123...",
  "verificationMethod": [
    {
      "id": "did:stellar:testnet:GABC123...#keys-1",
      "type": "Ed25519VerificationKey2020",
      "controller": "did:stellar:testnet:GABC123...",
      "publicKeyMultibase": "z6MkmL..."
    }
  ],
  "authentication": ["did:stellar:testnet:GABC123...#keys-1"],
  "assertionMethod": ["did:stellar:testnet:GABC123...#keys-1"]
}
```

## 3. Operations (CRUD)

### 3.1 Create

1. The subject generates a Stellar keypair.
2. The subject creates a DID Document according to Section 2.
3. The subject uploads the document to IPFS.
4. The subject calls the `anchor_did` function on the `identity-oracle` contract, passing the CID.

### 3.2 Read (Resolve)

To resolve a `did:stellar` identifier:

1. **Parse DID**: Extract the network and Stellar address.
2. **Fetch Anchor**: Call the `identity-oracle` contract's `anchor_did` stores the CID in persistent storage under `DIDDocument(subject)`. To resolve, read this storage entry directly via RPC (`getLedgerEntries`) to retrieve the current IPFS CID.
3. **Fetch Content**: Retrieve the DID Document from IPFS using the CID.
4. **Validate**: Confirm the `id` field in the document matches the requested DID.

### 3.3 Update

1. The subject generates a new DID Document (e.g., to rotate keys or add service endpoints).
2. The subject uploads the new document to IPFS.
3. The subject calls `anchor_did` with the new CID. This overwrites the previous state in the contract.

### 3.4 Deactivate

1. The subject calls a deactivation function (to be implemented) or anchors a null/empty CID to indicate deactivation.

## 4. Security Considerations

### 4.1 Authentication and Authorization

All updates to the DID anchor MUST require a valid signature from the Stellar address associated with the DID. This is enforced by the `identity-oracle` contract using `subject.require_auth()`.

### 4.2 Replay Attacks

The Stellar network's native transaction sequence numbers prevent replay attacks at the network level.

### 4.3 Key Compromise

If the Stellar private key is compromised, the attacker can update the DID anchor. Users are encouraged to use Stellar's multi-signature capabilities to secure their accounts.

## 5. Privacy Considerations

### 5.1 PII (Personally Identifiable Information)

Subjects SHOULD NOT include PII in their DID Documents, as IPFS data is public and immutable once uploaded.

### 5.2 Linkability

By anchoring the DID to a Stellar address, the identity is permanently linked to the history of that Stellar account. Users seeking higher privacy should use fresh Stellar accounts for their DIDs.

## 6. Reference Implementation

The canonical implementation of the anchor contract is located in `contracts/identity-oracle/`.
