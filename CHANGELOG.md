# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- TypeScript SDK (`@stellar-did-credit/sdk`): exported contract struct types `TxStats`, `ScoringWeights`, `RepaymentRecord`, and `VCRecord` (previously only `ScoreRecord` and `ProtocolConfig` were exported), with JSDoc Soroban-type annotations, export/structural tests, and a new "Types" section in the SDK README (#20)

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
