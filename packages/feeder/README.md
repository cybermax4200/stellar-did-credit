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
