## Summary

<!-- What does this PR do? One or two sentences. -->

## Related issue

Closes #

## Changes

<!-- Brief bullet list of what changed -->

## Testing

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings
- [ ] New functions have at least one test
- [ ] New public contract functions have a `///` doc comment
- [ ] Updated `docs/gas-costs.md` if any contract function was added or modified

For contract changes, re-run `bash scripts/estimate-gas.sh` and copy the output into the PR description.

## Changelog

- [ ] Added a `CHANGELOG.md` entry under `[Unreleased]` (required for contract behavior, SDK, or public API changes; N/A otherwise)
