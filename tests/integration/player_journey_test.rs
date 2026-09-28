#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, Address, Bytes, BytesN, Env};
use stellar_nebula_nomad::{
    NebulaNomadContract, NebulaNomadContractClient, TOTAL_CELLS,
};
use stellar_nebula_nomad::resource_minter::{balance_of, credit_balance, ResourceType};

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
        min_persistent_entry_ttl: 1_000,
        max_entry_ttl: 10_000,
    });
    let contract_id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &contract_id);
    let player = Address::generate(&env);
    (env, client, player)
}

#[test]
fn test_player_journey_onboarding_e2e() {
    // Step 1: Set up player environment
    let (env, client, player) = setup_env();

    // Step 2: Initialize player profile
    let profile_id = client.initialize_profile(&player);
    assert!(profile_id > 0);
    let profile = client.get_profile(&profile_id);
    assert_eq!(profile.id, profile_id);

    // Step 3: Mint starter exploration ship
    let metadata = Bytes::from_slice(&env, &[1u8; 4]);
    let ship = client.mint_ship(&player, &symbol_short!("scout"), &metadata);
    assert!(ship.id > 0);

    // Step 4: Verify ship properties
    let retrieved_ship = client.get_ship(&ship.id);
    assert_eq!(retrieved_ship.id, ship.id);

    // Step 5: Procedurally generate first nebula layout
    let seed = BytesN::from_array(&env, &[42u8; 32]);
    let layout = client.generate_nebula_layout(&seed, &player);
    assert_eq!(layout.cells.len(), TOTAL_CELLS);

    // Step 6: Verify scan incremented on profile
    let updated_profile = client.get_profile(&profile_id);
    assert!(updated_profile.total_scans >= 1);

    // Step 7: Credit discovery resources (StellarDust)
    let _ = credit_balance(&env, &player, &ResourceType::StellarDust, 100);
    assert_eq!(balance_of(&env, &player, &ResourceType::StellarDust), 100);

    // Step 8: Credit secondary discovery resources (DarkMatter)
    let _ = credit_balance(&env, &player, &ResourceType::DarkMatter, 50);
    assert_eq!(balance_of(&env, &player, &ResourceType::DarkMatter), 50);

    // Step 9: Re-scan with a new seed for journey progression
    let seed2 = BytesN::from_array(&env, &[43u8; 32]);
    let layout2 = client.generate_nebula_layout(&seed2, &player);
    assert_eq!(layout2.cells.len(), TOTAL_CELLS);

    // Step 10: Final state validation
    let final_profile = client.get_profile(&profile_id);
    assert_eq!(final_profile.total_scans, 2);
    assert_eq!(balance_of(&env, &player, &ResourceType::StellarDust), 100);
}
