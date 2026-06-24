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
cargo test --workspace
```

## PR guidelines

- One issue per PR
- All tests must pass (`cargo test --workspace`)
- Clippy must be clean (`cargo clippy --workspace -- -D warnings`)
- Follow conventional commit format (see below)
- Reference the issue number in your PR description

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
