#![no_main]
//! Fuzz `mint_ship` with arbitrary metadata bytes; must return a value or a
//! contract error, never a host panic from unexpected input shape.
use libfuzzer_sys::fuzz_target;
use soroban_sdk::{symbol_short, testutils::Address as _, Address, Env};
use stellar_nebula_nomad::{NebulaNomadContract, NebulaNomadContractClient};

fuzz_target!(|metadata: &[u8]| {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, NebulaNomadContract);
    let client = NebulaNomadContractClient::new(&env, &id);
    let player = Address::generate(&env);
    client.initialize_profile(&player);
    let _ = client.try_mint_ship(&player, &symbol_short!("explorer"), &soroban_sdk::Bytes::from_slice(&env, metadata));
});
