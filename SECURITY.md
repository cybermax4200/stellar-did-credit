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
