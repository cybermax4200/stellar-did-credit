# Contributing to stellar-did-credit

## ⚠️ Never commit secrets

> A pre-commit hook (via [lefthook](https://github.com/evilmartians/lefthook)) is installed automatically by `pnpm install`. It will block any commit containing a Stellar secret key pattern before it reaches GitHub.

**Stellar secret keys start with `S` and are 56 characters long.** Never commit them.

Common ways contributors accidentally expose secrets:
- Committing a `.env` file containing `STELLAR_SECRET_KEY=S...`
- Committing key files from `~/.config/stellar/identity/`
- Hardcoding a secret key in a test or script

GitHub's secret scanner will detect any committed Stellar secret key, flag your commit, and may restrict your account. The `.gitignore` already excludes `.env` files — do not work around it.

If you need a throwaway key for testing, generate one with `stellar keys generate` and let `stellar-cli` manage it locally. Never paste the secret into any file tracked by git.

---

## Reporting vulnerabilities

**Do not open a public issue for security bugs.**
Please use [GitHub Security Advisories](https://github.com/cybermax4200/stellar-did-credit/security/advisories/new) to report vulnerabilities privately. See [SECURITY.md](../SECURITY.md) for scope, response SLA, and disclosure policy.

---

## Prerequisites

- Rust stable (`rustup update stable`)
- `stellar-cli` 21+
- Node.js 18+
- pnpm (`npm i -g pnpm`)

## Setup

> **Important:** Do not clone this repo directly. You must fork it first so your PR targets the correct upstream repository.

1. Click **Fork** on [github.com/cybermax4200/stellar-did-credit](https://github.com/cybermax4200/stellar-did-credit) to create `your-username/stellar-did-credit`
2. Clone **your fork** (not the original):

```bash
git clone https://github.com/YOUR_USERNAME/stellar-did-credit.git
cd stellar-did-credit
pnpm install
cargo test --workspace
```

3. Add the upstream remote so you can pull future changes:

```bash
git remote add upstream https://github.com/cybermax4200/stellar-did-credit.git
```

## Running tests

```bash
pnpm test
```

This runs all Rust and TypeScript tests. See [Scripts](#scripts) below for details.

## Test Organization

The test suite is organized into distinct categories to keep unit verification and cross-contract flows separated:

- **Unit tests (in-module):** Located directly within each contract crate in `contracts/<contract-name>/src/` (inside `#[cfg(test)]` modules or dedicated test files like `test.rs`). Unit tests focus on isolated contract logic, parameter boundaries, error codes, and local state transitions without spinning up external contract dependencies. Run a specific contract's unit tests with:
  ```bash
  cargo test -p <contract-crate>
  ```
- **Integration tests (`contracts/tests/`):** Located in the dedicated `integration-tests` crate under `contracts/tests/` (primarily `contracts/tests/src/integration_test.rs`). These tests instantiate multiple Soroban contracts together (e.g., `identity-oracle`, `credit-oracle`, `revocation-registry`, and `governance`) to validate cross-contract calls, shared workflows, event emissions, and authentication delegation across contracts. Run integration tests with:
  ```bash
  cargo test -p integration-tests --lib tests::integration_test
  ```
- **TTL / Expiry tests:** Located in `contracts/tests/src/ttl_expiry_tests.rs`. These tests simulate Soroban ledger advancements and verify time-to-live (TTL) expiration semantics for both instance and persistent storage entries. They ensure that lifetime extensions (`extend_ttl`) maintain vital entries and that archived storage reads fail predictably rather than corrupting state. Run with:
  ```bash
  cargo test -p integration-tests --lib tests::ttl_expiry_tests
  ```
- **Gas profiling tests:** Located in `contracts/tests/src/gas_profiling.rs`. These tests measure CPU instruction execution and memory byte consumption across core contract operations and variable payload sizes, ensuring operations remain within Soroban gas budgets. Run with:
  ```bash
  cargo test -p integration-tests --lib tests::gas_profiling
  ```

## Snapshot Tests

Soroban tests make extensive use of deterministic test snapshots stored in `test_snapshots/` directories across contract crates (e.g., `contracts/credit-oracle/test_snapshots/`, `contracts/identity-oracle/test_snapshots/`, `contracts/revocation-registry/test_snapshots/`, `contracts/governance/test_snapshots/`, `contracts/score-range-verifier/test_snapshots/`, and `contracts/tests/test_snapshots/`).

Snapshots record execution traces, storage footprints, authorization trees, and emitted events for deterministic verification across environments. If contract logic, storage representations, or emitted events change, the corresponding snapshot tests will fail until regenerated.

### Updating and regenerating snapshots

When you make intentional changes to contract logic or test flows, regenerate all workspace snapshots by running:

```bash
SOROBAN_TEST_SNAPSHOT_FILE_UPDATE=true cargo test --workspace
```

(Note: `UPDATE_EXPECT=true cargo test --workspace` can also be used for expect-based snapshot assertions.)

**Important:** Snapshot files must be reviewed via `git diff` and committed in the same PR as the code change that causes them to update. Stale snapshots are a common source of CI failures.

For more details, see the [Soroban testutils snapshot documentation](https://docs.rs/soroban-sdk/latest/soroban_sdk/testutils/index.html).

## Scripts

Root-level commands for testing, linting, and building all Rust and TypeScript packages:

```bash
pnpm test       # Run Rust and TypeScript tests
pnpm lint       # Run Clippy and ESLint (check only, no writes)
pnpm lint:fix   # Auto-fix ESLint and Clippy warnings where possible
pnpm build      # Build Rust and TypeScript packages
```

Each command:
- Exits with non-zero status if any sub-command fails
- Runs Rust tests first, then TypeScript tests
- Is the recommended way to validate before opening a PR

`pnpm lint:fix` is safe to run on a dirty working tree — it uses `--allow-dirty` for
Clippy and writes ESLint fixes in-place. Review the diff before committing.

## Opening a pull request

1. Push your branch to **your fork**: `git push origin feat/your-feature`
2. Go to your fork on GitHub and click **"Contribute" → "Open pull request"**
3. Confirm the base repository is **`cybermax4200/stellar-did-credit`** and base branch is **`main`** — GitHub sometimes defaults to your fork's own `main`, which is wrong
4. Fill in the PR template and submit

PRs opened against your own fork instead of the upstream repo will not be seen by maintainers.

## PR guidelines

- Link the issue(s) in your PR description
- All tests must pass (`pnpm test`)
- Linting must pass (`pnpm lint`)
- Snapshot files must be committed if code changes them
- Follow conventional commit format (see below)
- Reference the issue number in your PR description
- Any PR that changes contract behavior, SDK methods, or public APIs must add an entry under `[Unreleased]` in [CHANGELOG.md](CHANGELOG.md)

## Changelog updates

User-facing changes should be documented in [CHANGELOG.md](CHANGELOG.md) before opening a PR whenever a change affects behavior, public APIs, CLI output, or other visible project capabilities.

- Add a bullet under the appropriate section of `[Unreleased]` in [CHANGELOG.md](CHANGELOG.md)
- Follow the Keep a Changelog structure already used in the file: `Added`, `Changed`, `Deprecated`, `Removed`, or `Fixed`
- Keep entries concise and user-focused, and include the issue or PR number at the end of the bullet (for example, `(#174)`)
- Internal-only changes that are not user-visible usually do not need a changelog entry

Example:

- `sdk`: added a convenience helper for reading the latest score from the chain (#174)

## Contract Code Rules

Contract code must be resilient, safe, and easily auditable. Contributors writing contract logic must adhere strictly to these rules:

### Explicit rule: No `unwrap()` in contract logic

**No `unwrap()` in contract logic — use `expect("descriptive message")`.**

Bare `.unwrap()` is forbidden in contract source files (`contracts/*/src/*.rs`). If an operation can fail or an `Option`/`Result` depends on caller input or storage data, handle it gracefully by returning a typed `ContractError` via `?` or `.ok_or(ContractError::Variant)?`.

When an `Option` or `Result` represents an internal invariant that is mathematically or logically guaranteed by preceding contract logic, use `.expect("descriptive message")` with a clear explanation of why the state is invariant:

```rust
// ❌ FORBIDDEN: Bare unwrap in contract logic
let admin = env.storage().instance().get(&DataKey::Admin).unwrap();
let record = anchors.get(i).unwrap();

// ✅ RECOMMENDED: Propagate typed error for storage lookups or user inputs
let admin: Address = env
    .storage()
    .instance()
    .get(&DataKey::Admin)
    .ok_or(ContractError::NotInitialized)?;

// ✅ ALLOWED: Use expect with a descriptive message for proven internal invariants
let record = anchors
    .get(i)
    .expect("index guaranteed to be within bounds by loop range");
```

### No `panic!()` in contract logic

**Bare `panic!()` is forbidden in contract source files** (`contracts/*/src/*.rs`) outside of `#[test]` blocks. CI enforces this via the `contract-lint` job.

Allowed:
- `return Err(ErrorVariant)`
- `soroban_sdk::panic_with_error!(ErrorVariant)`
- `expect("descriptive message")` for internal contract invariants
- `env.storage().instance().get(&key).ok_or(ErrorVariant)?`
- `env.storage().instance().get(&key).unwrap_or(default)`

Forbidden:
- `panic!("error message")`
- `todo!()`, `unimplemented!()`, `unreachable!()`
- `unwrap()` in contract logic
- `env.storage().instance().get(&key).unwrap()`
- `env.storage().persistent().get(&key).unwrap()`

`panic_with_error!` is the Soroban-idiomatic way to abort execution with a typed contract error. Use it when a function signature cannot return `Result` (for example, legacy `initialize` functions that return `()`). Prefer returning `Result<(), ErrorType>` when possible.

Use `.ok_or(ErrorType)?` to propagate a typed error when storage reads fail. Use `.unwrap_or(default)` only when a fallback value is well-defined and semantically correct (for example, optional configuration that was added in a later version).

Test code (`#[cfg(test)]`, `#[test]`) is exempt from this rule.

## Auth pattern for initialize functions

All `initialize` functions in protocol contracts **must** follow this exact order:

```rust
// Security pattern: check_already_initialized → admin.require_auth() → set_admin
pub fn initialize(env: Env, admin: Address) -> Result<(), ContractError> {
    if env.storage().instance().has(&DataKey::Admin) {
        return Err(ContractError::AlreadyInitialized);
    }
    admin.require_auth();
    env.storage().instance().set(&DataKey::Admin, &admin);
    Ok(())
}
```

Rationale:
1. **Check already-initialized first** — rejects duplicate calls cheaply, before any auth overhead
2. **`require_auth()` second** — verifies the caller is authorized before any state is written
3. **Write state last** — storage is only touched after all checks pass

Do not reorder these steps. Inconsistent ordering makes security audits harder and can introduce subtle vulnerabilities. New contract functions that set privileged state must follow the same pattern.

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
