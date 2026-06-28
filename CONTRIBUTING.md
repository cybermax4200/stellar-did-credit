# Contributing to stellar-did-credit

## ⚠️ Never commit secrets

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
- Any PR that changes contract behavior, SDK methods, or public APIs must add an entry under `[Unreleased]` in [CHANGELOG.md](../CHANGELOG.md)

## Auth pattern for initialize functions

All `initialize` functions in protocol contracts **must** follow this exact order:

```rust
// Security pattern: check_already_initialized → admin.require_auth() → set_admin
pub fn initialize(env: Env, admin: Address) {
    if env.storage().instance().has(&DataKey::Admin) {
        panic!("already initialized");
    }
    admin.require_auth();
    env.storage().instance().set(&DataKey::Admin, &admin);
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
