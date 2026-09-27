#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, vec, Address, Bytes, BytesN, Env};
use stellar_nebula_nomad::{
    NebulaNomadContract, NebulaNomadContractClient, TOTAL_CELLS,
};
use stellar_nebula_nomad::resource_minter::{balance_of, credit_balance, ResourceType};
use stellar_nebula_nomad::trading::{add_liquidity, create_pool, quote_swap, swap_exact_input};

fn setup_env() -> (Env, NebulaNomadContractClient<'static>, Address, Address) {
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
    let lp_provider = Address::generate(&env);
    (env, client, player, lp_provider)
}

#[test]
fn test_economic_cycle_workflow_e2e() {
    let (env, client, player, lp_provider) = setup_env();

    // Step 1: Initialize profile
    let profile_id = client.initialize_profile(&player);

    // Step 2: Mint explorer ship
    let ship = client.mint_ship(&player, &symbol_short!("trader"), &Bytes::new(&env));
    assert!(ship.id > 0);

    // Step 3: Scan nebula
    let seed = BytesN::from_array(&env, &[10u8; 32]);
    let layout = client.generate_nebula_layout(&seed, &player);
    assert_eq!(layout.cells.len(), TOTAL_CELLS);

    // Step 4: Harvest StellarDust & DarkMatter
    let _ = credit_balance(&env, &player, &ResourceType::StellarDust, 10_000);
    let _ = credit_balance(&env, &player, &ResourceType::DarkMatter, 5_000);
    assert_eq!(balance_of(&env, &player, &ResourceType::StellarDust), 10_000);

    // Step 5: Liquidity provider creates DEX pool
    let pool_id = create_pool(
        &env,
        &lp_provider,
        symbol_short!("hydro"),
        symbol_short!("plasma"),
    )
    .unwrap();

    // Step 6: LP adds initial liquidity
    add_liquidity(&env, &lp_provider, pool_id, 50_000, 50_000).unwrap();

    // Step 7: Player quotes swap price
    let amount_in = 2_000i128;
    let quote = quote_swap(&env, pool_id, symbol_short!("hydro"), amount_in).unwrap();
    assert!(quote > 0 && quote <= amount_in);

    // Step 8: Execute swap on DEX
    let route = vec![&env, pool_id];
    let received = swap_exact_input(
        &env,
        &player,
        symbol_short!("hydro"),
        amount_in,
        quote, // min expected
        route,
    )
    .unwrap();
    assert!(received >= quote);

    // Step 9: Re-quote after price impact
    let quote_after = quote_swap(&env, pool_id, symbol_short!("hydro"), amount_in).unwrap();
    assert!(quote_after < quote); // Pool moved along invariant curve

    // Step 10: Verify state consistency
    let final_profile = client.get_profile(&profile_id);
    assert_eq!(final_profile.id, profile_id);
}
