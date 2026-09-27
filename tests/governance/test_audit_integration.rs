#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, BytesN, Env};
use stellar_nebula_nomad::{
    audit_logger::{AuditEntry, log_audit_event, query_audit_logs, get_audit_count},
    NebulaNomadContract, NebulaNomadContractClient,
};

fn setup_env() -> (Env, NebulaNomadContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 100,
        timestamp: 1_700_000_000,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 10_000,
    });
    let contract_id = env.register_contract(None, NebulaNomadContract);
    let client = NebulaNomadContractClient::new(&env, &contract_id);
    (env, client)
}

#[test]
fn test_audit_log_creation() {
    let (env, _client) = setup_env();
    let player = soroban_sdk::Address::generate(&env);
    let action = symbol_short!("mship");
    let details = BytesN::from_array(&env, &[1u8; 128]);

    env.as_contract(&env.register_contract(None, NebulaNomadContract), || {
        let entry = log_audit_event(&env, Some(&player), action.clone(), details.clone())
            .expect("log should succeed");
        assert_eq!(entry.actor, Some(player.clone()));
        assert_eq!(entry.action, action);
        assert_eq!(entry.timestamp, 1_700_000_000);
    });
}

#[test]
fn test_audit_log_sequential_ids() {
    let (env, _client) = setup_env();
    let player = soroban_sdk::Address::generate(&env);
    let action = symbol_short!("scan");
    let details = BytesN::from_array(&env, &[0u8; 128]);

    env.as_contract(&env.register_contract(None, NebulaNomadContract), || {
        let entry1 = log_audit_event(&env, Some(&player), action.clone(), details.clone())
            .expect("first log should succeed");
        let entry2 = log_audit_event(&env, Some(&player), action.clone(), details.clone())
            .expect("second log should succeed");
        let entry3 = log_audit_event(&env, Some(&player), action.clone(), details.clone())
            .expect("third log should succeed");

        assert_eq!(entry1.id, 0);
        assert_eq!(entry2.id, 1);
        assert_eq!(entry3.id, 2);
    });
}

#[test]
fn test_audit_log_query_filter() {
    let (env, _client) = setup_env();
    let player = soroban_sdk::Address::generate(&env);
    let action1 = symbol_short!("mship");
    let action2 = symbol_short!("scan");
    let details = BytesN::from_array(&env, &[0u8; 128]);

    env.as_contract(&env.register_contract(None, NebulaNomadContract), || {
        let _ = log_audit_event(&env, Some(&player), action1.clone(), details.clone());
        let _ = log_audit_event(&env, Some(&player), action2.clone(), details.clone());
        let _ = log_audit_event(&env, Some(&player), action1.clone(), details.clone());

        let results = query_audit_logs(&env, action1.clone(), 10)
            .expect("query should succeed");
        assert_eq!(results.len(), 2);

        for entry in &results {
            let e = entry;
            assert_eq!(e.action, action1);
        }
    });
}

#[test]
fn test_audit_log_count_increments() {
    let (env, _client) = setup_env();
    let player = soroban_sdk::Address::generate(&env);
    let action = symbol_short!("test");
    let details = BytesN::from_array(&env, &[0u8; 128]);

    env.as_contract(&env.register_contract(None, NebulaNomadContract), || {
        assert_eq!(get_audit_count(&env), 0);

        let _ = log_audit_event(&env, Some(&player), action.clone(), details.clone());
        assert_eq!(get_audit_count(&env), 1);

        let _ = log_audit_event(&env, Some(&player), action.clone(), details.clone());
        assert_eq!(get_audit_count(&env), 2);

        let _ = log_audit_event(&env, Some(&player), action.clone(), details.clone());
        assert_eq!(get_audit_count(&env), 3);
    });
}

#[test]
fn test_audit_log_without_actor() {
    let (env, _client) = setup_env();
    let action = symbol_short!("system");
    let details = BytesN::from_array(&env, &[42u8; 128]);

    env.as_contract(&env.register_contract(None, NebulaNomadContract), || {
        let entry = log_audit_event(&env, None, action.clone(), details.clone())
            .expect("log should succeed");
        assert_eq!(entry.actor, None);
        assert_eq!(entry.action, action);
    });
}

#[test]
fn test_audit_log_query_respects_limit() {
    let (env, _client) = setup_env();
    let player = soroban_sdk::Address::generate(&env);
    let action = symbol_short!("bulk");
    let details = BytesN::from_array(&env, &[0u8; 128]);

    env.as_contract(&env.register_contract(None, NebulaNomadContract), || {
        for _ in 0..20 {
            let _ = log_audit_event(&env, Some(&player), action.clone(), details.clone());
        }

        let results = query_audit_logs(&env, action.clone(), 5)
            .expect("query should succeed");
        assert_eq!(results.len(), 5);
    });
}

#[test]
fn test_audit_log_query_all_with_zero_limit() {
    let (env, _client) = setup_env();
    let player = soroban_sdk::Address::generate(&env);
    let action = symbol_short!("all");
    let details = BytesN::from_array(&env, &[0u8; 128]);

    env.as_contract(&env.register_contract(None, NebulaNomadContract), || {
        for _ in 0..15 {
            let _ = log_audit_event(&env, Some(&player), action.clone(), details.clone());
        }

        let results = query_audit_logs(&env, action.clone(), 0)
            .expect("query should succeed");
        assert!(results.len() > 0);
    });
}

#[test]
fn test_audit_log_preserves_details() {
    let (env, _client) = setup_env();
    let player = soroban_sdk::Address::generate(&env);
    let action = symbol_short!("dtls");
    let mut details_arr = [0u8; 128];
    details_arr[0] = 42;
    details_arr[1] = 99;
    let details = BytesN::from_array(&env, &details_arr);

    env.as_contract(&env.register_contract(None, NebulaNomadContract), || {
        let entry = log_audit_event(&env, Some(&player), action, details.clone())
            .expect("log should succeed");
        assert_eq!(entry.details, details);
    });
}

#[test]
fn test_audit_log_timestamps_increase() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, NebulaNomadContract);
    let _client = NebulaNomadContractClient::new(&env, &contract_id);

    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 100,
        timestamp: 1_000_000,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 10_000,
    });

    let player = soroban_sdk::Address::generate(&env);
    let action = symbol_short!("time");
    let details = BytesN::from_array(&env, &[0u8; 128]);

    env.as_contract(&contract_id, || {
        let entry1 = log_audit_event(&env, Some(&player), action.clone(), details.clone())
            .expect("first log");

        env.ledger().set(LedgerInfo {
            protocol_version: 22,
            sequence_number: 101,
            timestamp: 2_000_000,
            network_id: [0u8; 32],
            base_reserve: 10,
            min_temp_entry_ttl: 100,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 10_000,
        });

        let entry2 = log_audit_event(&env, Some(&player), action, details)
            .expect("second log");

        assert!(entry2.timestamp > entry1.timestamp);
    });
}
