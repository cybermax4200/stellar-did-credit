$bkp = Join-Path $env:TEMP 'issuer_rep_bkp'
$repo = 'c:\Users\Aria\Desktop\stella\stellar-did-credit'
Set-Location $repo

if (Test-Path .git) { Remove-Item -Recurse -Force .git }

git init
git config user.email 'trae-agent@local'
git config user.name 'Trae Agent'
git remote add origin https://github.com/victor62-art/stellar-did-credit.git
git fetch origin main:refs/remotes/origin/main
if ($LASTEXITCODE -ne 0) { throw "fetch failed" }

# Reset index + HEAD to origin/main, keep working tree as-is (which already has our modified files in it)
git reset --mixed origin/main
if ($LASTEXITCODE -ne 0) { throw "reset failed" }

# Create feature branch on top of origin/main
git checkout -b feat/issuer-reputation-tiers
if ($LASTEXITCODE -ne 0) { throw "checkout feat branch failed" }

# Copy modified files from backup (in case reset --mixed reverted worktree contents to origin/main)
Copy-Item (Join-Path $bkp 'identity_oracle_lib.rs') (Join-Path $repo 'contracts\identity-oracle\src\lib.rs') -Force
Copy-Item (Join-Path $bkp 'governance_lib.rs')        (Join-Path $repo 'contracts\governance\src\lib.rs')      -Force
Copy-Item (Join-Path $bkp 'governance_Cargo.toml')    (Join-Path $repo 'contracts\governance\Cargo.toml')       -Force
Copy-Item (Join-Path $bkp 'credit_oracle_lib.rs')     (Join-Path $repo 'contracts\credit-oracle\src\lib.rs')    -Force
Copy-Item (Join-Path $bkp 'integration_test.rs')      (Join-Path $repo 'contracts\tests\src\integration_test.rs') -Force

Write-Host "--- git status after copying modified files ---"
git status --short

git add contracts/identity-oracle/src/lib.rs contracts/governance/src/lib.rs contracts/governance/Cargo.toml contracts/credit-oracle/src/lib.rs contracts/tests/src/integration_test.rs

$msg = @'
feat(identity-oracle, governance, credit-oracle): on-chain issuer reputation with tiered VC weighting

Tracks per-issuer metrics (vcs_issued, vcs_revoked) in identity-oracle, updated
on every anchor_vc / mark_vc_revoked. Introduces 4-level IssuerTier (Tier0..Tier3)
with bps VC weighting (2500/5000/7500/10000) and admin/governance-gated tier API.

Identity-oracle:
- IssuerTier enum (weight_bps), IssuerMetrics { vcs_issued, vcs_revoked }
- register_issuer initializes zeroed metrics + Tier3 default
- anchor_vc increments vcs_issued; mark_vc_revoked increments vcs_revoked (once-per-VC)
- set_governance / get_issuer_metrics / get_issuer_tier / set_issuer_tier
- get_weighted_vc_count(subject): floors per-VC tier contribution, 4 Tier0 = 1 eff

Governance:
- initialize now requires identity_oracle address
- adjust_issuer_tier rule engine: <5 VCs keep tier; rev-rate >=40% Tier0,
  >=25% Tier1, >=10% Tier2, else Tier3 (Sybil-dampening; deterministic off-chain)
- set_issuer_tier_override for admin appeals/bootstrap

Credit-oracle:
- UseIssuerTierWeighting opt-in flag (defaults false for backward compat)
- When enabled + identity-oracle set, compute_score uses get_weighted_vc_count
  (no circularity: subject score never feeds back into issuer metrics/tiers)

Integration test: GOOD (10% revocation) vs BAD (80% revocation) issuers, each
with 4 active VCs into identical subjects. Governance adjust_issuer_tier demotes
BAD to Tier0, GOOD to Tier2. subject_good weighted VC count = 3 vs subject_bad = 1,
produces score_good > score_bad with >=20-point delta (core acceptance criterion).
'@
git commit -m $msg
if ($LASTEXITCODE -ne 0) { throw "commit failed" }

Write-Host "--- commit log ---"
git log --oneline -n 3

Write-Host "--- push ---"
git push -u origin feat/issuer-reputation-tiers
if ($LASTEXITCODE -ne 0) { throw "push failed" }

Write-Host "--- DONE ---"
