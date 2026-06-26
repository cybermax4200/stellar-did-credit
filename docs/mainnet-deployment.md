# Mainnet Deployment Guide

This document covers everything required to deploy stellar-did-credit to Stellar mainnet safely. Read the full document **before** funding the deployer key.

---

## Table of contents

- [Pre-deployment security checklist](#pre-deployment-security-checklist)
- [Admin key ceremony](#admin-key-ceremony)
- [Initial weight configuration rationale](#initial-weight-configuration-rationale)
- [Feeder onboarding requirements](#feeder-onboarding-requirements)
- [Contract upgrade path](#contract-upgrade-path)
- [Monitoring and incident response](#monitoring-and-incident-response)

---

## Pre-deployment security checklist

### Audit & review

- [ ] All three contracts (`identity-oracle`, `credit-oracle`, `revocation-registry`) have passed a professional security audit.
- [ ] All findings from the audit have been remediated or explicitly accepted with documentation.
- [ ] The final audit report is published and linked from this repository.
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings on the release commit.
- [ ] `cargo test --workspace` passes with zero failures.
- [ ] All `unwrap()` calls have been eliminated — only `expect("descriptive message")` is used in contract logic.

### Environment separation

- [ ] A **dedicated mainnet deployer keypair** exists. This keypair has never been used on testnet.
- [ ] The `--network mainnet` flag is used in all `stellar-cli` commands. Never reuse a testnet deployer address on mainnet.
- [ ] `deployments.mainnet.json` is the sole record for mainnet contract addresses. It is gitignored or encrypted at rest.
- [ ] Testnet and mainnet RPC endpoints are configured in separate `stellar-cli` profiles.

### Key management

- [ ] The admin keypair is generated on a hardware wallet (see [key ceremony](#admin-key-ceremony)).
- [ ] The deployer keypair (one-time use) is discarded or locked after deployment.
- [ ] No private key material exists in plaintext on any networked machine, CI runner, or repository.

### Network readiness

- [ ] A mainnet Stellar account exists with sufficient XLM balance for contract deployment and storage rent (~500–1000 XLM recommended).
- [ ] Custom contract storage rent has been estimated and funded.
- [ ] The deployer key has been funded only from a known, auditable source (e.g., a centralized exchange with KYC).

---

## Admin key ceremony

The contract admin is the most sensitive role: it can register issuers, register feeders, register lenders, upgrade contracts, and change scoring weights. Losing the admin key or having it compromised means losing control of the protocol.

### Hardware wallet recommendation

Use a **Ledger Nano S / X / Stax** or a **Grid wallet** running a dedicated Stellar app. The admin key should:

- Be generated **offline** using the hardware wallet's true random number generator.
- Never exist as a software keypair (no `stellar keys generate`, no seed phrase digitized).
- Have its public key recorded in `deployments.mainnet.json` and published as the contract admin.

**Procedure:**

1. Initialize the hardware wallet with a new seed phrase (24 words).
2. Generate the Stellar keypair via the official Stellar app on the device.
3. Record the **public key** in multiple safe locations (e.g., encrypted password manager, printed QR code in a safe deposit box).
4. Store the seed phrase backup on a steel plate (e.g., Cryptosteel or Billfodl) in a fireproof safe. **Do not store digitally.**
5. Sign the `initialize()` transaction using the hardware wallet at deployment time.

### Multisig recommendation

For production, do not rely on a single admin key. Use Stellar's built-in multisig on the admin account:

| Signer                    | Weight | Entity            |
| ------------------------- | ------ | ----------------- |
| Hardware wallet (primary) | 10     | Protocol maintainer |
| Hardware wallet (backup)  | 10     | Protocol maintainer |
| Time-locked recovery      | 20     | Smart contract (see below) |

**Threshold:** `master_weight = 0`, `low_threshold = 0`, `med_threshold = 10`, `high_threshold = 20`.

- Medium threshold operations (register issuer, register feeder) require **1 primary signer**.
- High threshold operations (upgrade, initialize) require **2 primary signers** or **1 time-locked recovery**.

This prevents a single compromised device from upgrading the contract.

### Key compromise procedures

If an admin key is suspected compromised:

1. **Immediately** use the backup hardware wallet to register a new admin via the `upgrade()` function alongside a new WASM hash.
2. Revoke old signers from the admin Stellar account.
3. Publish a post-mortem and rotate all registered issuers/feeders/lenders.

---

## Initial weight configuration rationale

The default scoring weights are set at `initialize()` time:

| Component     | Weight | Rationale                                                                 |
| ------------- | ------ | ------------------------------------------------------------------------- |
| VC count      | 40     | Identity verification is the strongest signal — a verified user is a real user. |
| Tx volume     | 30     | On-chain activity demonstrates financial behavior but can be noisy.       |
| Repayment     | 30     | Repayment history is the most predictive signal but sparse at launch.     |

These weights are deliberately conservative:

- **VC count is highest** because the protocol's core value is self-sovereign identity. A user with 3+ VCs from independent issuers has strong identity verification.
- **Repayment weight is capped at 30** because the repayment record will be sparse during the initial feeder onboarding phase. Once the lender network matures and repayment data volume is statistically significant, the weight should be increased.
- **Tx volume is weighted equally with repayment** to reward early-stage users who transact on-chain before they have loan history.

Weights are upgradeable via `propose_weights()` / `apply_weights()` with a 24-hour timelock. In the first 90 days of mainnet, weights should **not** be changed unless a critical flaw is discovered.

---

## Feeder onboarding requirements

Feeders are off-chain indexers that supply VC count and transaction statistics to `credit-oracle`. They have write access to the scoring inputs of all subjects — a malicious or compromised feeder can arbitrarily inflate or deflate scores.

### Technical requirements

| Requirement                  | Specification                                                         |
| ---------------------------- | --------------------------------------------------------------------- |
| Uptime SLA                   | 99.5% monthly — scoring inputs must be updated at least once per 24h. |
| RPC endpoint                 | Dedicated Stellar mainnet RPC (no public endpoints).                  |
| Indexing delay               | Must process ledgers within 30 seconds of closing.                    |
| Redundancy                   | At least 2 feeder instances behind a load balancer.                   |
| Alerting                     | PagerDuty / OpsGenie integration for downtime > 5 minutes.            |
| Disaster recovery            | Restore from latest snapshot within 1 hour.                           |

### Operational requirements

- Feeder operators must complete KYC/KYB with the protocol foundation.
- Feeder addresses must be Stellar multisig accounts with at least 2-of-3 signers.
- Feeder operators must sign a **Feeder Service Agreement** specifying:
  - Data accuracy guarantees (no intentional inflation or suppression).
  - Notification obligations (planned maintenance, security incidents).
  - Offboarding procedure (graceful handover of feeder responsibilities).

### Onboarding process

1. Operator submits application with technical architecture document and operator identities.
2. Feeder address is registered on testnet and observed for 2 weeks.
3. Feeder address is registered on mainnet via `register_feeder(admin, feeder)`.
4. Feeder begins publishing `update_tx_stats` and `set_vc_count` transactions.
5. Operator is added to the incident response contact list.

---

## Contract upgrade path

All three contracts support in-place WASM hash replacement via `env.deployer().update_current_contract_wasm()`. This preserves the contract address and all stored state.

For the full step-by-step upgrade process (build, upload, invoke, verify), see [upgrade-guide.md](upgrade-guide.md).

### Mainnet-specific upgrade precautions

- **Always test upgrades on testnet first.** Deploy an identical copy of the mainnet state to testnet and verify the upgrade produces correct results.
- **Use a staging multisig.** The `upgrade` call should require M-of-N signatures (see [key ceremony](#admin-key-ceremony)). Never upgrade from a single key on mainnet.
- **Time-lock the upgrade.** Coordinate the upgrade window with all feeders and lenders. Announce the upgrade at least 7 days in advance on the protocol status page.
- **Rollback plan.** Keep the previous WASM hash accessible. If the upgrade is defective, the admin can re-upload the previous WASM and call `upgrade` again — as long as the previous WASM blob is still present on the network (it remains available indefinitely once uploaded).
- **Gas budget.** An `upgrade` call on mainnet requires enough XLM to cover the contract's storage rent for the new WASM. Ensure the admin account holds a sufficient balance.
- **Verify the WASM hash.** After upload, confirm the WASM hash matches a local `sha256sum` of the `.wasm` file before invoking `upgrade`:
  ```bash
  sha256sum target/wasm32-unknown-unknown/release/<contract_name>.wasm
  ```

### Emergency upgrade path

In the event of a critical vulnerability:

1. Upload the patched WASM using the deployer key (the deployer key must be retained in escrow for this purpose).
2. Invoke `upgrade` with admin multisig.
3. Publish an incident report within 24 hours.

---

## Monitoring and incident response

### On-chain monitoring

Monitor the following metrics for all three contracts:

| Metric                       | Tool                            | Alert threshold                    |
| ---------------------------- | ------------------------------- | ---------------------------------- |
| Admin key activity           | Stellar Expert account alerts   | Any transaction from admin address |
| Contract upgrade events      | Event scanner (custom indexer)  | Any `upgrade` call                 |
| Feeder update frequency      | Feeder heartbeat metric         | No update in > 24 hours            |
| Scoring weight changes       | `WeightsProposed` event stream  | Any `propose_weights` call         |
| Unauthorized access attempts | Failed `require_auth()` tracker | > 5 failed invocations / hour      |
| Storage rent balance         | Contract balance monitor        | Balance < 100 XLM                  |

### Incident response tiers

| Tier  | Severity | Example                              | Response time | Action                                                                 |
| ----- | -------- | ------------------------------------ | ------------- | ---------------------------------------------------------------------- |
| P1    | Critical | Admin key compromised, contract bug  | 15 minutes    | Emergency upgrade or pause. Notify all feeders/lenders within 1 hour.  |
| P2    | High     | Feeder offline, scoring outdated     | 1 hour        | Contact feeder operator. Activate backup feeder if available.          |
| P3    | Medium   | Unusual `compute_score` traffic      | 4 hours       | Investigate source. Rate-limit if abusive.                             |
| P4    | Low      | RPC endpoint degradation             | 24 hours      | Rotate RPC provider. No user-facing impact.                            |

### Communication channels

- **Protocol status page** — `/status` subpage or external statuspage.io. Updated within 30 minutes of any P1/P2 event.
- **Incident report** — Published to the repository `docs/incidents/` directory within 72 hours of resolution. Template:

  ```markdown
  # Incident YYYY-MM-DD: [Title]

  **Date:** YYYY-MM-DD HH:MM UTC
  **Duration:** X hours
  **Severity:** P1/P2/P3/P4
  **Root cause:** ...
  **Impact:** ...
  **Resolution:** ...
  **Prevention:** ...
  ```

### Runbooks

- [ ] **Feeder offline runbook** — indexer failure, restart steps, backup feeder activation.
- [ ] **Admin key rotation runbook** — compromised key procedure, multisig reconfiguration.
- [ ] **Emergency contract upgrade runbook** — vulnerability response, WASM build-upload-invoke sequence.
- [ ] **Storage rent top-up runbook** — fund admin account from foundation treasury.
