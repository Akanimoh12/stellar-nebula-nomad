#![cfg(test)]

use soroban_sdk::contracttype;
use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, Address, Bytes, Env, Symbol};
use stellar_nebula_nomad::{
    NebulaNomadContract, NebulaNomadContractClient, RepairConfig, ShipDataKey, ShipNft,
    ShipRepairError, DEFAULT_COST_PER_POINT, DEFAULT_EMERGENCY_SURCHARGE_BPS,
    DEFAULT_MAX_REPAIR_PER_CALL,
};

const DUST: Symbol = symbol_short!("dust");

fn setup() -> (Env, NebulaNomadContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 100,
        timestamp: 1_000_000,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 6_312_000,
        max_entry_ttl: 6_312_000,
    });
    let contract_id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    (env, client, admin)
}

/// Mirror of resource_minter::ResourceKey::ResourceBalance.
#[contracttype]
#[derive(Clone)]
enum ResourceKey {
    ResourceBalance(Address, Symbol),
}

fn seed_ship(
    env: &Env,
    contract_id: &Address,
    ship_id: u64,
    owner: &Address,
    durability: u32,
    max_durability: u32,
) {
    let record = ShipNft {
        id: ship_id,
        owner: owner.clone(),
        ship_type: symbol_short!("fighter"),
        hull: 150,
        scanner_power: 20,
        durability,
        max_durability,
        metadata: Bytes::new(env),
        metadata_uri: Bytes::new(env),
    };
    env.as_contract(contract_id, || {
        env.storage()
            .persistent()
            .set(&ShipDataKey::Ship(ship_id), &record);
    });
}

fn credit_resource(
    env: &Env,
    contract_id: &Address,
    player: &Address,
    asset_id: Symbol,
    amount: u32,
) {
    env.as_contract(contract_id, || {
        let key = ResourceKey::ResourceBalance(player.clone(), asset_id);
        env.storage().instance().set(&key, &amount);
    });
}

fn read_balance(env: &Env, contract_id: &Address, player: &Address, asset_id: Symbol) -> u32 {
    env.as_contract(contract_id, || {
        let key = ResourceKey::ResourceBalance(player.clone(), asset_id);
        env.storage().instance().get(&key).unwrap_or(0)
    })
}

fn read_durability(env: &Env, contract_id: &Address, ship_id: u64) -> u32 {
    env.as_contract(contract_id, || {
        let key = ShipDataKey::Ship(ship_id);
        let record: ShipNft = env.storage().persistent().get(&key).unwrap();
        record.durability
    })
}

/// Cost of restoring `points` at the default 2 dust/point schedule.
fn base_cost(points: u32) -> u32 {
    points * DEFAULT_COST_PER_POINT
}

/// Default field-patch surcharge: +50% on top of the base price.
fn emergency_cost(points: u32) -> u32 {
    let base = base_cost(points);
    base + (base * DEFAULT_EMERGENCY_SURCHARGE_BPS) / 10_000
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn test_quote_repair_reports_missing_points_and_cost() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 60, 100);

    let quote = client.quote_repair(&player, &1u64, &DUST, &false);
    assert_eq!(quote.missing, 40);
    assert_eq!(quote.restore_points, 40);
    assert_eq!(quote.resource_cost, base_cost(40));
    assert_eq!(quote.emergency, false);
}

#[test]
fn test_quote_repair_clamps_to_max_per_call() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    // 100 missing, but only max_repair_per_call is repairable per call.
    seed_ship(&env, &client.address, 1, &player, 0, 100);

    let quote = client.quote_repair(&player, &1u64, &DUST, &false);
    assert_eq!(quote.missing, 100);
    assert_eq!(quote.restore_points, DEFAULT_MAX_REPAIR_PER_CALL);
    assert_eq!(quote.resource_cost, base_cost(DEFAULT_MAX_REPAIR_PER_CALL));
}

#[test]
fn test_quote_repair_applies_emergency_surcharge() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 40, 100);

    let quote = client.quote_repair(&player, &1u64, &DUST, &true);
    assert_eq!(quote.emergency, true);
    // 60 missing, clamped to the 50-point per-call cap.
    assert_eq!(quote.missing, 60);
    assert_eq!(quote.restore_points, DEFAULT_MAX_REPAIR_PER_CALL);
    assert_eq!(quote.resource_cost, emergency_cost(50));
}

#[test]
fn test_quote_repair_rejects_non_owner() {
    let (env, client, _admin) = setup();
    let owner = Address::generate(&env);
    let stranger = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &owner, 0, 100);

    let err = client
        .try_quote_repair(&stranger, &1u64, &DUST, &false)
        .unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::NotOwner));
}

#[test]
fn test_quote_repair_rejects_unknown_ship() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    let err = client.try_quote_repair(&player, &99u64, &DUST, &false).unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::ShipNotFound));
}

#[test]
fn test_repair_ship_burns_dust_and_restores_durability() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 50, 100);
    credit_resource(&env, &client.address, &player, DUST, 1_000);

    let receipt = client.repair_ship(&player, &1u64, &DUST, &false);
    assert_eq!(receipt.durability_before, 50);
    assert_eq!(receipt.durability_after, 100);
    assert_eq!(receipt.resource_burned, base_cost(50));
    assert_eq!(receipt.emergency, false);

    assert_eq!(read_durability(&env, &client.address, 1), 100);
    // 1000 - 100 burned.
    assert_eq!(read_balance(&env, &client.address, &player, DUST), 900);
}

#[test]
fn test_repair_ship_emergency_charges_surcharge() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 70, 100);
    credit_resource(&env, &client.address, &player, DUST, 1_000);

    let receipt = client.repair_ship(&player, &1u64, &DUST, &true);
    assert_eq!(receipt.durability_after, 100);
    assert_eq!(receipt.resource_burned, emergency_cost(30));
    assert_eq!(
        read_balance(&env, &client.address, &player, DUST),
        1_000 - emergency_cost(30)
    );
}

#[test]
fn test_repair_ship_rejects_insufficient_resources_without_mutating() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 0, 100);
    credit_resource(&env, &client.address, &player, DUST, 10);

    let err = client
        .try_repair_ship(&player, &1u64, &DUST, &false)
        .unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::InsufficientResources));
    // No durability gained and no dust taken: the affordability check
    // precedes the burn.
    assert_eq!(read_durability(&env, &client.address, 1), 0);
    assert_eq!(read_balance(&env, &client.address, &player, DUST), 10);
}

#[test]
fn test_repair_ship_rejects_non_owner() {
    let (env, client, _admin) = setup();
    let owner = Address::generate(&env);
    let thief = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &owner, 0, 100);
    credit_resource(&env, &client.address, &thief, DUST, 10_000);

    let err = client.try_repair_ship(&thief, &1u64, &DUST, &false).unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::NotOwner));
    assert_eq!(read_durability(&env, &client.address, 1), 0);
    assert_eq!(read_balance(&env, &client.address, &thief, DUST), 10_000);
}

#[test]
fn test_repair_ship_rejects_full_hull() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 100, 100);
    credit_resource(&env, &client.address, &player, DUST, 10_000);

    let err = client.try_repair_ship(&player, &1u64, &DUST, &false).unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::ShipAlreadyFull));
    assert_eq!(read_balance(&env, &client.address, &player, DUST), 10_000);
}

#[test]
fn test_repair_ship_rejects_unknown_ship() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    credit_resource(&env, &client.address, &player, DUST, 10_000);

    let err = client.try_repair_ship(&player, &99u64, &DUST, &false).unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::ShipNotFound));
}

#[test]
fn test_repair_ship_is_rate_limited() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    // Four damaged ships so the limiter, not the hull cap, is what trips.
    for ship_id in 1..=4u64 {
        seed_ship(&env, &client.address, ship_id, &player, 50, 100);
    }
    credit_resource(&env, &client.address, &player, DUST, 100_000);

    for ship_id in 1..=3u64 {
        client.repair_ship(&player, &ship_id, &DUST, &false);
    }
    let err = client.try_repair_ship(&player, &4u64, &DUST, &false).unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::RateLimitExceeded));
}

#[test]
fn test_repair_sink_total_accumulates_across_ships() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 60, 100);
    seed_ship(&env, &client.address, 2, &player, 80, 100);
    credit_resource(&env, &client.address, &player, DUST, 100_000);

    assert_eq!(client.get_total_repair_burn(), 0);

    client.repair_ship(&player, &1u64, &DUST, &false);
    let after_first = client.get_total_repair_burn();
    assert_eq!(after_first, base_cost(40));

    client.repair_ship(&player, &2u64, &DUST, &false);
    assert_eq!(client.get_total_repair_burn(), after_first + base_cost(20));
}

#[test]
fn test_repair_sink_total_stays_zero_on_failed_repairs() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 100, 100);
    credit_resource(&env, &client.address, &player, DUST, 10_000);

    assert!(client.try_repair_ship(&player, &1u64, &DUST, &false).is_err());
    assert!(client
        .try_repair_ship(&player, &1u64, &DUST, &true)
        .is_err());
    assert_eq!(client.get_total_repair_burn(), 0);
}

#[test]
fn test_repair_config_defaults_are_issue_453_values() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 0, 100);

    // Unconfigured sink falls back to the balanced defaults.
    let config = client.get_repair_config();
    assert_eq!(
        config,
        RepairConfig {
            cost_per_point: DEFAULT_COST_PER_POINT,
            emergency_surcharge_bps: DEFAULT_EMERGENCY_SURCHARGE_BPS,
            max_repair_per_call: DEFAULT_MAX_REPAIR_PER_CALL,
        }
    );
    let _ = client.quote_repair(&player, &1u64, &DUST, &false);
}

#[test]
fn test_init_repair_config_seeds_admin() {
    let (env, client, admin) = setup();
    let config = RepairConfig {
        cost_per_point: 3,
        emergency_surcharge_bps: 1_000,
        max_repair_per_call: 20,
    };
    client.init_repair_config(&admin, &config);
    assert_eq!(client.get_repair_config(), config);
}

#[test]
fn test_init_repair_config_rejects_double_init() {
    let (_env, client, admin) = setup();
    let config = RepairConfig {
        cost_per_point: 3,
        emergency_surcharge_bps: 1_000,
        max_repair_per_call: 20,
    };
    client.init_repair_config(&admin, &config);
    let err = client.try_init_repair_config(&admin, &config).unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::InvalidConfig));
}

#[test]
fn test_admin_can_retune_repair_pricing() {
    let (env, client, admin) = setup();
    let player = Address::generate(&env);
    seed_ship(&env, &client.address, 1, &player, 0, 100);
    credit_resource(&env, &client.address, &player, DUST, 100_000);
    client.init_repair_config(&admin, &RepairConfig::default_rebalanced());

    client.set_repair_config(
        &admin,
        &RepairConfig {
            cost_per_point: 7,
            emergency_surcharge_bps: 0,
            max_repair_per_call: 10,
        },
    );

    // 100 missing, capped at 10 per call, 7 dust per point.
    let quote = client.quote_repair(&player, &1u64, &DUST, &false);
    assert_eq!(quote.missing, 100);
    assert_eq!(quote.restore_points, 10);
    assert_eq!(quote.resource_cost, 70);

    let receipt = client.repair_ship(&player, &1u64, &DUST, &true);
    assert_eq!(receipt.durability_after, 10);
    assert_eq!(receipt.resource_burned, 70);
    assert_eq!(read_durability(&env, &client.address, 1), 10);
}

#[test]
fn test_set_repair_config_rejects_non_admin() {
    let (env, client, admin) = setup();
    let impostor = Address::generate(&env);
    client.init_repair_config(&admin, &RepairConfig::default_rebalanced());

    let err = client
        .try_set_repair_config(
            &impostor,
            &RepairConfig {
                cost_per_point: 1,
                emergency_surcharge_bps: 0,
                max_repair_per_call: 10,
            },
        )
        .unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::NotOwner));
}

#[test]
fn test_set_repair_config_rejects_implausible_values() {
    let (env, client, admin) = setup();
    client.init_repair_config(&admin, &RepairConfig::default_rebalanced());

    // Zero cost per point would make the sink free.
    let err = client
        .try_set_repair_config(
            &admin,
            &RepairConfig {
                cost_per_point: 0,
                emergency_surcharge_bps: 0,
                max_repair_per_call: 10,
            },
        )
        .unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::InvalidConfig));

    // Zero per-call cap would make every repair a no-op.
    let err = client
        .try_set_repair_config(
            &admin,
            &RepairConfig {
                cost_per_point: 2,
                emergency_surcharge_bps: 0,
                max_repair_per_call: 0,
            },
        )
        .unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::InvalidConfig));

    // Absurd surcharge.
    let err = client
        .try_set_repair_config(
            &admin,
            &RepairConfig {
                cost_per_point: 2,
                emergency_surcharge_bps: 10_000_000,
                max_repair_per_call: 10,
            },
        )
        .unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::InvalidConfig));
}

#[test]
fn test_set_repair_config_rejects_before_init() {
    let (env, client, _admin) = setup();
    let caller = Address::generate(&env);
    let err = client
        .try_set_repair_config(&caller, &RepairConfig::default_rebalanced())
        .unwrap_err();
    assert_eq!(err, Ok(ShipRepairError::NotOwner));
}

#[test]
fn test_repair_rate_limit_config_matches_defaults() {
    use stellar_nebula_nomad::rate_limiter::RateLimitConfig;
    // 3 repairs per 5 minutes: generous enough for run recovery, tight enough
    // that a scripted burn loop cannot drain the resource supply.
    let config = RateLimitConfig::default_ship_repair();
    assert_eq!(config.max_calls, 3);
    assert_eq!(config.window_seconds, 300);
}
