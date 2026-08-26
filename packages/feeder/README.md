# @stellar-did-credit/feeder

Reference feeder implementation for the `stellar-did-credit` protocol.

## Overview

The feeder is an off-chain daemon that:
1. Polls or subscribes to events to determine when to sync subject data.
2. Reads get_active_vc_count(subject) from the `identity-oracle` contract.
3. Queries the Horizon API for 30-day payment statistics for each subject.
4. Submits statistics and VC count updates to the `credit-oracle` contract.

For details on how to index events and implement event-driven syncing, please refer to the [Event Indexing Guide](../../docs/event-indexing.md).

## Usage

See the package source code for configuration variables.

## Dead-Letter Queue

The feeder tracks subjects that fail consecutively across polling cycles. When a subject fails `MAX_CONSECUTIVE_FAILURES` (default: 5) consecutive cycles, it enters a **dead-letter state** and is logged at ERROR level with a distinct `[dead-letter]` prefix.

### Behavior

- **Sub-threshold failures**: Subjects that have failed fewer than `MAX_CONSECUTIVE_FAILURES` cycles are logged at WARN level with a progress indicator (e.g. `[dead-letter] subject failure 3/5 — will retry next cycle`).
- **At/above threshold**: Subjects at or above the threshold are logged at ERROR: `[dead-letter] subject has failed N consecutive cycles (threshold: 5)`.
- **Recovery**: When a dead-letter subject feeds successfully, it is removed from the dead-letter set and a recovery message is logged.
- **No permanent drops**: Dead-letter subjects are **still retried** each cycle — they are never silently skipped.

### Configuration

| Environment Variable       | Default | Description                                              |
|---------------------------|---------|----------------------------------------------------------|
| `MAX_CONSECUTIVE_FAILURES` | `5`     | Number of consecutive failures before entering dead-letter state |

### Programmatic API

```typescript
import { Feeder } from "@stellar-did-credit/feeder";

const feeder = new Feeder(config, keypair);

// Get the current set of dead-letter subjects
const deadLetters: string[] = feeder.getDeadLetterSubjects();
console.log("Subjects in dead-letter:", deadLetters);
```

The dead-letter state is tracked in-memory and resets when the feeder process restarts.
```bash
FEEDER_SECRET=YOUR_STELLAR_SECRET_KEY \
SUBJECTS=G1...,G2... \
CREDIT_ORACLE_ID=C... IDENTITY_ORACLE_ID=C... \
npm start
```

## Environment Variables

All variables are read from the environment. Copy `packages/feeder/.env.example`
and fill in the values, or export them before running `npm start`.

### Required

| Variable            | Description                                                            |
| ------------------- | ---------------------------------------------------------------------- |
| `FEEDER_SECRET`     | Stellar secret key (S...) of a registered feeder account. Must be registered on-chain first via `register_feeder(admin, FEEDER_PUBLIC_KEY)`. |
| `SUBJECTS`          | Comma-separated list of subject G... addresses to sync each cycle.     |
| `CREDIT_ORACLE_ID`  | Soroban contract address (C...) of the credit-oracle.                  |
| `IDENTITY_ORACLE_ID`| Soroban contract address (C...) of the identity-oracle.                |

### Network

| Variable              | Default                              | Description                                                                 |
| --------------------- | ------------------------------------ | --------------------------------------------------------------------------- |
| `NETWORK`             | `testnet`                            | Network to use: `testnet` (default), `mainnet`, or `futurenet`. Sets defaults for the passphrase, RPC, Horizon, and sim account. |
| `NETWORK_PASSPHRASE`  | per `NETWORK`                        | Stellar network passphrase (e.g. `Test SDF Network ; September 2015`).      |
| `RPC_URL`             | per `NETWORK`                        | Soroban RPC endpoint.                                                       |
| `HORIZON_URL`         | per `NETWORK`                        | Horizon REST API endpoint.                                                  |
| `SIM_ACCOUNT`         | testnet/futurenet sim account        | Any funded G... account used as a fee source for read-only contract simulations. **Required** on `mainnet`. |

### Polling & retry

| Variable                | Default    | Description                                                                   |
| ----------------------- | ---------- | ----------------------------------------------------------------------------- |
| `POLL_INTERVAL_MS`      | `3600000`  | How often to run a full feed cycle, in milliseconds. Minimum enforced value: 1 minute. |
| `MAX_RETRIES`           | `3`        | Max retry attempts for transient RPC/Horizon failures.                         |
| `RETRY_BASE_DELAY_MS`   | `1000`     | Base delay for exponential backoff, in milliseconds.                           |
| `EVENT_DRIVEN`          | `false`    | Enables event-driven mode. Subscribes to `VCAnch` and `Revoked` events to trigger immediate feed cycles. |
| `EVENT_POLL_INTERVAL_MS`| `30000`    | How often to poll for events, in milliseconds. Used when `EVENT_DRIVEN=true`.  |

### Optional contract integrations

| Variable                  | Default   | Description                                                                                                |
| ------------------------- | --------- | ---------------------------------------------------------------------------------------------------------- |
| `REVOCATION_REGISTRY_ID`  | unset     | Soroban contract address (C...) of the revocation-registry. Needed for event-driven mode so the feeder can watch for `Revoked` events and re-sync affected subjects. Validated as a contract address when set. |
| `GOVERNANCE_ID`           | unset     | Soroban contract address (C...) of the governance contract. Reserved for future governance-aware features — the feeder does not call governance today. Validated as a contract address when set. |

> **Note:** `REVOCATION_REGISTRY_ID` and `GOVERNANCE_ID` are optional. The feeder
> starts normally without them and logs which optional integrations are
> configured at startup.
