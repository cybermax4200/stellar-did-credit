# ADR-004: Relative governance quorum

## Status

Accepted.

## Decision

Governance retains `TotalRegisteredWeight` and uses it to support an optional
percentage-based quorum. The administrator can call `set_quorum_percent` with
a value from 1 through 100. Each proposal then snapshots
`TotalRegisteredWeight * percent / 100` when it is created.

The snapshot keeps the voting rules stable for a proposal already in progress:
later voter registrations, deregistrations, or weight changes affect only
future proposals. `set_quorum` remains available for deployments that need an
explicit absolute quorum; calling it disables percentage-based quorum mode.

## Consequences

The aggregate weight storage entry is actively read in quorum calculation and
is no longer merely a registry-maintenance counter. Integer division rounds a
percentage quorum down to whole vote-weight units.
