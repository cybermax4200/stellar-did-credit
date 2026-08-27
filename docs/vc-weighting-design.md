# VC Weighting Design

This document proposes how verifiable credential (VC) metadata — issuer trust, credential type, and issuance recency — can influence the credit score beyond a raw VC count.

## Problem

The legacy formula treats every active VC identically:

```
vc_score = min(vc_count × 20, 100)
```

A regulated KYC credential and a low-assurance self-issued credential contribute the same 20 points. Lenders need issuer trust and credential quality reflected in the score.

## Proposed model

Each **active** VC contributes weighted points. The VC component is the sum of per-credential contributions, capped at 100:

```
vc_score = min( Σ credential_points(vc), 100 )

credential_points(vc) = base_points × issuer_tier_bps × type_weight_bps ÷ 10_000
```

| Parameter | Default | Where configured | Description |
| --------- | ------- | ---------------- | ----------- |
| `base_points` | 20 | credit-oracle (fixed in prototype) | Base contribution per VC before multipliers |
| `issuer_tier_bps` | 100 (1×) | identity-oracle admin via `set_issuer_tier` | Issuer trust multiplier in basis points |
| `type_weight_bps` | 100 (1×) | credit-oracle admin via `set_credential_type_weight` | Credential-type multiplier in basis points |

Basis points use 100 = 1.00×, 200 = 2.00×, 150 = 1.50×.

### Issuer trust (implemented)

Trusted issuers are already registered in identity-oracle. The prototype adds an admin-configurable tier per issuer:

- `set_issuer_tier(admin, issuer, weight_bps)` — e.g. regulated bank at 200 bps (2×)
- `get_issuer_tier(issuer)` — returns 100 bps when unset (backward compatible)

Existing anchored VCs automatically pick up tier changes on the next `compute_score` call.

### Credential type (implemented)

VC anchors remain backward compatible: `anchor_vc` stores records without an explicit type. Untyped VCs default to the `generic` symbol (100 bps unless overridden).

New anchors can call `anchor_vc_typed(issuer, subject, vc_hash, credential_type)` to attach a type label (e.g. `kyc`, `employment`, `email`).

Type weights are configured on credit-oracle:

- `set_credential_type_weight(admin, credential_type, weight_bps)` — e.g. `kyc` at 150 bps
- `get_credential_type_weight(credential_type)` — returns 100 bps when unset

### Recency (implemented — opt-in, see #530)

Recency rewards fresh credentials and decays stale ones without deleting anchors:

```
recency_multiplier = max(min_recency_bps, 10_000 − age_days × decay_bps_per_day)

credential_points(vc) = base_points × type_weight_bps ÷ 100 × recency_multiplier ÷ 10_000
```

`age_days = (now − anchored_at) ÷ 86_400`, in whole days — a credential keeps full
weight for its first 24 hours. `anchored_at` is the ledger timestamp (Unix
seconds) recorded by identity-oracle when the VC was anchored.

Configuration lives on credit-oracle:

- `set_recency_decay(admin, enabled, decay_bps_per_day, min_recency_bps)` — admin only
- `get_recency_decay()` — returns `RecencyDecayConfig { enabled, decay_bps_per_day, min_recency_bps }`

Defaults, applied when the admin has never configured decay:

| Parameter | Default | Note |
| --------- | ------- | ---- |
| `enabled` | `false` | Opt-in — deployments score identically until switched on |
| `decay_bps_per_day` | 5 (0.05% per day) | |
| `min_recency_bps` | 5000 (50% floor) | Values above 10_000 are rejected with `InvalidRecencyConfig` |

**Requires a configured identity-oracle.** `anchored_at` is only available from
`get_vc_details`, so decay applies on the cross-contract path only. With no
identity-oracle link, the local `VcList` / `VcCount` fallbacks are unchanged.

Decay is uniform across credential types; per-type decay rates remain out of
scope. No storage migration is required — the three new instance keys are
optional and read back as "disabled" on contracts deployed before this change.

## Data flow

```mermaid
sequenceDiagram
    participant CR as credit-oracle
    participant ID as identity-oracle

    CR->>ID: get_vc_details(subject)
    ID-->>CR: Vec<VCRecord> (active only)
    loop each VC
        CR->>ID: get_issuer_tier(record.issuer)
        ID-->>CR: issuer_tier_bps
        CR->>ID: get_vc_credential_type(subject, vc_hash)
        ID-->>CR: credential_type Symbol
        CR->>CR: lookup type_weight_bps (local storage)
    end
    CR->>CR: vc_score = min(Σ weighted points, 100)
```

When identity-oracle is not configured, credit-oracle falls back to the legacy path:

```
vc_score = min(vc_count × 20, 100)
```

## Examples

All examples use `base_points = 20`, default weights 100 bps unless noted.

| VC | Issuer tier | Type | Type weight | Points |
| -- | ----------- | ---- | ----------- | ------ |
| 1× generic | 100 bps | generic | 100 bps | 20 |
| 1× KYC | 200 bps | kyc | 150 bps | 60 |
| 2× generic (tier-1 issuer) | 100 bps | generic | 100 bps | 40 |

Three tier-1 generic VCs → `vc_score = 60` (legacy-equivalent for uniform credentials).

One tier-2 KYC VC → `vc_score = 60`, matching three generic VCs — reflecting higher assurance.

## Backward compatibility

| Scenario | Behavior |
| -------- | -------- |
| Recency decay never configured | Disabled — per-VC points identical to pre-#530 |
| Recency decay disabled again | Next `compute_score` drops the multiplier entirely |
| Recency enabled, no identity-oracle | No decay — `anchored_at` is unavailable |
| Existing `anchor_vc` records | Treated as `generic` type, issuer tier 100 bps |
| No identity-oracle link on credit-oracle | Legacy `vc_count × 20` via cached feeder count |
| Issuer tier unset | Defaults to 100 bps (same as legacy per-VC value) |
| Type weight unset | Defaults to 100 bps |

## Future work (out of scope for #243)

- Per-credential-type decay rates (#530 implements a single uniform rate)
- Standardized credential schema (SEP) validation
- Real-time off-chain verification callbacks
- Storing `credential_type` inline on `VCRecord` after a migration window
