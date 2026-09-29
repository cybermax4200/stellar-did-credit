# Stellar DID Method Specification

This document defines the `did:stellar` method, which utilizes the Stellar network and IPFS to provide decentralized identity.

## 1. DID Method Format

The DID method name is `stellar`. The method-specific identifier is composed of the network identifier and the Stellar account address (G-address).

**Format:** `did:stellar:<network>:<account_address>`

- **network**: Either `mainnet` or `testnet`.
- **account_address**: A standard Stellar public key (G-address).

**Example:**
`did:stellar:testnet:GABC123...`

## 2. DID Document Schema

DID documents are JSON-LD files stored on IPFS. They MUST comply with the W3C DID Core specification.

### 2.1 DID Document Schema

- **`@context`** (array): JSON-LD context URLs. MUST include `https://www.w3.org/ns/did/v1` and any credential-specific contexts (e.g., `https://w3id.org/security/suites/ed25519-2020/v1` for Ed25519 keys).
  - Reference: [W3C DID Core — Context](https://www.w3.org/TR/did-core/#context)

- **`id`** (string): The DID identifier. MUST match the DID being resolved: `did:stellar:<network>:<account_address>`. This prevents substitution attacks.
  - Reference: [W3C DID Core — Identifier](https://www.w3.org/TR/did-core/#did-identifier)

- **`verificationMethod`** (array): An array of public keys and cryptographic material used for authentication and verification. Each entry describes one key with:
  - `id` (string): Unique identifier for this method, typically `<did>#<fragment>` (e.g., `did:stellar:testnet:GABC123...#keys-1`).
  - `type` (string): The cryptographic suite type (e.g., `Ed25519VerificationKey2020`).
  - `controller` (string): The DID that controls this key, typically the subject's own DID.
  - `publicKeyMultibase` (string): The public key encoded in multibase format (e.g., `z6MkmL...` for Ed25519).
  - Reference: [W3C DID Core — Verification Method](https://www.w3.org/TR/did-core/#verification-method)

- **`authentication`** (array): An array of references to verification methods in this document that are allowed for authentication. Typically references entries from `verificationMethod` by their `id`.
  - Reference: [W3C DID Core — Authentication](https://www.w3.org/TR/did-core/#authentication)

### 2.2 Optional Fields

- **`service`** (array): An array of service endpoints for off-chain interactions (e.g., messaging, data retrieval). Each entry describes one service with:
  - `id` (string): Unique identifier (e.g., `did:stellar:testnet:GABC123...#credentials`).
  - `type` (string): The service type (e.g., `CredentialService`, `MessagingService`).
  - `serviceEndpoint` (string or object): The URL or endpoint configuration for this service.
  - Reference: [W3C DID Core — Service Endpoint](https://www.w3.org/TR/did-core/#service-endpoint)

- **`assertionMethod`** (array): References to verification methods allowed for assertion (signing claims or credentials). Typically used by issuers.
  - Reference: [W3C DID Core — Assertion Method](https://www.w3.org/TR/did-core/#assertion-method)

- **`keyAgreement`** (array): References to verification methods for key agreement (encryption). Used for encrypted communications.
  - Reference: [W3C DID Core — Key Agreement](https://www.w3.org/TR/did-core/#key-agreement)

### 2.3 Complete Example Document

```json
{
  "@context": [
    "https://www.w3.org/ns/did/v1",
    "https://w3id.org/security/suites/ed25519-2020/v1"
  ],
  "id": "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP",
  "verificationMethod": [
    {
      "id": "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP#keys-1",
      "type": "Ed25519VerificationKey2020",
      "controller": "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP",
      "publicKeyMultibase": "z6MkmL7R6S8TuvA2FzTvxY9ZaAb2CdEfG3HiJkLmNoPqRsT"
    },
    {
      "id": "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP#keys-2",
      "type": "Ed25519VerificationKey2020",
      "controller": "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP",
      "publicKeyMultibase": "z6NnmN8S9VwBgHcIjKlMnOpQrStUvWxYzAbCdEfGhIjKlM"
    }
  ],
  "authentication": [
    "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP#keys-1"
  ],
  "assertionMethod": [
    "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP#keys-1"
  ],
  "keyAgreement": [
    "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP#keys-2"
  ],
  "service": [
    {
      "id": "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP#credentials",
      "type": "CredentialService",
      "serviceEndpoint": "https://issuer.example.com/credentials"
    },
    {
      "id": "did:stellar:testnet:GABC123DEFG456HIJ789KLMNOP234RST567UVWXYZ890ABCDEFGH234IJKLMNOP#messaging",
      "type": "MessagingService",
      "serviceEndpoint": "https://messaging.example.com/inbox"
    }
  ]
}
```

### 2.4 JSON Schema for Validation

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "required": ["@context", "id", "verificationMethod", "authentication"],
  "properties": {
    "@context": {
      "type": "array",
      "items": { "type": "string" },
      "minItems": 1
    },
    "id": {
      "type": "string",
      "pattern": "^did:stellar:(mainnet|testnet):[GAID][A-Z0-9]{55}$"
    },
    "verificationMethod": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "type", "controller", "publicKeyMultibase"],
        "properties": {
          "id": { "type": "string" },
          "type": { "type": "string" },
          "controller": { "type": "string" },
          "publicKeyMultibase": { "type": "string" }
        }
      }
    },
    "authentication": {
      "type": "array",
      "items": { "type": "string" }
    },
    "assertionMethod": {
      "type": "array",
      "items": { "type": "string" }
    },
    "keyAgreement": {
      "type": "array",
      "items": { "type": "string" }
    },
    "service": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "type", "serviceEndpoint"],
        "properties": {
          "id": { "type": "string" },
          "type": { "type": "string" },
          "serviceEndpoint": {}
        }
      }
    }
  }
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

### 3.1.1 CID Validation

The `anchor_did` function validates the `did_doc_cid` argument before storing it.
Invalid CIDs are rejected with `IdentityOracleError::InvalidCID` so that DID resolution
does not silently fail on malformed IPFS pointers.

**Accepted formats:**

- **CIDv0** — IPFS CID version 0, base58btc-encoded multihash. The string must
  start with `Qm` and be exactly 46 characters long.
- **CIDv1** — IPFS CID version 1, multibase base32 (lowercase). The string must
  start with `bafy`.
- **URI form** — either of the above prefixed with `ipfs://`.

**Length bounds:** the stored string must be between 7 and 128 bytes
(`MAX_CID_LENGTH`). The 7-byte lower bound rejects empty strings and trivially
short garbage; the upper bound keeps the storage slot bounded.

**Out of scope:** the contract does not perform full multibase / multihash parsing
or checksum verification. Prefix and length checks are sufficient to reject
malformed pointers at anchor time. Off-chain resolvers MUST still validate the
CID checksum before fetching from IPFS.

### 3.2 Read (Resolve)

To resolve a `did:stellar` identifier:

1. **Parse DID**: Extract the network and Stellar address.
2. **Fetch Anchor**: Call the `identity-oracle` contract's `anchor_did` stores the CID in persistent storage under `DIDDocument(subject)`. To resolve, read this storage entry directly via RPC (`getLedgerEntries`) to retrieve the current IPFS CID.
3. **Fetch Content**: Retrieve the DID Document from IPFS using the CID.
4. **Validate**: Confirm the `id` field in the document matches the requested DID.

### 3.3 Update

DID documents in the Stellar DID Credit protocol are **mutable**. Subjects may update their
DID document at any time by calling `anchor_did` again.

1. The subject generates a new DID Document (e.g., to rotate keys or add service endpoints).
2. The subject uploads the new document to IPFS.
3. The subject calls `anchor_did` with the new CID. This **silently overwrites** the
   previous CID in contract storage.
4. The `DIDAnch` event is emitted on each call, allowing consumers to track updates
   via event streams. However, consumers SHOULD read the current CID from storage rather
   than relying solely on historical events, since multiple updates are possible.

### 3.4 DID Deactivation

The Stellar DID protocol provides two distinct on-chain deactivation entry points on the `identity-oracle` contract: [`deactivate_did`](file:///Users/tejaschavan1907/stellar-did-credit/contracts/identity-oracle/src/lib.rs#L734) and [`deactivate_identity`](file:///Users/tejaschavan1907/stellar-did-credit/contracts/identity-oracle/src/lib.rs#L939). Both functions enforce subject signature authorization (`subject.require_auth()`) and share foundational revocation logic, but they differ in storage retention of the DID document, event emission, return types, and intended operational lifecycles.

#### 3.4.1 Shared Deactivation Core (`deactivate_core`)

When either `deactivate_did` or `deactivate_identity` is invoked, the contract executes [`deactivate_core`](file:///Users/tejaschavan1907/stellar-did-credit/contracts/identity-oracle/src/lib.rs#L398), which enforces the following state transitions:

1. **Sets Deactivation Flag**: Writes `true` to `DataKey::Deactivated(subject)` in persistent storage. As a result, [`is_deactivated(subject)`](file:///Users/tejaschavan1907/stellar-did-credit/contracts/identity-oracle/src/lib.rs#L907) returns `true`, and [`is_verified(subject)`](file:///Users/tejaschavan1907/stellar-did-credit/contracts/identity-oracle/src/lib.rs#L978) immediately returns `false`.
2. **Revokes All Anchored Verifiable Credentials**: Iterates over all active credential anchors (`DataKey::VCAnchors(subject)`) and transitions each record to `revoked = true`.
3. **Clears Active VC Count**: Resets `DataKey::ActiveVCCount(subject)` to `0`, ensuring [`get_vc_count(subject)`](file:///Users/tejaschavan1907/stellar-did-credit/contracts/identity-oracle/src/lib.rs#L998) returns `0`.
4. **Bumps Protocol Revocation Counter**: Increments the protocol-wide `total_vcs_revoked` metric by the count of active VCs transitioned to revoked.
5. **Records State Change Timestamp**: Updates `DataKey::LastStateChange(subject)` with the current ledger sequence (`env.ledger().sequence()`). Downstream contracts such as `credit-oracle` rely on this timestamp to invalidate cached credit scores.

#### 3.4.2 Behavioral Differences: `deactivate_did` vs `deactivate_identity`

The two entry points diverge in DID Document storage retention, return values, and event notifications:

| Dimension | `deactivate_did(subject)` | `deactivate_identity(subject)` |
| :--- | :--- | :--- |
| **Primary Purpose** | Canonical, permanent-style DID deactivation conforming to W3C DID Core | Operational / temporary identity suspension and credential flush |
| **DID Document CID in Storage** | **Removed** (`DataKey::DIDDocument` deleted) | **Preserved** (`DataKey::DIDDocument` untouched) |
| **DID Resolution (`get_did_document`)** | Returns `None` (unanchored / 404) | Returns `Some(CID)` (still resolvable on-chain) |
| **Verifiable Credentials** | All active anchored VCs marked `revoked = true` | All active anchored VCs marked `revoked = true` |
| **`is_deactivated` Flag** | Set to `true` | Set to `true` |
| **`is_verified` Result** | Returns `false` | Returns `false` |
| **Credit Scoring Impact** | Suppressed in `credit-oracle` | Suppressed in `credit-oracle` |
| **Return Type** | `Result<(), IdentityOracleError>` | `u32` (count of VCs revoked) |
| **Emitted Event Topic** | `[Symbol("DIDDeact")]` | `[Symbol("IdDeact")]` |
| **Emitted Event Data** | `subject: Address` | `(subject: Address, revoked_count: u32)` |
| **Reactivation Restoration** | Must re-anchor DID document + reissue VCs | Only reissue VCs (DID document already present) |

#### 3.4.3 When to Use Each Function

- **Use `deactivate_did` when:**
  - **Full DID Lifecycle Termination:** The subject is permanently decommissioning their decentralized identifier or keypair.
  - **W3C DID Deactivation Compliance:** Off-chain resolvers must receive `None` / deactivated status when resolving `did:stellar:<network>:<address>`.
  - **Key Compromise or Complete Teardown:** The private key or DID document is compromised and the document pointer must be purged from on-chain state immediately.

- **Use `deactivate_identity` when:**
  - **Temporary Suspension:** The subject wants to pause credential verification and suppress credit score computation without deleting their published DID Document pointer.
  - **Credential Flush / Reset:** The subject needs to invalidate all active credentials in one atomic call while keeping the DID document resolvable for public identity queries, history, or auditing.
  - **Programmatic Revocation Feedback:** Integrators need immediate on-chain return values or event payloads containing the exact count of credentials revoked (`revoked_count`).

#### 3.4.4 State Restoration via `reactivate_identity`

A subject who previously deactivated their identity can restore active status by calling [`reactivate_identity(subject)`](file:///Users/tejaschavan1907/stellar-did-credit/contracts/identity-oracle/src/lib.rs#L961) with valid subject authorization (`subject.require_auth()`).

```
[ Active Identity ]
       │
       ├──── deactivate_did ────────► [ DID Deactivated: Doc Removed, VCs Revoked, Flag=True ]
       │                                     │
       ├──── deactivate_identity ───► [ Identity Suspended: Doc Kept, VCs Revoked, Flag=True ]
       │                                     │
       ▼                                     │
 reactivate_identity ◄───────────────────────┘
       │
       ▼
 [ Reactivated: Flag Cleared ]
       ├── If deactivate_identity was used: DID Doc is intact; VCs must be re-anchored by issuers
       └── If deactivate_did was used: Subject must call anchor_did; VCs must be re-anchored by issuers
```

**State semantics of `reactivate_identity`:**
1. **What is restored:**
   - Clears `DataKey::Deactivated(subject)`, causing [`is_deactivated(subject)`](file:///Users/tejaschavan1907/stellar-did-credit/contracts/identity-oracle/src/lib.rs#L907) to return `false`.
   - Records the state change ledger sequence in `DataKey::LastStateChange(subject)`.
   - Emits `IdReact` event (`topic: [Symbol("IdReact")], data: subject`).
   - If the identity was deactivated via `deactivate_identity`, the DID document CID remained in storage throughout and is immediately resolvable.
2. **What is NOT restored (Critical for Integrators):**
   - **Verifiable Credentials Remain Revoked:** Calling `reactivate_identity` does **not** un-revoke previously revoked VCs. All historical records remain `revoked = true` to prevent replaying stale or invalidated credentials. Issuers must explicitly issue and anchor new credentials via `anchor_vc` or `anchor_vc_typed`.
   - **DID Document after `deactivate_did`:** If the subject was deactivated using `deactivate_did`, the DID document CID was removed from persistent storage. `reactivate_identity` does **not** restore the document CID; the subject must call [`anchor_did(subject, did_doc_cid)`](file:///Users/tejaschavan1907/stellar-did-credit/contracts/identity-oracle/src/lib.rs#L702) to re-anchor their DID document.

## 4. Security Considerations

### 4.1 Authentication and Authorization

All updates to the DID anchor MUST require a valid signature from the Stellar address associated with the DID. This is enforced by the `identity-oracle` contract using `subject.require_auth()`.

**Overwrite semantics:** The `anchor_did` function allows unconditional overwrites. Any subject
with signing authority can replace their DID CID at any time without restrictions. There is no
versioning or history retention in-contract; external systems MUST monitor `DIDAnch` events
if they need to track document evolution.

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
