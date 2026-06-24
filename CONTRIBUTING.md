# Contributing to stellar-did-credit

## Prerequisites

- Rust stable (`rustup update stable`)
- `stellar-cli` 21+
- Node.js 18+
- pnpm (`npm i -g pnpm`)

## Setup

```bash
git clone https://github.com/cybermax4200/stellar-did-credit.git
cd stellar-did-credit
pnpm install
cargo test --workspace
```

## Running tests

```bash
pnpm test
```

This runs all Rust and TypeScript tests. See [Scripts](#scripts) below for details.

## Test snapshots

Soroban tests use snapshots stored in `test_snapshots/` directories to capture expected contract state for deterministic verification. Snapshots are committed alongside code changes to ensure tests remain reproducible across environments.

### Updating snapshots

If you modify a Soroban contract (or its test), the snapshot may change. Update it with:

```bash
UPDATE_EXPECT=true cargo test --workspace
```

**Important:** Snapshot files must be committed in the same PR as the code change that causes them to change. Stale snapshots are a common source of CI failures and reviewer confusion.

For more details, see the [Soroban testutils snapshot documentation](https://docs.rs/soroban-sdk/latest/soroban_sdk/testutils/index.html).

## Scripts

Root-level commands for testing, linting, and building all Rust and TypeScript packages:

```bash
pnpm test    # Run Rust and TypeScript tests
pnpm lint    # Run Clippy and linters
pnpm build   # Build Rust and TypeScript packages
```

Each command:
- Exits with non-zero status if any sub-command fails
- Runs Rust tests first, then TypeScript tests
- Is the recommended way to validate before opening a PR

## PR guidelines

- Link the issue(s) in your PR description
- All tests must pass (`pnpm test`)
- Linting must pass (`pnpm lint`)
- Snapshot files must be committed if code changes them
- Follow conventional commit format (see below)

## Commit format

```
type(scope): short description

feat(sdk): implement anchorDID wrapper
fix(identity-oracle): handle empty vc list in is_verified
docs(contributing): add contributing guidelines
test(revocation-registry): add batch revoke edge case
chore(deps): bump soroban-sdk to 25.3.1
```

Types: `feat`, `fix`, `docs`, `test`, `refactor`, `chore`
