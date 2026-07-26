#[cfg(test)]
mod tests {
    use credit_oracle::{CreditOracle, CreditOracleClient, ScoringWeights, TxStats};
    use governance::{Governance, GovernanceClient, GovernanceError};
    use identity_oracle::{IdentityOracle, IdentityOracleClient};
    use revocation_registry::{RevocationRegistry, RevocationRegistryClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        BytesN, Env, String,
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
        identity.anchor_vc(&issuer, &subject, &vc_hash);

        // 6. Assert is_verified returns true
        assert!(identity.is_verified(&subject));

        // 7. Register a lender and feeder in credit-oracle
        let lender = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_lender(&admin, &lender);
        credit.register_feeder(&admin, &feeder);

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
        identity.anchor_vc(&issuer, &subject, &vc_hash);

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
        credit.register_feeder(&admin, &feeder);
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
        identity.anchor_vc(&issuer, &subject, &vc_hash);

        let lender = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_lender(&admin, &lender);
        credit.register_feeder(&admin, &feeder);

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
        identity.anchor_vc(&issuer, &subject, &vc_hash);

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
            identity.anchor_vc(&issuer, &subject, &vc_hash);
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
        credit.register_lender(&admin, &lender);
        credit.register_feeder(&admin, &feeder);

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
            identity.anchor_vc(&issuer, &subject, vc_hash);
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

        // Verify score is lower after revocation (cross-contract path uses get_active_vc_count)
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

    #[test]
    fn test_batch_revoke_mixed_hashes_atomicity() {
        let env = Env::default();
        env.mock_all_auths();

        let revocation_id = env.register_contract(None, RevocationRegistry);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        revocation.initialize(&admin);

        let issuer1 = soroban_sdk::Address::generate(&env);
        let issuer2 = soroban_sdk::Address::generate(&env);

        let hash1 = BytesN::from_array(&env, &[1u8; 32]);
        let hash2 = BytesN::from_array(&env, &[2u8; 32]); // This will belong to issuer2
        let hash3 = BytesN::from_array(&env, &[3u8; 32]);

        // issuer2 revokes hash2 individually to claim authority
        revocation.revoke(&issuer2, &hash2);
        assert!(revocation.is_revoked(&hash2));

        // Create a batch with mixed hashes
        let mut batch = soroban_sdk::Vec::new(&env);
        batch.push_back(hash1.clone());
        batch.push_back(hash2.clone()); // belongs to issuer2
        batch.push_back(hash3.clone());

        // issuer1 attempts to batch revoke the hashes
        let res = revocation.try_batch_revoke(&issuer1, &batch);
        
        // Assert the call failed with IssuerMismatch
        assert_eq!(
            res,
            Err(Ok(revocation_registry::RevocationRegistryError::IssuerMismatch))
        );

        // Verify that hash1 and hash3 were NOT revoked (atomicity check)
        assert!(!revocation.is_revoked(&hash1));
        assert!(!revocation.is_revoked(&hash3));
    }

    /// Integration test: issuers with high revocation rates (and therefore
    /// lower reputation tiers, either applied manually by governance or via
    /// the metrics-based recommendation) produce lower subject scores when
    /// the credit oracle has issuer-tier weighting enabled.
    ///
    /// Scenario:
    ///   * Issuer G (Good): issues 4 VCs to SubjectG, revokes none.
    ///     → vcs_issued=4, vcs_revoked=0 → recommended Tier3 (gold).
    ///   * Issuer B (Bad):  issues 4 VCs to SubjectB (all active), PLUS
    ///     issues 4 additional VCs to a decoy SubjectX and revokes all 4.
    ///     → vcs_issued=8, vcs_revoked=4 (50% revocation ratio)
    ///     → recommended Tier0 (suspended / 0 weight).
    ///
    /// After applying the recommended tiers through governance and turning
    /// on UseIssuerTierWeighting in the credit oracle, SubjectG has
    /// 4 effective VCs (Tier3 × 4) while SubjectB has 0 effective VCs
    /// (Tier0 × 4) even though both subjects hold 4 active VCs on paper.
    /// The resulting credit scores reflect this difference.
    #[test]
    fn test_high_revocation_issuer_produces_lower_scores() {
        use governance::Governance;
        use identity_oracle::IssuerTier;

        let env = Env::default();
        env.mock_all_auths();

        // 1. Deploy and initialize the three contracts
        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);
        let gov_id = env.register_contract(None, Governance);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);
        let gov = GovernanceClient::new(&env, &gov_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);
        gov.initialize(&admin, &credit_id, &1000);

        // Link governance → identity-oracle for tier adjustments
        gov.set_identity_oracle(&admin, &identity_id);
        assert_eq!(gov.get_identity_oracle(), Some(identity_id.clone()));

        // Link credit-oracle → identity-oracle for live VC lookups
        credit.set_identity_oracle(&identity_id);

        // Register feeder + lender for the other scoring components so
        // the score delta is driven purely by the VC tier weighting.
        let feeder = soroban_sdk::Address::generate(&env);
        let lender = soroban_sdk::Address::generate(&env);
        credit.register_feeder(&feeder);
        credit.register_lender(&lender);

        // 2. Register Good issuer G and Bad issuer B
        let issuer_g = soroban_sdk::Address::generate(&env);
        let issuer_b = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer_g);
        identity.register_issuer(&issuer_b);

        // 3. Build reputation profiles
        //    Issuer G: 4 VCs issued to SubjectG, 0 revoked
        //    Issuer B: 4 VCs issued to SubjectB (active),
        //              + 4 VCs issued to SubjectX → all 4 revoked
        //              → 8 issued, 4 revoked (50% ratio)
        let subject_g = soroban_sdk::Address::generate(&env);
        let subject_b = soroban_sdk::Address::generate(&env);
        let subject_x = soroban_sdk::Address::generate(&env);

        for i in 0..4u8 {
            let mut h = [0u8; 32];
            h[0] = i;
            identity.anchor_vc(&issuer_g, &subject_g, &BytesN::from_array(&env, &h));
        }

        for i in 0..4u8 {
            let mut h = [10u8; 32];
            h[0] = 10 + i;
            identity.anchor_vc(&issuer_b, &subject_b, &BytesN::from_array(&env, &h));
        }

        for i in 0..4u8 {
            let mut h = [20u8; 32];
            h[0] = 20 + i;
            let hash = BytesN::from_array(&env, &h);
            identity.anchor_vc(&issuer_b, &subject_x, &hash);
            identity.mark_vc_revoked(&issuer_b, &subject_x, &hash);
        }

        // 4. Verify metrics tracked correctly on identity-oracle
        let pg = identity.get_issuer_profile(&issuer_g);
        let pb = identity.get_issuer_profile(&issuer_b);
        assert_eq!(pg.vcs_issued, 4);
        assert_eq!(pg.vcs_revoked, 0);
        assert_eq!(pb.vcs_issued, 8);
        assert_eq!(pb.vcs_revoked, 4);

        // 5. Governance's pure recommendation rule produces the expected tiers
        assert_eq!(
            Governance::recommend_tier_from_metrics(pg.vcs_issued, pg.vcs_revoked),
            IssuerTier::Tier3
        );
        // 50% revocation ratio → Tier0 recommendation
        assert_eq!(
            Governance::recommend_tier_from_metrics(pb.vcs_issued, pb.vcs_revoked),
            IssuerTier::Tier0
        );

        // 6. Governance applies both recommended tiers via adjust_issuer_tier
        gov.adjust_issuer_tier(&admin, &issuer_g, &IssuerTier::Tier3);
        gov.adjust_issuer_tier(&admin, &issuer_b, &IssuerTier::Tier0);

        assert_eq!(
            identity.get_issuer_profile(&issuer_g).tier,
            IssuerTier::Tier3
        );
        assert_eq!(
            identity.get_issuer_profile(&issuer_b).tier,
            IssuerTier::Tier0
        );

        // 7. Raw active VC counts are equal (no tier weighting applied yet)
        assert_eq!(identity.get_active_vc_count(&subject_g), 4);
        assert_eq!(identity.get_active_vc_count(&subject_b), 4);

        // 8. Weighted VC counts diverge dramatically
        //    subject_g: 4 × Tier3 (1.00) = 400 hundredths → 4 effective
        //    subject_b: 4 × Tier0 (0.00) =   0 hundredths → 0 effective
        assert_eq!(identity.get_weighted_vc_count(&subject_g), 400);
        assert_eq!(identity.get_weighted_vc_count(&subject_b), 0);

        // 9. Give both subjects IDENTICAL tx_stats + repayment records so
        //    any score delta comes exclusively from issuer-tier weighting.
        let identical_tx = TxStats {
            volume_30d: 2_000_000_000i128, // = 20 tx_score points
            tx_count_30d: 10,
            avg_counterparties: 0,
        };
        credit.update_tx_stats(&feeder, &subject_g, &identical_tx);
        credit.update_tx_stats(&feeder, &subject_b, &identical_tx);
        for _ in 0..6 {
            credit.record_repayment(&lender, &subject_g, &100_000_000i128, &true);
            credit.record_repayment(&lender, &subject_b, &100_000_000i128, &true);
        }

        // 10. WITHOUT weighting enabled → both scores are EQUAL
        let score_g_noweight = credit.compute_score(&subject_g);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_b_noweight = credit.compute_score(&subject_b);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        assert_eq!(
            score_g_noweight, score_b_noweight,
            "without tier weighting both subjects should score the same"
        );

        // 11. Enable issuer-tier weighting
        credit.set_issuer_tier_weighting(&true);
        assert!(credit.get_issuer_tier_weighting());

        // 12. WITH weighting enabled → subject_g outscores subject_b
        let score_g_weighted = credit.compute_score(&subject_g);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_b_weighted = credit.compute_score(&subject_b);

        assert!(
            score_g_weighted > score_b_weighted,
            "with tier weighting, subject with good-issuer VCs (score {}) \
             should outscore subject with bad-issuer VCs (score {})",
            score_g_weighted,
            score_b_weighted
        );

        // subject_b should also score WORSE after weighting is turned on,
        // relative to its own no-weight score.
        assert!(
            score_b_weighted < score_b_noweight,
            "bad issuer's subject should score lower after weighting: \
             before={}, after={}",
            score_b_noweight,
            score_b_weighted
        );
    }

    /// Integration test for governance execution timelock:
    /// vote passes → advance past voting → execution rejected (timelock) →
    /// advance past delay → execution succeeds.
    #[test]
    fn test_governance_execution_timelock_integration() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let gov_id = env.register_contract(None, Governance);

        let credit = CreditOracleClient::new(&env, &credit_id);
        let gov = GovernanceClient::new(&env, &gov_id);

        let admin = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);
        gov.initialize(&admin, &credit_id, &100);

        // Transfer oracle admin to governance contract
        credit.propose_new_admin(&gov_id);
        gov.accept_oracle_admin();

        let proposed_weights = ScoringWeights {
            vc_weight: 50,
            tx_weight: 20,
            repayment_weight: 30,
        };

        let proposer = soroban_sdk::Address::generate(&env);
        // voting_period = 100 ledgers, execution_delay = 50 ledgers
        let proposal_id = gov.create_proposal(&proposer, &proposed_weights, &100, &50);

        // Cast passing votes
        let voter = soroban_sdk::Address::generate(&env);
        gov.vote(&voter, &proposal_id, &true, &200);

        // Step 1: advance just past voting period (expiry_ledger + 1)
        // but still within the execution timelock window
        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });

        // Execution must fail — timelock not yet expired
        let res = gov.try_execute(&proposal_id);
        assert_eq!(
            res,
            Err(Ok(GovernanceError::TimelockNotExpired)),
            "expected TimelockNotExpired while within execution delay window"
        );

        let proposal = gov.get_proposal(&proposal_id).unwrap();
        assert!(!proposal.executed, "proposal must not be executed yet");

        // Step 2: advance past the execution timelock (50 more ledgers)
        env.ledger().with_mut(|l| {
            l.sequence_number += 50;
        });

        // Execution must now succeed
        gov.execute(&proposal_id);

        let proposal = gov.get_proposal(&proposal_id).unwrap();
        assert!(proposal.executed, "proposal must be executed after timelock");

        // Verify weights were applied to the credit oracle
        let weights = credit.get_scoring_weights();
        assert_eq!(weights.vc_weight, 50);
        assert_eq!(weights.tx_weight, 20);
        assert_eq!(weights.repayment_weight, 30);
    }
}
