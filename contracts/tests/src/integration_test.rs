#[cfg(test)]
mod tests {
    use credit_oracle::{CreditOracle, CreditOracleClient, TxStats};
    use identity_oracle::{IdentityOracle, IdentityOracleClient};
    use revocation_registry::{RevocationRegistry, RevocationRegistryClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        BytesN, Env, String, Symbol,
    };

    #[test]
    fn test_full_protocol_flow() {
        // 1. Create Env with mock_all_auths
        let env = Env::default();
        env.mock_all_auths();

        // 2. Register and initialize all 3 contracts
        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);
        let _revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);
        let revocation = RevocationRegistryClient::new(&env, &_revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);
        revocation.initialize(&admin);

        // 3. Register an issuer in identity-oracle
        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        // 4. Call anchor_did for a test subject
        let subject = soroban_sdk::Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDID");
        identity.anchor_did(&subject, &cid);

        // 5. Call anchor_vc for the subject with a test hash
        let vc_hash = BytesN::from_array(&env, &[42u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash, &None);

        // 6. Assert is_verified returns true
        assert!(identity.is_verified(&subject));

        // 7. Register a lender and feeder in credit-oracle
        let lender = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_lender(&lender);
        credit.register_feeder(&feeder);

        // 8. Call set_vc_count(subject, 1)
        credit.set_vc_count(&feeder, &subject, &1);

        // 9. Call update_tx_stats with volume_30d = 500_000_000 stroops
        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 500_000_000i128,
                tx_count_30d: 10,
                avg_counterparties: 3,
            },
        );

        // 10. Call record_repayment 5 times on_time=true
        for _ in 0..5 {
            credit.record_repayment(&lender, &subject, &100_000_000i128, &true);
        }

        // 11. Call compute_score
        let score = credit.compute_score(&subject);

        // 12. Assert score > 300
        assert!(score > 300, "expected score > 300, got {}", score);

        // 13. Assert score <= 850
        assert!(score <= 850, "expected score <= 850, got {}", score);
    }

    #[test]
    fn test_cross_contract_vc_count() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        let subject = soroban_sdk::Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDID");
        identity.anchor_did(&subject, &cid);

        let vc_hash = BytesN::from_array(&env, &[7u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash, &None);

        // Configure credit-oracle to call identity-oracle directly
        credit.set_identity_oracle(&identity_id);

        // Do not set cached VcCount; compute_score should read identity-oracle
        let score_live = credit.compute_score(&subject);
        assert!(
            score_live > 300,
            "expected live score > 300, got {}",
            score_live
        );

        // Now set the cached value to 0 to ensure the cross-contract path is used
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_feeder(&feeder);
        credit.set_vc_count(&feeder, &subject, &0);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_after_cached_zero = credit.compute_score(&subject);
        assert_eq!(
            score_live, score_after_cached_zero,
            "expected compute_score to prefer identity-oracle over cached VcCount"
        );
    }

    #[test]
    fn test_revoked_vc_lowers_score() {
        let env = Env::default();
        env.mock_all_auths();

        // Setup: register and initialize all 3 contracts
        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);
        let _revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);
        _revocation.initialize(&admin);

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        let subject = soroban_sdk::Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDID");
        identity.anchor_did(&subject, &cid);

        let vc_hash = BytesN::from_array(&env, &[99u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash, &None);

        let lender = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_lender(&lender);
        credit.register_feeder(&feeder);

        // 1. Get initial score with vc_count = 1
        credit.set_vc_count(&feeder, &subject, &1);
        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 500_000_000i128,
                tx_count_30d: 10,
                avg_counterparties: 3,
            },
        );
        for _ in 0..5 {
            credit.record_repayment(&lender, &subject, &100_000_000i128, &true);
        }
        let initial_score = credit.compute_score(&subject);
        assert!(initial_score > 300);

        // 2. Revoke the VC on identity-oracle
        identity.mark_vc_revoked(&issuer, &subject, &vc_hash);

        // 3. Assert is_verified returns false
        assert!(!identity.is_verified(&subject));

        // 4. Update vc_count to 0 and recompute score
        credit.set_vc_count(&feeder, &subject, &0);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let new_score = credit.compute_score(&subject);

        // 5. Assert new score < initial score
        assert!(
            new_score < initial_score,
            "expected new_score ({}) < initial_score ({})",
            new_score,
            initial_score
        );
    }

    #[test]
    fn test_revocation_registry_identity_oracle_integration() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        revocation.initialize(&admin);

        // Link identity-oracle to revocation-registry
        identity.set_revocation_registry(&revocation_id);

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        let subject = soroban_sdk::Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[123u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash, &None);

        // Assert verified initially
        assert!(identity.is_verified(&subject));

        // Revoke via revocation-registry
        revocation.revoke(&issuer, &vc_hash);

        // Verify that is_revoked returns true on the registry
        assert!(revocation.is_revoked(&vc_hash));

        // Verify that identity-oracle verify_vc returns false
        assert!(!identity.verify_vc(&subject, &vc_hash));

        // Also verify that is_verified and get_active_vc_count correctly reflect the revocation
        assert!(!identity.is_verified(&subject));
        assert_eq!(identity.get_active_vc_count(&subject), 0);
    }

    #[test]
    fn test_only_registered_issuer_can_revoke_vc_hash_integration() {
        let env = Env::default();
        env.mock_all_auths();

        // Setup: register and initialize all 3 contracts
        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let _identity = IdentityOracleClient::new(&env, &identity_id);
        let _credit = CreditOracleClient::new(&env, &credit_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        revocation.initialize(&admin);

        // Two different issuers
        let issuer_a = soroban_sdk::Address::generate(&env);
        let issuer_b = soroban_sdk::Address::generate(&env);

        // A VC hash that issuer_b should not be able to revoke after issuer_a registered it
        let vc_hash = BytesN::from_array(&env, &[7u8; 32]);

        // First revoke by issuer_a registers the authority.
        revocation.revoke(&issuer_a, &vc_hash);
        assert!(revocation.is_revoked(&vc_hash));

        // Second revoke by issuer_b must fail.
        let res = revocation.try_revoke(&issuer_b, &vc_hash);
        assert_eq!(
            res,
            Err(Ok(
                revocation_registry::RevocationRegistryError::IssuerMismatch
            ))
        );
    }

    #[test]
    fn test_batch_revoke_integration() {
        let env = Env::default();
        env.mock_all_auths();

        // 1. Register and initialize all 3 contracts
        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);
        revocation.initialize(&admin);

        // 2. Register issuer
        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        // 3. Create subject and DID
        let subject = soroban_sdk::Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmBatchTestDID");
        identity.anchor_did(&subject, &cid);

        // 4. Anchor 5 VCs for the subject
        let mut vc_hashes = soroban_sdk::Vec::new(&env);
        for i in 0..5u8 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = i;
            let vc_hash = BytesN::from_array(&env, &hash_arr);
            identity.anchor_vc(&issuer, &subject, &vc_hash, &None);
            vc_hashes.push_back(vc_hash);
        }

        // 5. Assert is_verified is true (5 active VCs)
        assert!(identity.is_verified(&subject));

        // 6. Assert get_vc_count returns 5
        assert_eq!(identity.get_vc_count(&subject), 5);

        // 7. Create a vector of the first 3 hashes to batch revoke
        let mut batch_revoke_hashes = soroban_sdk::Vec::new(&env);
        for i in 0..3usize {
            batch_revoke_hashes.push_back(vc_hashes.get(i as u32).unwrap());
        }

        // 8. Batch revoke the 3 VCs on revocation-registry
        revocation.batch_revoke(&issuer, &batch_revoke_hashes);

        // 9. Assert is_revoked returns true for each of the 3 revoked hashes
        for i in 0..3usize {
            let revoked_hash = vc_hashes.get(i as u32).unwrap();
            assert!(
                revocation.is_revoked(&revoked_hash),
                "VC hash {} should be revoked",
                i
            );
        }

        // 10. Assert is_revoked returns false for the 2 non-revoked hashes
        for i in 3..5usize {
            let active_hash = vc_hashes.get(i as u32).unwrap();
            assert!(
                !revocation.is_revoked(&active_hash),
                "VC hash {} should not be revoked",
                i
            );
        }

        // 11. Mark the 3 VCs as revoked on identity-oracle
        for i in 0..3usize {
            let revoked_hash = vc_hashes.get(i as u32).unwrap();
            identity.mark_vc_revoked(&issuer, &subject, &revoked_hash);
        }

        // 12. Assert is_verified is still true (2 active VCs remain)
        assert!(
            identity.is_verified(&subject),
            "Subject should still be verified with 2 active VCs"
        );

        // 13. Assert get_vc_count returns 5 (total count unchanged)
        assert_eq!(
            identity.get_vc_count(&subject),
            5,
            "Total VC count should remain 5"
        );

        // 14. Setup credit-oracle to test score changes
        let lender = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_lender(&lender);
        credit.register_feeder(&feeder);

        // 15. Set initial VC count to 5 and compute score
        credit.set_vc_count(&feeder, &subject, &5);
        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 500_000_000i128,
                tx_count_30d: 10,
                avg_counterparties: 3,
            },
        );
        for _ in 0..5 {
            credit.record_repayment(&lender, &subject, &100_000_000i128, &true);
        }
        let score_with_5_vcs = credit.compute_score(&subject);

        // 16. Update VC count to 2 (after batch revocation) and recompute score
        credit.set_vc_count(&feeder, &subject, &2);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_with_2_vcs = credit.compute_score(&subject);

        // 17. Assert score decreased due to fewer active VCs
        assert!(
            score_with_2_vcs < score_with_5_vcs,
            "Score with 2 VCs ({}) should be less than score with 5 VCs ({})",
            score_with_2_vcs,
            score_with_5_vcs
        );
    }

    #[test]
    fn test_cross_contract_score_not_inflated_after_revocation() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        let subject = soroban_sdk::Address::generate(&env);

        // Anchor 3 VCs for the subject
        let vc_hashes: [BytesN<32>; 3] = [
            BytesN::from_array(&env, &[1u8; 32]),
            BytesN::from_array(&env, &[2u8; 32]),
            BytesN::from_array(&env, &[3u8; 32]),
        ];
        for vc_hash in &vc_hashes {
            identity.anchor_vc(&issuer, &subject, vc_hash, &None);
        }

        // Configure credit-oracle to use cross-contract VC count lookup
        credit.set_identity_oracle(&identity_id);

        // Compute initial score (3 active VCs)
        let initial_score = credit.compute_score(&subject);
        assert!(
            initial_score > 300,
            "expected initial score > 300, got {}",
            initial_score
        );

        // Revoke 2 of the 3 VCs
        identity.mark_vc_revoked(&issuer, &subject, &vc_hashes[0]);
        identity.mark_vc_revoked(&issuer, &subject, &vc_hashes[1]);

        // Advance ledger to allow recomputation
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);

        // Compute new score (1 active VC)
        let score_after_revocation = credit.compute_score(&subject);

        // Verify score is lower after revocation (cross-contract path uses
        // get_active_vc_type_counts as of #163)
        assert!(
            score_after_revocation < initial_score,
            "expected score after revocation ({}) < initial score ({}) when using cross-contract lookup",
            score_after_revocation,
            initial_score
        );

        // Also verify get_active_vc_count returns correct count
        assert_eq!(identity.get_active_vc_count(&subject), 1);
        assert_eq!(identity.get_total_vc_count(&subject), 3);
    }

    /// Issue #166: every persistent write in all three contracts must call
    /// `extend_ttl`, otherwise the entry can be archived and reads silently
    /// fall back to `None`/default as if the data had never existed.
    ///
    /// This test writes one representative entry per contract, jumps the
    /// ledger sequence forward by an amount well past what a *default*
    /// (un-extended) persistent write would survive, then confirms every
    /// entry is still readable with its original value. Without the
    /// `extend_ttl` calls added for #166, this test fails because the
    /// entries would be archived and reads would return their "not found"
    /// defaults instead of the values asserted below.
    ///
    /// The jump (500,000 ledgers, ~29 days at 5s/ledger) is chosen to sit
    /// comfortably above `PERSISTENT_BUMP_THRESHOLD` (~7 days) in all three
    /// contracts, while staying well under `PERSISTENT_BUMP_AMOUNT`
    /// (~365 days) so the test isn't sensitive to the exact outer bound.
    #[test]
    fn test_persistent_data_survives_ledger_advance() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);
        revocation.initialize(&admin);

        // --- identity-oracle: issuer registration, DID document, VC anchor ---
        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        let subject = soroban_sdk::Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTTLSurvivalTest");
        identity.anchor_did(&subject, &cid);

        let vc_hash = BytesN::from_array(&env, &[7u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash, &None);

        // --- revocation-registry: a revoked hash ---
        let revoked_hash = BytesN::from_array(&env, &[9u8; 32]);
        revocation.revoke(&issuer, &revoked_hash);

        // --- credit-oracle: feeder registration + cached tx stats ---
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_feeder(&feeder);
        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 123_000_000i128,
                tx_count_30d: 4,
                avg_counterparties: 2,
            },
        );

        // Advance the ledger sequence, simulating the passage of time
        // between writes and a later read (e.g. a lender querying an
        // identity that hasn't been touched in weeks).
        let jump: u32 = 500_000;
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + jump);

        // --- Assertions: everything must still be readable and correct ---
        assert!(
            identity.is_verified(&subject),
            "VC anchor for subject was archived despite extend_ttl"
        );
        assert_eq!(
            identity.get_active_vc_count(&subject),
            1,
            "VC anchors list for subject was archived or lost despite extend_ttl"
        );

        assert!(
            revocation.is_revoked(&revoked_hash),
            "revocation status was archived despite extend_ttl \
             (a revoked credential must never silently appear valid again)"
        );

        // register_feeder + update_tx_stats both require reading the
        // TrustedFeeder entry, so calling update_tx_stats again here also
        // proves TrustedFeeder itself survived the jump — if it had been
        // archived, this call would fail with FeederNotRegistered.
        let result = credit.try_update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 456_000_000i128,
                tx_count_30d: 8,
                avg_counterparties: 5,
            },
        );
        assert!(
            result.is_ok(),
            "TrustedFeeder registration was archived despite extend_ttl"
        );
    }

    /// Issue #163: a credential type with a registered weight above 100
    /// must raise the score relative to the same VC counted as untyped
    /// (or as a type at the default weight), and a weight of 0 must
    /// exclude that type from scoring entirely.
    #[test]
    fn test_credential_type_weight_raises_score() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);
        credit.set_identity_oracle(&identity_id);

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        // --- Baseline subject: one untyped VC (pre-#163 behavior) ---
        let baseline_subject = soroban_sdk::Address::generate(&env);
        let baseline_cid = String::from_str(&env, "ipfs://QmBaselineDID");
        identity.anchor_did(&baseline_subject, &baseline_cid);
        let baseline_hash = BytesN::from_array(&env, &[10u8; 32]);
        identity.anchor_vc(&issuer, &baseline_subject, &baseline_hash, &None);
        let baseline_score = credit.compute_score(&baseline_subject);

        // --- Weighted subject: one "kyc"-typed VC, same otherwise ---
        let weighted_subject = soroban_sdk::Address::generate(&env);
        let weighted_cid = String::from_str(&env, "ipfs://QmWeightedDID");
        identity.anchor_did(&weighted_subject, &weighted_cid);
        let weighted_hash = BytesN::from_array(&env, &[11u8; 32]);
        let kyc_type = Symbol::new(&env, "kyc");
        identity.anchor_vc(
            &issuer,
            &weighted_subject,
            &weighted_hash,
            &Some(kyc_type.clone()),
        );

        // Before any weight is registered for "kyc", it defaults to 100
        // (1x) — identical to the untyped baseline.
        let score_before_weight = credit.compute_score(&weighted_subject);
        assert_eq!(
            score_before_weight, baseline_score,
            "an unregistered credential type must score identically to untyped (default weight = 100)"
        );

        // Register "kyc" at 2x weight and recompute (past the cooldown).
        credit.set_credential_type_weight(&kyc_type, &200);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_after_weight = credit.compute_score(&weighted_subject);
        assert!(
            score_after_weight > baseline_score,
            "expected 2x-weighted kyc VC score ({}) > untyped baseline score ({})",
            score_after_weight,
            baseline_score
        );

        // A weight of 0 must exclude the type entirely, dropping the
        // subject back to the base score with no active VC contribution.
        credit.set_credential_type_weight(&kyc_type, &0);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_zero_weight = credit.compute_score(&weighted_subject);
        assert_eq!(
            score_zero_weight, 300,
            "a credential type weighted to 0 must contribute nothing to the score"
        );
    }

    /// Issue #163: `get_credential_type_weight` must default to 100 for any
    /// type that was never registered, and reflect whatever was last set
    /// via `set_credential_type_weight` otherwise.
    #[test]
    fn test_credential_type_weight_registry_defaults_and_updates() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);

        let income_type = Symbol::new(&env, "income");
        assert_eq!(credit.get_credential_type_weight(&income_type), 100);

        credit.set_credential_type_weight(&income_type, &150);
        assert_eq!(credit.get_credential_type_weight(&income_type), 150);
    }
}
