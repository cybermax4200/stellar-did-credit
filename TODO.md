# Issue #257: No event emitted when initialize is called

## Status: ✅ COMPLETED

### Changes Made

| Step | File | Change |
|------|------|--------|
| 1 | `contracts/identity-oracle/src/lib.rs` | `initialize()`: `symbol_short!("Init")` → `Symbol::new(&env, "Initialized")` |
| 2 | `contracts/credit-oracle/src/lib.rs` | `initialize()`: `symbol_short!("Init")` → `Symbol::new(&env, "Initialized")` |
| 3 | `contracts/revocation-registry/src/lib.rs` | Added `Symbol` to imports; `initialize()`: `symbol_short!("Init")` → `Symbol::new(&env, "Initialized")` |
| 4 | `contracts/tests/src/integration_test.rs` | Already uses `Symbol::new(&env, "Initialized")` — no change needed |
| 5 | `docs/event-indexing.md` | Already documents `Initialized` event — no change needed |
| 6 | In-unit tests | Updated all three contract unit tests to check for `Symbol::new(&env, "Initialized")` |

### Summary

All three contracts now emit an `Initialized` event (instead of `Init`) when `initialize()` is called:

- **Topic:** `[Symbol("Initialized")]`
- **Data:** `admin: Address`
- **Emitted:** Exactly once, protected by the `AlreadyInitialized` error

The event name was changed from `Init` (via `symbol_short!`) to `Initialized` (via `Symbol::new`) because `"Initialized"` (11 chars) exceeds the `symbol_short!` 9-character limit.

