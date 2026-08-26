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

### Health monitoring

When `HEALTH_PORT` is set, the feeder starts a lightweight HTTP server (using Node's built-in `http` module — no extra dependencies) alongside the polling loop. This is intended for Kubernetes, ECS, or other orchestrator liveness/readiness probes.

| Variable      | Default | Description |
| ------------- | ------- | ----------- |
| `HEALTH_PORT` | unset   | TCP port for the health HTTP server. When unset, no health server is started. |

**Endpoints** (available only when `HEALTH_PORT` is set):

| Endpoint       | Status | Description |
| -------------- | ------ | ----------- |
| `GET /health`  | 200    | Always returns liveness info: `{"status":"ok","lastCycleAt":"<iso or null>","successCount":<n>,"failureCount":<n>}`. Counts are cumulative per-subject sync outcomes across all completed cycles. |
| `GET /ready`   | 200    | Last feed cycle completed with zero failures. |
| `GET /ready`   | 503    | Feeder has never completed a cycle, or the last cycle had at least one failure. |

Example:

```bash
HEALTH_PORT=8080 FEEDER_SECRET=S... SUBJECTS=G... \
CREDIT_ORACLE_ID=C... IDENTITY_ORACLE_ID=C... \
npm start
```

> **Note:** `REVOCATION_REGISTRY_ID` and `GOVERNANCE_ID` are optional. The feeder
> starts normally without them and logs which optional integrations are
> configured at startup.
