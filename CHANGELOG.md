# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `credit-oracle`: on-chain dispute mechanism for score inputs. Subjects can call `flag_score_input(subject, input_key, reason)` to flag a `tx_stats`, `repayment`, or `vc_count` input as incorrect; admins resolve disputes via `resolve_dispute(subject, input_key, accepted)`. Anti-griefing enforced: only one `Pending` dispute per `(subject, input_key)` pair at a time. Emits `DsptFild`, `DsptRslv`, and `DsptRjct` events for off-chain feeder indexing. Read helpers: `get_dispute` and `list_disputes`. Dispute records stored with 30-day TTL (#244)
- `packages/cli` (`@stellar-did-credit/cli`): new command-line interface with four commands — `anchor-did` (stores a DID document CID on-chain), `get-score` (reads a credit score with formatted table or JSON output), `verify-vc` (checks whether a VC hash is valid and non-revoked), and `compute-score` (submits a score computation transaction and returns the result). Reads contract IDs from environment variables, a `stellar-did-config.json` file, or `deployments.testnet.json`-style config. Built on `commander` with `--help` for every command (#161)

### Fixed

- SDK (`@stellar-did-credit/sdk`): removed duplicate `revokeVC` method that referenced undefined helpers; added missing `SorobanRpc.Server` instance property to the class constructor; fixed `computeScore` to use the class-level server and inline helpers; resolved type error in `waitForTransactionConfirmation` where `GetTransactionStatus` union was compared against string literals (#161)
- `credit-oracle`: `record_repayment` now records the public `amount` parameter in `RepaymentRecord.total_repaid` and includes capped repayment volume in the repayment score component (#221)

### Added

- `identity-oracle` and `credit-oracle`: aggregate protocol-level counters stored in instance storage for on-chain operational metrics. Each contract exposes a `get_protocol_stats()` getter that returns a struct with counters updated on every write operation. Identity-oracle tracks `total_dids_anchored`, `total_vcs_anchored`, and `total_vcs_revoked`. Credit-oracle tracks `total_subjects_scored` and `total_repayments_recorded`. Unit tests verify counters increment correctly and do not double-count deduplicated operations (#256)
- `identity-oracle`: `get_did_document(subject)` — read-only function that returns the anchored DID document CID for a subject, or `None` if no DID is anchored (#229)
- TypeScript SDK (`@stellar-did-credit/sdk`): `getDIDDocument(subjectAddress)` — fetches the anchored DID document CID from identity-oracle, returning `null` if not set (#229)
- `credit-oracle`: `ScoreRecord` now includes `stale: bool` — `get_score` computes staleness at read time by comparing `computed_at_ledger` against the current ledger sequence; a score is stale when the ledger delta exceeds `STALE_LEDGER_AGE` (~30 days at 5-second ledgers). `compute_score` always writes `stale = false` for freshly computed scores (#241)
- SDK: `ScoreRecord.stale` field added to the TypeScript type and parser (#241)
- `docs/scoring-spec.md`: updated stale-score section to explain the `stale` flag and how consumers should interpret freshness (#241)
- `docs/zk-proof-design.md`: updated commitment preimage and witness tables to include `stale` (#241)

### Added

- `cargo doc --workspace` now generates complete Rust API docs with no warnings; all public items across `credit-oracle`, `identity-oracle`, `revocation-registry`, and `governance` have `///` doc comments (#266)
- `typedoc` generates TypeScript API docs from JSDoc comments in `packages/sdk/src/index.ts`; added `typedoc` as a dev dependency and a `docs` script to the SDK package (#266)
- Root `package.json` exposes a `docs` script that regenerates both Rust and TypeScript API docs in one command (#266)
- `typedoc.json` workspace-level TypeDoc configuration file (#266)
- CI `docs` job: runs `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"` and `typedoc` to catch doc regressions on every PR (#266)
- `identity-oracle`: `migrate()` — admin-gated function to upgrade the contract storage layout version (#255)
- `credit-oracle`: `migrate(subjects)` — admin-gated function to migrate historical user repayment records to version 2 storage layout (#255)
- `credit-oracle`: `StorageVersion` instance storage key to version-gate storage layout decoding and compatibility (#255)
- `credit-oracle`: `RepaymentRecordV1` struct to decode legacy repayment records during migration (#255)
- `credit-oracle`: `test_storage_migration_flow` integration test to verify the upgrade and migration process (#255)

### Changed

- `identity-oracle`: `deregister_issuer` no longer rebuilds the full `IssuersIndex` vector on every call. `TrustedIssuer(Address)` is now a tombstone flag (`true` while trusted, `false` once deregistered, absent if never registered) instead of being removed on deregistration; `IssuersIndex` becomes an append-only record of every address ever registered. Deregistration is now a single storage write instead of an O(n) scan + rewrite. `list_issuers()` keeps its public signature and still returns only currently-registered issuers, now by filtering `IssuersIndex` against each entry's `TrustedIssuer` flag. No storage migration is required — both storage keys keep their original value types (#224)
- TypeScript SDK (`@stellar-did-credit/sdk`): reuse a single `SorobanRpc.Server` instance created in the constructor instead of creating a new server on every method call (#231)
- `credit-oracle`: `RepaymentRecord` struct layout modified to add `total_repaid` field (#255)

### Added

- `credit-oracle`: `compute_score` now emits a `Score` event with topic `Symbol("Score")` and data `(subject, score)` on every successful score computation (#223)
- `credit-oracle`: `set_identity_oracle(admin, identity_oracle_id)` — admin-gated function that stores the identity-oracle contract ID for live VC count lookups (#176)
- `credit-oracle`: cross-contract `compute_score` now calls `get_active_vc_count` on identity-oracle (excluding revoked VCs) when `IdentityOracleId` is configured, falling back to the cached `VcCount` otherwise (#176)
- Integration test `test_cross_contract_score_not_inflated_after_revocation` verifies that revoking VCs via identity-oracle immediately lowers the credit score when the cross-contract path is active (#176)
- TypeScript SDK (`@stellar-did-credit/sdk`): exported contract struct types `TxStats`, `ScoringWeights`, `RepaymentRecord`, and `VCRecord` (previously only `ScoreRecord` and `ProtocolConfig` were exported), with JSDoc Soroban-type annotations, export/structural tests, and a new "Types" section in the SDK README (#20)

### Deprecated

- `credit-oracle`: `set_vc_count(feeder, subject, count)` is deprecated; use `set_identity_oracle` + `compute_score` for live cross-contract VC count resolution instead (#176)

## [0.1.0] - 2026-06-24

### Added

- `identity-oracle` contract: DID anchoring (`anchor_did`), VC hash registry (`anchor_vc`, `verify_vc`, `get_vc_count`, `is_verified`), VC revocation (`mark_vc_revoked`), issuer management (`register_issuer`)
- `credit-oracle` contract: credit score computation (`compute_score`, `get_score`), transaction stats ingestion (`update_tx_stats`), repayment recording (`record_repayment`), configurable scoring weights (`update_weights`), feeder/lender registration
- `revocation-registry` contract: single and batch VC revocation (`revoke`, `batch_revoke`, `is_revoked`)
- Composite scoring formula (300–850) with default weights: VC 40%, transaction volume 30%, repayment history 30%
- Cross-contract integration test suite (21 tests)
- TypeScript SDK (`@stellar-did-credit/sdk`): `getScore` method
- Testnet deployment of all three contracts
- Deployment script (`scripts/deploy.sh`)
- Docs: architecture, DID method spec, scoring spec

[Unreleased]: https://github.com/cybermax4200/stellar-did-credit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/cybermax4200/stellar-did-credit/releases/tag/v0.1.0
