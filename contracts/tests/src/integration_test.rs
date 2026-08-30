#[cfg(test)]
mod tests {
    use soroban_sdk::{
        contract, contractimpl, symbol_short,
        testutils::{Address as _, Events, Ledger as _},
        Address, BytesN, Env, String, Symbol, TryIntoVal,
    };
    use credit_oracle::{
        CreditOracle, CreditOracleClient, CreditOracleError, DataKey, DisputeStatus,
        RepaymentRecord, RepaymentRecordV1, ScoringWeights, TxStats,
    };
    use governance::{Governance, GovernanceClient, GovernanceError};
    use identity_oracle::{IdentityOracle, IdentityOracleClient, IdentityOracleError};
    use revocation_registry::{RevocationRegistry, RevocationRegistryClient};

    #[test]
    fn test_score_freshness_enforcement() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = Address::generate(&env);

        // Initialize identity-oracle and verify Init event
        identity.initialize(&admin);
        let events = env.events().all();
        let id_events: Vec<_> = events
            .iter()
            .filter(|(id, _, _)| *id == identity_id)
            .collect();
        assert_eq!(id_events.len(), 1, "identity-oracle should emit 1 event");
        let (_, topics, data) = &id_events[0];
        assert_eq!(topics.len(), 1);
        let topic0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(topic0, symbol_short!("Init"));
        let event_admin: soroban_sdk::Address = data.clone().try_into_val(&env).unwrap();
        assert_eq!(
            event_admin, admin,
            "Initialized event admin mismatch for identity-oracle"
        );

        // Initialize credit-oracle and verify Initialized event
        credit.initialize(&admin);
        let events = env.events().all();
        let credit_events: Vec<_> = events
            .iter()
            .filter(|(id, _, _)| *id == credit_id)
            .collect();
        assert_eq!(credit_events.len(), 1, "credit-oracle should emit 1 event");
        let (_, topics, data) = &credit_events[0];
        assert_eq!(topics.len(), 1);
        let topic1: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(topic1, symbol_short!("Init"));
        let event_admin: soroban_sdk::Address = data.clone().try_into_val(&env).unwrap();
        assert_eq!(
            event_admin, admin,
            "Initialized event admin mismatch for credit-oracle"
        );

        // Initialize revocation-registry and verify Initialized event
        revocation.initialize(&admin);
        let events = env.events().all();
        let rev_events: Vec<_> = events
            .iter()
            .filter(|(id, _, _)| *id == revocation_id)
            .collect();
        assert_eq!(
            rev_events.len(),
            1,
            "revocation-registry should emit 1 event"
        );
        let (_, topics, data) = &rev_events[0];
        assert_eq!(topics.len(), 1);
        let topic2: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(topic2, symbol_short!("Init"));
        let event_admin: soroban_sdk::Address = data.clone().try_into_val(&env).unwrap();
        assert_eq!(
            event_admin, admin,
            "Init event admin mismatch for revocation-registry"
        );

        // Issue #302: governance contract must also emit an Initialized event
        // with the admin and credit-oracle target addresses so off-chain
        // indexers can detect deployments before the first admin action.
        let gov_id = env.register_contract(None, Governance);
        let gov = GovernanceClient::new(&env, &gov_id);
        gov.initialize(&admin, &credit_id, &100i128);

        let events = env.events().all();
        let gov_events: Vec<_> = events.iter().filter(|(id, _, _)| *id == gov_id).collect();
        assert_eq!(
            gov_events.len(),
            1,
            "governance contract should emit exactly 1 event on initialize"
        );
        let (_, topics, data) = &gov_events[0];
        assert_eq!(
            topics.len(),
            1,
            "topic count mismatch for governance Initialized"
        );
        let gov_topic: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(
            gov_topic,
            Symbol::new(&env, "Initialized"),
            "governance Initialized event topic mismatch"
        );

        let event_payload: (soroban_sdk::Address, soroban_sdk::Address) =
            data.clone().try_into_val(&env).unwrap();
        assert_eq!(
            event_payload.0, admin,
            "governance Initialized event admin mismatch"
        );
        assert_eq!(
            event_payload.1, credit_id,
            "governance Initialized event credit_oracle mismatch"
        );
    }

    #[test]
    fn test_record_repayment_accumulates_total_repaid() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let lender = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);

        credit.initialize(&admin);
        credit.register_lender(&admin, &lender);

        credit.record_repayment(&lender, &subject, &100_000_000i128, &true);
        credit.record_repayment(&lender, &subject, &250_000_000i128, &false);

        let record: RepaymentRecord = env.as_contract(&credit_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject.clone()))
                .unwrap()
        });

        assert_eq!(record.on_time_count, 1);
        assert_eq!(record.total_count, 2);
        assert_eq!(record.total_repaid, 350_000_000);
    }

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

        let retrieved_cid = identity
            .get_did_document(&subject)
            .expect("DID doc should exist");
        assert_eq!(retrieved_cid, cid);

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
        credit.set_identity_oracle(&admin, &identity_id);

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
        assert!(
            initial_score > 300,
            "expected initial_score > 300, got {}",
            initial_score
        );

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
    fn test_set_identity_oracle_rejects_invalid_contract() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let bad_address = soroban_sdk::Address::generate(&env);

        identity.initialize(&admin);
        credit.initialize(&admin);

        let result = credit.try_set_identity_oracle(&admin, &bad_address);
        assert_eq!(result, Err(Ok(CreditOracleError::InvalidIdentityOracle)));
    }

    #[test]
    fn test_set_revocation_registry_rejects_invalid_contract() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        let bad_address = soroban_sdk::Address::generate(&env);

        identity.initialize(&admin);
        revocation.initialize(&admin);

        let result = identity.try_set_revocation_registry(&bad_address);
        assert_eq!(
            result,
            Err(Ok(IdentityOracleError::InvalidRevocationRegistry))
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
        revocation.set_identity_oracle(&identity_id);

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        let subject = soroban_sdk::Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[123u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash);

        // Assert verified initially
        assert!(identity.is_verified(&subject));

        // Revoke via revocation-registry
        revocation.revoke(&issuer, &subject, &vc_hash);

        // Verify that is_revoked returns true on the registry
        assert!(revocation.is_revoked(&vc_hash));

        // Verify that identity-oracle verify_vc returns false
        assert!(!identity.verify_vc(&subject, &vc_hash));

        // Also verify that is_verified and get_active_vc_count correctly reflect the revocation
        assert!(!identity.is_verified(&subject));
        assert_eq!(identity.get_active_vc_count(&subject), 0);
    }

    #[test]
    fn test_get_active_vc_count_uses_cache_with_registry_configured() {
        // Acceptance criteria for #481:
        // configure revocation registry, anchor 3 VCs, revoke 1 via registry,
        // assert get_active_vc_count returns 2.
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        revocation.initialize(&admin);

        identity.set_revocation_registry(&revocation_id);
        revocation.set_identity_oracle(&identity_id);

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        let subject = soroban_sdk::Address::generate(&env);
        let hash_a = BytesN::from_array(&env, &[0u8; 32]);
        let hash_b = BytesN::from_array(&env, &[1u8; 32]);
        let hash_c = BytesN::from_array(&env, &[2u8; 32]);
        identity.anchor_vc(&issuer, &subject, &hash_a);
        identity.anchor_vc(&issuer, &subject, &hash_b);
        identity.anchor_vc(&issuer, &subject, &hash_c);

        assert_eq!(identity.get_active_vc_count(&subject), 3);

        // Revoke exactly one of the VCs via the revocation registry. This flows
        // through mark_vc_revoked, which must decrement the ActiveVCCount cache.
        revocation.revoke(&issuer, &subject, &hash_a);

        assert_eq!(identity.get_active_vc_count(&subject), 2);

        // The cache must remain authoritative even when the registry stays linked.
        revocation.revoke(&issuer, &subject, &hash_b);
        assert_eq!(identity.get_active_vc_count(&subject), 1);
    }

    #[test]
    fn test_revoke_unknown_vc_rolls_back_registry_state() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        revocation.initialize(&admin);
        revocation.set_identity_oracle(&identity_id);

        let issuer = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);
        let unknown_hash = BytesN::from_array(&env, &[124u8; 32]);

        assert!(!revocation.is_revoked(&unknown_hash));
        assert!(
            revocation
                .try_revoke(&issuer, &subject, &unknown_hash)
                .is_err(),
            "identity-oracle must reject an unknown VC"
        );

        assert!(
            !revocation.is_revoked(&unknown_hash),
            "the failed identity-oracle callback must roll back registry state"
        );
        assert_eq!(revocation.get_revocation_count(&issuer), 0);
        assert_eq!(revocation.list_revoked(&issuer, &0, &10).len(), 0);
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
        let subject = soroban_sdk::Address::generate(&env);

        // A VC hash that issuer_b should not be able to revoke after issuer_a registered it
        let vc_hash = BytesN::from_array(&env, &[7u8; 32]);

        // First revoke by issuer_a registers the authority.
        revocation.revoke(&issuer_a, &subject, &vc_hash);
        assert!(revocation.is_revoked(&vc_hash));

        // Second revoke by issuer_b must fail.
        let res = revocation.try_revoke(&issuer_b, &subject, &vc_hash);
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

        // 6. Assert get_total_vc_count returns 5
        assert_eq!(identity.get_total_vc_count(&subject), 5);

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

        // 13. Assert get_total_vc_count returns 5 (total count unchanged after revocation)
        assert_eq!(
            identity.get_total_vc_count(&subject),
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
        credit.set_identity_oracle(&admin, &identity_id);

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
        let subject = soroban_sdk::Address::generate(&env);

        let hash1 = BytesN::from_array(&env, &[1u8; 32]);
        let hash2 = BytesN::from_array(&env, &[2u8; 32]); // This will belong to issuer2
        let hash3 = BytesN::from_array(&env, &[3u8; 32]);

        // issuer2 revokes hash2 individually to claim authority
        revocation.revoke(&issuer2, &subject, &hash2);
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
            Err(Ok(
                revocation_registry::RevocationRegistryError::IssuerMismatch
            ))
        );

        // Verify that hash1 and hash3 were NOT revoked (atomicity check)
        assert!(!revocation.is_revoked(&hash1));
        assert!(!revocation.is_revoked(&hash3));
    }

    /// An oversized batch submitted without the issuer's authorization must
    /// fail with an auth error, not `BatchTooLarge`: `require_auth()` runs
    /// before the batch-size check so unauthenticated callers cannot probe
    /// validation logic without paying for authorization.
    #[test]
    fn test_batch_revoke_requires_auth_before_size_check() {
        let env = Env::default();
        // Deliberately do NOT mock authorizations: nothing is signed.
        let revocation_id = env.register_contract(None, RevocationRegistry);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let issuer = soroban_sdk::Address::generate(&env);

        // 101 hashes: exceeds the batch size limit.
        let mut batch = soroban_sdk::Vec::new(&env);
        for i in 0..101u32 {
            let mut hash_arr = [0u8; 32];
            hash_arr[0] = (i % 256) as u8;
            hash_arr[1] = (i / 256) as u8;
            batch.push_back(BytesN::from_array(&env, &hash_arr));
        }

        let res = revocation.try_batch_revoke(&issuer, &batch);

        match res {
            // Host-level auth error: expected, authorization is enforced first.
            Err(Err(soroban_sdk::InvokeError::Abort)) => {}
            // A contract-level BatchTooLarge would mean the size check ran
            // before auth.
            Err(Ok(e)) => panic!(
                "expected auth error before size check, got contract error {:?}",
                e
            ),
            other => panic!(
                "unauthorized batch_revoke must fail with an auth error, got {:?}",
                other.map(|_| ())
            ),
        }
    }

    #[test]
    fn test_revocation_registry_count_and_list_integration() {
        let env = Env::default();
        env.mock_all_auths();

        let revocation_id = env.register_contract(None, RevocationRegistry);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        revocation.initialize(&admin);

        let issuer1 = soroban_sdk::Address::generate(&env);
        let issuer2 = soroban_sdk::Address::generate(&env);

        assert_eq!(revocation.get_revocation_count(&issuer1), 0);
        assert_eq!(revocation.get_revocation_count(&issuer2), 0);
        assert_eq!(revocation.list_revoked(&issuer1, &0, &10).len(), 0);

        let subject = soroban_sdk::Address::generate(&env);
        // 1. Single revocation by issuer1
        let hash_a = BytesN::from_array(&env, &[101u8; 32]);
        revocation.revoke(&issuer1, &subject, &hash_a);

        assert_eq!(revocation.get_revocation_count(&issuer1), 1);
        let list1 = revocation.list_revoked(&issuer1, &0, &10);
        assert_eq!(list1.len(), 1);
        assert_eq!(list1.get(0).unwrap(), hash_a);

        // 2. Batch revocation by issuer1
        let hash_b = BytesN::from_array(&env, &[102u8; 32]);
        let hash_c = BytesN::from_array(&env, &[103u8; 32]);
        let mut batch = soroban_sdk::Vec::new(&env);
        batch.push_back(hash_b.clone());
        batch.push_back(hash_c.clone());
        revocation.batch_revoke(&issuer1, &batch);

        assert_eq!(revocation.get_revocation_count(&issuer1), 3);
        let list1_all = revocation.list_revoked(&issuer1, &0, &10);
        assert_eq!(list1_all.len(), 3);
        assert_eq!(list1_all.get(0).unwrap(), hash_a);
        assert_eq!(list1_all.get(1).unwrap(), hash_b);
        assert_eq!(list1_all.get(2).unwrap(), hash_c);

        // 3. Single revocation by issuer2
        let hash_d = BytesN::from_array(&env, &[104u8; 32]);
        revocation.revoke(&issuer2, &subject, &hash_d);

        assert_eq!(revocation.get_revocation_count(&issuer2), 1);
        assert_eq!(revocation.get_revocation_count(&issuer1), 3);
        let list2 = revocation.list_revoked(&issuer2, &0, &10);
        assert_eq!(list2.len(), 1);
        assert_eq!(list2.get(0).unwrap(), hash_d);
    }

    /// Integration test: create a proposal, cancel it, then verify execute fails
    /// with ProposalAlreadyCancelled.
    ///
    /// Acceptance criteria for cancel_proposal:
    ///   - cancel_proposal(proposer, proposal_id) succeeds.
    ///   - Cancelled proposals cannot be executed.
    ///   - ProposalCancelled event is emitted.
    #[test]
    fn test_cancel_proposal_create_cancel_execute_fails() {
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

        // Create a proposal
        let proposer = soroban_sdk::Address::generate(&env);
        let proposal_id = gov.create_proposal(&proposer, &proposed_weights, &100, &0);

        // Cast a passing vote (so execute would succeed if not cancelled)
        let voter = soroban_sdk::Address::generate(&env);
        gov.register_voter(&admin, &voter, &200);
        gov.vote(&voter, &proposal_id, &true, &200);

        // Proposer cancels the proposal
        gov.cancel_proposal(&proposer, &proposal_id);

        // Capture events immediately after cancel_proposal, before any other call drains them.
        // env.events().all() is a draining operation — any subsequent client call will
        // clear the buffer, so we must read it before get_proposal.
        let events = env.events().all();

        // Verify the proposal is marked cancelled on-chain
        let proposal = gov.get_proposal(&proposal_id).unwrap();
        assert!(proposal.cancelled, "proposal must be marked cancelled");
        assert!(!proposal.executed, "proposal must not be executed");

        // Verify the PropCanc event was emitted
        let cancel_events: Vec<_> = events
            .iter()
            .filter(|(id, topics, _)| {
                if *id != gov_id || topics.len() != 2 {
                    return false;
                }
                let sym: Result<soroban_sdk::Symbol, _> = topics.get(0).unwrap().try_into_val(&env);
                sym.map(|s| s == soroban_sdk::symbol_short!("PropCanc"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(
            cancel_events.len(),
            1,
            "exactly one PropCanc event must be emitted"
        );
        let (_, topics, data) = &cancel_events[0];
        let event_proposal_id: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(event_proposal_id, proposal_id);
        let event_canceller: soroban_sdk::Address = data.clone().try_into_val(&env).unwrap();
        assert_eq!(event_canceller, proposer);

        // Advance ledger past expiry and execution delay so execute would normally proceed
        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });

        // Execute must fail with ProposalAlreadyCancelled
        let res = gov.try_execute(&proposal_id);
        assert_eq!(
            res,
            Err(Ok(GovernanceError::ProposalAlreadyCancelled)),
            "expected ProposalAlreadyCancelled when executing a cancelled proposal"
        );
    }

    /// Admin can also cancel a proposal (not just the proposer).
    #[test]
    fn test_admin_can_cancel_proposal() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let gov_id = env.register_contract(None, Governance);

        let credit = CreditOracleClient::new(&env, &credit_id);
        let gov = GovernanceClient::new(&env, &gov_id);

        let admin = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);
        gov.initialize(&admin, &credit_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };

        let proposer = soroban_sdk::Address::generate(&env);
        let proposal_id = gov.create_proposal(&proposer, &proposed_weights, &100, &0);

        // Admin (not the proposer) cancels the proposal
        gov.cancel_proposal(&admin, &proposal_id);

        let proposal = gov.get_proposal(&proposal_id).unwrap();
        assert!(
            proposal.cancelled,
            "admin should be able to cancel a proposal"
        );
    }

    /// A third party (neither proposer nor admin) cannot cancel a proposal.
    #[test]
    fn test_unauthorized_cancel_is_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let gov_id = env.register_contract(None, Governance);

        let credit = CreditOracleClient::new(&env, &credit_id);
        let gov = GovernanceClient::new(&env, &gov_id);

        let admin = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);
        gov.initialize(&admin, &credit_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };

        let proposer = soroban_sdk::Address::generate(&env);
        let proposal_id = gov.create_proposal(&proposer, &proposed_weights, &100, &0);

        let third_party = soroban_sdk::Address::generate(&env);
        let res = gov.try_cancel_proposal(&third_party, &proposal_id);
        assert_eq!(
            res,
            Err(Ok(GovernanceError::NotAuthorized)),
            "third party must not be able to cancel a proposal"
        );

        // Proposal must remain active
        let proposal = gov.get_proposal(&proposal_id).unwrap();
        assert!(!proposal.cancelled, "proposal must not be cancelled");
    }

    /// Cancelling an already-cancelled proposal returns ProposalAlreadyCancelled.
    #[test]
    fn test_double_cancel_is_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let gov_id = env.register_contract(None, Governance);

        let credit = CreditOracleClient::new(&env, &credit_id);
        let gov = GovernanceClient::new(&env, &gov_id);

        let admin = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);
        gov.initialize(&admin, &credit_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };

        let proposer = soroban_sdk::Address::generate(&env);
        let proposal_id = gov.create_proposal(&proposer, &proposed_weights, &100, &0);

        // First cancel succeeds
        gov.cancel_proposal(&proposer, &proposal_id);

        // Second cancel must fail
        let res = gov.try_cancel_proposal(&proposer, &proposal_id);
        assert_eq!(
            res,
            Err(Ok(GovernanceError::ProposalAlreadyCancelled)),
            "second cancel must return ProposalAlreadyCancelled"
        );
    }

    /// Voting on a cancelled proposal must be rejected with
    /// ProposalAlreadyCancelled, even while the voting period is still open.
    /// Execution of the cancelled proposal must also remain rejected.
    #[test]
    fn test_vote_after_cancel_is_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let gov_id = env.register_contract(None, Governance);

        let credit = CreditOracleClient::new(&env, &credit_id);
        let gov = GovernanceClient::new(&env, &gov_id);

        let admin = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);
        gov.initialize(&admin, &credit_id, &100);

        let proposed_weights = ScoringWeights {
            vc_weight: 40,
            tx_weight: 30,
            repayment_weight: 30,
        };

        let proposer = soroban_sdk::Address::generate(&env);
        let proposal_id = gov.create_proposal(&proposer, &proposed_weights, &100, &0);

        // Register a voter and cast a vote before cancellation.
        let voter1 = soroban_sdk::Address::generate(&env);
        let voter2 = soroban_sdk::Address::generate(&env);
        gov.register_voter(&admin, &voter1, &500);
        gov.register_voter(&admin, &voter2, &500);
        gov.vote(&voter1, &proposal_id, &true, &300);

        // Proposer cancels while voting is still open.
        gov.cancel_proposal(&proposer, &proposal_id);

        // New votes must be rejected even though the voting period has not
        // expired.
        let res = gov.try_vote(&voter2, &proposal_id, &true, &100);
        assert_eq!(
            res,
            Err(Ok(GovernanceError::ProposalAlreadyCancelled)),
            "vote after cancel must return ProposalAlreadyCancelled"
        );

        // Votes cast before cancellation are preserved for audit.
        let proposal = gov.get_proposal(&proposal_id).unwrap();
        assert!(proposal.cancelled, "proposal must be marked cancelled");
        assert_eq!(
            proposal.votes_for, 300,
            "pre-cancel votes must be preserved"
        );

        // Advance past the voting period: execution stays rejected.
        env.ledger().with_mut(|l| {
            l.sequence_number += 101;
        });
        let res = gov.try_execute(&proposal_id);
        assert_eq!(
            res,
            Err(Ok(GovernanceError::ProposalAlreadyCancelled)),
            "executing a cancelled proposal must return ProposalAlreadyCancelled"
        );
    }

    /// Integration test for governance execution timelock:
    /// vote passes → advance past voting → execution rejected (timelock) →
    /// advance past delay → execution succeeds.
    ///
    /// This follows the double-timelock model from docs/governance.md §2.2:
    /// `execute()` only queues weights in the credit-oracle via
    /// `propose_weights()` (starting the fixed 17,280-ledger timelock); the
    /// weights do NOT become active until `apply_weights()` is called after
    /// that timelock expires. This test therefore:
    ///   1. execute() → active weights unchanged (still default)
    ///   2. advance 17,282 ledgers and bump instance TTL on both contracts
    ///   3. apply_weights()
    ///   4. verify active weights now equal the proposal's values.
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

        // Register voter with sufficient weight
        gov.register_voter(&admin, &voter, &200);

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
        assert!(
            proposal.executed,
            "proposal must be executed after timelock"
        );

        // Verify weights are NOT changed immediately (timelock in effect)
        let weights_after_execute = credit.get_scoring_weights().unwrap();
        assert_eq!(weights_after_execute.vc_weight, 40); // Still default

        // Advance ledger past timelock (~24 hours = 17,280 ledgers)
        let jump = 17_280 + 2;
        env.as_contract(&credit_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.as_contract(&gov_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger().with_mut(|l| {
            l.sequence_number += jump;
        });

        // Apply weights after timelock
        gov.apply_weights();

        // Verify weights were applied to the credit oracle
        let weights = credit.get_scoring_weights().unwrap();
        assert_eq!(weights.vc_weight, 50);
        assert_eq!(weights.tx_weight, 20);
        assert_eq!(weights.repayment_weight, 30);
    }

    #[test]
    fn test_governance_integration_deploy_all_contracts_and_admin_flow() {
        let env = Env::default();
        env.mock_all_auths();

        // Deploy all four contracts
        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);
        let gov_id = env.register_contract(None, Governance);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);
        let gov = GovernanceClient::new(&env, &gov_id);

        let admin = soroban_sdk::Address::generate(&env);

        // Initialize all contracts with the same admin for simplicity
        identity.initialize(&admin);
        credit.initialize(&admin);
        revocation.initialize(&admin);
        gov.initialize(&admin, &credit_id, &100);

        // Wire identity <-> revocation and credit <-> identity
        identity.set_revocation_registry(&revocation_id);
        credit.set_identity_oracle(&admin, &identity_id);

        // 1) Two-step admin transfer on `credit`: admin -> new_admin
        let new_admin = soroban_sdk::Address::generate(&env);
        // Propose new admin (signed by current admin)
        credit.propose_new_admin(&new_admin);
        // Accept as new admin
        credit.accept_admin(&new_admin);

        // Verify admin changed by exercising an admin-only call using `new_admin`
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_feeder(&new_admin, &feeder);

        // 2) Transfer oracle admin to governance contract
        credit.propose_new_admin(&gov_id);
        // Governance accepts the oracle admin on its behalf
        gov.accept_oracle_admin();

        // 3) Governance proposal lifecycle: create -> vote -> execute -> apply
        let proposed_weights = ScoringWeights {
            vc_weight: 45,
            tx_weight: 25,
            repayment_weight: 30,
        };

        let proposer = soroban_sdk::Address::generate(&env);
        let proposal_id = gov.create_proposal(&proposer, &proposed_weights, &10u32, &0u32);

        // Cast votes to pass the proposal
        let voter = soroban_sdk::Address::generate(&env);

        // Register voter with sufficient weight
        gov.register_voter(&admin, &voter, &200i128);

        gov.vote(&voter, &proposal_id, &true, &200i128);

        // Advance ledger past voting expiry
        env.ledger().with_mut(|l| {
            l.sequence_number += 11;
        });

        // Execute proposal (governance is now credit admin and will propose weights)
        gov.execute(&proposal_id);

        // Advance ledger to pass credit-oracle timelock and apply the proposed weights
        let jump = 100_000u32;
        env.as_contract(&credit_id, || {
            env.storage().instance().extend_ttl(jump, jump);
        });
        env.ledger().with_mut(|l| {
            l.sequence_number += jump;
        });

        credit.apply_weights();

        // Verify the credit oracle weights were updated
        let active_weights = credit.get_scoring_weights().unwrap();
        assert_eq!(active_weights.vc_weight, 45);
        assert_eq!(active_weights.tx_weight, 25);
        assert_eq!(active_weights.repayment_weight, 30);
    }

    #[test]
    fn test_list_issuers_reflects_register_and_deregister_operations() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let identity = IdentityOracleClient::new(&env, &identity_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);

        let issuer_a = soroban_sdk::Address::generate(&env);
        let issuer_b = soroban_sdk::Address::generate(&env);
        let issuer_c = soroban_sdk::Address::generate(&env);

        identity.register_issuer(&issuer_a);
        identity.register_issuer(&issuer_b);
        identity.register_issuer(&issuer_c);

        let all = identity.list_issuers();
        assert_eq!(all.len(), 3);

        identity.deregister_issuer(&issuer_b);
        let after = identity.list_issuers();
        assert_eq!(after.len(), 2);
        // remaining entries should be a and c in some order
        let mut found_a = false;
        let mut found_c = false;
        for i in 0..after.len() {
            let a = after.get(i).unwrap();
            if a == issuer_a {
                found_a = true;
            }
            if a == issuer_c {
                found_c = true;
            }
        }
        assert!(
            found_a && found_c,
            "expected issuer_a and issuer_c to remain"
        );
    }

    #[test]
    fn test_reregistering_deregistered_issuer_does_not_duplicate_index() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let identity = IdentityOracleClient::new(&env, &identity_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);

        let issuer = soroban_sdk::Address::generate(&env);

        identity.register_issuer(&issuer);
        let first = identity.list_issuers();
        assert_eq!(first.len(), 1);

        identity.deregister_issuer(&issuer);
        let after_dereg = identity.list_issuers();
        assert_eq!(after_dereg.len(), 0);

        // Re-register — should not create a duplicate entry in the compacted index
        identity.register_issuer(&issuer);
        let after_rereg = identity.list_issuers();
        assert_eq!(after_rereg.len(), 1);
    }

    #[test]
    fn test_protocol_stats_identity_oracle_integration() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let identity = IdentityOracleClient::new(&env, &identity_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        // Verify initial stats are zero
        let stats0 = identity.get_protocol_stats();
        assert_eq!(stats0.total_dids_anchored, 0);
        assert_eq!(stats0.total_vcs_anchored, 0);
        assert_eq!(stats0.total_vcs_revoked, 0);

        // Anchor a DID
        let subject = soroban_sdk::Address::generate(&env);
        let cid = soroban_sdk::String::from_str(&env, "ipfs://QmStatsTestDID");
        identity.anchor_did(&subject, &cid);

        let stats1 = identity.get_protocol_stats();
        assert_eq!(stats1.total_dids_anchored, 1);

        // Anchor two VCs
        let vc_hash1 = BytesN::from_array(&env, &[1u8; 32]);
        let vc_hash2 = BytesN::from_array(&env, &[2u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash1);
        identity.anchor_vc(&issuer, &subject, &vc_hash2);

        let stats2 = identity.get_protocol_stats();
        assert_eq!(stats2.total_vcs_anchored, 2);
        assert_eq!(stats2.total_vcs_revoked, 0);

        // Revoke one VC
        identity.mark_vc_revoked(&issuer, &subject, &vc_hash1);

        let stats3 = identity.get_protocol_stats();
        assert_eq!(stats3.total_vcs_anchored, 2);
        assert_eq!(stats3.total_vcs_revoked, 1);
    }

    #[test]
    fn test_protocol_stats_credit_oracle_integration() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let lender = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);
        credit.register_lender(&admin, &lender);

        // Verify initial stats are zero
        let stats0 = credit.get_protocol_stats();
        assert_eq!(stats0.total_subjects_scored, 0);
        assert_eq!(stats0.total_repayments_recorded, 0);

        // Record some repayments
        let subject = soroban_sdk::Address::generate(&env);
        for _ in 0..3 {
            credit.record_repayment(&lender, &subject, &100_000_000i128, &true);
        }

        let stats1 = credit.get_protocol_stats();
        assert_eq!(stats1.total_repayments_recorded, 3);
        assert_eq!(stats1.total_subjects_scored, 0);

        // Compute score for the subject
        credit.compute_score(&subject);

        let stats2 = credit.get_protocol_stats();
        assert_eq!(stats2.total_subjects_scored, 1);

        // Advance ledger to bypass cooldown
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);

        // Compute score again for same subject — should NOT double count
        credit.compute_score(&subject);
        let stats3 = credit.get_protocol_stats();
        assert_eq!(stats3.total_subjects_scored, 1);

        // Compute score for a different subject
        let subject2 = soroban_sdk::Address::generate(&env);
        credit.compute_score(&subject2);

        let stats4 = credit.get_protocol_stats();
        assert_eq!(stats4.total_subjects_scored, 2);
    }

    /// Verify that deactivate/reactivate lifecycle works across identity-oracle
    /// and credit-oracle, and that compute_score returns 300 for deactivated subjects.
    #[test]
    fn test_deactivate_identity_affects_compute_score() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);

        // Link credit-oracle to identity-oracle for cross-contract lookups
        credit.set_identity_oracle(&admin, &identity_id);

        // Register an issuer
        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        // Create a subject with a DID and 2 VCs
        let subject = soroban_sdk::Address::generate(&env);
        let cid = String::from_str(&env, "ipfs://QmTestDID");
        identity.anchor_did(&subject, &cid);

        let vc_hash1 = BytesN::from_array(&env, &[1u8; 32]);
        let vc_hash2 = BytesN::from_array(&env, &[2u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash1);
        identity.anchor_vc(&issuer, &subject, &vc_hash2);

        // Register a lender and record repayments so score > 300
        let lender = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_lender(&admin, &lender);
        credit.register_feeder(&admin, &feeder);

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

        // 1. Initial score should be > 300 (has active VCs)
        let initial_score = credit.compute_score(&subject);
        assert!(
            initial_score > 300,
            "expected initial score > 300, got {}",
            initial_score
        );

        // 2. Deactivate the identity
        let revoked = identity.deactivate_identity(&subject);
        assert_eq!(revoked, 2);
        assert!(identity.is_deactivated(&subject));
        assert!(!identity.is_verified(&subject));
        // Advance ledger to satisfy compute_score cooldown
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);

        // 3. compute_score should now return 300 for deactivated subject
        let score_after_deactivation = credit.compute_score(&subject);
        assert_eq!(
            score_after_deactivation, 300,
            "expected score 300 for deactivated subject, got {}",
            score_after_deactivation
        );

        // 4. Reactivate the identity
        identity.reactivate_identity(&subject);
        assert!(!identity.is_deactivated(&subject));

        // 5. is_verified still false because VCs remain revoked
        assert!(!identity.is_verified(&subject));
        assert_eq!(identity.get_active_vc_count(&subject), 0);

        // 6. compute_score after reactivation is < initial but >= 300
        //    because repayment history and tx stats remain in the credit-oracle
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_after_reactivation = credit.compute_score(&subject);
        assert!(
            score_after_reactivation >= 300,
            "expected score >= 300 after reactivation, got {}",
            score_after_reactivation
        );
        assert!(
            score_after_reactivation < initial_score,
            "expected score after reactivation ({}) < initial score ({}), got {} (VCs are 0 but repayment data remains)",
            score_after_reactivation,
            initial_score,
            score_after_reactivation
        );

        // 7. Anchor a new VC — subject becomes verified again
        let vc_hash3 = BytesN::from_array(&env, &[3u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash3);
        assert!(identity.is_verified(&subject));
        assert_eq!(identity.get_active_vc_count(&subject), 1);

        // 8. Score should now be > 300 again
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_final = credit.compute_score(&subject);
        assert!(
            score_final > 300,
            "expected score > 300 after new VC, got {}",
            score_final
        );
    }

    /// Verify that computing a score twice captures the previous score in the record.
    #[test]
    fn test_score_record_preserves_previous_score() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);
        let lender = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);

        credit.initialize(&admin);
        credit.register_feeder(&admin, &feeder);
        credit.register_lender(&admin, &lender);

        // First computation: base score with no data
        let first_score = credit.compute_score(&subject);
        assert_eq!(first_score, 300);

        // Verify previous_score is None on first write
        let record1 = credit.get_score(&subject).unwrap();
        assert_eq!(record1.previous_score, None);

        // Now add some data so the second computation yields a different score
        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 500_000_000i128,
                tx_count_30d: 10,
                avg_counterparties: 3,
            },
        );
        credit.set_vc_count(&feeder, &subject, &1);
        for _ in 0..5 {
            credit.record_repayment(&lender, &subject, &100_000_000i128, &true);
        }

        // Advance ledger so the new write is not skipped as unchanged
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);

        // Second computation
        let second_score = credit.compute_score(&subject);
        assert!(second_score > first_score);

        // Verify previous_score is set to the first score
        let record2 = credit.get_score(&subject).unwrap();
        assert_eq!(record2.previous_score, Some(first_score));
        assert_eq!(record2.score, second_score);
    }

    #[contract]
    pub struct CreditOracleV1Mock;

    #[contractimpl]
    impl CreditOracleV1Mock {
        pub fn initialize(env: Env, admin: soroban_sdk::Address) {
            env.storage().instance().set(&DataKey::Admin, &admin);
            let default_weights = ScoringWeights {
                vc_weight: 40,
                tx_weight: 30,
                repayment_weight: 30,
            };
            env.storage()
                .instance()
                .set(&DataKey::Config, &default_weights);
            env.storage()
                .instance()
                .set(&DataKey::ComputeCooldownLedgers, &1u32);
        }

        pub fn register_lender(env: Env, lender: soroban_sdk::Address) {
            env.storage()
                .persistent()
                .set(&DataKey::TrustedLender(lender), &true);
        }

        pub fn record_repayment(
            env: Env,
            _lender: soroban_sdk::Address,
            subject: soroban_sdk::Address,
            on_time: bool,
        ) {
            let mut record = env
                .storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject.clone()))
                .unwrap_or(RepaymentRecordV1 {
                    on_time_count: 0,
                    total_count: 0,
                });
            if on_time {
                record.on_time_count += 1;
            }
            record.total_count += 1;
            env.storage()
                .persistent()
                .set(&DataKey::RepaymentRecord(subject), &record);
        }
    }

    #[test]
    fn test_storage_migration_flow() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, CreditOracleV1Mock);
        let client_v1 = CreditOracleV1MockClient::new(&env, &contract_id);

        let admin = soroban_sdk::Address::generate(&env);
        client_v1.initialize(&admin);

        let lender = soroban_sdk::Address::generate(&env);
        client_v1.register_lender(&lender);

        let subject1 = soroban_sdk::Address::generate(&env);
        let subject2 = soroban_sdk::Address::generate(&env);

        client_v1.record_repayment(&lender, &subject1, &true);
        client_v1.record_repayment(&lender, &subject1, &false);
        client_v1.record_repayment(&lender, &subject2, &true);

        let rec1_v1: RepaymentRecordV1 = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject1.clone()))
                .unwrap()
        });
        assert_eq!(rec1_v1.on_time_count, 1);
        assert_eq!(rec1_v1.total_count, 2);

        env.register_contract(Some(&contract_id), CreditOracle);
        let client_v2 = CreditOracleClient::new(&env, &contract_id);

        let score1_before = client_v2.compute_score(&subject1);
        assert!(score1_before > 300);

        client_v2.record_repayment(&lender, &subject1, &1000i128, &true);
        let rec1_v1_updated: RepaymentRecordV1 = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject1.clone()))
                .unwrap()
        });
        assert_eq!(rec1_v1_updated.on_time_count, 2);
        assert_eq!(rec1_v1_updated.total_count, 3);

        let mut subjects = soroban_sdk::Vec::new(&env);
        subjects.push_back(subject1.clone());
        subjects.push_back(subject2.clone());
        client_v2.migrate(&subjects);

        let rec1_v2: RepaymentRecord = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject1.clone()))
                .unwrap()
        });
        assert_eq!(rec1_v2.on_time_count, 2);
        assert_eq!(rec1_v2.total_count, 3);
        assert_eq!(rec1_v2.total_repaid, 0);

        client_v2.record_repayment(&lender, &subject1, &5000i128, &true);
        let rec1_v2_updated: RepaymentRecord = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&DataKey::RepaymentRecord(subject1.clone()))
                .unwrap()
        });
        assert_eq!(rec1_v2_updated.on_time_count, 3);
        assert_eq!(rec1_v2_updated.total_count, 4);
        assert_eq!(rec1_v2_updated.total_repaid, 5000);
    }

    // ── Compute-score cooldown tests ──────────────────────────────────────

    /// cooldown = 1 (default): second call within the same ledger is rejected.
    #[test]
    fn test_compute_score_cooldown_rejects_same_ledger() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);

        // First call succeeds
        credit.compute_score(&subject);

        // Second call in the same ledger is rejected by cooldown
        let result = credit.try_compute_score(&subject);
        assert_eq!(
            result,
            Err(Ok(credit_oracle::CreditOracleError::ComputeCooldownActive))
        );
    }

    /// cooldown = 0: two compute_score calls within the same ledger both succeed.
    #[test]
    fn test_compute_score_no_cooldown_allows_same_ledger() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);

        // Override cooldown to 0
        env.as_contract(&credit_id, || {
            env.storage()
                .instance()
                .set(&DataKey::ComputeCooldownLedgers, &0u32);
        });

        let score1 = credit.compute_score(&subject);
        let score2 = credit.compute_score(&subject);

        // Both calls succeed and return identical scores (no input changed)
        assert_eq!(score1, score2);
    }

    /// Verify computed_at_ledger is updated after every successful write.
    #[test]
    fn test_compute_score_updates_last_computed_ledger() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);

        // First computation
        credit.compute_score(&subject);
        let record1 = credit.get_score(&subject).unwrap();
        assert_eq!(
            record1.computed_at_ledger,
            env.ledger().sequence(),
            "first computed_at_ledger should match current ledger"
        );

        // Advance ledger and change an input so a write actually occurs
        let feeder = soroban_sdk::Address::generate(&env);
        credit.register_feeder(&admin, &feeder);
        credit.set_vc_count(&feeder, &subject, &3);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);

        // Second computation — write occurs because VC count changed
        credit.compute_score(&subject);
        let record2 = credit.get_score(&subject).unwrap();
        assert_eq!(
            record2.computed_at_ledger,
            env.ledger().sequence(),
            "second computed_at_ledger should match updated ledger"
        );
        assert!(
            record2.computed_at_ledger > record1.computed_at_ledger,
            "computed_at_ledger should increase after recomputation"
        );
    }

    /// Deterministic scoring: identical inputs produce identical scores.
    #[test]
    fn test_compute_score_deterministic() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);
        let lender = soroban_sdk::Address::generate(&env);
        let subject_a = soroban_sdk::Address::generate(&env);
        let subject_b = soroban_sdk::Address::generate(&env);

        credit.initialize(&admin);
        credit.register_feeder(&admin, &feeder);
        credit.register_lender(&admin, &lender);

        // Identical setup for both subjects
        for subject in [&subject_a, &subject_b] {
            credit.set_vc_count(&feeder, subject, &2);
            credit.update_tx_stats(
                &feeder,
                subject,
                &TxStats {
                    volume_30d: 1_000_000_000i128,
                    tx_count_30d: 50,
                    avg_counterparties: 10,
                },
            );
            for _ in 0..8 {
                credit.record_repayment(&lender, subject, &1000, &true);
            }
            for _ in 0..2 {
                credit.record_repayment(&lender, subject, &1000, &false);
            }
        }

        let score_a = credit.compute_score(&subject_a);
        let score_b = credit.compute_score(&subject_b);

        assert_eq!(
            score_a, score_b,
            "deterministic scoring: identical inputs must produce identical scores"
        );
    }

    #[test]
    fn test_revocation_registry_missing_does_not_break_is_verified() {
        // This test verifies that the identity oracle works correctly even
        // when RevocationRegistryId is NOT configured. In this state,
        // revocations through the registry are silently ignored, but the
        // contract should still function (backward compatibility).
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        revocation.initialize(&admin);

        // Intentionally do NOT call identity.set_revocation_registry()
        // Verify get_revocation_registry returns None
        assert!(identity.get_revocation_registry().is_none());

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        let subject = soroban_sdk::Address::generate(&env);
        let vc_hash = BytesN::from_array(&env, &[210u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash);

        assert!(identity.is_verified(&subject));
        assert_eq!(identity.get_active_vc_count(&subject), 1);

        // Revoke via revocation-registry (NOT via mark_vc_revoked)
        revocation.revoke(&issuer, &subject, &vc_hash);

        // Without registry linkage, revocations are silently ignored
        // This is the known limitation - the contract still works
        // but does not check the external registry
        assert!(identity.is_verified(&subject));
        assert_eq!(identity.get_active_vc_count(&subject), 1);
        assert!(identity.verify_vc(&subject, &vc_hash));

        // Now set the registry and confirm the revocation IS detected.
        //
        // `is_verified` / `verify_vc` perform a live cross-contract check against
        // the registry, so they reflect the revocation immediately. However,
        // `get_active_vc_count` is served from the cached `ActiveVCCount`, which is
        // only decremented through `mark_vc_revoked`. Because the registry was never
        // linked as the identity-oracle (revocation.set_identity_oracle was not
        // called), `revocation.revoke` did not invoke `mark_vc_revoked`, so the cache
        // is unchanged. The cached count is authoritative (issue #481).
        identity.set_revocation_registry(&revocation_id);

        assert!(!identity.is_verified(&subject));
        assert_eq!(identity.get_active_vc_count(&subject), 1);
        assert!(!identity.verify_vc(&subject, &vc_hash));
    }

    /// Full flow: file dispute → admin resolves → feeder re-syncs → score updates.
    ///
    /// Covers acceptance criteria for issue #244:
    /// file dispute → admin resolves → score updates.
    #[test]
    fn test_dispute_file_resolve_score_updates() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);
        let lender = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);

        credit.initialize(&admin);
        credit.register_feeder(&admin, &feeder);
        credit.register_lender(&admin, &lender);

        // Feeder submits tx_stats with an inflated volume (incorrect data).
        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 100_000_000_000i128,
                tx_count_30d: 500,
                avg_counterparties: 50,
            },
        );
        for _ in 0..5 {
            credit.record_repayment(&lender, &subject, &100_000_000i128, &true);
        }
        let inflated_score = credit.compute_score(&subject);
        assert!(
            inflated_score > 300,
            "expected inflated score > 300, got {}",
            inflated_score
        );

        // Step 1: Subject files a dispute against tx_stats.
        let input_key = soroban_sdk::Symbol::new(&env, "tx_stats");
        let reason = soroban_sdk::String::from_str(
            &env,
            "My 30d volume is much lower; feeder data is incorrect",
        );
        credit.flag_score_input(&subject, &input_key, &reason);

        let dispute = credit.get_dispute(&subject, &input_key).unwrap();
        assert_eq!(dispute.status, DisputeStatus::Pending);

        // Step 2: Admin accepts the dispute.
        credit.resolve_dispute(&subject, &input_key, &true);

        // Step 3: Verify DsptRslv event was emitted (feeder monitors this).
        let events = env.events().all();

        let resolved = credit.get_dispute(&subject, &input_key).unwrap();
        assert_eq!(resolved.status, DisputeStatus::Resolved);

        let mut rslv_count = 0;
        for (id, topics, _) in events.iter() {
            if id == credit_id && !topics.is_empty() {
                let topic_res: Result<soroban_sdk::Symbol, _> =
                    topics.get(0).unwrap().try_into_val(&env);
                if let Ok(topic) = topic_res {
                    if topic == soroban_sdk::symbol_short!("DsptRslv") {
                        rslv_count += 1;
                    }
                }
            }
        }
        assert_eq!(rslv_count, 1, "expected exactly one DsptRslv event");

        // Step 4: Feeder corrects tx_stats after re-sync.
        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 500_000_000i128, // corrected realistic value
                tx_count_30d: 5,
                avg_counterparties: 2,
            },
        );

        // Step 5: Recompute score; it must reflect the corrected input.
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let corrected_score = credit.compute_score(&subject);

        assert!(
            corrected_score < inflated_score,
            "score after correction ({}) should be lower than inflated score ({})",
            corrected_score,
            inflated_score
        );
        assert!(
            corrected_score >= 300,
            "score must be >= 300, got {}",
            corrected_score
        );
    }

    /// Subjects cannot file a dispute for an unrecognised input key.
    #[test]
    fn test_dispute_invalid_input_key_rejected_integration() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);

        let bad_key = soroban_sdk::Symbol::new(&env, "bad_key");
        let reason = soroban_sdk::String::from_str(&env, "test");
        let result = credit.try_flag_score_input(&subject, &bad_key, &reason);
        assert_eq!(
            result,
            Err(Ok(CreditOracleError::InvalidInputKey)),
            "expected InvalidInputKey for unrecognised input key"
        );
    }

    /// Anti-griefing: re-filing a pending dispute for the same key is blocked.
    #[test]
    fn test_dispute_anti_griefing_duplicate_pending_integration() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);
        credit.initialize(&admin);

        let input_key = soroban_sdk::Symbol::new(&env, "repayment");
        let reason = soroban_sdk::String::from_str(&env, "Missed repayment was on-time");
        credit.flag_score_input(&subject, &input_key, &reason);

        let result = credit.try_flag_score_input(&subject, &input_key, &reason);
        assert_eq!(
            result,
            Err(Ok(CreditOracleError::DisputeAlreadyPending)),
            "expected DisputeAlreadyPending when re-filing a pending dispute"
        );
    }

    /// Verifies that persistent storage entries (DID document, VC anchors, revocation status)
    /// survive a simulated ledger advance of 200,000 ledgers (> initial unextended threshold).
    #[test]
    fn test_persistent_storage_ttl_survival_after_ledger_advance() {
        let env = Env::default();
        env.mock_all_auths();

        let id_oracle_id = env.register_contract(None, IdentityOracle);
        let identity = IdentityOracleClient::new(&env, &id_oracle_id);

        let rev_reg_id = env.register_contract(None, RevocationRegistry);
        let revocation = RevocationRegistryClient::new(&env, &rev_reg_id);

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        let issuer = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);

        identity.initialize(&admin);
        identity.register_issuer(&issuer);

        revocation.initialize(&admin);
        identity.set_revocation_registry(&rev_reg_id);

        credit.initialize(&admin);
        credit.register_feeder(&admin, &issuer);
        credit.register_lender(&admin, &issuer);

        // 1. Write persistent entries
        let cid = String::from_str(&env, "ipfs://QmPersistentTestDoc");
        identity.anchor_did(&subject, &cid);

        let vc_hash = BytesN::from_array(&env, &[7u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash);

        revocation.revoke(&issuer, &subject, &vc_hash);

        credit.record_repayment(&issuer, &subject, &100i128, &true);

        // 2. Advance ledger sequence by 200,000 ledgers (~11.5 days of ledgers)
        let jump = 200_000u32;
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + jump);

        // 3. Verify persistent entries survive and remain accessible
        let retrieved_did = identity.get_did_document(&subject);
        assert_eq!(retrieved_did, Some(cid));

        let is_rev = revocation.is_revoked(&vc_hash);
        assert_eq!(is_rev, true);

        let active_count = identity.get_active_vc_count(&subject);
        assert_eq!(active_count, 0); // Revoked VC => 0 active
    }

    #[test]
    fn test_full_deployment_sequence() {
        let env = Env::default();
        env.mock_all_auths();

        // 1. Deploy all four contracts
        let credit_id = env.register_contract(None, CreditOracle);
        let identity_id = env.register_contract(None, IdentityOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);
        let gov_id = env.register_contract(None, Governance);

        let credit = CreditOracleClient::new(&env, &credit_id);
        let identity = IdentityOracleClient::new(&env, &identity_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);
        let gov = GovernanceClient::new(&env, &gov_id);

        let admin = soroban_sdk::Address::generate(&env);

        credit.initialize(&admin);
        identity.initialize(&admin);
        revocation.initialize(&admin);
        gov.initialize(&admin, &credit_id, &100i128);

        let issuer = soroban_sdk::Address::generate(&env);
        let subject = soroban_sdk::Address::generate(&env);
        let feeder = soroban_sdk::Address::generate(&env);

        identity.register_issuer(&issuer);
        credit.register_feeder(&admin, &feeder);

        let cid = String::from_str(&env, "ipfs://QmTestDID");
        identity.anchor_did(&subject, &cid);

        let vc_hash = BytesN::from_array(&env, &[42u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash);

        // Ensure initially verified
        assert_eq!(identity.is_verified(&subject), true);

        revocation.revoke(&issuer, &subject, &vc_hash);

        // (a) Verify that without set_revocation_registry, revocation-registry revocations are ignored
        assert_eq!(identity.is_verified(&subject), true);

        // (b) Verify that after set_revocation_registry, they are respected
        identity.set_revocation_registry(&revocation_id);

        // Now the revocation is respected
        assert_eq!(identity.is_verified(&subject), false);

        // Call set_identity_oracle on credit-oracle
        credit.set_identity_oracle(&admin, &identity_id);

        // Verify compute_score reflects the revoked VC
        let score_revoked = credit.compute_score(&subject);

        // Add a new VC to see the score increase, showing the score reflects active VCs
        let vc_hash2 = BytesN::from_array(&env, &[43u8; 32]);
        identity.anchor_vc(&issuer, &subject, &vc_hash2);

        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let score_active = credit.compute_score(&subject);

        assert!(
            score_active > score_revoked,
            "Score should increase when an active VC is present"
        );
    }

    /// Issue #530: recency decay must lower the score of stale credentials,
    /// clamp at the configured floor, and be fully reversible.
    #[test]
    fn test_recency_decay_affects_score() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = soroban_sdk::Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);
        // Decay needs `anchored_at`, which only the cross-contract path exposes.
        credit.set_identity_oracle(&admin, &identity_id);

        let issuer = soroban_sdk::Address::generate(&env);
        identity.register_issuer(&issuer);

        let subject = soroban_sdk::Address::generate(&env);
        identity.anchor_did(&subject, &String::from_str(&env, "ipfs://QmRecency"));

        // Five generic VCs anchored at T0 => 5 x 20 = 100 undecayed VC points.
        let t0 = 1_700_000_000u64;
        let day = 86_400u64;
        env.ledger().set_timestamp(t0);
        for i in 0..5u8 {
            identity.anchor_vc(&issuer, &subject, &BytesN::from_array(&env, &[i; 32]));
        }

        let weights = credit.get_scoring_weights().unwrap();
        let expected = |vc_points: u32| -> u32 {
            credit_oracle::compute_score_pure(
                vc_points,
                0,
                0,
                0,
                0,
                0,
                weights.vc_weight,
                weights.tx_weight,
                weights.repayment_weight,
            )
        };

        // Decay defaults to disabled: full weight, pre-#530 behavior.
        let baseline = credit.compute_score(&subject);
        assert_eq!(baseline, expected(100));

        // Enable the documented defaults: 5 bps/day with a 50% floor.
        credit.set_recency_decay(&admin, &true, &5, &5_000);

        // 100 days old => 10_000 - 500 = 9_500 bps => 19 points per VC.
        env.ledger().set_timestamp(t0 + 100 * day);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let aged_100d = credit.compute_score(&subject);
        assert_eq!(aged_100d, expected(95));
        assert!(
            aged_100d < baseline,
            "a 100-day-old credential must score below a fresh one"
        );

        // 5 years old => 9_125 bps of decay clamps to the 5_000 bps floor.
        env.ledger().set_timestamp(t0 + 1_825 * day);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        let aged_5y = credit.compute_score(&subject);
        assert_eq!(aged_5y, expected(50));
        assert!(aged_5y < aged_100d, "decay must be monotonic in age");

        // Turning decay off restores the pre-#530 score exactly.
        credit.set_recency_decay(&admin, &false, &5, &5_000);
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 1);
        assert_eq!(credit.compute_score(&subject), baseline);
    }
}
