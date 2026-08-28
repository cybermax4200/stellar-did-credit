# Security Model

This document consolidates the security assumptions, trust boundaries, and threat model for the stellar-did-credit protocol. It serves as a single reference for lenders, integrators, and auditors to understand who is trusted, what happens if they are compromised, and what defenses exist.

## 1. Trust Hierarchy

The protocol relies on a tiered trust model:

1. **Admin (Highest):** Controls protocol parameters, contract upgrades, and role registration.
2. **Feeders / Lenders / Issuers:** Whitelisted entities that supply off-chain data (transaction stats, repayment history, credential anchors).
3. **Subjects:** Individual users (DIDs) who accumulate credentials and receive a credit score.
4. **Anyone (Lowest):** Unauthenticated callers who can permissionlessly read scores, compute scores, or verify credentials.

## 2. Trusted Roles & Compromise Impact

### Admin
- **Capabilities:** Upgrade contract WASM, change scoring weights, register/deregister trusted data providers (feeders, lenders, issuers).
- **Compromise Damage:** Total protocol compromise. An attacker could register a malicious feeder to assign perfect scores to arbitrary addresses, or upgrade the contract to steal instance storage.

### Credential Issuers (identity-oracle)
- **Capabilities:** Anchor Verifiable Credential (VC) hashes for subjects, and revoke them.
- **Compromise Damage:** An attacker could anchor fake credentials for colluding subjects to artificially inflate their c_count score component, or maliciously revoke valid credentials of legitimate subjects.

### Data Feeders (credit-oracle)
- **Capabilities:** Push 30-day transaction statistics and update cached VC counts.
- **Compromise Damage:** An attacker could push falsified on-chain transaction volume for specific subjects, manipulating the 	x_score component.

### Lenders (credit-oracle)
- **Capabilities:** Record loan repayment outcomes (on-time or late).
- **Compromise Damage:** An attacker could spam fake "on-time" repayments to inflate a subject's epay_score, or log fake "late" payments to grief legitimate subjects.

## 3. Defenses & Mitigations

The protocol employs several mechanisms to limit the impact of compromised roles and operational errors.

* **Two-step Admin Transfer:** **(Implemented)** Prevents accidentally transferring the admin role to a dead or incorrect address. See [architecture.md](architecture.md).
* **Dispute System:** **(Implemented)** Allows subjects to flag incorrect score inputs (e.g., from a compromised or buggy feeder) on-chain. Admins can resolve disputes, prompting feeders to correct the data.
* **Governance Timelock:** **(Implemented)** Any scoring weight changes proposed via governance are subject to a 24-hour timelock (17,280 ledgers) before they can be applied, giving downstream lenders time to react. See [governance.md](governance.md) and [EPOCH_MODEL.md](EPOCH_MODEL.md).
* **Cross-Contract Reentrancy Guards:** **(Implemented)** Prevents circular dependency state corruption between the identity-oracle and revocation-registry. See [security-reentrancy.md](security-reentrancy.md).
* **Pause Mechanisms:** **(Planned - Phase 5)** Contract-level pause functionality to halt operations during an emergency.

## 4. Attack Scenarios

* **Compromised Feeder:** If a feeder's private key is leaked, the attacker can inflate transaction stats.
  * *Defense:* The admin can deregister the feeder. Existing manipulated scores will decay or can be corrected via the dispute system.
* **Fake Issuer:** A malicious entity gains issuer status and anchors fake credentials.
  * *Defense:* Lenders should verify the identity of the issuer before trusting the credit score. The admin can deregister the issuer, but currently existing anchored VCs remain valid unless revoked by the issuer.
* **Governance Spam:** An attacker spams the governance contract with malicious proposals to change weights.
  * *Defense:* Proposals require a registered voter to submit. Voting power is currently admin-assigned, preventing Sybil attacks.

## 5. Known Gaps

* **No emergency pause on credit-oracle (Issue 10):** There is currently no way to pause score computations or data ingestion if a vulnerability is found.
* **Dispute deadline (Issue 15):** The dispute system currently lacks a strict deadline for admin resolution, potentially leaving subjects in a pending state indefinitely.

## 6. Audit Scope (Phase 6)

The upcoming security audit (Phase 6) will cover:
* identity-oracle, credit-oracle, and evocation-registry smart contracts.
* Cross-contract call safety and reentrancy analysis.
* governance contract voting logic, quorum enforcement, and timelock mechanisms.
* Storage TTL management and state bloat vectors.
