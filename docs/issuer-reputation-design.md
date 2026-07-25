# Issuer Reputation System — Design Specification

## 1. Motivation / Why This Matters

Credential issuers who issue low-quality, fraudulent, or frequently-revoked verifiable credentials (VCs) historically faced no on-chain consequences. A registered boolean was the sole on-chain signal — an issuer could anchor arbitrary VCs that artificially inflate subjects' credit scores while lenders had zero visibility into an issuer's track record.

This design introduces a per-issuer reputation profile with the following properties:

- **Metrics tracking:** each issuer accumulates on-chain counters for VCs issued and VCs revoked.
- **Reputation tiers:** issuers are bucketed into 4 tiers (0–3) that control how heavily their VCs count toward subject scores.
- **Governance-adjustable tiers:** the governance contract can demote or promote issuers based on metrics, applying a consistent, auditable rule.
- **Sybil resistance:** reputation counters are preserved across deregister/re-register cycles, preventing issuers from wiping their track record by cycling through registration.
- **No circular dependence:** tier recommendations derive exclusively from revocation ratio and issuance count — never from downstream subject credit scores.

## 2. In-Scope / Out-of-Scope

### In scope
- Per-issuer `vcs_issued` and `vcs_revoked` counters on the identity-oracle.
- 4-tier reputation system (Tier0 suspended … Tier3 gold).
- Tier-to-weight mapping for VC score contribution.
- Metrics-based pure tier recommendation on the governance contract.
- Governance-driven tier adjustment cross-contract call to identity-oracle.
- Opt-in issuer-tier weighting in the credit-oracle scoring formula.
- Integration test demonstrating a high-revocation issuer producing lower subject scores.

### Out of scope (per issue)
- Off-chain reputation systems.
- Token-based reputation / staking slashing.
- Cross-protocol reputation sharing.
- Average subject score tracking (avoided explicitly to prevent circular dependence).
- Dispute resolution flow (disputes are out of scope; revocation events alone drive metrics).

## 3. Data Structures

### 3.1 `IssuerTier` — `identity-oracle/src/lib.rs`

A 4-variant `#[contracttype]` enum describing the reputation tier of a trusted issuer. Higher tiers correspond to better track records.

| Variant | Numeric value | Weight numerator | Effective weight | Meaning |
|---------|--------------|------------------|------------------|---------|
| `Tier0` | 0 | 0 | 0.00 | Suspended — VCs contribute nothing. |
| `Tier1` | 1 | 1 | 0.25 | Bronze / probationary. |
| `Tier2` | 2 | 2 | 0.50 | Silver / standard. |
| `Tier3` | 3 | 4 | 1.00 | Gold / trusted — **default for newly registered issuers.** |

Weights use integer arithmetic (numerator ÷ `TIER_WEIGHT_DENOMINATOR = 4`) so the WASM contract never needs floating point.

### 3.2 `IssuerProfile` — `identity-oracle/src/lib.rs`

The `#[contracttype]` struct stored under `DataKey::TrustedIssuer(Address)` replacing the previous `bool` flag:

```rust
pub struct IssuerProfile {
    pub active: bool,        // currently registered (false = deregistered tombstone)
    pub vcs_issued: u32,     // all-time VCs anchored by this issuer
    pub vcs_revoked: u32,    // all-time VCs revoked by this issuer (no double count)
    pub tier: IssuerTier,    // reputation tier controlling VC weight
}
```

- **Append-only counters:** `vcs_issued` and `vcs_revoked` use `saturating_add(1)` and are never reset, even on deregistration.
- **`active` tombstone:** deregister flips `active` to `false` without removing storage, so `IssuersIndex` (the append-only list) never needs an O(n) rewrite.
- **Sybil safety:** re-registering a previously-deregistered issuer flips `active` back to `true` but leaves the counters and `tier` untouched, preserving the full track record.

## 4. Storage Keys

### 4.1 Identity Oracle

| Key | Type | Durability | Value (new vs. legacy) |
|-----|------|------------|------------------------|
| `Admin` | singleton | Instance | Address (unchanged). |
| `PendingAdmin` | singleton | Instance | Address (unchanged). |
| `RevocationRegistryId` | singleton | Instance | Address (unchanged). |
| `IssuersIndex` | singleton | Persistent | `Vec<Address>` — **append-only.** Never pruned; list_issuers filters against `active`. |
| `TrustedIssuer(Address)` | per-issuer | Persistent | **`IssuerProfile`** (was `bool` in legacy). |
| `DIDDocument(Address)` | per-subject | Persistent | String CID (unchanged). |
| `VCAnchors(Address)` | per-subject | Persistent | `Vec<VCRecord>` — each record now includes `issuer` for weighted lookup. |

### 4.2 Governance

| Key | Type | Durability | Value |
|-----|------|------------|-------|
| `IdentityOracle` | singleton | Instance | Address of the identity-oracle contract. Configured via `set_identity_oracle`; required before `adjust_issuer_tier` can operate. |

### 4.3 Credit Oracle

| Key | Type | Durability | Default | Purpose |
|-----|------|------------|---------|---------|
| `IdentityOracleId` | singleton | Instance | None | Enables cross-contract VC count lookup. |
| `UseIssuerTierWeighting` | singleton | Instance | `false` (opt-in) | When `true`, `compute_score` calls `get_weighted_vc_count` instead of `get_active_vc_count`. |

## 5. Metrics Lifecycle

### 5.1 Incrementing `vcs_issued`

On every successful `anchor_vc(issuer, subject, vc_hash)` in the identity-oracle:

1. Issuer auth is verified and `IssuerProfile.active` must be `true` (otherwise `IssuerNotRegistered`).
2. Duplicate VC hash for the same subject is rejected with `DuplicateVC`.
3. A new `VCRecord` { `vc_hash`, `issuer`, `anchored_at`, `revoked: false` } is appended to the subject's `VCAnchors` list.
4. **`vcs_issued = vcs_issued.saturating_add(1)`** is written back to the issuer's `IssuerProfile`.

### 5.2 Incrementing `vcs_revoked`

On every successful `mark_vc_revoked(issuer, subject, vc_hash)`:

1. Issuer auth is verified.
2. The matching `VCRecord` { `vc_hash`, `issuer: same issuer`, … } is located; otherwise `VCNotFound`.
3. If the record was already revoked, the counter is **not** incremented (prevents double-counting from repeated calls).
4. The record's `revoked` flag is set to `true` and persisted.
5. **`vcs_revoked = vcs_revoked.saturating_add(1)`** only on the first revocation of a given VC.

### 5.3 Global revocation registry integration

`is_record_revoked` in the identity-oracle consults an optional cross-contract `RevocationRegistryId` in addition to the record-local `revoked` flag. However, revocations performed via the global registry **do not** automatically increment the issuer's `vcs_revoked` counter — only explicit `mark_vc_revoked` calls do. This avoids implicit side effects from an independently-administered registry; issuers retain accountability through the explicit revocation path.

## 6. Reputation Tiers and the Recommendation Rule

### 6.1 Pure recommendation function — `governance/src/lib.rs`

`recommend_tier_from_metrics(vcs_issued, vcs_revoked) -> IssuerTier` is a stateless, deterministic pure function exposed both as a module-level utility and a governance contract method (so it can be invoked on-chain for auditable DAO decisions).

**Inputs:** only `vcs_issued` and `vcs_revoked`. **Deliberately excludes** subject credit scores, subject counts, or any downstream signal — this is the key invariant that prevents circular dependence.

**Algorithm (integer basis points, 10 000 = 1.00):**

```
if vcs_issued < 5:
    return Tier3                 (new / small-sample issuers are not punished)

revoked_bps = (vcs_revoked * 10_000) / max(vcs_issued, 1)

match revoked_bps:
    > 3_333  → Tier0            (> 33% revocation  → suspended, 0 weight)
    > 1_000  → Tier1            (> 10% … ≤ 33%    → bronze, 0.25 weight)
    > 0      → Tier2            ( any > 0 … ≤ 10% → silver, 0.50 weight)
    == 0     → Tier3            (flawless          → gold, 1.00 weight)
```

The 5-VC minimum sample prevents early issuers from being demoted after a single unlucky revocation (e.g. a user losing their phone and revoking their own VC).

### 6.2 Tier adjustment via governance

Governance retains full discretion — the recommendation is informational only. The flow for applying a tier change is:

1. Governance admin calls `set_identity_oracle(admin, identity_oracle_id)` once during setup.
2. For each tier action, governance admin calls `adjust_issuer_tier(admin, issuer, target_tier)`:
   - Auth: governance admin signature verified.
   - Cross-contract call: `IdentityOracleClient.set_issuer_tier(issuer, target_tier)`.
   - Identity-oracle validates admin auth, updates the issuer's `IssuerProfile.tier`, and emits an `IssTier` event.
3. For proposals that want to follow the metrics rule transparently, voters can run `Governance::recommend_tier_from_metrics(vcs_issued, vcs_revoked)` on-chain or off-chain and bind the proposal to the recommended tier.

### 6.3 Direct admin adjustment on identity-oracle

The identity-oracle also exposes `set_issuer_tier(env, issuer, tier)` directly for the admin account, independent of the governance contract. This supports pre-DAO operation and emergency override scenarios.

## 7. Integration with Credit Scoring

### 7.1 Weighted VC count — identity-oracle

`get_weighted_vc_count(subject) -> u32` returns the sum of tier-adjusted VC "hundredths":

- For every active (non-revoked) VC anchored for the subject:
  - Look up the issuing issuer's `IssuerProfile.tier`.
  - Convert via `tier_weight_numerator(tier) * 100 / TIER_WEIGHT_DENOMINATOR`:
    - Tier3 → `4 * 100 / 4 = 100` hundredths (= 1 full effective VC).
    - Tier2 → `2 * 100 / 4 = 50` hundredths.
    - Tier1 → `1 * 100 / 4 = 25` hundredths.
    - Tier0 → `0 * 100 / 4 = 0` hundredths.
- Sum with `saturating_add`; return the aggregate as integer hundredths.

Returning hundredths rather than a fractional float preserves the invariant that all on-chain arithmetic is integer-only.

### 7.2 Opt-in weighting — credit-oracle

The credit-oracle `compute_score(subject)` selects between two VC-count strategies:

```
if UseIssuerTierWeighting == true AND IdentityOracleId is configured:
    vc_count_effective = get_weighted_vc_count(subject) / 100   (floor div)
else if IdentityOracleId is configured:
    vc_count_effective = get_active_vc_count(subject)
else:
    vc_count_effective = cached VcCount(subject)                (legacy fallback)
```

`vc_count_effective` is then plugged into the standard formula:

```
vc_score = min(vc_count_effective * 20, 100)
```

**Conservative rounding:** because `weighted_hundredths / 100` uses floor division, a single Tier2 VC (50/100 = 0 effective count) doesn't inflate the score at all. Fractions only surface in aggregate (e.g. 2 × Tier2 = 100 hundredths = 1 effective VC). This rounding is intentionally Sybil-hostile: low-tier issuers must issue proportionally more VCs to match a single Tier3 issuer.

### 7.3 Enabling the feature

```rust
CreditOracleClient.set_issuer_tier_weighting(&true);        // admin-gated
CreditOracleClient.set_identity_oracle(&identity_oracle_id); // admin-gated
```

Default is `UseIssuerTierWeighting = false` for **backwards compatibility** — deployments upgrading the contract preserve the existing uniform-weight scoring behavior until admin opts in.

## 8. Sybil Resistance Guarantees

| Attack vector | Mitigation |
|---------------|------------|
| **Deregister → re-register to wipe counters** | `deregister_issuer` flips `active = false` only; `vcs_issued`, `vcs_revoked`, and `tier` are untouched. `register_issuer` on a known issuer restores `active = true` without resetting anything. |
| **Spin up a new issuer address for each batch** | Each new address starts at Tier3 with zero counters — the same starting privilege. The *first* bad batch still demotes the new address once 5+ VCs are issued and revocations exceed thresholds. Gaining trust requires sustained good behavior per address, which has a linear cost proportional to the number of Sybils. |
| **Issue many cheap VCs to boost `vcs_issued` denominator** | The recommendation only demotes (never promotes) based on ratio. Inflating `vcs_issued` without corresponding revocations keeps the issuer at Tier3 — which is already the default. No incentive to Sybil for issuance volume alone. |
| **Register → deregister → re-register repeatedly** | No cost reduction; counters are monotonic and never reset. The track record simply accumulates across every cycle. |

## 9. Circular Dependence Prevention

The design avoids the reputation ↔ score loop:

1. **Tier recommendation inputs:** `vcs_issued`, `vcs_revoked` only. Neither is derived from subject credit scores.
2. **Revocation is an issuer action:** revocations reflect the issuer's decision to invalidate a credential (or the credential being proven fraudulent), not downstream performance of the subject's score.
3. **No average-subject-score tracking:** out of scope explicitly. Including it would create the exact cycle we're avoiding (issuer reputation → subject scores → issuer reputation).

## 10. Cross-Contract Call Flow

```
  Governance                       Identity Oracle                Credit Oracle
      │                                  │                               │
      │ set_identity_oracle(admin, ido)  │                               │
      ├─────────────────────────────────►│                               │
      │                                  │                               │
      │ adjust_issuer_tier(admin, iss, tier)                             │
      │  ┌─ auth: gov admin ─┐           │                               │
      │  └───────────────────┘           │                               │
      │     set_issuer_tier(iss, tier)   │                               │
      ├─────────────────────────────────►│                               │
      │                                  │  IssuerProfile.tier = tier    │
      │                                  │                               │
      │                                  │                               │ compute_score(subject)
      │                                  │                               │ ┌─ UseIssuerTierWeighting? ─┐
      │                                  │                               │ └─ yes, IdentityOracleId set ┘
      │                                  │  get_weighted_vc_count(sub)   │
      │                                  │◄──────────────────────────────┤
      │                                  │  sum(tier_weight * 100/4)     │
      │                                  ├──────────────────────────────►│
      │                                  │                               │ vc_eff = weighted/100
      │                                  │                               │ vc_score = min(vc_eff*20, 100)
      │                                  │                               │ → persist ScoreRecord
```

## 11. Test Coverage

### 11.1 Identity Oracle unit tests

| Test | What it verifies |
|------|------------------|
| `test_anchor_vc_by_trusted_issuer` | Sanity: registered issuer anchors successfully. |
| `test_unregistered_issuer_fails` | Non-registered calls to `anchor_vc` get `IssuerNotRegistered`. |
| `test_deregister_issuer_succeeds` | `active` → false; counters and tier are preserved. |
| `test_issuer_metrics_increment_on_issue_and_revoke` | `vcs_issued` increments on each anchor; `vcs_revoked` increments exactly once per revocation even on repeated calls. |
| `test_set_issuer_tier_changes_weighted_vc_count` | Tier promotion/demotion propagates to `get_weighted_vc_count` (Tier3+Tier2 → 150 hundredths, then Tier1+Tier0 → 25 hundredths). |
| `test_deregistered_issuer_cannot_anchor_vc` | Deregistered tombstone issuer is locked out of anchoring. |
| `test_list_issuers_reflects_register_and_deregister_operations` | Append-only `IssuersIndex` with `active` filtering produces the expected set. |
| `test_reregistering_deregistered_issuer_does_not_duplicate_index` | Re-registration is idempotent and Sybil-safe. |

### 11.2 Governance tests

- `test_governance_proposal_creation_voting_and_execution` — baseline governance flow sanity.
- `test_proposal_with_exactly_quorum_votes_succeeds` — quorum edge case.
- `test_vote_rejects_non_positive_weight` — vote integrity.

### 11.3 Credit Oracle tests

- `test_base_score_is_300` / `test_exceptional_score_equals_850` — score bounds.
- `test_score_bounded_300_850` — clamping invariant.
- `test_score_in_range_for_all_weight_boundaries` — all weight combinations stay in range.
- Property-based (`proptest!`): `proptest_score_always_in_range`, `proptest_score_monotone_on_repayment`, `proptest_no_panic_on_any_valid_weights`.

### 11.4 Integration test — high revocation issuer

`test_high_revocation_issuer_produces_lower_scores` — the acceptance-criteria cornerstone:

1. **Setup:** Deploy and link identity-oracle, credit-oracle, and governance. Enable `UseIssuerTierWeighting`.
2. **Two issuers:**
   - **Issuer G (Good):** issues 4 VCs to SubjectG, revokes none → `vcs_issued=4, vcs_revoked=0` → recommended Tier3.
   - **Issuer B (Bad):** issues 4 VCs to SubjectB (all active) PLUS issues 4 VCs to SubjectX and revokes all 4 → `vcs_issued=8, vcs_revoked=4` (50% revocation ratio) → recommended Tier0.
3. **Metrics verification:** identity-oracle `get_issuer_profile` returns the expected counters for both issuers.
4. **Recommendation:** `Governance::recommend_tier_from_metrics` returns Tier3 / Tier0 as predicted.
5. **Governance applies tiers:** `adjust_issuer_tier` forwards each to identity-oracle.
6. **Raw counts are equal:** `get_active_vc_count(subject_g) == get_active_vc_count(subject_b) == 4`.
7. **Weighted counts diverge:** `get_weighted_vc_count(subject_g) == 400` (4 Tier3 = 4·100) vs. `subject_b == 0` (4 Tier0 = 4·0).
8. **Without weighting:** subjects have identical tx/repayment inputs → scores are EQUAL.
9. **With weighting enabled:** subject_g's score strictly exceeds subject_b's score. Additionally, subject_b's weighted score is strictly less than its own unweighted score — confirming the penalty is real and not just a relative reshuffle.

## 12. Events (for off-chain indexing)

| Contract | Symbol | Data | When emitted |
|----------|--------|------|--------------|
| Identity Oracle | `IssReg` | `(issuer: Address)` | Issuer registered or re-registered (active restored). |
| Identity Oracle | `IssDeReg` | `(issuer: Address)` | Issuer deregistered (active flipped to false). |
| Identity Oracle | `IssTier` | `(issuer: Address, tier: u32)` | Issuer tier changed via `set_issuer_tier`. |
| Identity Oracle | `VCAnch` | `(issuer, subject, vc_hash)` | VC anchored; `vcs_issued` incremented. |
| Credit Oracle | `WtTier` | `(enabled: bool)` | `set_issuer_tier_weighting` toggled. |

Event consumers (off-chain indexers, dashboards, dispute UIs) can combine `IssTier`, `IssReg`, and the per-VC events to reconstruct a live reputation timeline for every issuer.

## 13. Upgrade / Deployment Notes

1. **Backwards compatibility:** `TrustedIssuer` storage format changes from `bool` to `IssuerProfile`. For new deployments this is a no-op; existing deployments planning an in-place upgrade must plan a migration (not required for this issue — greenfield assumption).
2. **Opt-in feature flag:** `UseIssuerTierWeighting` defaults to `false`. After the upgrade, run `set_issuer_tier_weighting(true)` in a separate admin transaction to avoid surprising integrators.
3. **Governance linking:** run `governance.set_identity_oracle(admin, ido_id)` in the same deployment batch so tier adjustments are available from day one.
4. **Instance TTL:** every admin-gated call `extend_ttl(INSTANCE_BUMP_THRESHOLD = 5000, INSTANCE_BUMP_AMOUNT = 500_000)` which equates to ~170 days of ledgers — safe default that can be bumped on any admin call.
