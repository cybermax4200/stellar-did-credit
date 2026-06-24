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

### 2.1 DID Document Schema

The DID document uses the JSON-LD format to define decentralized identifiers and their associated metadata. All DID documents MUST conform to the W3C DID Core specification.

#### 2.1.1 Core Fields (Required)

| Field | Type | Description | Reference |
|-------|------|-------------|-----------|
| `@context` | Array of URIs | JSON-LD contexts that define the vocabulary | [W3C DID Core - Context](https://www.w3.org/TR/did-core/#contexts) |
| `id` | String (DID) | The DID identifier that corresponds to this document | [W3C DID Core - DID Subject](https://www.w3.org/TR/did-core/#did-subject) |
| `verificationMethod` | Array of Objects | Array of verification methods used for authentication and verification | [W3C DID Core - Verification Methods](https://www.w3.org/TR/did-core/#verification-methods) |
| `authentication` | Array of Strings/Objects | Array of references to verification methods for authentication purposes | [W3C DID Core - Authentication](https://www.w3.org/TR/did-core/#authentication) |

#### 2.1.2 Optional Fields

| Field | Type | Description | Reference |
|-------|------|-------------|-----------|
| `service` | Array of Objects | Array of service endpoints for interaction | [W3C DID Core - Service Endpoints](https://www.w3.org/TR/did-core/#service-endpoints) |
| `assertionMethod` | Array of Strings/Objects | Array of references to verification methods used for assertions | [W3C DID Core - Assertion Method](https://www.w3.org/TR/did-core/#assertion) |
| `keyAgreement` | Array of Strings/Objects | Array of references to verification methods for key agreement | [W3C DID Core - Key Agreement](https://www.w3.org/TR/did-core/#key-agreement) |
| `capabilityInvocation` | Array of Strings/Objects | Array of references to verification methods for capability invocation | [W3C DID Core - Capability Invocation](https://www.w3.org/TR/did-core/#capability-invocation) |
| `capabilityDelegation` | Array of Strings/Objects | Array of references to verification methods for capability delegation | [W3C DID Core - Capability Delegation](https://www.w3.org/TR/did-core/#capability-delegation) |

#### 2.1.3 Verification Method Object Structure

Each verification method in the `verificationMethod` array MUST contain:

```json
{
  "id": "did:stellar:testnet:GABC123...#keys-1",
  "type": "Ed25519VerificationKey2020",
  "controller": "did:stellar:testnet:GABC123...",
  "publicKeyMultibase": "z6MkmL..."
}
```

| Property | Type | Description |
|----------|------|-------------|
| `id` | String | Fragment identifier combining the DID and a key identifier |
| `type` | String | The type of key material (e.g., Ed25519VerificationKey2020) |
| `controller` | String | The DID that controls this verification method |
| `publicKeyMultibase` | String | The public key encoded in multibase format |

### 2.2 Complete Example Document

```json
{
  "@context": [
    "https://www.w3.org/ns/did/v1",
    "https://w3id.org/security/suites/ed25519-2020/v1"
  ],
  "id": "did:stellar:testnet:GABC123XYZ789...",
  "verificationMethod": [
    {
      "id": "did:stellar:testnet:GABC123XYZ789...#keys-1",
      "type": "Ed25519VerificationKey2020",
      "controller": "did:stellar:testnet:GABC123XYZ789...",
      "publicKeyMultibase": "z6MkmL7XEep4mNh4xhCB8EXD2xRfB7bqr7V8zEQ8aK9TqzpN"
    },
    {
      "id": "did:stellar:testnet:GABC123XYZ789...#keys-2",
      "type": "Ed25519VerificationKey2020",
      "controller": "did:stellar:testnet:GABC123XYZ789...",
      "publicKeyMultibase": "z6MkjHCYK5qNh7F3Ac9qZ2eqQ4xpN1R5vD3sM2L8nJqZ9Pxk"
    }
  ],
  "authentication": [
    "did:stellar:testnet:GABC123XYZ789...#keys-1"
  ],
  "assertionMethod": [
    "did:stellar:testnet:GABC123XYZ789...#keys-1"
  ],
  "keyAgreement": [
    "did:stellar:testnet:GABC123XYZ789...#keys-2"
  ],
  "service": [
    {
      "id": "did:stellar:testnet:GABC123XYZ789...#endpoint-1",
      "type": "VerifiableCredentialService",
      "serviceEndpoint": "https://issuer.example.com/credentials"
    }
  ]
}
```

### 2.3 JSON Schema Validation

The JSON-LD document SHOULD validate against a JSON Schema to ensure structural integrity. Key validation rules:

- The `@context` field MUST be an array containing at minimum `https://www.w3.org/ns/did/v1`
- The `id` field MUST be a valid DID matching the format `did:stellar:(mainnet|testnet):[A-Z0-9]{56}`
- All fragments in `verificationMethod[].id` MUST be unique within the document
- The `controller` field in each verification method MUST reference a valid DID
- Verification method references in `authentication`, `assertionMethod`, etc. MUST correspond to entries in `verificationMethod`

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
