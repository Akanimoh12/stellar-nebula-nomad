#![no_main]
//! Fuzz `generate_nebula_layout`: arbitrary 32-byte seeds must never panic
//! and must be deterministic.
use libfuzzer_sys::fuzz_target;
use soroban_sdk::{symbol_short, testutils::Address as _, Address, BytesN, Env};
use stellar_nebula_nomad::{NebulaNomadContract, NebulaNomadContractClient};

fuzz_target!(|seed_arr: [u8; 32]| {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, NebulaNomadContract);
    let client = NebulaNomadContractClient::new(&env, &id);
    let player = Address::generate(&env);
    client.initialize_profile(&player);
    let _ = client.try_mint_ship(&player, &symbol_short!("explorer"), &soroban_sdk::Bytes::from_array(&env, &[0u8; 4]));

    let seed = BytesN::from_array(&env, &seed_arr);
    let a = client.generate_nebula_layout(&seed, &player);
    let b = client.generate_nebula_layout(&BytesN::from_array(&env, &seed_arr), &player);
    assert_eq!(a.total_energy, b.total_energy);
    assert_eq!(a.cells.len(), b.cells.len());
});
