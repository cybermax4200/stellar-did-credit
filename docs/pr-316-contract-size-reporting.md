# Add WASM contract size reporting to build output

Closes #316

## Summary

Adds WASM binary size reporting to the build process so integrators can see contract sizes before deployment, preventing surprise rejections due to Soroban's maximum WASM size limits.

## Changes

### `scripts/build.sh` (new)
- Builds all contracts with `cargo build --release --target wasm32-unknown-unknown`
- Prints a formatted size report table with each contract's size in **bytes** and **KB**
- Warns if any contract exceeds a configurable threshold (default: **100 KB**, set via `WASM_SIZE_LIMIT_KB` env var or `--threshold <KB>` flag)
- Follows existing script conventions (`set -euo pipefail`, `--help` support)

### `.github/workflows/ci.yml`
- Added a new `build` CI job that runs `scripts/build.sh`
- Installs `wasm32-unknown-unknown` target and caches cargo artifacts
- WASM size report appears in CI logs for every push/PR

## Output example

```
=================================================================
  WASM Binary Size Report
=================================================================
  OK       identity_oracle.wasm                    12345 bytes      12.06 KB
  OK       credit_oracle.wasm                      23456 bytes      22.91 KB
  WARNING  revocation_registry.wasm               123456 bytes     120.56 KB  (exceeds 100 KB threshold)
-----------------------------------------------------------------
Threshold: 100 KB per contract
Result: Some contracts exceed the size threshold.
=================================================================
```
