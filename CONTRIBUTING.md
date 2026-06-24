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
- Any PR that changes contract behavior, SDK methods, or public APIs must add an entry under `[Unreleased]` in [CHANGELOG.md](../CHANGELOG.md)

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
