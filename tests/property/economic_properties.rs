#![cfg(test)]

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, Address, Env};
use stellar_nebula_nomad::trading::{
    add_liquidity, create_pool, quote_swap,
};

fn make_env() -> Env {
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
    env
}

fn arbitrary_swap_amount() -> impl Strategy<Value = i128> {
    10i128..10_000i128
}

#[test]
fn prop_economic_no_value_creation_and_price_bounds() {
    let config = ProptestConfig::with_cases(50);
    let mut runner = TestRunner::new(config);

    runner
        .run(&arbitrary_swap_amount(), |amount_in| {
            let env = make_env();
            let admin = Address::generate(&env);
            let pool_id = create_pool(
                &env,
                &admin,
                symbol_short!("hydro"),
                symbol_short!("plasma"),
            )
            .unwrap();

            // Add liquidity 100,000 : 100,000
            let reserve = 100_000i128;
            add_liquidity(&env, &admin, pool_id, reserve, reserve).unwrap();

            let quote = quote_swap(&env, pool_id, symbol_short!("hydro"), amount_in).unwrap();

            // Property 1: Output must be non-negative
            prop_assert!(quote > 0);
            // Property 2: Cannot extract more than reserve (no value from nothing)
            prop_assert!(quote < reserve);
            // Property 3: With 1:1 initial ratio and fee, quote must be strictly <= amount_in
            prop_assert!(quote <= amount_in);

            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_economic_constant_product_preserved() {
    let env = make_env();
    let admin = Address::generate(&env);
    let pool_id = create_pool(
        &env,
        &admin,
        symbol_short!("hydro"),
        symbol_short!("plasma"),
    )
    .unwrap();

    let reserve_a = 50_000i128;
    let reserve_b = 50_000i128;
    add_liquidity(&env, &admin, pool_id, reserve_a, reserve_b).unwrap();

    let amount_in = 1_000i128;
    let quote_out = quote_swap(&env, pool_id, symbol_short!("hydro"), amount_in).unwrap();

    // Constant product invariant: (reserve_a + amount_in) * (reserve_b - quote_out) >= reserve_a * reserve_b
    let initial_k = reserve_a * reserve_b;
    let final_k = (reserve_a + amount_in) * (reserve_b - quote_out);
    assert!(final_k >= initial_k);
}
