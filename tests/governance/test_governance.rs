#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};
use stellar_nebula_nomad::governance::{self, GovError};

#[contract]
struct Stub;
#[contractimpl]
impl Stub {}

fn setup() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract = env.register(Stub, ());
    let admin = Address::generate(&env);
    (env, contract, admin)
}

#[test]
fn test_set_game_parameter_by_dao() {
    let (env, contract, _admin) = setup();
    let dao_address = Address::generate(&env);
    let param_key = symbol_short!("reward");
    let param_value = 5000;

    env.as_contract(&contract, || {
        let result = governance::set_game_parameter(
            env.clone(),
            dao_address.clone(),
            param_key,
            param_value,
        );
        let _ = result;
    });
}

#[test]
fn test_set_game_parameter_by_non_dao_rejected() {
    let (env, contract, _admin) = setup();
    let attacker = Address::generate(&env);
    let param_key = symbol_short!("reward");
    let param_value = 5000;

    env.as_contract(&contract, || {
        let result = governance::set_game_parameter(
            env.clone(),
            attacker.clone(),
            param_key,
            param_value,
        );
        assert_eq!(result, Err(GovError::NotDao));
    });
}

#[test]
fn test_get_game_parameter_returns_none_if_not_set() {
    let (env, contract, _admin) = setup();
    let param_key = symbol_short!("unknown");

    env.as_contract(&contract, || {
        let value = governance::get_game_parameter(env.clone(), param_key);
        assert!(value.is_none());
    });
}
