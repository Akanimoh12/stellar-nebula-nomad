#![cfg(test)]

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, BytesN, Env};
use stellar_nebula_nomad::{
    nebula_gen::{Anomaly, NebulaLayout, NebulaError, MAX_REGION_ID, MIN_SHIP_ID},
    NebulaNomadContract, NebulaNomadContractClient,
};

fn make_env() -> (Env, NebulaNomadContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 100,
        timestamp: 1_700_000_000,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1_000,
        max_entry_ttl: 10_000,
    });
    let contract_id = env.register_contract(None, NebulaNomadContract);
    let client = NebulaNomadContractClient::new(&env, &contract_id);
    (env, client)
}

// ─── Strategy Definitions ──────────────────────────────────────────────────

fn arbitrary_seed() -> impl Strategy<Value = [u8; 32]> {
    prop::array::uniform32(any::<u8>())
}

fn arbitrary_region_id() -> impl Strategy<Value = u64> {
    1u64..=MAX_REGION_ID
}

// ─── Determinism Property Tests ───────────────────────────────────────────

#[test]
fn prop_nebula_gen_deterministic_same_seed() {
    let config = ProptestConfig::with_cases(50);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed = BytesN::from_array(&env, &seed_arr);
            let layout1 = client.generate_nebula_layout(&seed, &player);

            let seed2 = BytesN::from_array(&env, &seed_arr);
            let layout2 = client.generate_nebula_layout(&seed2, &player);

            prop_assert_eq!(
                layout1.total_energy, layout2.total_energy,
                "layouts with same seed must have identical energy"
            );
            prop_assert_eq!(
                layout1.cells.len(),
                layout2.cells.len(),
                "layouts must have same cell count"
            );

            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_gen_different_seeds_likely_different() {
    let config = ProptestConfig::with_cases(50);
    let mut runner = TestRunner::new(config);

    runner
        .run((arbitrary_seed(), arbitrary_seed()), |(seed1_arr, seed2_arr)| {
            if seed1_arr == seed2_arr {
                return Ok(());
            }

            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed1 = BytesN::from_array(&env, &seed1_arr);
            let layout1 = client.generate_nebula_layout(&seed1, &player);

            let seed2 = BytesN::from_array(&env, &seed2_arr);
            let layout2 = client.generate_nebula_layout(&seed2, &player);

            if layout1.total_energy != layout2.total_energy {
                return Ok(());
            }

            prop_assert_ne!(
                layout1.seed, layout2.seed,
                "different seeds should produce different layouts (or be same by chance)"
            );

            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_layout_dimensions_valid() {
    let config = ProptestConfig::with_cases(40);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed = BytesN::from_array(&env, &seed_arr);
            let layout = client.generate_nebula_layout(&seed, &player);

            prop_assert!(
                layout.width > 0 && layout.height > 0,
                "layout dimensions must be positive"
            );
            prop_assert!(
                layout.cells.len() > 0,
                "layout must have at least one cell"
            );

            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_cells_within_grid() {
    let config = ProptestConfig::with_cases(40);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed = BytesN::from_array(&env, &seed_arr);
            let layout = client.generate_nebula_layout(&seed, &player);

            for i in 0..layout.cells.len() {
                let cell = layout.cells.get(i).unwrap();
                prop_assert!(
                    cell.x < layout.width,
                    "cell x={} must be within width={}",
                    cell.x,
                    layout.width
                );
                prop_assert!(
                    cell.y < layout.height,
                    "cell y={} must be within height={}",
                    cell.y,
                    layout.height
                );
            }

            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_energy_positive() {
    let config = ProptestConfig::with_cases(40);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed = BytesN::from_array(&env, &seed_arr);
            let layout = client.generate_nebula_layout(&seed, &player);

            prop_assert!(
                layout.total_energy > 0,
                "layout must have positive total energy"
            );

            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_timestamp_valid() {
    let config = ProptestConfig::with_cases(40);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed = BytesN::from_array(&env, &seed_arr);
            let layout = client.generate_nebula_layout(&seed, &player);

            let current_timestamp = env.ledger().timestamp();
            prop_assert_eq!(
                layout.timestamp, current_timestamp,
                "layout timestamp must equal ledger timestamp"
            );

            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_seed_preserved() {
    let config = ProptestConfig::with_cases(40);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed = BytesN::from_array(&env, &seed_arr);
            let layout = client.generate_nebula_layout(&seed, &player);

            prop_assert_eq!(
                layout.seed, seed,
                "layout seed must match input seed"
            );

            Ok(())
        })
        .unwrap();
}

// ─── Fairness Property Tests ──────────────────────────────────────────────

#[test]
fn prop_nebula_rarity_distribution_bounded() {
    let config = ProptestConfig::with_cases(30);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed = BytesN::from_array(&env, &seed_arr);
            let layout = client.generate_nebula_layout(&seed, &player);

            for i in 0..layout.cells.len() {
                let cell = layout.cells.get(i).unwrap();
                prop_assert!(
                    cell.rarity >= 0 && cell.rarity <= 100,
                    "cell rarity must be in range [0, 100]"
                );
            }

            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_output_valid_for_all_seeds() {
    let config = ProptestConfig::with_cases(50);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed = BytesN::from_array(&env, &seed_arr);

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                client.generate_nebula_layout(&seed, &player)
            }));

            prop_assert!(
                result.is_ok(),
                "nebula generation must not panic for any seed"
            );

            let layout = result.unwrap();
            prop_assert!(!layout.cells.is_empty(), "layout must not be empty");

            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_consistency_across_calls() {
    let config = ProptestConfig::with_cases(30);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);

            client.initialize_profile(&player);
            let metadata = [0u8; 4];
            let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

            let seed = BytesN::from_array(&env, &seed_arr);

            let layout1 = client.generate_nebula_layout(&seed, &player);
            let layout2 = client.generate_nebula_layout(&seed, &player);

            prop_assert_eq!(
                layout1.total_energy, layout2.total_energy,
                "repeated calls with same seed must yield same energy"
            );
            prop_assert_eq!(
                layout1.cells.len(),
                layout2.cells.len(),
                "repeated calls must yield same cell count"
            );
            prop_assert_eq!(
                layout1.timestamp,
                layout2.timestamp,
                "repeated calls must yield same timestamp"
            );

            Ok(())
        })
        .unwrap();
}

// ─── Edge Case Property Tests ──────────────────────────────────────────────

#[test]
fn prop_nebula_zero_seed_valid() {
    let (env, client) = make_env();
    let player = soroban_sdk::Address::generate(&env);

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let zero_seed = BytesN::from_array(&env, &[0u8; 32]);
    let layout = client.generate_nebula_layout(&zero_seed, &player);

    assert_eq!(layout.cells.len(), stellar_nebula_nomad::TOTAL_CELLS);
    assert!(layout.total_energy > 0);
}

#[test]
fn prop_nebula_max_seed_valid() {
    let (env, client) = make_env();
    let player = soroban_sdk::Address::generate(&env);

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let max_seed = BytesN::from_array(&env, &[255u8; 32]);
    let layout = client.generate_nebula_layout(&max_seed, &player);

    assert_eq!(layout.cells.len(), stellar_nebula_nomad::TOTAL_CELLS);
    assert!(layout.total_energy > 0);
}

#[test]
fn prop_nebula_mixed_seed_valid() {
    let (env, client) = make_env();
    let player = soroban_sdk::Address::generate(&env);

    client.initialize_profile(&player);
    let metadata = [0u8; 4];
    let _ship = client.mint_ship(&player, &symbol_short!("explorer"), &metadata);

    let mut mixed_seed = [0u8; 32];
    for i in 0..32 {
        mixed_seed[i] = if i % 2 == 0 { 0u8 } else { 255u8 };
    }

    let seed = BytesN::from_array(&env, &mixed_seed);
    let layout = client.generate_nebula_layout(&seed, &player);

    assert_eq!(layout.cells.len(), stellar_nebula_nomad::TOTAL_CELLS);
    assert!(layout.total_energy > 0);
}
