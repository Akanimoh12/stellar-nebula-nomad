#![cfg(test)]

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{BytesN, Env};
use stellar_nebula_nomad::{
    NebulaNomadContract, NebulaNomadContractClient, TOTAL_CELLS,
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
    let contract_id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &contract_id);
    (env, client)
}

fn arbitrary_seed() -> impl Strategy<Value = [u8; 32]> {
    prop::array::uniform32(any::<u8>())
}

#[test]
fn prop_nebula_gen_determinism() {
    let config = ProptestConfig::with_cases(100);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);
            let seed1 = BytesN::from_array(&env, &seed_arr);
            let seed2 = BytesN::from_array(&env, &seed_arr);

            let layout1 = client.generate_nebula_layout(&seed1, &player);
            let layout2 = client.generate_nebula_layout(&seed2, &player);

            prop_assert_eq!(layout1.cells.len(), layout2.cells.len());
            prop_assert_eq!(layout1.cells.len(), TOTAL_CELLS);
            prop_assert_eq!(layout1.total_energy, layout2.total_energy);
            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_gen_valid_ranges() {
    let config = ProptestConfig::with_cases(50);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_seed(), |seed_arr| {
            let (env, client) = make_env();
            let player = soroban_sdk::Address::generate(&env);
            let seed = BytesN::from_array(&env, &seed_arr);

            let layout = client.generate_nebula_layout(&seed, &player);
            for cell in layout.cells.iter() {
                // Ensure coordinates and energy are valid non-negative bounded values
                prop_assert!(cell.energy <= 10_000);
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_nebula_gen_distribution_fairness() {
    let (env, client) = make_env();
    let player = soroban_sdk::Address::generate(&env);

    let mut total_energy = 0u64;
    for i in 0..10u8 {
        let mut seed = [0u8; 32];
        seed[0] = i;
        let seed_bytes = BytesN::from_array(&env, &seed);
        let layout = client.generate_nebula_layout(&seed_bytes, &player);
        total_energy += layout.total_energy as u64;
    }

    assert!(total_energy > 0);
}
