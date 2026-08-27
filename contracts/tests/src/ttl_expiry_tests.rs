//! TTL expiry simulation tests (issue #527).
//!
//! # Finding: Soroban testutils **does** enforce TTL expiry
//!
//! Issue #527 asked whether `soroban-sdk`'s test environment simulates storage
//! archival, and to mark the tests `#[ignore]` if it does not. It does — so
//! none of these tests are ignored. What it does *not* do is expire entries
//! gracefully. Reading an archived entry **panics**:
//!
//! ```text
//! HostError: Error(Storage, InternalError)
//! [testing-only] Accessed contract data key that has been archived.
//! Important: this error may only appear in tests; in the real network
//! contracts aren't called at all if any archived entry is accessed.
//! ```
//!
//! This contradicts the "graceful default, not panic" outcome the issue
//! hypothesised, and the contradiction is the point: `.get(&key).unwrap_or(d)`
//! does **not** protect a caller from an archived entry. The host rejects the
//! access before `unwrap_or` is ever reached. On the real network the whole
//! invocation is rejected before execution starts, so a `#[should_panic]` test
//! here is the faithful encoding of "this call becomes impossible".
//!
//! # Measured environment (soroban-sdk 22.0.11, `Env::default()`)
//!
//! | Property | Value | Notes |
//! | -------- | ----- | ----- |
//! | `sequence_number` | 0 | tests start at ledger 0 |
//! | `min_temp_entry_ttl` | 16 | |
//! | `min_persistent_entry_ttl` | 4096 | ~5.7 h at 5 s/ledger |
//! | `max_entry_ttl` | 6_312_000 | ceiling for `extend_ttl` |
//!
//! A persistent entry written **without** `extend_ttl` therefore lives only
//! ~4096 ledgers, not the ~30 days the `PERS_TTL_EXTEND` constant suggests.
//!
//! # What these tests establish
//!
//! | Entry | Written with `extend_ttl`? | Lifetime |
//! | ----- | -------------------------- | -------- |
//! | `identity-oracle` `VCAnchors` | yes | 518_400 |
//! | `credit-oracle` `TrustedFeeder` | yes | 518_400 |
//! | `credit-oracle` `Score` | yes | 518_400 |
//! | `credit-oracle` `TxStats` | **no** | 4096 (default) |
//! | `credit-oracle` instance | **never bumped** | 4096 (default) |
//!
//! The last two rows are live defects and are exactly what issues 5 and 6 must
//! fix. Each is covered by a pair of tests: one pinning today's expiry
//! behaviour, and one proving the entry survives once its TTL *is* extended.
//! The survival test is the harness a contributor fixing those issues runs to
//! confirm the fix — it passes today only because the test extends the TTL by
//! hand, and should keep passing unchanged once the contract does it instead.
//!
//! # Why `keep_instance_alive` appears in every test
//!
//! `credit-oracle` never calls `instance().extend_ttl`, so its instance entry
//! expires after the default 4096 ledgers — sooner than any persistent entry
//! under test. Without pinning the instance open, every jump would panic on
//! *instance* archival and tell us nothing about the persistent key we meant
//! to test. `test_credit_oracle_instance_archives_after_default_ttl` covers
//! that failure mode deliberately, on its own.

#[cfg(test)]
mod tests {
    use credit_oracle::{CreditOracle, CreditOracleClient, DataKey as CreditKey, TxStats};
    use identity_oracle::{IdentityOracle, IdentityOracleClient};
    use soroban_sdk::testutils::storage::{Instance as _, Persistent as _};
    use soroban_sdk::testutils::{Address as _, Ledger as _};
    use soroban_sdk::{Address, BytesN, Env};

    /// `min_persistent_entry_ttl` in `Env::default()`; the lifetime an entry
    /// gets when the contract never calls `extend_ttl`.
    const DEFAULT_PERSISTENT_TTL: u32 = 4096;

    /// Mirrors `PERS_TTL_EXTEND` in the contracts (~30 days at 5 s/ledger).
    const PERS_TTL_EXTEND: u32 = 518_400;

    /// Mirrors `PERS_TTL_THRESHOLD` in the contracts (~7 days at 5 s/ledger).
    const PERS_TTL_THRESHOLD: u32 = 120_960;

    /// Substring of the panic raised when an archived entry is accessed.
    const ARCHIVED: &str = "Error(Storage, InternalError)";

    /// Pins a contract's instance entry open so a ledger jump can only archive
    /// the *persistent* key under test. See the module docs.
    fn keep_instance_alive(env: &Env, id: &Address) {
        env.as_contract(id, || {
            env.storage().instance().extend_ttl(6_000_000, 6_000_000);
        });
    }

    fn advance(env: &Env, ledgers: u32) {
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + ledgers);
    }

    // -----------------------------------------------------------------
    // 0. Environment assumptions
    // -----------------------------------------------------------------

    /// Pins the two TTL values every other test reasons about. If a future SDK
    /// bump changes `min_persistent_entry_ttl` or the effect of `extend_ttl`,
    /// this test fails first and explains why the rest did.
    #[test]
    fn test_testutils_ttl_baseline() {
        let env = Env::default();
        env.mock_all_auths();

        assert_eq!(env.ledger().sequence(), 0, "tests start at ledger 0");

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = Address::generate(&env);
        let feeder = Address::generate(&env);
        let subject = Address::generate(&env);

        credit.initialize(&admin);
        credit.register_feeder(&admin, &feeder);
        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 1,
                tx_count_30d: 1,
                avg_counterparties: 1,
            },
        );

        env.as_contract(&credit_id, || {
            // register_feeder calls extend_ttl(PERS_TTL_THRESHOLD, PERS_TTL_EXTEND).
            assert_eq!(
                env.storage()
                    .persistent()
                    .get_ttl(&CreditKey::TrustedFeeder(feeder.clone())),
                PERS_TTL_EXTEND,
                "TrustedFeeder should carry the extended TTL",
            );

            // update_tx_stats does not call extend_ttl, so TxStats only gets
            // the environment default — the defect issues 5/6 must fix.
            assert_eq!(
                env.storage()
                    .persistent()
                    .get_ttl(&CreditKey::TxStats(subject.clone())),
                DEFAULT_PERSISTENT_TTL - 1,
                "TxStats is written without extend_ttl, so it only gets min_persistent_entry_ttl",
            );

            // credit-oracle never bumps its instance TTL either.
            assert_eq!(
                env.storage().instance().get_ttl(),
                DEFAULT_PERSISTENT_TTL - 1,
                "credit-oracle never calls instance().extend_ttl",
            );
        });
    }

    // -----------------------------------------------------------------
    // 1. VC anchors (identity-oracle)
    // -----------------------------------------------------------------

    fn anchor_one_vc(env: &Env) -> (Address, IdentityOracleClient<'_>, Address) {
        let id = env.register_contract(None, IdentityOracle);
        let identity = IdentityOracleClient::new(env, &id);

        let admin = Address::generate(env);
        let issuer = Address::generate(env);
        let subject = Address::generate(env);

        identity.initialize(&admin);
        identity.register_issuer(&issuer);
        identity.anchor_vc(&issuer, &subject, &BytesN::from_array(env, &[9u8; 32]));

        (id, identity, subject)
    }

    /// `anchor_vc` extends `VCAnchors` to PERS_TTL_EXTEND, so the credential is
    /// still verifiable well beyond the 4096-ledger default.
    #[test]
    fn test_vc_anchor_survives_within_extended_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, identity, subject) = anchor_one_vc(&env);
        assert!(identity.is_verified(&subject));

        keep_instance_alive(&env, &id);
        advance(&env, PERS_TTL_EXTEND - 1);

        assert!(
            identity.is_verified(&subject),
            "an extended VC anchor must survive up to PERS_TTL_EXTEND",
        );
    }

    /// One ledger past PERS_TTL_EXTEND the anchor is archived. Note the failure
    /// mode: `is_verified` does not return `false`, it panics — a consumer
    /// cannot distinguish "no credential" from "credential archived" by calling
    /// it, because the call never returns.
    #[test]
    #[should_panic(expected = "Error(Storage, InternalError)")]
    fn test_vc_anchor_expires_past_extended_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, identity, subject) = anchor_one_vc(&env);
        keep_instance_alive(&env, &id);
        advance(&env, PERS_TTL_EXTEND + 1);

        let _ = identity.is_verified(&subject);
    }

    // -----------------------------------------------------------------
    // 2. TxStats (credit-oracle) — written WITHOUT extend_ttl
    // -----------------------------------------------------------------

    fn credit_with_stats(env: &Env) -> (Address, CreditOracleClient<'_>, Address, Address) {
        let id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(env, &id);

        let admin = Address::generate(env);
        let feeder = Address::generate(env);
        let subject = Address::generate(env);

        credit.initialize(&admin);
        credit.register_feeder(&admin, &feeder);
        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 5_000_000_000,
                tx_count_30d: 8,
                avg_counterparties: 10,
            },
        );

        (id, credit, feeder, subject)
    }

    /// Issue #527 predicted `compute_score` would "default to zero inputs" once
    /// TxStats expired. It does not — the read panics before `unwrap_or` runs.
    /// TxStats is the shortest-lived persistent entry in credit-oracle because
    /// `update_tx_stats` never calls `extend_ttl`.
    #[test]
    #[should_panic(expected = "Error(Storage, InternalError)")]
    fn test_tx_stats_expires_without_ttl_extension() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, credit, _feeder, subject) = credit_with_stats(&env);
        keep_instance_alive(&env, &id);

        // Well inside PERS_TTL_EXTEND, but past the 4096-ledger default.
        advance(&env, DEFAULT_PERSISTENT_TTL + 1);

        let _ = credit.compute_score(&subject);
    }

    /// Harness for issues 5 and 6: extend the TxStats TTL the way
    /// `update_tx_stats` should, and the same jump that panics above becomes a
    /// clean score computation. A contributor fixing the contract deletes the
    /// manual `extend_ttl` block and this test must still pass.
    #[test]
    fn test_tx_stats_survives_when_ttl_is_extended() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, credit, _feeder, subject) = credit_with_stats(&env);
        keep_instance_alive(&env, &id);

        // --- stands in for the missing extend_ttl inside update_tx_stats ---
        env.as_contract(&id, || {
            env.storage().persistent().extend_ttl(
                &CreditKey::TxStats(subject.clone()),
                PERS_TTL_THRESHOLD,
                PERS_TTL_EXTEND,
            );
        });
        // -------------------------------------------------------------------

        advance(&env, DEFAULT_PERSISTENT_TTL + 1);

        let score = credit.compute_score(&subject);
        assert!(
            score > 300,
            "surviving TxStats must still lift the score above the 300 floor, got {}",
            score,
        );
    }

    // -----------------------------------------------------------------
    // 3. Score records (credit-oracle)
    // -----------------------------------------------------------------

    fn credit_with_score(env: &Env) -> (Address, CreditOracleClient<'_>, Address) {
        let id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(env, &id);

        let admin = Address::generate(env);
        let subject = Address::generate(env);

        credit.initialize(&admin);
        // No TxStats written, so only the Score entry is under test here.
        credit.compute_score(&subject);

        (id, credit, subject)
    }

    /// `compute_score` extends the Score entry, so `get_score` keeps returning
    /// the record for the full PERS_TTL_EXTEND window.
    #[test]
    fn test_score_survives_within_extended_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, credit, subject) = credit_with_score(&env);
        assert!(credit.get_score(&subject).is_some());

        keep_instance_alive(&env, &id);
        advance(&env, PERS_TTL_EXTEND - 1);

        assert!(
            credit.get_score(&subject).is_some(),
            "an extended Score entry must survive up to PERS_TTL_EXTEND",
        );
    }

    /// Issue #527 predicted `get_score` would return `None` after expiry. It
    /// panics instead, so `Option::None` cannot be used to detect archival —
    /// `None` only ever means "never computed".
    #[test]
    #[should_panic(expected = "Error(Storage, InternalError)")]
    fn test_score_expires_past_extended_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, credit, subject) = credit_with_score(&env);
        keep_instance_alive(&env, &id);
        advance(&env, PERS_TTL_EXTEND + 1);

        let _ = credit.get_score(&subject);
    }

    // -----------------------------------------------------------------
    // 4. Feeder registration (credit-oracle)
    // -----------------------------------------------------------------

    /// Registers a feeder but writes no TxStats, so `TrustedFeeder` is the only
    /// entry whose expiry these tests can observe. Sharing `credit_with_stats`
    /// here would be a silent trap: TxStats lives 4096 ledgers against
    /// TrustedFeeder's 518_400, so it archives first and every feeder assertion
    /// would really be measuring TxStats.
    fn credit_with_feeder(env: &Env) -> (Address, CreditOracleClient<'_>, Address, Address) {
        let id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(env, &id);

        let admin = Address::generate(env);
        let feeder = Address::generate(env);
        let subject = Address::generate(env);

        credit.initialize(&admin);
        credit.register_feeder(&admin, &feeder);

        (id, credit, feeder, subject)
    }

    /// `register_feeder` extends `TrustedFeeder`, so authorization holds for
    /// the full window.
    #[test]
    fn test_feeder_authorization_survives_within_extended_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, credit, feeder, subject) = credit_with_feeder(&env);
        keep_instance_alive(&env, &id);
        advance(&env, PERS_TTL_EXTEND - 1);

        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 1,
                tx_count_30d: 1,
                avg_counterparties: 1,
            },
        );
    }

    /// Past the window the feeder is not "no longer authorized" in the sense
    /// issue #527 anticipated — the call does not return `FeederNotRegistered`,
    /// it panics on the archived `TrustedFeeder` lookup. Expiry is therefore a
    /// liveness failure, not a permission change, and cannot be handled by a
    /// caller inspecting the error variant.
    #[test]
    #[should_panic(expected = "Error(Storage, InternalError)")]
    fn test_feeder_authorization_expires_past_extended_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, credit, feeder, subject) = credit_with_feeder(&env);
        keep_instance_alive(&env, &id);
        advance(&env, PERS_TTL_EXTEND + 1);

        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 1,
                tx_count_30d: 1,
                avg_counterparties: 1,
            },
        );
    }

    /// Archival is not read-only: *writing* an archived key panics too, so a
    /// feeder cannot repair an expired entry by pushing fresh data over it.
    /// `update_tx_stats` here fails even though it only ever writes TxStats —
    /// the host rejects the access before the contract body runs. Recovery
    /// requires restoring the entry, which no contract function exposes.
    #[test]
    #[should_panic(expected = "Error(Storage, InternalError)")]
    fn test_writing_over_an_archived_entry_does_not_repair_it() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, credit, feeder, subject) = credit_with_stats(&env);
        keep_instance_alive(&env, &id);
        advance(&env, DEFAULT_PERSISTENT_TTL + 1);

        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 42,
                tx_count_30d: 1,
                avg_counterparties: 1,
            },
        );
    }

    // -----------------------------------------------------------------
    // 5. Instance archival (credit-oracle)
    // -----------------------------------------------------------------

    /// The most urgent finding. credit-oracle never calls
    /// `instance().extend_ttl`, so its instance entry — holding `Admin`,
    /// `Config`, and every other singleton — expires after the default 4096
    /// ledgers (~5.7 h at 5 s/ledger) and archives the whole contract. This is
    /// why every other test in this module calls `keep_instance_alive`: without
    /// it, the instance is what expires first, long before any persistent key.
    #[test]
    #[should_panic(expected = "Error(Storage, InternalError)")]
    fn test_credit_oracle_instance_archives_after_default_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &id);

        let admin = Address::generate(&env);
        let subject = Address::generate(&env);
        credit.initialize(&admin);

        // Deliberately NOT calling keep_instance_alive.
        advance(&env, DEFAULT_PERSISTENT_TTL + 1);

        let _ = credit.compute_score(&subject);
    }

    /// Counterpart to the test above: identity-oracle *does* bump its instance
    /// TTL (INSTANCE_BUMP_AMOUNT = 500_000) on admin calls, so it survives the
    /// same jump that archives credit-oracle. The two tests together isolate
    /// the difference to the missing bump rather than to the environment.
    #[test]
    fn test_identity_oracle_instance_survives_default_ttl_window() {
        let env = Env::default();
        env.mock_all_auths();

        let (_id, identity, subject) = anchor_one_vc(&env);

        // Same jump as the credit-oracle archival test, no manual pinning.
        advance(&env, DEFAULT_PERSISTENT_TTL + 1);

        assert!(
            identity.is_verified(&subject),
            "identity-oracle bumps its instance TTL, so it survives this jump",
        );
    }

    /// Guards the assumption the whole module rests on: if a future SDK ever
    /// stops enforcing archival, this test starts failing and issue #527's
    /// `#[ignore]` fallback becomes the correct response.
    #[test]
    fn test_archived_entry_access_is_enforced_not_silently_ignored() {
        let env = Env::default();
        env.mock_all_auths();

        let (id, _credit, _feeder, subject) = credit_with_stats(&env);
        keep_instance_alive(&env, &id);
        advance(&env, DEFAULT_PERSISTENT_TTL + 1);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.as_contract(&id, || {
                env.storage()
                    .persistent()
                    .has(&CreditKey::TxStats(subject.clone()))
            })
        }));

        let err = result.expect_err("testutils must enforce TTL archival");
        let msg = err
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_default();
        assert!(
            msg.contains(ARCHIVED),
            "expected an archival panic, got: {}",
            msg,
        );
    }
}
