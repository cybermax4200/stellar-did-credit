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
