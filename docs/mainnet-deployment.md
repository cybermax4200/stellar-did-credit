# Mainnet Deployment Guide

This guide covers the planning, security, and operational requirements for deploying stellar-did-credit to Stellar mainnet. A rushed deployment risks compromising the protocol's admin key, credential state, or scoring integrity. This document provides a step-by-step checklist, best practices, and incident response procedures.

---

## Table of contents

- [Pre-deployment security checklist](#pre-deployment-security-checklist)
- [Admin key ceremony](#admin-key-ceremony)
- [Initial scoring weight configuration](#initial-scoring-weight-configuration)
- [Feeder and lender onboarding](#feeder-and-lender-onboarding)
- [Contract upgrade path](#contract-upgrade-path)
- [Monitoring and observability](#monitoring-and-observability)
- [Incident response](#incident-response)

---

## Pre-deployment security checklist

### Code and audit (REQUIRED)

- [ ] All contracts pass `cargo clippy --workspace -- -D warnings` with zero warnings
- [ ] All contracts pass `cargo test --workspace` on stable Rust
- [ ] No unwrap() or panic!() calls in contract logic — only expect("descriptive message")
- [ ] Independent security audit completed by a third-party Soroban specialist
- [ ] All audit findings (critical, high, medium) are resolved and re-audited
- [ ] Audit report is public or available to key stakeholders
- [ ] Contract source code is pinned (tag or commit hash) and reproducible

### Stellar network prerequisites

- [ ] Deployer account has XLM balance ≥ 10 XLM (buffer for transaction fees)
- [ ] Deployer key is generated on an air-gapped machine or hardware wallet
- [ ] Deployer secret key is **never** stored in plaintext or version control
- [ ] Admin key is **different** from deployer key and is kept offline until needed
- [ ] All keys have backups in secure, geographically distributed locations

### Contract configuration

- [ ] Initial admin is set to multi-sig or hardware wallet address (see [Admin key ceremony](#admin-key-ceremony))
- [ ] Initial scoring weights (vc: 40, tx: 30, repayment: 30) are ratified by governance or stakeholders
- [ ] Timelock duration for weight changes (default 24h) is documented and agreed upon
- [ ] Contract behavior is tested on testnet with production-like parameters for ≥ 2 weeks
- [ ] All three contracts (identity-oracle, credit-oracle, revocation-registry) are deployed and initialized atomically

### Operational readiness

- [ ] Issuer onboarding process is documented (see [Feeder and lender onboarding](#feeder-and-lender-onboarding))
- [ ] Feeder nodes are running, tested, and monitored
- [ ] Error handling, logging, and alerting are in place for all contract calls
- [ ] Runbooks for common issues (stuck weight proposal, revocation backlog) are written
- [ ] A communication channel (Discord, Slack, mailing list) is established for incident coordination

### Testnet validation (REQUIRED)

- [ ] At least three independent teams have deployed and tested the contracts on testnet
- [ ] Testnet contracts have been active for ≥ 2 weeks with realistic transaction volume
- [ ] Issuer registration, VC anchoring, and score computation work end-to-end on testnet
- [ ] Cross-contract calls (if Phase 3 is deployed) are tested under load
- [ ] Rollback or emergency pause procedures are documented

---

## Admin key ceremony

The admin key is the single point of control for both contract upgrades and operational configuration (weight changes, issuer registration). Compromising it allows an attacker to steal all VC data, inject false scores, or lock legitimate users out of the protocol. Protecting the admin key is critical.

### Admin key generation

**Requirements:**

- Admin key must be a Stellar account address (public key only)
- Admin key **must not** be the deployer key
- Admin key **must not** be an exchange, custodian, or third-party service account
- Admin key **must** be controlled by your organization via cold storage

**Recommended approach: Hardware wallet**

1. **Use a hardware wallet** (Ledger Nano S Plus, Trezor Model T, or equivalent) connected to an air-gapped machine
2. **Generate a new Stellar keypair** on the hardware wallet using a Stellar-compatible derivation path (m/44'/148'/0')
3. **Derive the public key** and note it — this is your admin address
4. **Do not export the private key** — it remains on the hardware wallet
5. **Require physical confirmation** from the hardware wallet for every admin action (upgrade, weight change, issuer registration)

**Multi-signature alternative (if available)**

If your organization requires split control:

- Use Stellar's native multisig feature (Soroban does not directly support contract-level multisig)
- Establish that any admin action must be approved by N-of-M signers (e.g., 3-of-5)
- Document the signatory set and backup procedures

**Do not use:**

- ❌ Exchange-hosted wallets (you do not control the key)
- ❌ Cloud key management services without HSM
- ❌ Plaintext secret keys in files or environment variables
- ❌ A single developer's personal account

### Admin key storage

- **Primary:** Hardware wallet stored in a physical safe or bank vault
- **Backup:** Seed phrase written on paper, stored in a separate physical location (fireproof safe, attorney's office, etc.)
- **Access:** Only authorized signers should know the location and have access
- **Rotation:** Admin key should not be rotated frequently; rotate only if there is reason to suspect compromise

### Testnet admin key ceremony

Before mainnet deployment, rehearse the admin key ceremony on testnet:

1. Generate a testnet admin keypair on your hardware wallet
2. Perform a test upgrade on a testnet contract instance
3. Test issuer registration (register_issuer) and weight proposal (propose_weights) flows
4. Document exact steps so the mainnet ceremony can be executed identically
5. Time the operation — admin actions should complete within a known window

---

## Initial scoring weight configuration

The scoring formula combines three signals: verifiable credentials (VCs), transaction volume, and repayment history. The initial weights influence protocol incentives for months. Choose them carefully.

### Default weights

The contracts ship with default weights:

```
vc_weight = 40
tx_weight = 30
repayment_weight = 30
Total = 100
```

These weights balance:

- **VC weight (40%)**: Rewards identity verification; lower weight reflects that issuers are still onboarding
- **TX weight (30%)**: Rewards on-chain activity; equal to repayment weight to avoid over-weighting new users
- **Repayment weight (30%)**: Rewards loan history; equal to TX weight to encourage both savings and borrowing behavior

### Rationale for mainnet launch

**Recommended initial mainnet weights:**

| Component | Weight | Rationale                                                                                   |
| --------- | ------ | ------------------------------------------------------------------------------------------- |
| VC        | 40     | Keep default; issuer network is still growing; lower weight avoids over-rewarding early VCs |
| TX        | 30     | Keep default; Stellar on-chain activity is the most trustworthy signal available            |
| Repayment | 30     | Keep default; repayment data will be sparse at launch; equal weight prevents over-weighting |

**Do not deploy with:**

- ❌ VC weight > 60% (creates perverse incentive to collect worthless VCs)
- ❌ Repayment weight > 50% (biases toward lending ecosystem; excludes pure savers)
- ❌ TX weight < 20% (ignores on-chain activity, the most available signal)

### Weight upgrade procedure

- Weights must be changed via `propose_weights()` followed by `apply_weights()` after a 24-hour timelock
- The timelock prevents accidental or malicious changes from taking effect immediately
- All weight changes must be announced publicly in your community channels
- Monitor scores for anomalies after each change — if a large cohort's score suddenly drops, investigate

### Monitoring weight changes

After deploying new weights, track:

```
1. Score distribution shift (compare percentiles before/after)
2. User complaints or support tickets mentioning score changes
3. Issuer adoption rate (do weight increases cause issuer adoption to rise or fall?)
4. Feeder volume (do weight changes correlate with more or fewer updates?)
```

Document the business impact of each change in a log for future governance decisions.

---

## Feeder and lender onboarding

Feeders (off-chain indexers) and lenders (dApps, microfinance platforms, or credit products) drive the protocol's utility. Onboard them carefully to ensure data quality and avoid spam.

### Feeder onboarding

**Feeder responsibility:**
Update transaction statistics (`update_tx_stats`) for users on a regular schedule. Feeders need access to Stellar transaction history and must compute 30-day volume accurately.

**Feeder requirements (MUST verify):**

1. **Identity verification**
   - Feeder must identify itself (real name, organization, website, GitHub account)
   - Feeder operator must pass basic KYC (match government ID or business license if applicable)

2. **Technical capability**
   - Feeder must demonstrate ability to index Stellar transactions
   - Feeder must provide test data from testnet showing 30-day volume calculations for known accounts
   - Feeder must commit to ≥ 99.5% uptime or document intended downtime windows

3. **Data quality**
   - Feeder must agree to sign a data accuracy commitment (off-chain, nonbinding)
   - Feeder must commit to updating each user's stats at least once per day
   - Feeder must not submit obviously fraudulent volume (e.g., self-dealing via circular transfers)

4. **Monitoring**
   - Feeder must expose metrics: last update timestamp, volume calculation confidence, error count
   - Your protocol team must spot-check feeder data against independent calculations weekly

**Feeder registration flow:**

```
1. Candidate feeder submits application (via GitHub issue or web form)
2. Protocol team reviews identity, technical capability, and commitment
3. Protocol team invites feeder to testnet and provides test account
4. Feeder updates stats for test account over 1 week on testnet
5. If satisfactory, admin calls register_feeder(feeder_address) on mainnet
6. Feeder is added to the network
```

**Feeder deregistration:**

If a feeder becomes malicious or unreliable:

- Call `deregister_feeder(feeder_address)`
- Deregistration takes effect immediately (no retroactive recomputation of past data)
- Lenders relying on that feeder's data must refresh their queries; cached scores are not invalidated

### Lender onboarding

**Lender responsibility:**
Record loan repayment outcomes (`record_repayment`) and query credit scores (`get_score`).

**Lender requirements (MUST verify):**

1. **Identity and legitimacy**
   - Lender must be a registered financial entity, dApp, or platform
   - Lender must have a public website and community presence
   - Lender must pass basic KYC and sanctions screening

2. **Repayment data accuracy**
   - Lender must commit to reporting only **actual** repayments (no fabricated transactions)
   - Lender must report the **true on-time status** of each repayment (no marking late payments as on-time)
   - Lender must report all repayments (not just on-time ones; the score calculation needs both numerator and denominator)

3. **Score interpretation**
   - Lender must not misrepresent the score as a traditional credit score or make false claims about its predictive power
   - Lender must disclose that scores are early-stage and based on limited data
   - Lender must include a link to [docs/scoring-spec.md](scoring-spec.md) in their score disclosure

4. **Monitoring**
   - Lender must expose repayment submission logs (for audit purposes)
   - Your protocol team must periodically spot-check lender submissions for fraud

**Lender registration flow:**

```
1. Candidate lender submits application with proof of legitimacy
2. Protocol team reviews financial registration, website, community presence
3. Protocol team creates test account for lender on testnet
4. Lender submits test repayment data (e.g., 100 test transactions) on testnet
5. If satisfactory, admin calls register_lender(lender_address) on mainnet
6. Lender is added to the network
```

**Lender deregistration:**

If a lender becomes unreliable or malicious:

- Call `deregister_lender(lender_address)`
- Deregistration takes effect immediately
- All future repayments from that lender are rejected
- Past repayment records are retained (scores do not change retroactively)

### Auditing feeder and lender data

At least monthly, perform spot checks:

1. **Pick 10 random users** and verify their TX stats against independent block data
2. **Pick 10 random repayments** and verify they match lender's records
3. **Look for patterns of abuse** (identical volume for 1000 accounts, impossible repayment rates, etc.)
4. If you find discrepancies ≥ 5%, notify the feeder/lender immediately and consider deregistration

---

## Contract upgrade path

Soroban contracts are immutable at the bytecode level, but can be upgraded in-place via the `upgrade(admin, new_wasm_hash)` function. This function replaces the contract's WASM code while preserving storage state (scores, issuers, feeders).

### When to upgrade

**Upgrade to fix:**

- Bugs in score computation or data validation
- Performance issues (gas cost optimization)
- New protocol features (e.g., new VC types, additional scoring signals)

**Do NOT upgrade to:**

- Change the scoring formula retroactively (this breaks user expectations; use weight changes instead)
- Lock users out or seize their data (this violates the protocol's trust model)

### Upgrade procedure

#### Step 1: Build the new WASM

```bash
cd contracts/credit-oracle
cargo build --release --target wasm32-unknown-unknown
```

The resulting bytecode is at:

```
target/wasm32-unknown-unknown/release/stellar_did_credit_oracle.wasm
```

#### Step 2: Compute the SHA-256 hash

```bash
sha256sum target/wasm32-unknown-unknown/release/stellar_did_credit_oracle.wasm
```

Example output:

```
a3f4c21e5b8d9e2f1c7a4b6d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6 target/wasm32-unknown-unknown/release/stellar_did_credit_oracle.wasm
```

#### Step 3: Publish the WASM (optional but recommended)

Upload the WASM to a public storage location (GitHub release, IPFS, your CDN):

- Makes the upgrade auditable (anyone can verify the bytecode)
- Allows users to independently verify that the deployed code matches the source

Example:

```bash
# Upload to GitHub releases
gh release create v2.0.0 target/wasm32-unknown-unknown/release/stellar_did_credit_oracle.wasm

# Or upload to IPFS
ipfs add target/wasm32-unknown-unknown/release/stellar_did_credit_oracle.wasm
# Returns: Qm...
```

#### Step 4: Call upgrade on-chain

Using the admin key (hardware wallet or multi-sig):

```bash
stellar contract invoke \
  --id <CONTRACT_ID> \
  --source-account admin \
  --network mainnet \
  -- upgrade \
  --new_wasm_hash <SHA256_HASH>
```

#### Step 5: Verify the upgrade

Query the contract to confirm the upgrade took effect:

```bash
stellar contract invoke \
  --id <CONTRACT_ID> \
  --network mainnet \
  -- get_scoring_weights
# Should return weights unchanged if the upgrade was successful
```

### Upgrade rollback

If an upgrade introduces a critical bug:

1. **Identify the bug** via monitoring alerts or user reports
2. **Build a hotfix** addressing the issue
3. **Build new WASM** and compute SHA-256 hash
4. **Call upgrade** again with the hotfix WASM hash
5. **Announce the incident** transparently to the community

Rollback should take < 1 hour from bug detection to fix on-chain.

### Upgrade testing (pre-mainnet)

Before upgrading on mainnet:

1. Deploy the new WASM to testnet
2. Run the full test suite against the upgraded testnet contract
3. Perform manual end-to-end testing with real feeder and lender calls
4. Simulate a rollback to ensure you can quickly revert if needed
5. Have at least two independent reviewers sign off on the upgrade

---

## Monitoring and observability

Running a live credit protocol requires continuous monitoring. Anomalies in score distribution, feeder data, or lender behavior indicate problems that need urgent attention.

### Essential metrics

#### By contract

**identity-oracle:**

- Count of registered issuers (should grow slowly over time)
- Count of unique subjects with ≥ 1 VC (onboarding rate)
- VC anchor rate (VCs/day)
- VC revocation rate (% of VCs revoked over 30-day window)

**credit-oracle:**

- Count of active feeders (should remain stable or grow)
- TX stats update rate (updates/day)
- Repayment submission rate (repayments/day)
- Score distribution (p10, p50, p90 percentiles)
- Score computation gas cost (monitor for performance regressions)

**revocation-registry:**

- Revocation rate (revocations/day)
- Batch revocation usage (% of revocations submitted via batch_revoke)

#### By user cohort

Track these metrics **stratified by user segment** to catch segment-specific attacks or bugs:

- **New users (< 30 days activity):** Average score, dropout rate
- **Long-term users (> 90 days activity):** Score distribution shift, repayment rate
- **By geography/issuer:** If you track it, monitor score distribution by trusted issuer

### Alerting thresholds

Set up automated alerts for:

| Metric                         | Threshold              | Action                                   |
| ------------------------------ | ---------------------- | ---------------------------------------- |
| Feeder update rate drops       | < 50% of baseline      | Page on-call; contact feeder             |
| VC revocation rate spikes      | > 10× normal rate      | Investigate issuer; check for malware    |
| Score p50 drops/rises          | > 50 points per day    | Review weight changes; check for bugs    |
| Contract call error rate rises | > 1% of calls          | Check gas cost or data size; review logs |
| Lender repayment backlog grows | > 1000 pending records | Page on-call; investigate lender         |

### Logging and audit trail

Every admin action must be logged and auditable:

- ✅ Issuer registration: timestamp, issuer address, admin address
- ✅ Weight change proposal: timestamp, old weights, new weights, admin address
- ✅ Weight change application: timestamp, effective weights
- ✅ Feeder/lender registration and deregistration
- ✅ Contract upgrade: timestamp, old WASM hash, new WASM hash, admin address

Logs should be stored in a tamper-evident system (write-once storage, or regularly signed log digests) so that you can audit the protocol's history after the fact.

### Dashboard recommendations

Build or subscribe to a dashboard that displays:

1. **System health**
   - Feeder/lender count and status
   - Last contract call timestamp
   - Error rate (%)

2. **User onboarding**
   - Cumulative users with ≥ 1 VC
   - Daily new VC anchors
   - Daily new repayment records

3. **Data quality**
   - VC revocation rate (%)
   - Feeder data freshness (hours since last update per feeder)
   - Lender submission rate (repayments/day per lender)

4. **Score health**
   - Score distribution (histogram or box plot)
   - Median score trend over time
   - Top 10 highest/lowest scores (anonymized)

---

## Incident response

### Incident classification

| Severity | Definition                                      | Response time  |
| -------- | ----------------------------------------------- | -------------- |
| P0       | Data loss, fund theft, or protocol halt         | Immediate      |
| P1       | Score computation error affecting > 1% of users | 1 hour         |
| P2       | Feeder/lender outage or data inaccuracy         | 4 hours        |
| P3       | Minor UI issue, low-impact data discrepancy     | 1 business day |

### P0 incident: Protocol halt

**Symptoms:**

- All contract calls return errors
- Contract is unresponsive or gas costs are impossible

**Steps:**

1. **Confirm on-chain** — call `get_score` manually to verify the contract is truly unresponsive
2. **Check Stellar network** — verify Stellar is not down; check [status.stellar.org](https://status.stellar.org)
3. **Examine recent logs** — look for failed deployments, malicious upgrades, or storage corruption
4. **If upgrade is the cause:**
   - Call `upgrade()` with a known-good previous WASM hash to rollback
   - Do NOT make further changes until you understand the root cause
5. **Communicate** — announce the incident to issuers, feeders, and lenders via all channels
6. **Post-mortem** — after recovery, hold a blameless post-mortem within 24 hours

### P1 incident: Score computation error

**Symptoms:**

- Scores computed for a cohort are incorrect (e.g., all VC scores are 0, or negative scores appear)
- Users report inconsistent scores between two queries

**Steps:**

1. **Isolate the affected cohort** — determine which users are affected (all users, specific issuers, specific feeders?)
2. **Verify the bug** — manually recompute 10 scores using the formula and compare to on-chain results
3. **Determine root cause:**
   - Is the bug in score computation (math error)?
   - Is the bug in feeder data (wrong TX stats)?
   - Is the bug in lender data (wrong repayment count)?
4. **Assess impact:**
   - How many users are affected?
   - How long was the bug active?
   - What is the financial impact (money lent to users with wrong scores)?
5. **Fix and verify:**
   - If the bug is in the contract: build a hotfix, test on testnet, upgrade on mainnet
   - If the bug is in feeder/lender data: contact feeder/lender to correct the data
   - After fix, recompute affected scores and verify correctness
6. **Communicate** — announce the bug, the fix, and the mitigation to affected users

### P2 incident: Feeder or lender failure

**Symptoms:**

- A feeder stops updating TX stats for hours
- A lender submits malformed or duplicate repayment records
- Score computation stalls because required data is missing

**Steps:**

1. **Confirm the failure** — check feeder/lender status manually (query their endpoint, check their logs)
2. **Attempt to contact** — reach out via email, Discord, or phone
3. **If no response after 30 min:**
   - If feeder failure: call `deregister_feeder()` to remove the feeder from the network
   - If lender failure: call `deregister_lender()` to remove the lender from the network
4. **Mitigate:**
   - Notify users that scores may not reflect the latest data (feeder failure) or accept new repayments (lender failure)
   - If a critical feeder is down, activate backup feeder(s) if available
5. **Post-incident:**
   - Investigate why the feeder/lender failed
   - If it was operator error, provide training and re-enable
   - If it was infrastructure failure, require the feeder/lender to upgrade infrastructure before re-registering

### Communication template

When an incident occurs, send this template to all stakeholders:

```
Subject: [INCIDENT] stellar-did-credit - [SERVICE] - [DURATION]

We are investigating an incident affecting the stellar-did-credit protocol.

Severity: [P0 / P1 / P2 / P3]
Service(s) affected: [identity-oracle / credit-oracle / revocation-registry / feeder / lender]
Detected at: [TIMESTAMP]

Symptom: [What users are experiencing]
Root cause: [If known; otherwise "under investigation"]
Impact: [How many users, financial impact if applicable]

Next update: [TIMESTAMP + 30 min / 1 hour]

For questions, join our Discord: [LINK]
```

### After-incident review

After any P1 or P0 incident:

1. Hold a blameless post-mortem within 24 hours (Slack thread or meeting)
2. Document:
   - Timeline of events
   - Root cause
   - How the incident was detected
   - How the incident was mitigated
   - What improvements prevent recurrence
3. Assign action items (e.g., "add monitoring for metric X", "document runbook for scenario Y")
4. Share the post-mortem with the community (redacting if necessary)

---

## Checklist for mainnet launch

Before going live, check every item:

### Security and audit

- [ ] Security audit completed and findings resolved
- [ ] No unwrap() calls in production code
- [ ] Testnet contracts have been stable for ≥ 2 weeks
- [ ] Admin key is in hardware wallet (or multi-sig)

### Deployment

- [ ] Deployer and admin keys are different
- [ ] All three contracts deploy successfully
- [ ] Initial scoring weights are set (vc: 40, tx: 30, repayment: 30)
- [ ] Admin account is set correctly
- [ ] Deployment addresses are recorded in a version control system

### Feeder and lender readiness

- [ ] At least 2 feeders are on-boarded and tested
- [ ] At least 2 lenders are on-boarded and tested
- [ ] Feeder/lender SLAs are documented
- [ ] Spot-check procedures are in place

### Monitoring and incident response

- [ ] All essential metrics are being collected
- [ ] Alerts are configured for P0 and P1 incidents
- [ ] Incident response runbooks are written
- [ ] Communication channels (Discord, email, Slack) are set up

### Documentation

- [ ] Deployment addresses are published
- [ ] Runbooks are shared with the team
- [ ] User documentation is available (link to [docs/scoring-spec.md](scoring-spec.md))
- [ ] Known limitations are clearly disclosed

---

## Resources

- [Stellar Developer Docs](https://developers.stellar.org/)
- [Soroban Smart Contracts](https://developers.stellar.org/docs/smart-contracts)
- [Stellar Testnet Status](https://status.stellar.org/)
- [Scoring Specification](scoring-spec.md)
- [DID Method Specification](did-spec.md)
- [NIST Cybersecurity Framework](https://www.nist.gov/cyberframework)
