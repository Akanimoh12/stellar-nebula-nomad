#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    symbol_short, vec, Address, BytesN, Env,
};
use stellar_nebula_nomad::{
    emergency_controls::{
        require_not_paused, EmergencyError, initialize_admins, pause_contract,
        schedule_unpause, execute_unpause, is_paused, get_admins,
    },
    NebulaNomadContract, NebulaNomadContractClient, UNPAUSE_DELAY,
};

fn setup_env() -> (Env, NebulaNomadContractClient<'static>, Address) {
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
    let admin = Address::generate(&env);
    (env, client, admin)
}

fn advance_time(env: &Env, seconds: u64) {
    let ts = env.ledger().timestamp();
    let seq = env.ledger().sequence();
    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: seq + 1,
        timestamp: ts + seconds,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 10_000,
    });
}

// ─── Emergency Controls Integration Tests ───────────────────────────────────

#[test]
fn test_pause_prevents_ship_minting() {
    let (env, client, admin) = setup_env();
    let player = Address::generate(&env);

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    client.initialize_profile(&player);
    let metadata = [0u8; 4];

    let ship1 = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);
    assert!(ship1.id > 0);

    client.pause_contract(&admin);
    assert!(client.is_paused());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.mint_ship(&player, &symbol_short!("fighter"), &metadata)
    }));
    assert!(result.is_err());
}

#[test]
fn test_pause_prevents_treasure_deposit() {
    let (env, client, admin) = setup_env();
    let player = Address::generate(&env);

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    client.deposit_treasure(&player, &ship.id, &500u64);
    let balance = client.get_vault_balance(&player, &ship.id);
    assert_eq!(balance, 500u64);

    client.pause_contract(&admin);
    assert!(client.is_paused());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.deposit_treasure(&player, &ship.id, &100u64)
    }));
    assert!(result.is_err());
}

#[test]
fn test_pause_prevents_nebula_generation() {
    let (env, client, admin) = setup_env();
    let player = Address::generate(&env);

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let seed = BytesN::from_array(&env, &[1u8; 32]);
    let layout1 = client.generate_nebula_layout(&seed, &player);
    assert!(layout1.total_energy > 0);

    client.pause_contract(&admin);
    assert!(client.is_paused());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let seed2 = BytesN::from_array(&env, &[2u8; 32]);
        client.generate_nebula_layout(&seed2, &player)
    }));
    assert!(result.is_err());
}

#[test]
fn test_multi_admin_pause_authorization() {
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

    let admin1 = Address::generate(&env);
    let admin2 = Address::generate(&env);
    let non_admin = Address::generate(&env);

    let admins = vec![&env, admin1.clone(), admin2.clone()];
    client.initialize_admins(&admins);

    client.pause_contract(&admin1);
    assert!(client.is_paused());

    advance_time(&env, UNPAUSE_DELAY + 1);
    client.schedule_unpause(&admin2);
    client.execute_unpause(&admin2);
    assert!(!client.is_paused());
}

#[test]
fn test_schedule_unpause_delays_recovery() {
    let (env, client, admin) = setup_env();

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    client.pause_contract(&admin);
    assert!(client.is_paused());

    let unpause_at = client.schedule_unpause(&admin);
    assert!(client.is_paused());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.execute_unpause(&admin)
    }));
    assert!(result.is_err());

    advance_time(&env, UNPAUSE_DELAY + 1);
    client.execute_unpause(&admin);
    assert!(!client.is_paused());

    assert_eq!(unpause_at, 1_700_000_000 + UNPAUSE_DELAY);
}

#[test]
fn test_game_operations_work_when_not_paused() {
    let (env, client, admin) = setup_env();
    let player = Address::generate(&env);

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    assert!(!client.is_paused());

    client.initialize_profile(&player);
    let profile1 = client.get_profile(&player);
    assert_eq!(profile1.level, 1);

    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);
    assert!(ship.id > 0);

    client.deposit_treasure(&player, &ship.id, &1000u64);
    let balance = client.get_vault_balance(&player, &ship.id);
    assert_eq!(balance, 1000u64);

    let seed = BytesN::from_array(&env, &[5u8; 32]);
    let layout = client.generate_nebula_layout(&seed, &player);
    assert!(layout.total_energy > 0);
}

#[test]
fn test_admin_list_persists() {
    let (env, client, admin) = setup_env();

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    let stored = client.get_admins();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored.get(0).unwrap(), admin);

    client.pause_contract(&admin);

    let stored_after_pause = client.get_admins();
    assert_eq!(stored_after_pause.len(), 1);
    assert_eq!(stored_after_pause.get(0).unwrap(), admin);
}

#[test]
fn test_pause_state_survives_operations() {
    let (env, client, admin) = setup_env();
    let player = Address::generate(&env);

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    client.pause_contract(&admin);
    assert!(client.is_paused());

    let player2 = Address::generate(&env);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.initialize_profile(&player2);
    }));
    assert!(result.is_err());

    assert!(client.is_paused());

    advance_time(&env, UNPAUSE_DELAY + 1);
    client.schedule_unpause(&admin);
    client.execute_unpause(&admin);
    assert!(!client.is_paused());
}

#[test]
fn test_emergency_withdraw_signal() {
    let (env, client, admin) = setup_env();

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    client.emergency_withdraw(&admin, &symbol_short!("ore"));
}

#[test]
fn test_unpause_requires_scheduled_delay() {
    let (env, client, admin) = setup_env();

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    client.pause_contract(&admin);
    client.schedule_unpause(&admin);

    for i in 1..UNPAUSE_DELAY {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.try_execute_unpause(&admin)
        }));

        if i < UNPAUSE_DELAY {
            assert!(result.is_err() || result.unwrap().is_err());
        }

        if i + 100 < UNPAUSE_DELAY {
            advance_time(&env, 100);
        }
    }

    advance_time(&env, 100);
    client.execute_unpause(&admin);
    assert!(!client.is_paused());
}

#[test]
fn test_multiple_pause_cycles() {
    let (env, client, admin) = setup_env();

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    for cycle in 0..3 {
        assert!(!client.is_paused());

        client.pause_contract(&admin);
        assert!(client.is_paused());

        client.schedule_unpause(&admin);
        advance_time(&env, UNPAUSE_DELAY + 1);
        client.execute_unpause(&admin);
        assert!(!client.is_paused());
    }
}

#[test]
fn test_only_admins_can_pause() {
    let (env, client, admin) = setup_env();
    let non_admin = Address::generate(&env);

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.pause_contract(&non_admin)
    }));
    assert!(result.is_err());
    assert!(!client.is_paused());
}

#[test]
fn test_only_admins_can_emergency_withdraw() {
    let (env, client, admin) = setup_env();
    let non_admin = Address::generate(&env);

    let admins = vec![&env, admin.clone()];
    client.initialize_admins(&admins);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.emergency_withdraw(&non_admin, &symbol_short!("res"))
    }));
    assert!(result.is_err());
}
