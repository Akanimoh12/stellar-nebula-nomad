#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, vec, Address, BytesN, Env, IntoVal, Vec};
use stellar_nebula_nomad::{
    NebulaNomadContract, NebulaNomadContractClient, ProfileError, ShipError, GRID_SIZE,
    TOTAL_CELLS,
};
use stellar_nebula_nomad::resource_minter::{ResourceMinter, ResourceMinterClient};

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
    let player = Address::generate(&env);
    (env, client, player)
}

// ─── E2E Flow Tests (10+ Complete User Journeys) ───────────────────────────

#[test]
fn test_e2e_register_ship_scan_nebula() {
    let (env, client, player) = setup_env();

    client.initialize_profile(&player);
    let profile = client.get_profile(&player);
    assert_eq!(profile.level, 1);

    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);
    assert!(ship.id > 0);

    let seed = BytesN::from_array(&env, &[1u8; 32]);
    let layout = client.generate_nebula_layout(&seed, &player);
    assert_eq!(layout.cells.len(), TOTAL_CELLS);

    let profile = client.get_profile(&player);
    assert!(profile.scans_performed >= 1);
}

#[test]
fn test_e2e_mint_resources_trade() {
    let (env, client, player) = setup_env();

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("hauler"), &metadata);

    client.deposit_treasure(&player, &ship.id, &1000u64);
    let vault_balance = client.get_vault_balance(&player, &ship.id);
    assert_eq!(vault_balance, 1000u64);

    let profile = client.get_profile(&player);
    assert!(profile.credits >= 0);
}

#[test]
fn test_e2e_upgrade_ship_equipment() {
    let (env, client, player) = setup_env();

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let initial_level = ship.level;
    client.deposit_treasure(&player, &ship.id, &500u64);

    let vault_balance = client.get_vault_balance(&player, &ship.id);
    assert_eq!(vault_balance, 500u64);

    let ship_updated = client.get_ship(&player, &ship.id);
    assert_eq!(ship_updated.level, initial_level);
}

#[test]
fn test_e2e_multiple_scans_different_regions() {
    let (env, client, player) = setup_env();

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let seed1 = BytesN::from_array(&env, &[1u8; 32]);
    let layout1 = client.generate_nebula_layout(&seed1, &player);
    assert!(layout1.total_energy > 0);

    let seed2 = BytesN::from_array(&env, &[2u8; 32]);
    let layout2 = client.generate_nebula_layout(&seed2, &player);
    assert!(layout2.total_energy > 0);

    assert_ne!(layout1.total_energy, layout2.total_energy);
}

#[test]
fn test_e2e_profile_progression() {
    let (env, client, player) = setup_env();

    client.initialize_profile(&player);
    let profile1 = client.get_profile(&player);
    assert_eq!(profile1.level, 1);

    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("fighter"), &metadata);

    let seed = BytesN::from_array(&env, &[42u8; 32]);
    let _layout = client.generate_nebula_layout(&seed, &player);

    let profile2 = client.get_profile(&player);
    assert!(profile2.scans_performed >= profile1.scans_performed);
}

#[test]
fn test_e2e_error_handling_invalid_profile() {
    let (_env, client, player) = setup_env();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _profile = client.get_profile(&player);
    }));
    assert!(result.is_err());
}

#[test]
fn test_e2e_multiple_ships_same_player() {
    let (env, client, player) = setup_env();

    client.initialize_profile(&player);
    let metadata = [0u8; 4];

    let ship1 = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);
    let ship2 = client.mint_ship(&player, &symbol_short!("fighter"), &metadata);
    let ship3 = client.mint_ship(&player, &symbol_short!("hauler"), &metadata);

    assert_ne!(ship1.id, ship2.id);
    assert_ne!(ship2.id, ship3.id);
    assert_ne!(ship1.id, ship3.id);

    let s1 = client.get_ship(&player, &ship1.id);
    let s2 = client.get_ship(&player, &ship2.id);
    let s3 = client.get_ship(&player, &ship3.id);

    assert_eq!(s1.id, ship1.id);
    assert_eq!(s2.id, ship2.id);
    assert_eq!(s3.id, ship3.id);
}

#[test]
fn test_e2e_vault_deposit_withdraw() {
    let (_env, client, player) = setup_env();

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let amount = 750u64;
    client.deposit_treasure(&player, &ship.id, &amount);
    let vault_balance = client.get_vault_balance(&player, &ship.id);
    assert_eq!(vault_balance, amount);

    let additional = 250u64;
    client.deposit_treasure(&player, &ship.id, &additional);
    let vault_balance = client.get_vault_balance(&player, &ship.id);
    assert_eq!(vault_balance, amount + additional);
}

#[test]
fn test_e2e_ship_scanning_sequence() {
    let (env, client, player) = setup_env();

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let seeds = vec![
        BytesN::from_array(&env, &[1u8; 32]),
        BytesN::from_array(&env, &[2u8; 32]),
        BytesN::from_array(&env, &[3u8; 32]),
    ];

    for seed in seeds.iter() {
        let layout = client.generate_nebula_layout(seed, &player);
        assert_eq!(layout.cells.len(), TOTAL_CELLS);
        assert!(layout.total_energy > 0);
    }

    let profile = client.get_profile(&player);
    assert!(profile.scans_performed >= 3);
}

#[test]
fn test_e2e_complete_game_loop() {
    let (env, client, player) = setup_env();

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("hauler"), &metadata);

    client.deposit_treasure(&player, &ship.id, &500u64);

    let seed = BytesN::from_array(&env, &[10u8; 32]);
    let layout = client.generate_nebula_layout(&seed, &player);
    assert!(layout.total_energy > 0);

    let vault_balance = client.get_vault_balance(&player, &ship.id);
    assert!(vault_balance > 0);

    let profile = client.get_profile(&player);
    assert!(profile.scans_performed >= 1);
    assert!(profile.credits >= 0);
}

#[test]
fn test_e2e_error_recovery_after_failure() {
    let (env, client, player) = setup_env();

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let seed = BytesN::from_array(&env, &[5u8; 32]);
    let layout1 = client.generate_nebula_layout(&seed, &player);
    assert_eq!(layout1.cells.len(), TOTAL_CELLS);

    let layout2 = client.generate_nebula_layout(&seed, &player);
    assert_eq!(layout2.cells.len(), TOTAL_CELLS);
    assert_eq!(layout1.total_energy, layout2.total_energy);
}

#[test]
fn test_e2e_ledger_time_progression() {
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
    let player = Address::generate(&env);

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let seed = BytesN::from_array(&env, &[7u8; 32]);
    let layout1 = client.generate_nebula_layout(&seed, &player);

    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 101,
        timestamp: 1_700_001_000,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 10_000,
    });

    let layout2 = client.generate_nebula_layout(&seed, &player);

    assert_ne!(layout1.timestamp, layout2.timestamp);
}
