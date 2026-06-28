# TODO - Credit Oracle property-based testing

- [x] Add `proptest = "1"` to `contracts/credit-oracle/Cargo.toml` dev-dependencies.
- [x] Implement proptest-based unit tests in `contracts/credit-oracle/src/lib.rs`:
  - [x] `proptest_score_always_in_range`
  - [x] `proptest_score_monotone_on_repayment`
  - [x] `proptest_no_panic_on_any_valid_weights`
- [ ] Run `cargo test -p credit-oracle` and confirm all tests pass. (blocked locally: Windows linker `link.exe` not found)
- [ ] Update TODO progress as property tests pass.



