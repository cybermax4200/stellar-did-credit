# Security Policy

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Please report security vulnerabilities through [GitHub Security Advisories](https://github.com/cybermax4200/stellar-did-credit/security/advisories/new). This keeps the disclosure private until a fix is released.

### Response SLA

| Step | Target |
| --- | --- |
| Acknowledgement | Within 72 hours |
| Status update | Within 7 days |
| Fix or mitigation | Dependent on severity |

You will receive a response confirming receipt within 72 hours. If you do not hear back, email the maintainer via the contact listed on their GitHub profile.

---

## Scope

The following are **in scope** for vulnerability reports:

- **Smart contracts** — `identity-oracle`, `credit-oracle`, `revocation-registry` (all Soroban contracts under `contracts/`)
- **TypeScript SDK** — `packages/sdk`
- **Transaction feeders and scoring weight governance logic**

The following are **out of scope**:

- Testnet-only deployments (no real funds at risk)
- Issues in third-party dependencies that are already publicly disclosed upstream
- Denial-of-service attacks against testnet infrastructure
- Social engineering or phishing attacks

---

## Supported Versions

| Version | Supported |
| --- | --- |
| `main` branch (pre-release) | ✅ Yes |
| `v0.1.x` testnet | ✅ Yes |

Once mainnet is deployed, only the latest mainnet release will receive security fixes.

---

## Disclosure Policy

This project follows [coordinated disclosure](https://en.wikipedia.org/wiki/Coordinated_vulnerability_disclosure). Once a fix is available:

1. A patch is released and deployed.
2. A GitHub Security Advisory is published crediting the reporter (unless they prefer anonymity).
3. The CHANGELOG is updated under the affected release.

We ask reporters to refrain from public disclosure until 30 days after a fix is released, or by mutual agreement if the timeline needs to change.

---

## Supply Chain & Anti-Malware Defenses

This repository incorporates defenses against automated malware and supply chain campaigns (including campaigns targeting Web3 developers, such as PolinRider):

1. **Automated Antivirus & Malware Scanning (CI)**:
   - [.github/workflows/antimalware.yml](.github/workflows/antimalware.yml) scans commits, PRs, and weekly builds with **ClamAV** and **Socket Security**.
   - Runner integrity and outbound network monitoring is enforced via **StepSecurity Harden-Runner**.

2. **Dependency & Lifecycle Script Isolation**:
   - `pnpm.onlyBuiltDependencies` is locked in `package.json` to prevent malicious third-party `preinstall` or `postinstall` script execution.
   - Rust dependencies and crate advisories are continuously audited via `cargo deny` and `cargo-audit`.

3. **Branch Protection & Force-Push Restrictions**:
   - Repository rulesets require pull requests, code reviews, and explicitly **block force pushes** on `main` to prevent malicious history overwriting (e.g. PolinRider's force-push wipe technique).

4. **Contributor Guidelines**:
   - Keep VS Code **Workspace Trust** enabled when inspecting external contributions. Do not authorize auto-running tasks (`folderOpen`) in `.vscode/tasks.json`.
   - Never commit `.env` files or hardcoded Stellar secret keys. Local pre-commit hooks (`lefthook`) automatically enforce secret and malicious task detection before commits are created.
