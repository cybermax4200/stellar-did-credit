# TODO - Active VC count fix

- [ ] Update `contracts/identity-oracle/src/lib.rs`
  - [ ] Rename `get_vc_count` to `get_total_vc_count` (and add backward-compatible wrapper if needed)
  - [ ] Add `get_active_vc_count(env: Env, subject: Address) -> u32`
  - [ ] Add unit test `test_get_active_vc_count_excludes_revoked`

- [ ] Update documentation to reference `get_active_vc_count`
  - [ ] `docs/architecture.md`
  - [ ] Confirm/adjust `docs/scoring-spec.md` if it mentions vc_count semantics

- [ ] Update any other repo references to `get_vc_count` (if found)
- [ ] Run test suite
  - [ ] `cargo test -p identity-oracle`
  - [ ] `cargo test -p contracts` (or workspace tests)
