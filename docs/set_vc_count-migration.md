# `set_vc_count` Migration Guide

This guide is for credit-oracle administrators and feeder operators migrating
from feeder-cached VC counts to live cross-contract VC counts from the
identity-oracle.

## Why migrate?

`set_vc_count` stores a count supplied by a feeder. That cached value can become
stale if the feeder stops running or misses an update. When an identity-oracle
is configured on the credit-oracle, `compute_score` instead reads the subject's
active VC count directly from the identity-oracle. This live count reflects
revocations and avoids making the score depend on a feeder-maintained cache.

There is no automatic fallback from the cross-contract lookup if the configured
identity-oracle call fails. Before switching over, verify that the configured
contract is the correct, accessible identity-oracle and that it supports
`get_active_vc_count`.

## Migration phases

### Phase 1: Configure the identity-oracle

Deploy and initialize the identity-oracle if it is not already deployed.
Configure any required identity-oracle integrations, and verify that
`get_active_vc_count(subject)` returns the expected count for representative
subjects.

The credit-oracle administrator must then call `set_identity_oracle` with the
identity-oracle contract ID. For example, using the Stellar CLI:

```bash
stellar contract invoke \
  --id "$CREDIT_ORACLE_ID" \
  --network testnet \
  --source "$ADMIN" \
  -- set_identity_oracle \
  --identity_oracle_id "$IDENTITY_ORACLE_ID"
```

Use the appropriate network and the authorized credit-oracle administrator for
your deployment. The contract validates the identity-oracle reference and
requires admin authorization. Verify the stored value after the transaction:

```bash
stellar contract invoke \
  --id "$CREDIT_ORACLE_ID" \
  --network testnet \
  -- get_identity_oracle
```

Once configured, subsequent `compute_score` calls use the cross-contract
identity-oracle lookup. There is no cached-count fallback on lookup failure, so
test score computation on the target network before proceeding.

### Phase 2: Stop feeder `set_vc_count` calls

The reference feeder checks `get_identity_oracle` on the credit-oracle each
sync cycle. When an identity-oracle is configured, it automatically skips
`set_vc_count` and continues submitting transaction statistics with
`update_tx_stats`.

For an explicit opt-out of the legacy write, set `skipLegacyVcCount: true` in
the feeder's `FeederConfig`:

```typescript
const config: FeederConfig = {
  ...existingConfig,
  skipLegacyVcCount: true,
};
```

Here, `existingConfig` is your existing, fully populated `FeederConfig`.

`skipLegacyVcCount` is a TypeScript configuration option; it is not currently
read from an environment variable by the feeder CLI. The CLI's
`IDENTITY_ORACLE_ID` configures which identity-oracle the feeder reads from,
but it does not configure the identity-oracle address stored on the
credit-oracle. Complete Phase 1 separately.

Do not set `skipLegacyVcCount: true` as a substitute for configuring the
credit-oracle. If the credit-oracle has no identity-oracle configured,
`compute_score` continues using its legacy fallback path, and skipping feeder
updates can leave cached counts stale.

### Phase 3: Future contract version

The current credit-oracle still exposes `set_vc_count`. A future contract
version is expected to remove it, but no removal release or date is specified.
Treat the removal as a planned breaking change: update and test integrations
against the release notes before upgrading. Until then, deployments that have
not configured an identity-oracle can continue to use the legacy path.

## Monitoring `VcCntDep`

The credit-oracle emits a `VcCntDep` event when `set_vc_count` is called while
an identity-oracle is configured. The event is diagnostic; it does not reject
the legacy call. The legacy count is still written to storage, but
`compute_score` ignores it while cross-contract lookup is enabled.

Filter Soroban contract events by:

| Field | Value |
| --- | --- |
| Contract | The credit-oracle contract ID |
| Topic | `Symbol("VcCntDep")` |
| Data | `(subject: Address, count: u32)` |

Use a Soroban RPC event filter or an indexer, starting from a saved ledger
cursor and advancing it after each successful poll. For Soroban event query
and indexing conventions, see the [event indexing guide](event-indexing.md).
Alert on new events after Phase 1: they identify feeders or other integrations
that are still submitting redundant `set_vc_count` calls. If events continue,
check that every feeder instance is using the expected configuration and
version.

## Timeline and compatibility

- **Now:** Configure `set_identity_oracle`; the current contract switches
  `compute_score` to live cross-contract VC counts. The reference feeder
  automatically stops sending `set_vc_count` after it detects this setting.
- **Transition:** Existing contract deployments retain `set_vc_count`; direct
  callers may still invoke it. `VcCntDep` signals such calls after oracle
  configuration.
- **Future:** A later contract version is planned to remove `set_vc_count`.
  There is no published removal date in this repository. Review that
  version's migration notes before upgrading integrations.

The transition is compatible for the current reference feeder and contract:
older workflows can continue using cached counts until the identity-oracle is
configured. However, once configured, the live lookup has no automatic
fallback if the cross-contract call fails. Keep the identity-oracle deployed
and reachable, and verify the configured address before switching production
scoring to it.

## See also

- [Feeder setup and operation](../packages/feeder/README.md)
- [Cross-contract VC scoring architecture](architecture.md#migration-to-cross-contract-vc-scoring)
- [Contract event indexing](event-indexing.md)
