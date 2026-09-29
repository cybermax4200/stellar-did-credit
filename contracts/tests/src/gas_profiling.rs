#[cfg(test)]
mod tests {
    use credit_oracle::{CreditOracle, CreditOracleClient, TxStats};
    use identity_oracle::{IdentityOracle, IdentityOracleClient};
    use revocation_registry::{RevocationRegistry, RevocationRegistryClient};
    use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, Vec};

    #[derive(Debug)]
    pub struct GasProfileResult {
        pub operation: &'static str,
        pub input_size: usize,
        pub cpu_instructions: u64,
        pub memory_bytes: u64,
    }

    #[test]
    fn profile_core_operations_gas_budget() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let credit_id = env.register_contract(None, CreditOracle);
        let revocation_id = env.register_contract(None, RevocationRegistry);

        let identity = IdentityOracleClient::new(&env, &identity_id);
        let credit = CreditOracleClient::new(&env, &credit_id);
        let revocation = RevocationRegistryClient::new(&env, &revocation_id);

        let admin = Address::generate(&env);
        identity.initialize(&admin);
        credit.initialize(&admin);
        revocation.initialize(&admin);

        let issuer = Address::generate(&env);
        identity.register_issuer(&issuer);

        let feeder = Address::generate(&env);
        credit.register_feeder(&admin, &feeder);

        let lender = Address::generate(&env);
        credit.register_lender(&admin, &lender);

        let subject = Address::generate(&env);

        let mut profiles: std::vec::Vec<GasProfileResult> = std::vec::Vec::new();

        for count in 1..=3 {
            let vc_hash = BytesN::from_array(&env, &[count as u8; 32]);
            env.budget().reset_default();
            identity.anchor_vc(&issuer, &subject, &vc_hash);
            profiles.push(GasProfileResult {
                operation: "anchor_vc",
                input_size: count,
                cpu_instructions: env.budget().cpu_instruction_cost(),
                memory_bytes: env.budget().memory_bytes_cost(),
            });
        }

        for count in 1..=3 {
            env.budget().reset_default();
            credit.record_repayment(&lender, &subject, &100_000_000i128, &true);
            profiles.push(GasProfileResult {
                operation: "record_repayment",
                input_size: count,
                cpu_instructions: env.budget().cpu_instruction_cost(),
                memory_bytes: env.budget().memory_bytes_cost(),
            });
        }

        credit.update_tx_stats(
            &feeder,
            &subject,
            &TxStats {
                volume_30d: 500_000_000i128,
                tx_count_30d: 10,
                avg_counterparties: 4,
            },
        );

        env.budget().reset_default();
        credit.compute_score(&subject);
        profiles.push(GasProfileResult {
            operation: "compute_score",
            input_size: 3,
            cpu_instructions: env.budget().cpu_instruction_cost(),
            memory_bytes: env.budget().memory_bytes_cost(),
        });

        env.budget().reset_default();
        let _score = credit.get_score(&subject);
        profiles.push(GasProfileResult {
            operation: "get_score",
            input_size: 1,
            cpu_instructions: env.budget().cpu_instruction_cost(),
            memory_bytes: env.budget().memory_bytes_cost(),
        });

        for size in [1, 5, 10, 25, 50] {
            let mut hashes = Vec::new(&env);
            for i in 0..size {
                let h = BytesN::from_array(&env, &[((i % 250) + 1) as u8; 32]);
                hashes.push_back(h);
            }
            env.budget().reset_default();
            revocation.batch_revoke(&issuer, &hashes);
            profiles.push(GasProfileResult {
                operation: "batch_revoke",
                input_size: size,
                cpu_instructions: env.budget().cpu_instruction_cost(),
                memory_bytes: env.budget().memory_bytes_cost(),
            });
        }

        assert!(!profiles.is_empty());
        assert!(profiles.iter().any(|p| p.operation == "anchor_vc"));
        assert!(profiles.iter().any(|p| p.operation == "record_repayment"));
        assert!(profiles.iter().any(|p| p.operation == "compute_score"));
        assert!(profiles.iter().any(|p| p.operation == "get_score"));
        assert!(profiles.iter().any(|p| p.operation == "batch_revoke"));

        std::println!("\n=================== GAS PROFILING HARNESS RESULTS ===================");
        std::println!(
            "| {:<18} | {:<10} | {:<16} | {:<12} |",
            "Operation",
            "Input Size",
            "CPU Instructions",
            "Memory Bytes"
        );
        std::println!("|--------------------|------------|------------------|--------------|");
        for p in &profiles {
            std::println!(
                "| {:<18} | {:<10} | {:<16} | {:<12} |",
                p.operation,
                p.input_size,
                p.cpu_instructions,
                p.memory_bytes
            );
        }
        std::println!("=====================================================================\n");
    }

    #[test]
    fn profile_list_feeders_with_50_entries() {
        let env = Env::default();
        env.mock_all_auths();

        let credit_id = env.register_contract(None, CreditOracle);
        let credit = CreditOracleClient::new(&env, &credit_id);

        let admin = Address::generate(&env);
        credit.initialize(&admin);

        for _ in 0..50u8 {
            let feeder = Address::generate(&env);
            credit.register_feeder(&admin, &feeder);
        }

        env.budget().reset_default();
        let feeders = credit.list_feeders();
        let cpu = env.budget().cpu_instruction_cost();

        assert_eq!(feeders.len(), 50);

        const MAINNET_CPU_LIMIT: u64 = 600_000_000;
        assert!(
            cpu < MAINNET_CPU_LIMIT,
            "list_feeders with 50 entries exceeded CPU limit: {} > {}",
            cpu,
            MAINNET_CPU_LIMIT
        );
    }

    #[test]
    fn profile_list_issuers_after_deregistration_compaction() {
        let env = Env::default();
        env.mock_all_auths();

        let identity_id = env.register_contract(None, IdentityOracle);
        let identity = IdentityOracleClient::new(&env, &identity_id);
        let admin = Address::generate(&env);
        identity.initialize(&admin);

        let mut issuers = std::vec::Vec::new();
        for _ in 0..50u8 {
            let issuer = Address::generate(&env);
            identity.register_issuer(&issuer);
            issuers.push(issuer);
        }

        for issuer in issuers.iter().take(49) {
            identity.deregister_issuer(issuer);
        }

        env.budget().reset_default();
        let active = identity.list_issuers();
        let cpu = env.budget().cpu_instruction_cost();

        assert_eq!(active.len(), 1);
        const MAINNET_CPU_LIMIT: u64 = 600_000_000;
        assert!(
            cpu < MAINNET_CPU_LIMIT,
            "list_issuers after compaction exceeded CPU limit: {} > {}",
            cpu,
            MAINNET_CPU_LIMIT
        );
    }
}