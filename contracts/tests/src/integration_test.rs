#[cfg(test)]
mod tests {
    use credit_oracle::{CreditOracle, CreditOracleClient, TxStats};
    use identity_oracle::{IdentityOracle, IdentityOracleClient};
    use revocation_registry::{RevocationRegistry, RevocationRegistryClient};
    use soroban_sdk::{testutils::Address as _, BytesN, Env, String};

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
        identity.register_issuer(&admin, &issuer);

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
        identity.register_issuer(&admin, &issuer);

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
        assert_eq!(res, Err(Ok(revocation_registry::RevocationRegistryError::IssuerMismatch)));
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
        identity.register_issuer(&admin, &issuer);

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
        let score_with_2_vcs = credit.compute_score(&subject);

        // 17. Assert score decreased due to fewer active VCs
        assert!(
            score_with_2_vcs < score_with_5_vcs,
            "Score with 2 VCs ({}) should be less than score with 5 VCs ({})",
            score_with_2_vcs,
            score_with_5_vcs
        );
    }
}
