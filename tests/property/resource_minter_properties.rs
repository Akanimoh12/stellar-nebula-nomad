#![cfg(test)]

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{Address, Env};
use stellar_nebula_nomad::resource_minter::{
    balance_of, credit_balance, total_minted, ResourceType,
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

fn arbitrary_amount() -> impl Strategy<Value = u64> {
    1u64..100_000u64
}

#[test]
fn prop_resource_minter_conservation() {
    let config = ProptestConfig::with_cases(50);
    let mut runner = TestRunner::new(config);

    runner
        .run(&(arbitrary_amount(), arbitrary_amount()), |(amt1, amt2)| {
            let env = make_env();
            let user1 = Address::generate(&env);
            let user2 = Address::generate(&env);
            let resource = ResourceType::StellarDust;

            let initial_total = total_minted(&env, &resource);

            let _ = credit_balance(&env, &user1, &resource, amt1);
            let _ = credit_balance(&env, &user2, &resource, amt2);

            let bal1 = balance_of(&env, &user1, &resource);
            let bal2 = balance_of(&env, &user2, &resource);

            prop_assert_eq!(bal1, amt1);
            prop_assert_eq!(bal2, amt2);
            prop_assert_eq!(bal1 + bal2, amt1 + amt2);
            prop_assert_eq!(total_minted(&env, &resource), initial_total + amt1 + amt2);
            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_resource_minter_ownership_integrity() {
    let env = make_env();
    let owner = Address::generate(&env);
    let other = Address::generate(&env);
    let resource = ResourceType::DarkMatter;

    let _ = credit_balance(&env, &owner, &resource, 500);

    assert_eq!(balance_of(&env, &owner, &resource), 500);
    assert_eq!(balance_of(&env, &other, &resource), 0);
}

#[test]
fn prop_resource_minter_zero_amount_conservation() {
    let env = make_env();
    let user = Address::generate(&env);
    let resource = ResourceType::ExoticMatter;

    let _ = credit_balance(&env, &user, &resource, 0);
    assert_eq!(balance_of(&env, &user, &resource), 0);
}
